use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};
use wsl_code_switch_lib::domain::{
    LocalDifferenceKind, LocalScanDomain, LocalScanEntrySummary, LocalScanEvent,
    LocalScanFailureKind, LocalScanSummary, LocalScanTarget, ManagedClientId,
};
use wsl_code_switch_lib::ports::{
    LocalReconciliationState, LocalReconciliationStatePort, LocalScanParsedRecord,
    LocalScanParsedSnapshot, LocalScanParserPort, LocalScanReadFailure, LocalScanSummaryPort,
};
use wsl_code_switch_lib::{
    reconciliation_snapshot_from_parsed, record_local_writes, LocalScanCoordinator,
    LocalScanExecutor, LocalScanWriteTracker,
};

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn target(domain: LocalScanDomain, client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget { domain, client_id }
}

fn summary(scan_target: LocalScanTarget, generation: u64) -> LocalScanSummary {
    LocalScanSummary::new(
        scan_target,
        hash(&format!("scope-{scan_target:?}-{generation}")),
        vec![LocalScanEntrySummary::new(
            "live",
            hash(&format!("record-{scan_target:?}-{generation}")),
            generation,
            None,
        )
        .unwrap()],
    )
    .unwrap()
}

#[derive(Default)]
struct MutableSummarySource {
    summaries: Mutex<HashMap<LocalScanTarget, LocalScanSummary>>,
    expected: Mutex<HashMap<LocalScanTarget, LocalScanSummary>>,
}

impl MutableSummarySource {
    fn set(&self, value: LocalScanSummary) {
        self.summaries.lock().unwrap().insert(value.target, value);
    }

    fn set_expected(&self, value: LocalScanSummary) {
        self.expected.lock().unwrap().insert(value.target, value);
    }

    fn remove(&self, target: LocalScanTarget) {
        self.summaries.lock().unwrap().remove(&target);
    }
}

impl LocalScanSummaryPort for MutableSummarySource {
    fn scan_summary(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        self.summaries
            .lock()
            .unwrap()
            .get(&scan_target)
            .cloned()
            .ok_or(LocalScanReadFailure {
                kind: LocalScanFailureKind::NotFound,
                record_id: None,
            })
    }

    fn expected_after_write(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        if let Some(summary) = self.expected.lock().unwrap().get(&scan_target).cloned() {
            Ok(summary)
        } else {
            self.scan_summary(scan_target)
        }
    }
}

struct BlockingSummarySource {
    inner: Arc<MutableSummarySource>,
    reads: AtomicUsize,
    block_next: AtomicBool,
    block_state: Mutex<(bool, bool)>,
    block_changed: Condvar,
}

impl BlockingSummarySource {
    fn new(inner: Arc<MutableSummarySource>) -> Self {
        Self {
            inner,
            reads: AtomicUsize::new(0),
            block_next: AtomicBool::new(false),
            block_state: Mutex::new((false, false)),
            block_changed: Condvar::new(),
        }
    }

    fn block_next_read(&self) {
        *self.block_state.lock().unwrap() = (false, false);
        self.block_next.store(true, Ordering::Release);
    }

    fn wait_until_blocked(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut state = self.block_state.lock().unwrap();
        while !state.0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "summary read did not block in time");
            let (next, timeout) = self.block_changed.wait_timeout(state, remaining).unwrap();
            state = next;
            assert!(
                !timeout.timed_out() || state.0,
                "summary read did not block in time"
            );
        }
    }

    fn release(&self) {
        let mut state = self.block_state.lock().unwrap();
        state.1 = true;
        self.block_changed.notify_all();
    }
}

impl LocalScanSummaryPort for BlockingSummarySource {
    fn scan_summary(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        if self.block_next.swap(false, Ordering::AcqRel) {
            let mut state = self.block_state.lock().unwrap();
            state.0 = true;
            self.block_changed.notify_all();
            while !state.1 {
                state = self.block_changed.wait(state).unwrap();
            }
        }
        self.inner.scan_summary(scan_target)
    }

