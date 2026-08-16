use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serial_test::serial;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
#[cfg(windows)]
use wsl_code_switch_lib::adapters::local_scan_parser::FixedLocalScanParserAdapter;
use wsl_code_switch_lib::adapters::local_scan_summary::FixedLocalScanSummaryAdapter;
use wsl_code_switch_lib::domain::{
    LocalScanDomain, LocalScanEntrySummary, LocalScanEvent, LocalScanFailureKind, LocalScanSummary,
    LocalScanTarget, ManagedClientId,
};
use wsl_code_switch_lib::ports::{
    LocalScanParsedSnapshot, LocalScanParserPort, LocalScanReadFailure, LocalScanSummaryPort,
};
#[cfg(windows)]
use wsl_code_switch_lib::record_runtime_local_writes;
use wsl_code_switch_lib::{
    LocalScanCadence, LocalScanCoordinator, LocalScanExecutor, LocalScanScheduler,
    LocalScanWriteTracker,
};

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn target(domain: LocalScanDomain, client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget { domain, client_id }
}

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

#[test]
#[serial]
fn fixed_summary_adapter_scans_all_domains_without_writes_or_secret_leaks() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    let fixtures = [
        (
            ".claude/settings.json",
            br#"{"apiKey":"CLAUDE_SECRET"}"#.as_slice(),
        ),
        (
            ".claude.json",
            br#"{"mcpServers":{"x":{"token":"MCP_SECRET"}}}"#.as_slice(),
        ),
        (".claude/CLAUDE.md", b"CLAUDE PROMPT".as_slice()),
        (
            ".codex/auth.json",
            br#"{"OPENAI_API_KEY":"CODEX_SECRET"}"#.as_slice(),
        ),
        (
            ".codex/config.toml",
            b"model = \"fixture\"\n[mcp_servers.x]\ncommand = \"echo\"\n".as_slice(),
        ),
        (
            ".codex/cc-switch-model-catalog.json",
            br#"{"models":[]}"#.as_slice(),
        ),
        (".codex/AGENTS.md", b"CODEX PROMPT".as_slice()),
        (
            ".config/opencode/opencode.json",
            br#"{"provider":{"x":{"apiKey":"OPENCODE_SECRET"}},"mcp":{}}"#.as_slice(),
        ),
        (".config/opencode/AGENTS.md", b"OPENCODE PROMPT".as_slice()),
        (
            ".claude/skills/example/SKILL.md",
            b"---\nname: example\n---\nClaude".as_slice(),
        ),
        (
            ".codex/skills/example/SKILL.md",
            b"---\nname: example\n---\nCodex".as_slice(),
        ),
        (
            ".config/opencode/skills/example/SKILL.md",
            b"---\nname: example\n---\nOpenCode".as_slice(),
        ),
    ];
    for (relative, contents) in fixtures {
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    let before: HashMap<_, _> = fixtures
        .iter()
        .map(|(relative, _)| {
            (
                (*relative).to_string(),
                fs::read(temp.path().join(relative)).unwrap(),
            )
        })
        .collect();
    let adapter = FixedLocalScanSummaryAdapter::runtime();
    let mut summaries = Vec::new();
    for domain in LocalScanDomain::ALL {
        for client_id in ManagedClientId::ALL {
            let summary = adapter.scan_summary(target(domain, client_id)).unwrap();
            assert_eq!(summary.target, target(domain, client_id));
            assert!(!summary.entries.is_empty(), "{domain:?}/{client_id:?}");
            summaries.push(summary);
        }
    }

    assert_eq!(summaries.len(), 12);
    let encoded = serde_json::to_string(&summaries).unwrap();
    for secret in [
        "CLAUDE_SECRET",
        "MCP_SECRET",
        "CODEX_SECRET",
        "OPENCODE_SECRET",
    ] {
        assert!(!encoded.contains(secret));
    }
    for (relative, contents) in before {
        assert_eq!(fs::read(temp.path().join(relative)).unwrap(), contents);
    }
}

#[derive(Default)]
struct FakeSummarySource {
    generations: Mutex<HashMap<LocalScanTarget, u64>>,
    calls: Mutex<Vec<LocalScanTarget>>,
}

impl FakeSummarySource {
    fn advance(&self, target: LocalScanTarget) {
        *self.generations.lock().unwrap().entry(target).or_default() += 1;
    }
}

