use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serial_test::serial;
use wsl_code_switch_lib::adapters::local_scan_parser::DatabaseLocalScanParserAdapter;
use wsl_code_switch_lib::adapters::local_scan_summary::DatabaseLocalScanSummaryAdapter;
use wsl_code_switch_lib::adapters::local_skill_tree::LocalSkillTreeAdapter;
use wsl_code_switch_lib::domain::{
    LocalScanDomain, LocalScanEvent, LocalScanFailureKind, LocalScanRecordChange, LocalScanSummary,
    LocalScanTarget, LocalSkill, ManagedClientApps, ManagedClientId,
};
use wsl_code_switch_lib::ports::{
    LocalScanParsedSnapshot, LocalScanParserPort, LocalScanReadFailure, LocalScanSummaryPort,
    LocalSkillTreePort, ManagedSkillInventoryPort,
};
use wsl_code_switch_lib::{Database, LocalScanCoordinator, LocalScanWriteTracker};

struct TestHomeGuard(Option<std::ffi::OsString>);

impl TestHomeGuard {
    #[allow(deprecated)]
    fn set(home: &std::path::Path) -> Self {
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", home);
        Self(previous)
    }
}

impl Drop for TestHomeGuard {
    #[allow(deprecated)]
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

fn target(client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget {
        domain: LocalScanDomain::Skill,
        client_id,
    }
}

fn skill_dir(
    home: &std::path::Path,
    client: ManagedClientId,
    directory: &str,
) -> std::path::PathBuf {
    match client {
        ManagedClientId::Claude => home.join(".claude/skills").join(directory),
        ManagedClientId::Codex => home.join(".codex/skills").join(directory),
        ManagedClientId::Opencode => home.join(".config/opencode/skills").join(directory),
    }
}

fn write_skill(home: &std::path::Path, client: ManagedClientId, directory: &str, body: &str) {
    let root = skill_dir(home, client, directory);
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {directory}\ndescription: managed fixture\n---\n{body}\n"),
    )
    .unwrap();
}