    fn expected_after_write(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        self.inner.expected_after_write(scan_target)
    }
}

#[derive(Default)]
struct RecordingParser {
    calls: Mutex<Vec<LocalScanTarget>>,
    failures_remaining: Mutex<usize>,
}

impl RecordingParser {
    fn fail_once(&self) {
        *self.failures_remaining.lock().unwrap() = 1;
    }
}

impl LocalScanParserPort for RecordingParser {
    fn parse_changed(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        self.calls.lock().unwrap().push(scan_target);
        let mut failures = self.failures_remaining.lock().unwrap();
        if *failures > 0 {
            *failures -= 1;
            return Err(LocalScanReadFailure {
                kind: LocalScanFailureKind::ParseFailed,
                record_id: Some("live".to_string()),
            });
        }
        LocalScanParsedSnapshot::new(
            scan_target,
            vec![LocalScanParsedRecord::new("live", json!({ "valid": true })).unwrap()],
        )
        .map_err(|_| LocalScanReadFailure {
            kind: LocalScanFailureKind::ParseFailed,
            record_id: None,
        })
    }
}

#[derive(Default)]
struct RecordingReconciliationStateSource {
    states: Mutex<HashMap<LocalScanTarget, LocalReconciliationState>>,
    reads: Mutex<Vec<LocalScanTarget>>,
}

impl RecordingReconciliationStateSource {
    fn set(&self, state: LocalReconciliationState) {
        self.states.lock().unwrap().insert(state.target, state);
    }
}

impl LocalReconciliationStatePort for RecordingReconciliationStateSource {
    fn read_reconciliation_state(
        &self,
        scan_target: LocalScanTarget,
    ) -> Result<LocalReconciliationState, LocalScanReadFailure> {
        self.reads.lock().unwrap().push(scan_target);
        self.states
            .lock()
            .unwrap()
            .get(&scan_target)
            .cloned()
            .ok_or(LocalScanReadFailure {
                kind: LocalScanFailureKind::ReadFailed,
                record_id: None,
            })
    }
}

fn coordinator(
    source: Arc<MutableSummarySource>,
    parser: Arc<RecordingParser>,
    writes: Arc<LocalScanWriteTracker>,
) -> LocalScanCoordinator {
    LocalScanCoordinator::new(source, parser, writes)
}

#[test]
fn post_commit_registration_expands_shared_files_deduplicates_and_fails_closed() {
    let provider_codex = target(LocalScanDomain::Provider, ManagedClientId::Codex);
    let mcp_codex = target(LocalScanDomain::Mcp, ManagedClientId::Codex);
    let prompt_claude = target(LocalScanDomain::Prompt, ManagedClientId::Claude);
    let source = MutableSummarySource::default();
    source.set(summary(provider_codex, 1));
    source.set(summary(mcp_codex, 1));
    source.set(summary(prompt_claude, 1));
    let writes = LocalScanWriteTracker::default();

    let registrations = record_local_writes(
        &writes,
        &source,
        [provider_codex, provider_codex, prompt_claude],
    );

    assert_eq!(registrations.len(), 3);
    assert_eq!(registrations[0].target, provider_codex);
    assert_eq!(registrations[0].write_generation, 1);
    assert_eq!(registrations[1].target, mcp_codex);
    assert_eq!(registrations[1].write_generation, 2);
    assert_eq!(registrations[2].target, prompt_claude);
    assert_eq!(registrations[2].write_generation, 3);
    assert_eq!(writes.pending_count(), 3);

    let missing_skill = target(LocalScanDomain::Skill, ManagedClientId::Opencode);
    let failed = record_local_writes(&writes, &source, [missing_skill]);
    assert!(failed.is_empty());
    assert_eq!(writes.pending_count(), 3);
    assert_eq!(writes.last_generation(), 3);
}