impl LocalScanSummaryPort for FakeSummarySource {
    fn scan_summary(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        self.calls.lock().unwrap().push(scan_target);
        if scan_target == target(LocalScanDomain::Prompt, ManagedClientId::Codex) {
            return Err(LocalScanReadFailure {
                kind: LocalScanFailureKind::PermissionDenied,
                record_id: Some("prompt-live".to_string()),
            });
        }
        let generation = *self
            .generations
            .lock()
            .unwrap()
            .get(&scan_target)
            .unwrap_or(&0);
        let record = LocalScanEntrySummary::new(
            "live",
            hash(&format!("record-{scan_target:?}-{generation}")),
            generation,
            None,
        )
        .unwrap();
        Ok(LocalScanSummary::new(
            scan_target,
            hash(&format!("scope-{scan_target:?}-{generation}")),
            vec![record],
        )
        .unwrap())
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
fn coordinator_scans_only_requested_domains_and_compares_observations() {
    let source = Arc::new(FakeSummarySource::default());
    let coordinator = LocalScanCoordinator::new(
        source.clone(),
        Arc::new(EmptyParser),
        Arc::new(LocalScanWriteTracker::default()),
    );

    let first = coordinator.scan_domains(&LocalScanDomain::ALL);
    assert_eq!(first.len(), 12);
    assert_eq!(source.calls.lock().unwrap().len(), 12);
    assert_eq!(
        first
            .iter()
            .filter(|event| matches!(event, LocalScanEvent::Failed { .. }))
            .count(),
        1
    );
    assert!(serde_json::to_string(&first)
        .unwrap()
        .contains("permission_denied"));

    source.calls.lock().unwrap().clear();
    let changed_target = target(LocalScanDomain::Provider, ManagedClientId::Claude);
    source.advance(changed_target);
    let provider_events = coordinator.scan_domains(&[LocalScanDomain::Provider]);
    assert_eq!(source.calls.lock().unwrap().len(), 3);
    assert!(provider_events.iter().any(
        |event| matches!(event, LocalScanEvent::Changed { target, .. } if *target == changed_target)
    ));
    assert!(provider_events.iter().all(|event| match event {
        LocalScanEvent::Unchanged { target, .. }
        | LocalScanEvent::Changed { target, .. }
        | LocalScanEvent::SelfWriteSuppressed { target, .. }
        | LocalScanEvent::Failed { target, .. } => target.domain == LocalScanDomain::Provider,
    }));
}

struct RecordingExecutor {
    calls: mpsc::UnboundedSender<Vec<LocalScanDomain>>,
}

impl LocalScanExecutor for RecordingExecutor {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent> {
        self.calls.send(domains.to_vec()).unwrap();
        Vec::new()
    }
}

async fn next_call(
    receiver: &mut mpsc::UnboundedReceiver<Vec<LocalScanDomain>>,
    timeout: Duration,
) -> Vec<LocalScanDomain> {
    tokio::time::timeout(timeout, receiver.recv())
        .await
        .expect("scheduled scan timed out")
        .expect("scheduler stopped unexpectedly")
}

#[tokio::test]
async fn scheduler_uses_exact_modes_immediate_triggers_and_cancellation() {
    let production = LocalScanCadence::production();
    assert_eq!(production.foreground, Duration::from_secs(5));
    assert_eq!(production.background, Duration::from_secs(30));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cadence = LocalScanCadence {
        foreground: Duration::from_millis(25),
        background: Duration::from_millis(100),
    };
    let (scheduler, worker) =
        LocalScanScheduler::new(Arc::new(RecordingExecutor { calls: tx }), cadence, false);
    let task = tokio::spawn(worker.run());

    assert_eq!(
        next_call(&mut rx, Duration::from_millis(200)).await,
        LocalScanDomain::ALL
    );
    assert_eq!(
        next_call(&mut rx, Duration::from_millis(100)).await,
        LocalScanDomain::ALL
    );

    scheduler.enter_page(LocalScanDomain::Skill).unwrap();
    assert_eq!(
        next_call(&mut rx, Duration::from_millis(100)).await,
        vec![LocalScanDomain::Skill]
    );

    scheduler.set_background().unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(50), rx.recv())
        .await
        .is_err());
    assert_eq!(
        next_call(&mut rx, Duration::from_millis(100)).await,
        LocalScanDomain::ALL
    );

    scheduler.window_restored().unwrap();
    assert_eq!(
        next_call(&mut rx, Duration::from_millis(100)).await,
        LocalScanDomain::ALL
    );

    scheduler.cancel().unwrap();
    tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .expect("scheduler cancellation timed out")
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(60), rx.recv())
            .await
            .expect("closed scheduler channel should resolve immediately"),
        None
    );
}

struct SlowExecutor {
    calls: mpsc::UnboundedSender<Vec<LocalScanDomain>>,
    scan_duration: Duration,
}