fn confirmed_skill(
    client: ManagedClientId,
    directory: &str,
    apps: ManagedClientApps,
) -> LocalSkill {
    let tree = LocalSkillTreeAdapter::runtime()
        .capture(client, directory)
        .unwrap()
        .tree
        .unwrap();
    LocalSkill {
        id: directory.to_string(),
        name: directory.to_string(),
        description: Some("managed fixture".to_string()),
        directory: directory.to_string(),
        content_hash: Some(tree.content_hash.clone()),
        total_size_bytes: tree.total_size_bytes,
        file_count: tree.file_count,
        apps,
        cloud_eligible: tree.is_cloud_eligible(),
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

struct RecordingParser {
    inner: DatabaseLocalScanParserAdapter,
    calls: Mutex<Vec<LocalScanTarget>>,
}

impl RecordingParser {
    fn new(inventory: Arc<dyn ManagedSkillInventoryPort>) -> Self {
        Self {
            inner: DatabaseLocalScanParserAdapter::new(inventory),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<LocalScanTarget> {
        self.calls.lock().unwrap().clone()
    }
}

impl LocalScanParserPort for RecordingParser {
    fn parse_changed(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        self.calls.lock().unwrap().push(target);
        self.inner.parse_changed(target)
    }
}

fn runtime(
    database: Arc<Database>,
) -> (
    Arc<DatabaseLocalScanSummaryAdapter>,
    Arc<RecordingParser>,
    LocalScanCoordinator,
) {
    let (source, inventory) = DatabaseLocalScanSummaryAdapter::runtime(database);
    let source = Arc::new(source);
    let parser = Arc::new(RecordingParser::new(inventory));
    let coordinator = LocalScanCoordinator::new(
        source.clone(),
        parser.clone(),
        Arc::new(LocalScanWriteTracker::default()),
    );
    (source, parser, coordinator)
}

#[test]
#[serial]
fn empty_managed_inventory_is_a_trusted_empty_baseline_and_ignores_unknown_trees() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "unknown-valid",
        "import-only body",
    );
    let unknown_invalid = skill_dir(temp.path(), ManagedClientId::Claude, "unknown-invalid");
    fs::create_dir_all(&unknown_invalid).unwrap();
    fs::write(unknown_invalid.join("not-a-manifest.txt"), b"ignored").unwrap();

    let (_source, parser, coordinator) = runtime(Arc::new(Database::memory().unwrap()));
    let scan_target = target(ManagedClientId::Claude);

    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(parser.calls().is_empty());
    assert!(coordinator.pending_change(scan_target).is_none());
}

#[test]
#[serial]
fn matching_persisted_baseline_is_unchanged_and_unknown_trees_are_never_read() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "confirmed body",
    );
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "unknown-valid",
        "unknown body",
    );
    let unknown_invalid = skill_dir(temp.path(), ManagedClientId::Claude, "unknown-invalid");
    fs::create_dir_all(&unknown_invalid).unwrap();
    fs::write(unknown_invalid.join("not-a-manifest.txt"), b"ignored").unwrap();

    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[confirmed_skill(
            ManagedClientId::Claude,
            "managed",
            ManagedClientApps::only(ManagedClientId::Claude),
        )])
        .unwrap();
    let (source, parser, coordinator) = runtime(database);
    let scan_target = target(ManagedClientId::Claude);

    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(
        parser.calls().is_empty(),
        "matching baseline must not parse"
    );
    assert_eq!(
        source
            .scan_summary(scan_target)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.record_id)
            .collect::<Vec<_>>(),
        ["managed"]
    );

    fs::write(
        skill_dir(temp.path(), ManagedClientId::Claude, "unknown-valid").join("SKILL.md"),
        b"unknown changed and still ignored",
    )
    .unwrap();
    fs::write(unknown_invalid.join("more-bad-data"), b"still ignored").unwrap();
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(parser.calls().is_empty());

    fs::remove_file(skill_dir(temp.path(), ManagedClientId::Claude, "managed").join("SKILL.md"))
        .unwrap();
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Failed {
            failure: wsl_code_switch_lib::domain::LocalScanFailure {
                kind: LocalScanFailureKind::ReadFailed,
                ..
            },
            ..
        }
    ));
    assert!(
        parser.calls().is_empty(),
        "invalid known tree must fail before parse"
    );
}

#[test]
#[serial]
fn expected_after_write_projects_the_committed_skill_hash_instead_of_current_live_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "committed write",
    );
    let database = Arc::new(Database::memory().unwrap());
    let committed = confirmed_skill(
        ManagedClientId::Claude,
        "managed",
        ManagedClientApps::only(ManagedClientId::Claude),
    );
    let committed_hash = committed.content_hash.clone().unwrap();
    database.save_core_skills(&[committed]).unwrap();
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "third-party edit immediately after commit",
    );
    let (source, _) = DatabaseLocalScanSummaryAdapter::runtime(database);
    let scan_target = target(ManagedClientId::Claude);

    let expected = source.expected_after_write(scan_target).unwrap();
    let live = source.scan_summary(scan_target).unwrap();

    assert_eq!(expected.entries[0].content_digest, committed_hash);
    assert_ne!(live.scope_digest, expected.scope_digest);
    assert_ne!(
        live.entries[0].content_digest,
        expected.entries[0].content_digest
    );
}