#[test]
fn write_registration_uses_projected_expected_state_not_an_immediate_live_rescan() {
    let scan_target = target(LocalScanDomain::Skill, ManagedClientId::Claude);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));

    let expected = summary(scan_target, 1);
    let third_party = summary(scan_target, 2);
    source.set_expected(expected);
    source.set(third_party);
    let registered = record_local_writes(&writes, source.as_ref(), [scan_target]);
    assert_eq!(registered.len(), 1);

    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[scan_target]);
    assert!(coordinator.pending_change(scan_target).is_some());
    assert_eq!(writes.pending_count(), 0);
}

#[test]
fn late_matching_write_expectation_clears_stale_pending_then_detects_third_party_change() {
    let scan_target = target(LocalScanDomain::Skill, ManagedClientId::Claude);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    coordinator.rescan_target(scan_target);

    let written = summary(scan_target, 1);
    source.set(written.clone());
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert!(coordinator.pending_change(scan_target).is_some());

    source.set_expected(written);
    let registered = record_local_writes(&writes, source.as_ref(), [scan_target]);
    assert_eq!(registered.len(), 1);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::SelfWriteSuppressed {
            write_generation: 1,
            ..
        }
    ));
    assert!(coordinator.pending_change(scan_target).is_none());
    assert_eq!(writes.pending_count(), 0);

    source.set(summary(scan_target, 2));
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert!(coordinator.pending_change(scan_target).is_some());
    assert_eq!(
        parser.calls.lock().unwrap().as_slice(),
        &[scan_target, scan_target]
    );
}

#[test]
fn authoritative_restart_discards_a_stale_write_expectation() {
    let scan_target = target(LocalScanDomain::Skill, ManagedClientId::Claude);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    coordinator.rescan_target(scan_target);

    let stale_expected = summary(scan_target, 1);
    assert_eq!(writes.record_expected(&stale_expected).unwrap(), 1);
    source.set(summary(scan_target, 2));
    assert!(matches!(
        coordinator.restart_target_observation(scan_target),
        LocalScanEvent::Unchanged { .. }
    ));
    assert_eq!(writes.pending_count(), 0);

    source.set(stale_expected);
    assert!(matches!(
        coordinator.rescan_target(scan_target),
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[scan_target]);
}

#[test]
fn busy_authoritative_restart_is_queued_without_waiting_for_the_active_read() {
    let scan_target = target(LocalScanDomain::Skill, ManagedClientId::Codex);
    let inner = Arc::new(MutableSummarySource::default());
    inner.set(summary(scan_target, 0));
    let source = Arc::new(BlockingSummarySource::new(inner.clone()));
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    let coordinator = Arc::new(LocalScanCoordinator::new(
        source.clone(),
        parser.clone(),
        writes,
    ));
    coordinator.rescan_target(scan_target);

    inner.set(summary(scan_target, 1));
    source.block_next_read();
    let active = {
        let coordinator = coordinator.clone();
        std::thread::spawn(move || coordinator.rescan_target(scan_target))
    };
    source.wait_until_blocked();

    let started = Instant::now();
    assert!(matches!(
        coordinator.restart_target_observation(scan_target),
        LocalScanEvent::Failed {
            failure: wsl_code_switch_lib::domain::LocalScanFailure {
                kind: LocalScanFailureKind::ReadFailed,
                ..
            },
            ..
        }
    ));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "restart must not wait behind an active UNC read"
    );

    source.release();
    assert!(matches!(
        active.join().unwrap(),
        LocalScanEvent::Unchanged { .. }
    ));
    assert_eq!(source.reads.load(Ordering::Acquire), 3);
    assert!(coordinator.pending_change(scan_target).is_none());
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[scan_target]);
}