impl LocalScanExecutor for SlowExecutor {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent> {
        self.calls.send(domains.to_vec()).unwrap();
        std::thread::sleep(self.scan_duration);
        Vec::new()
    }
}

struct FlakyExecutor {
    calls: mpsc::UnboundedSender<Vec<LocalScanDomain>>,
    fail: std::sync::atomic::AtomicBool,
}

impl LocalScanExecutor for FlakyExecutor {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent> {
        self.calls.send(domains.to_vec()).unwrap();
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            vec![LocalScanEvent::failed(
                target(LocalScanDomain::Skill, ManagedClientId::Claude),
                LocalScanFailureKind::ReadFailed,
                None,
            )
            .expect("valid failure event")]
        } else {
            Vec::new()
        }
    }
}

#[tokio::test]
async fn slow_scans_rest_at_least_as_long_as_the_scan_itself() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cadence = LocalScanCadence {
        foreground: Duration::from_millis(25),
        background: Duration::from_millis(100),
    };
    let (scheduler, worker) = LocalScanScheduler::new(
        Arc::new(SlowExecutor {
            calls: tx,
            scan_duration: Duration::from_millis(100),
        }),
        cadence,
        false,
    );
    let task = tokio::spawn(worker.run());

    let _ = next_call(&mut rx, Duration::from_millis(200)).await;
    let second = next_call(&mut rx, Duration::from_millis(400)).await;
    let second_arrived = std::time::Instant::now();
    let _third = tokio::time::timeout(Duration::from_millis(400), rx.recv())
        .await
        .expect("rest must stay bounded");
    let elapsed = second_arrived.elapsed();
    // Without pacing, a slow scan is followed by the 25ms foreground tick
    // (~125ms gap). Pacing must rest for the 100ms scan duration (~200ms gap).
    assert!(
        elapsed >= Duration::from_millis(160),
        "scheduler chained scans after a slow cycle: {elapsed:?} after {second:?}"
    );

    scheduler.cancel().unwrap();
    tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("scheduler cancellation timed out")
        .unwrap();
}

#[tokio::test]
async fn failing_targets_back_off_to_background_until_a_clean_cycle() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let executor = Arc::new(FlakyExecutor {
        calls: tx,
        fail: std::sync::atomic::AtomicBool::new(true),
    });
    let cadence = LocalScanCadence {
        foreground: Duration::from_millis(25),
        background: Duration::from_millis(100),
    };
    let (scheduler, worker) = LocalScanScheduler::new(executor.clone(), cadence, false);
    let task = tokio::spawn(worker.run());

    // Initial scan fails -> degraded backoff.
    let _ = next_call(&mut rx, Duration::from_millis(200)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(60), rx.recv())
            .await
            .is_err(),
        "degraded scheduler must not tick at the foreground cadence"
    );
    let _ = next_call(&mut rx, Duration::from_millis(200)).await;
    // Let the in-flight failed scan finish reading the flag, then recover.
    tokio::time::sleep(Duration::from_millis(20)).await;
    executor
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = next_call(&mut rx, Duration::from_millis(200)).await;
    // Clean cycle restores the foreground cadence for the next tick.
    let _ = next_call(&mut rx, Duration::from_millis(90)).await;

    scheduler.cancel().unwrap();
    tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("scheduler cancellation timed out")
        .unwrap();
}

struct WedgedExecutor {
    calls: mpsc::UnboundedSender<Vec<LocalScanDomain>>,
    hang: Duration,
}

impl LocalScanExecutor for WedgedExecutor {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent> {
        let _ = self.calls.send(domains.to_vec());
        std::thread::sleep(self.hang);
        Vec::new()
    }
}

#[tokio::test]
async fn wedged_scan_cycles_are_cut_off_by_the_deadline() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cadence = LocalScanCadence {
        foreground: Duration::from_millis(25),
        background: Duration::from_millis(100),
    };
    // Deadline = background * 4 = 400ms; each scan hangs for 2s.
    let (scheduler, worker) = LocalScanScheduler::new(
        Arc::new(WedgedExecutor {
            calls: tx,
            hang: Duration::from_secs(2),
        }),
        cadence,
        false,
    );
    let task = tokio::spawn(worker.run());

    let _ = next_call(&mut rx, Duration::from_millis(200)).await;
    // Without the cycle deadline the worker would stay parked on the first
    // wedged read forever; with it, a fresh cycle starts within ~1.5s.
    let _ = next_call(&mut rx, Duration::from_millis(1500)).await;

    scheduler.cancel().unwrap();
    tokio::time::timeout(Duration::from_millis(500), task)
        .await
        .expect("scheduler cancellation timed out")
        .unwrap();
}