#[test]
#[serial]
fn stopped_process_modification_and_deletion_are_changed_on_the_first_scan() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    for directory in ["modified", "deleted"] {
        write_skill(
            temp.path(),
            ManagedClientId::Claude,
            directory,
            "persisted body",
        );
    }
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[
            confirmed_skill(
                ManagedClientId::Claude,
                "deleted",
                ManagedClientApps::only(ManagedClientId::Claude),
            ),
            confirmed_skill(
                ManagedClientId::Claude,
                "modified",
                ManagedClientApps::only(ManagedClientId::Claude),
            ),
        ])
        .unwrap();

    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "modified",
        "external edit while stopped",
    );
    fs::remove_dir_all(skill_dir(temp.path(), ManagedClientId::Claude, "deleted")).unwrap();

    let (_source, parser, coordinator) = runtime(database);
    let scan_target = target(ManagedClientId::Claude);
    let LocalScanEvent::Changed { records, .. } = coordinator.rescan_target(scan_target) else {
        panic!("the persisted baseline must detect first-scan changes");
    };
    assert!(records.iter().any(|change| matches!(
        change,
        LocalScanRecordChange::Deleted { previous } if previous.record_id == "deleted"
    )));
    assert!(records.iter().any(|change| matches!(
        change,
        LocalScanRecordChange::Modified { current, .. } if current.record_id == "modified"
    )));
    assert_eq!(parser.calls(), [scan_target]);
    let pending = coordinator.pending_change(scan_target).unwrap();
    assert_eq!(
        pending
            .parsed_snapshot()
            .unwrap()
            .records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect::<Vec<_>>(),
        ["modified"]
    );
}

#[test]
#[serial]
fn disabled_known_external_addition_is_detected_but_unmanaged_trees_stay_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Codex,
        "known-disabled",
        "persisted elsewhere",
    );
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[confirmed_skill(
            ManagedClientId::Codex,
            "known-disabled",
            ManagedClientApps::only(ManagedClientId::Codex),
        )])
        .unwrap();

    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "known-disabled",
        "external addition",
    );
    let unknown = skill_dir(temp.path(), ManagedClientId::Claude, "unknown-bad");
    fs::create_dir_all(&unknown).unwrap();
    fs::write(unknown.join("broken"), b"no SKILL.md").unwrap();

    let (_source, parser, coordinator) = runtime(database);
    let scan_target = target(ManagedClientId::Claude);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { records, .. }
            if matches!(records.as_slice(), [LocalScanRecordChange::Added { current }]
                if current.record_id == "known-disabled")
    ));
    assert_eq!(parser.calls(), [scan_target]);
    assert_eq!(
        coordinator
            .pending_change(scan_target)
            .unwrap()
            .parsed_snapshot()
            .unwrap()
            .records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect::<Vec<_>>(),
        ["known-disabled"]
    );
}

#[test]
#[serial]
fn enabled_unconfirmed_record_forces_first_parse_even_when_live_is_absent() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[LocalSkill {
            id: "unconfirmed".to_string(),
            name: "Unconfirmed".to_string(),
            description: None,
            directory: "unconfirmed".to_string(),
            content_hash: None,
            total_size_bytes: 0,
            file_count: 0,
            apps: ManagedClientApps::only(ManagedClientId::Claude),
            cloud_eligible: false,
            created_at_ms: 1,
            updated_at_ms: 1,
        }])
        .unwrap();

    let (_source, parser, coordinator) = runtime(database);
    let scan_target = target(ManagedClientId::Claude);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls(), [scan_target]);
    assert!(coordinator.pending_change(scan_target).is_some());
    assert!(coordinator
        .pending_change(scan_target)
        .unwrap()
        .parsed_snapshot()
        .unwrap()
        .records
        .is_empty());
}

struct FlakyInventory {
    skills: Vec<LocalSkill>,
    fail: AtomicBool,
    refreshes: AtomicUsize,
}

impl ManagedSkillInventoryPort for FlakyInventory {
    fn list_managed_skills(
        &self,
        _client: ManagedClientId,
    ) -> Result<Vec<LocalSkill>, LocalScanReadFailure> {
        if self.fail.load(Ordering::SeqCst) {
            Err(LocalScanReadFailure {
                kind: LocalScanFailureKind::ReadFailed,
                record_id: None,
            })
        } else {
            Ok(self.skills.clone())
        }
    }