#[test]
fn unchanged_summaries_never_parse_and_changed_summaries_parse_once() {
    let scan_target = target(LocalScanDomain::Provider, ManagedClientId::Claude);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes);

    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Provider])[0],
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(parser.calls.lock().unwrap().is_empty());

    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Provider])[0],
        LocalScanEvent::Unchanged { .. }
    ));
    assert!(parser.calls.lock().unwrap().is_empty());

    source.set(summary(scan_target, 1));
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Provider])[0],
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[scan_target]);
}

#[test]
fn expected_write_suppresses_only_one_matching_target_and_generation() {
    let claude = target(LocalScanDomain::Prompt, ManagedClientId::Claude);
    let codex = target(LocalScanDomain::Prompt, ManagedClientId::Codex);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(claude, 0));
    source.set(summary(codex, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    let baseline = coordinator.scan_domains(&[LocalScanDomain::Prompt]);
    assert_eq!(baseline.len(), 3);

    let expected = summary(claude, 1);
    let generation = writes.record_expected(&expected).unwrap();
    assert_eq!(generation, 1);
    source.set(expected.clone());
    source.set(summary(codex, 1));

    let events = coordinator.scan_domains(&[LocalScanDomain::Prompt]);
    assert!(events.iter().any(|event| matches!(
        event,
        LocalScanEvent::SelfWriteSuppressed {
            target,
            scope_digest,
            write_generation: 1,
        } if *target == claude && scope_digest == &expected.scope_digest
    )));
    assert!(events
        .iter()
        .any(|event| matches!(event, LocalScanEvent::Changed { target, .. } if *target == codex)));
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[codex]);

    let repeated = coordinator.scan_domains(&[LocalScanDomain::Prompt]);
    assert!(repeated
        .iter()
        .all(|event| !matches!(event, LocalScanEvent::SelfWriteSuppressed { .. })));
    assert_eq!(parser.calls.lock().unwrap().as_slice(), &[codex]);
}

#[test]
fn third_party_change_after_or_instead_of_expected_write_is_never_hidden() {
    let scan_target = target(LocalScanDomain::Mcp, ManagedClientId::Opencode);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    let _ = coordinator.scan_domains(&[LocalScanDomain::Mcp]);

    let expected = summary(scan_target, 1);
    assert_eq!(writes.record_expected(&expected).unwrap(), 1);
    source.set(summary(scan_target, 2));
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Mcp])[2],
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().len(), 1);

    source.set(expected);
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Mcp])[2],
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().len(), 2);
}

#[test]
fn parse_failure_keeps_last_good_summary_and_retries_without_writing() {
    let scan_target = target(LocalScanDomain::Skill, ManagedClientId::Codex);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser.clone(), writes.clone());
    let _ = coordinator.scan_domains(&[LocalScanDomain::Skill]);

    parser.fail_once();
    source.set(summary(scan_target, 1));
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Skill])[1],
        LocalScanEvent::Failed {
            failure: wsl_code_switch_lib::domain::LocalScanFailure {
                kind: LocalScanFailureKind::ParseFailed,
                ..
            },
            ..
        }
    ));
    assert_eq!(parser.calls.lock().unwrap().len(), 1);
    assert_eq!(writes.pending_count(), 0);

    let local = reconciliation_snapshot_from_parsed(
        &LocalScanParsedSnapshot::new(
            scan_target,
            vec![LocalScanParsedRecord::new("live", json!({ "valid": false })).unwrap()],
        )
        .unwrap(),
    )
    .unwrap();
    let batch = coordinator
        .classify_pending(scan_target, Some(local.clone()), local)
        .unwrap()
        .expect("parse failure remains available for conflict preview");
    assert!(batch.differences.is_empty());
    assert_eq!(batch.conflicts.len(), 1);
    assert_eq!(
        batch.conflicts[0].kind,
        wsl_code_switch_lib::domain::LocalConflictKind::ParseFailed
    );
    assert!(coordinator.pending_change(scan_target).is_some());

    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Skill])[1],
        LocalScanEvent::Changed { .. }
    ));
    assert_eq!(parser.calls.lock().unwrap().len(), 2);
}