#[test]
fn production_wiring_owns_lifecycle_without_sync_or_write_dependencies() {
    let lib = include_str!("../src/lib.rs");
    let tray = include_str!("../src/tray.rs");
    let commands = include_str!("../src/commands/local_scan.rs");
    let scheduler = include_str!("../src/services/local_scan.rs");
    let adapter = include_str!("../src/adapters/local_scan_summary.rs");

    for required in [
        "LocalScanScheduler::new(",
        "LocalScanCadence::production()",
        "app.manage(services::LocalScanRuntimeState::new(local_scan_scheduler))",
        "tauri::async_runtime::spawn(local_scan_worker.run())",
        "commands::local_scan_enter_page",
        "scan.cancel()",
    ] {
        assert!(
            lib.contains(required),
            "missing lifecycle wiring: {required}"
        );
    }
    assert!(
        !lib.contains("prevent_close"),
        "closing the window must exit directly instead of hiding to the tray"
    );
    assert!(tray.contains("scan.window_restored()"));
    assert!(commands.contains("state.enter_page(domain)"));

    let production_scan = format!("{scheduler}\n{adapter}").to_ascii_lowercase();
    for forbidden in [
        "webdav",
        "reqwest",
        "rusqlite",
        "atomic_write(",
        "remove_file(",
    ] {
        assert!(
            !production_scan.contains(forbidden),
            "local scan gained a forbidden dependency: {forbidden}"
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires CC_SWITCH_WSL_TEST_DIR and CC_SWITCH_TEST_HOME on a real WSL2 UNC root"]
fn native_windows_scan_parse_and_self_write_tracking_use_real_wsl_unc() {
    let root = std::path::PathBuf::from(
        std::env::var_os("CC_SWITCH_WSL_TEST_DIR")
            .expect("CC_SWITCH_WSL_TEST_DIR must identify the isolated WSL fixture root"),
    );
    let home = std::path::PathBuf::from(
        std::env::var_os("CC_SWITCH_TEST_HOME")
            .expect("CC_SWITCH_TEST_HOME must identify the isolated WSL home"),
    );
    assert!(home.starts_with(&root));

    let fixtures = [
        (".claude/settings.json", b"{\"future\":true}".as_slice()),
        (
            ".claude.json",
            b"{\"mcpServers\":{\"native\":{\"command\":\"echo\"}}}".as_slice(),
        ),
        (".claude/CLAUDE.md", b"native prompt".as_slice()),
        (
            ".claude/skills/native/SKILL.md",
            b"---\nname: native\n---\nUNC".as_slice(),
        ),
    ];
    for (relative, contents) in fixtures {
        let path = home.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let before: HashMap<_, _> = fixtures
        .iter()
        .map(|(relative, _)| {
            (
                (*relative).to_string(),
                fs::read(home.join(relative)).unwrap(),
            )
        })
        .collect();

    let adapter = FixedLocalScanSummaryAdapter::runtime();
    let parser = FixedLocalScanParserAdapter::runtime();
    for domain in LocalScanDomain::ALL {
        let scan_target = target(domain, ManagedClientId::Claude);
        let summary = adapter.scan_summary(scan_target).unwrap();
        assert!(!summary.entries.is_empty(), "missing {domain:?} summary");
        let parsed = parser.parse_changed(scan_target).unwrap();
        assert!(
            !parsed.records.is_empty(),
            "missing {domain:?} parsed records"
        );
    }

    let prompt_target = target(LocalScanDomain::Prompt, ManagedClientId::Claude);
    let tracker = Arc::new(LocalScanWriteTracker::default());
    let coordinator = LocalScanCoordinator::new(
        Arc::new(FixedLocalScanSummaryAdapter::runtime()),
        Arc::new(FixedLocalScanParserAdapter::runtime()),
        tracker.clone(),
    );
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Prompt])[0],
        LocalScanEvent::Unchanged { .. }
    ));

    let prompt_path = home.join(".claude/CLAUDE.md");
    fs::write(&prompt_path, b"application write").unwrap();
    let registrations = record_runtime_local_writes(tracker.as_ref(), [prompt_target]);
    assert_eq!(registrations.len(), 1);
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Prompt])[0],
        LocalScanEvent::SelfWriteSuppressed {
            target,
            write_generation: 1,
            ..
        } if target == prompt_target
    ));

    fs::write(&prompt_path, b"third-party write").unwrap();
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Prompt])[0],
        LocalScanEvent::Changed { target, .. } if target == prompt_target
    ));
    assert!(coordinator.pending_change(prompt_target).is_some());

    for (relative, contents) in before {
        fs::write(home.join(&relative), &contents).unwrap();
        assert_eq!(fs::read(home.join(relative)).unwrap(), contents);
    }
}