    fn refresh_managed_skills(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkill>, LocalScanReadFailure> {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.list_managed_skills(client)
    }
}

#[test]
#[serial]
fn inventory_failure_does_not_remember_current_and_the_next_scan_retries() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "confirmed body",
    );
    let inventory = Arc::new(FlakyInventory {
        skills: vec![confirmed_skill(
            ManagedClientId::Claude,
            "managed",
            ManagedClientApps::only(ManagedClientId::Claude),
        )],
        fail: AtomicBool::new(true),
        refreshes: AtomicUsize::new(0),
    });
    let source = Arc::new(DatabaseLocalScanSummaryAdapter::new(inventory.clone()));
    let parser = Arc::new(RecordingParser::new(inventory.clone()));
    let coordinator = LocalScanCoordinator::new(
        source,
        parser.clone(),
        Arc::new(LocalScanWriteTracker::default()),
    );
    let scan_target = target(ManagedClientId::Claude);

    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Failed {
            failure: wsl_code_switch_lib::domain::LocalScanFailure {
                kind: LocalScanFailureKind::ReadFailed,
                ..
            },
            ..
        }
    ));
    inventory.fail.store(false, Ordering::SeqCst);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert_eq!(inventory.refreshes.load(Ordering::SeqCst), 2);
    assert!(parser.calls().is_empty());
}

#[test]
#[serial]
fn inaccessible_fixed_home_is_failed_instead_of_an_empty_deletion_scope() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-home");
    let _home = TestHomeGuard::set(&missing);
    let database = Arc::new(Database::memory().unwrap());
    let (_source, parser, coordinator) = runtime(database);

    assert!(matches!(
        coordinator.rescan_target(target(ManagedClientId::Claude)),
        LocalScanEvent::Failed {
            failure: wsl_code_switch_lib::domain::LocalScanFailure {
                kind: LocalScanFailureKind::ReadFailed,
                ..
            },
            ..
        }
    ));
    assert!(parser.calls().is_empty());
}