#[test]
fn transient_read_failure_hides_but_does_not_discard_last_parsed_change() {
    let scan_target = target(LocalScanDomain::Mcp, ManagedClientId::Claude);
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    source.set(summary(scan_target, 0));
    let coordinator = coordinator(source.clone(), parser, writes);
    let _ = coordinator.scan_domains(&[LocalScanDomain::Mcp]);

    let local = reconciliation_snapshot_from_parsed(
        &LocalScanParsedSnapshot::new(
            scan_target,
            vec![LocalScanParsedRecord::new("live", json!({ "valid": false })).unwrap()],
        )
        .unwrap(),
    )
    .unwrap();
    source.set(summary(scan_target, 1));
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Mcp])[0],
        LocalScanEvent::Changed { .. }
    ));
    let before_failure = coordinator
        .classify_pending(scan_target, Some(local.clone()), local.clone())
        .unwrap()
        .unwrap();
    assert_eq!(before_failure.differences.len(), 1);

    source.remove(scan_target);
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Mcp])[0],
        LocalScanEvent::Failed { .. }
    ));
    let during_failure = coordinator
        .classify_pending(scan_target, Some(local.clone()), local.clone())
        .unwrap()
        .unwrap();
    assert!(during_failure.differences.is_empty());
    assert_eq!(
        during_failure.conflicts[0].kind,
        wsl_code_switch_lib::domain::LocalConflictKind::IntegrityMismatch
    );

    source.set(summary(scan_target, 1));
    assert!(matches!(
        coordinator.scan_domains(&[LocalScanDomain::Mcp])[0],
        LocalScanEvent::Unchanged { .. }
    ));
    let recovered = coordinator
        .classify_pending(scan_target, Some(local.clone()), local)
        .unwrap()
        .unwrap();
    assert_eq!(recovered, before_failure);
}

#[test]
fn all_domains_use_read_only_state_port_without_consuming_pending_changes() {
    let source = Arc::new(MutableSummarySource::default());
    let parser = Arc::new(RecordingParser::default());
    let writes = Arc::new(LocalScanWriteTracker::default());
    let states = RecordingReconciliationStateSource::default();
    let mut targets = Vec::new();
    for domain in LocalScanDomain::ALL {
        for client_id in ManagedClientId::ALL {
            let scan_target = target(domain, client_id);
            targets.push(scan_target);
            source.set(summary(scan_target, 0));
            let local = reconciliation_snapshot_from_parsed(
                &LocalScanParsedSnapshot::new(
                    scan_target,
                    vec![LocalScanParsedRecord::new("live", json!({ "valid": false })).unwrap()],
                )
                .unwrap(),
            )
            .unwrap();
            states.set(
                LocalReconciliationState::new(scan_target, Some(local.clone()), local).unwrap(),
            );
        }
    }
    let coordinator = coordinator(source.clone(), parser, writes.clone());
    assert_eq!(coordinator.scan_domains(&LocalScanDomain::ALL).len(), 12);

    for scan_target in &targets {
        source.set(summary(*scan_target, 1));
    }
    let changed = coordinator.scan_domains(&LocalScanDomain::ALL);
    assert_eq!(changed.len(), 12);
    assert!(changed
        .iter()
        .all(|event| matches!(event, LocalScanEvent::Changed { .. })));

    for scan_target in &targets {
        let batch = coordinator
            .classify_pending_from(&states, *scan_target)
            .unwrap()
            .unwrap();
        assert_eq!(batch.target, *scan_target);
        assert_eq!(batch.differences.len(), 1);
        assert_eq!(batch.differences[0].record_id, "live");
        assert_eq!(batch.differences[0].kind, LocalDifferenceKind::Modified);
        assert!(batch.conflicts.is_empty());
        assert!(coordinator.pending_change(*scan_target).is_some());
    }
    assert_eq!(states.reads.lock().unwrap().as_slice(), targets.as_slice());
    assert_eq!(writes.pending_count(), 0);
}