#[test]
#[serial]
fn restart_clears_stale_pending_when_the_refreshed_database_matches_live() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "before refresh",
    );
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[confirmed_skill(
            ManagedClientId::Claude,
            "managed",
            ManagedClientApps::only(ManagedClientId::Claude),
        )])
        .unwrap();
    let (_source, parser, coordinator) = runtime(database.clone());
    let scan_target = target(ManagedClientId::Claude);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));

    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "managed",
        "external edit adopted by explicit refresh",
    );
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert!(coordinator.pending_change(scan_target).is_some());
    let adopted = confirmed_skill(
        ManagedClientId::Claude,
        "managed",
        ManagedClientApps::only(ManagedClientId::Claude),
    );
    database.save_core_skills(&[adopted]).unwrap();

    assert!(matches!(
        coordinator.restart_target_observation(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(coordinator.pending_change(scan_target).is_none());
    assert_eq!(parser.calls(), [scan_target]);
}

#[test]
#[serial]
fn restart_rebuilds_divergent_pending_against_the_database_canonical_hash() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    write_skill(temp.path(), ManagedClientId::Claude, "managed", "canonical");
    write_skill(temp.path(), ManagedClientId::Codex, "managed", "canonical");
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_core_skills(&[confirmed_skill(
            ManagedClientId::Claude,
            "managed",
            ManagedClientApps {
                claude: true,
                codex: true,
                opencode: false,
            },
        )])
        .unwrap();
    write_skill(
        temp.path(),
        ManagedClientId::Codex,
        "managed",
        "divergent copy",
    );
    let (_source, parser, coordinator) = runtime(database);

    assert!(matches!(
        coordinator.restart_target_observation(target(ManagedClientId::Claude)),
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(coordinator
        .pending_change(target(ManagedClientId::Claude))
        .is_none());
    assert!(matches!(
        coordinator.restart_target_observation(target(ManagedClientId::Codex)),
        LocalScanEvent::Changed { .. }
    ));
    assert!(coordinator
        .pending_change(target(ManagedClientId::Codex))
        .is_some());
    assert_eq!(parser.calls(), [target(ManagedClientId::Codex)]);
}

struct BlockingSummarySource {
    summary: Mutex<LocalScanSummary>,
    block_next: Mutex<bool>,
    entered: Condvar,
    release: Condvar,
    is_entered: Mutex<bool>,
}

impl BlockingSummarySource {
    fn new(summary: LocalScanSummary) -> Self {
        Self {
            summary: Mutex::new(summary),
            block_next: Mutex::new(false),
            entered: Condvar::new(),
            release: Condvar::new(),
            is_entered: Mutex::new(false),
        }
    }

    fn set(&self, summary: LocalScanSummary) {
        *self.summary.lock().unwrap() = summary;
    }

    fn block_next(&self) {
        *self.block_next.lock().unwrap() = true;
        *self.is_entered.lock().unwrap() = false;
    }

    fn wait_until_entered(&self) {
        let mut entered = self.is_entered.lock().unwrap();
        while !*entered {
            entered = self.entered.wait(entered).unwrap();
        }
    }

    fn release(&self) {
        *self.block_next.lock().unwrap() = false;
        self.release.notify_all();
    }
}

impl LocalScanSummaryPort for BlockingSummarySource {
    fn scan_summary(
        &self,
        _target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let captured = self.summary.lock().unwrap().clone();
        let mut blocked = self.block_next.lock().unwrap();
        if *blocked {
            *self.is_entered.lock().unwrap() = true;
            self.entered.notify_all();
            while *blocked {
                blocked = self.release.wait(blocked).unwrap();
            }
        }
        Ok(captured)
    }
}

#[test]
fn restart_waits_for_an_older_observation_then_rebuilds_the_final_state() {
    let scan_target = target(ManagedClientId::Claude);
    let baseline = LocalScanSummary::new(scan_target, "a".repeat(64), Vec::new()).unwrap();
    let stale = LocalScanSummary::new(scan_target, "b".repeat(64), Vec::new()).unwrap();
    let refreshed = LocalScanSummary::new(scan_target, "c".repeat(64), Vec::new()).unwrap();
    let source = Arc::new(BlockingSummarySource::new(baseline));
    let coordinator = Arc::new(LocalScanCoordinator::new(
        source.clone(),
        Arc::new(EmptyParser),
        Arc::new(LocalScanWriteTracker::default()),
    ));
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    source.set(stale);
    source.block_next();

    std::thread::scope(|scope| {
        let old = coordinator.clone();
        let old_scan = scope.spawn(move || old.rescan_target(scan_target));
        source.wait_until_entered();
        source.set(refreshed.clone());
        let restarting = coordinator.clone();
        let restart = scope.spawn(move || restarting.restart_target_observation(scan_target));
        source.release();

        assert!(matches!(
            old_scan.join().unwrap(),
            LocalScanEvent::Changed { .. }
        ));
        assert!(matches!(
            restart.join().unwrap(),
            LocalScanEvent::Unchanged { .. }
        ));
    });

    assert!(coordinator.pending_change(scan_target).is_none());
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { scope_digest, .. } if scope_digest == refreshed.scope_digest
    ));
}

struct SlowSummarySource {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl LocalScanSummaryPort for SlowSummarySource {
    fn scan_summary(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        self.active.fetch_sub(1, Ordering::SeqCst);
        LocalScanSummary::new(scan_target, "a".repeat(64), Vec::new()).map_err(|_| {
            LocalScanReadFailure {
                kind: LocalScanFailureKind::DigestFailed,
                record_id: None,
            }
        })
    }
}

struct EmptyParser;

impl LocalScanParserPort for EmptyParser {
    fn parse_changed(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        LocalScanParsedSnapshot::new(target, Vec::new()).map_err(|_| LocalScanReadFailure {
            kind: LocalScanFailureKind::ParseFailed,
            record_id: None,
        })
    }
}

#[test]
fn one_targets_observation_is_serialized_across_overlapping_rescans() {
    let source = Arc::new(SlowSummarySource {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    });
    let coordinator = Arc::new(LocalScanCoordinator::new(
        source.clone(),
        Arc::new(EmptyParser),
        Arc::new(LocalScanWriteTracker::default()),
    ));
    let scan_target = target(ManagedClientId::Claude);

    std::thread::scope(|scope| {
        for _ in 0..2 {
            let coordinator = coordinator.clone();
            scope.spawn(move || {
                coordinator.rescan_target(scan_target);
            });
        }
    });

    assert_eq!(source.max_active.load(Ordering::SeqCst), 1);
}
