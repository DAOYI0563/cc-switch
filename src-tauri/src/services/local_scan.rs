use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::adapters::local_scan_summary::{
    DatabaseLocalScanSummaryAdapter, FixedLocalScanSummaryAdapter,
};
use crate::database::Database;
use crate::domain::{
    classify_local_reconciliation, compare_local_scan_summaries, DomainError, DomainErrorCode,
    LocalReconciliationBatch, LocalReconciliationExternal, LocalReconciliationInput,
    LocalReconciliationRecord, LocalReconciliationSnapshot, LocalScanDomain, LocalScanEvent,
    LocalScanFailureKind, LocalScanRecordChange, LocalScanSummary, LocalScanTarget,
    ManagedClientId,
};
use crate::ports::{
    LocalReconciliationStatePort, LocalScanParsedSnapshot, LocalScanParserPort,
    LocalScanSummaryPort,
};

pub trait LocalScanExecutor: Send + Sync + 'static {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalScanParsedChange {
    pub event: LocalScanEvent,
    pub snapshot: LocalScanParsedSnapshot,
}

impl LocalScanParsedChange {
    pub fn classify_against(
        &self,
        baseline: Option<LocalReconciliationSnapshot>,
        local: LocalReconciliationSnapshot,
    ) -> Result<LocalReconciliationBatch, DomainError> {
        let target = match &self.event {
            LocalScanEvent::Changed { target, .. } => *target,
            _ => {
                return Err(DomainError::new(
                    DomainErrorCode::InvalidRecord,
                    "only a changed scan event can be classified",
                ))
            }
        };
        if self.snapshot.target != target {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "parsed change target does not match its scan event",
            ));
        }
        let external = reconciliation_snapshot_from_parsed(&self.snapshot)?;
        classify_local_reconciliation(LocalReconciliationInput {
            target,
            baseline,
            local,
            external: LocalReconciliationExternal::Parsed {
                snapshot: external,
                scope_changed: true,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalScanPendingChange {
    Parsed(LocalScanParsedChange),
    Failed(LocalScanEvent),
}

impl LocalScanPendingChange {
    pub fn event(&self) -> &LocalScanEvent {
        match self {
            Self::Parsed(change) => &change.event,
            Self::Failed(event) => event,
        }
    }

    pub fn parsed_snapshot(&self) -> Option<&LocalScanParsedSnapshot> {
        match self {
            Self::Parsed(change) => Some(&change.snapshot),
            Self::Failed(_) => None,
        }
    }

    pub fn classify_against(
        &self,
        baseline: Option<LocalReconciliationSnapshot>,
        local: LocalReconciliationSnapshot,
    ) -> Result<LocalReconciliationBatch, DomainError> {
        match self {
            Self::Parsed(change) => change.classify_against(baseline, local),
            Self::Failed(LocalScanEvent::Failed { target, failure }) => {
                classify_local_reconciliation(LocalReconciliationInput {
                    target: *target,
                    baseline,
                    local,
                    external: LocalReconciliationExternal::Failed {
                        failure: failure.clone(),
                    },
                })
            }
            Self::Failed(_) => Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "a failed pending scan must contain a failed event",
            )),
        }
    }
}

/// Converts sensitive parsed values to stable digests before they cross into
/// the serializable reconciliation contract.
pub fn reconciliation_snapshot_from_parsed(
    snapshot: &LocalScanParsedSnapshot,
) -> Result<LocalReconciliationSnapshot, DomainError> {
    let records = snapshot
        .records
        .iter()
        .map(|record| {
            let canonical = canonical_json(&record.value);
            let encoded = serde_json::to_vec(&canonical).map_err(|_| {
                DomainError::new(
                    DomainErrorCode::InvalidRecord,
                    "failed to canonicalize parsed local record",
                )
            })?;
            LocalReconciliationRecord::new(
                record.record_id.clone(),
                format!("{:x}", Sha256::digest(encoded)),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    LocalReconciliationSnapshot::new(snapshot.target, records)
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

#[derive(Debug, Clone)]
struct ExpectedLocalWrite {
    generation: u64,
    summary: LocalScanSummary,
}

#[derive(Debug, Default)]
struct LocalScanWriteState {
    last_generation: u64,
    expected: HashMap<LocalScanTarget, ExpectedLocalWrite>,
}

/// Tracks post-commit live summaries in memory. A matching expectation is
/// consumed once; any different changed summary clears the stale expectation.
#[derive(Debug, Default)]
pub struct LocalScanWriteTracker {
    state: Mutex<LocalScanWriteState>,
}

impl LocalScanWriteTracker {
    pub fn record_expected(&self, summary: &LocalScanSummary) -> Result<u64, DomainError> {
        summary.validate()?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = state.last_generation.checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorCode::InvalidRecord,
                "local scan write generation overflow",
            )
        })?;
        state.last_generation = generation;
        state.expected.insert(
            summary.target,
            ExpectedLocalWrite {
                generation,
                summary: summary.clone(),
            },
        );
        Ok(generation)
    }

    pub fn pending_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .expected
            .len()
    }

    pub fn last_generation(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .last_generation
    }

    fn consume_matching_unchanged(&self, summary: &LocalScanSummary) -> Option<u64> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = state
            .expected
            .get(&summary.target)
            .filter(|expected| expected.summary.scope_digest == summary.scope_digest)
            .map(|expected| expected.generation)?;
        state.expected.remove(&summary.target);
        Some(generation)
    }

    fn resolve_changed(&self, summary: &LocalScanSummary) -> Option<u64> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let expected = state.expected.remove(&summary.target)?;
        (expected.summary.scope_digest == summary.scope_digest).then_some(expected.generation)
    }

    fn discard_through(&self, target: LocalScanTarget, generation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state
            .expected
            .get(&target)
            .is_some_and(|expected| expected.generation <= generation)
        {
            state.expected.remove(&target);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalScanWriteRegistration {
    pub target: LocalScanTarget,
    pub write_generation: u64,
}

/// Records post-commit summaries without making an already committed operation
/// fail. Codex and OpenCode Provider/MCP targets are coupled because each pair
/// shares one physical live configuration file.
pub fn record_local_writes(
    tracker: &LocalScanWriteTracker,
    source: &dyn LocalScanSummaryPort,
    targets: impl IntoIterator<Item = LocalScanTarget>,
) -> Vec<LocalScanWriteRegistration> {
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        push_unique_target(&mut expanded, &mut seen, target);
        if target.client_id != ManagedClientId::Claude {
            let coupled_domain = match target.domain {
                LocalScanDomain::Provider => Some(LocalScanDomain::Mcp),
                LocalScanDomain::Mcp => Some(LocalScanDomain::Provider),
                LocalScanDomain::Prompt | LocalScanDomain::Skill => None,
            };
            if let Some(domain) = coupled_domain {
                push_unique_target(
                    &mut expanded,
                    &mut seen,
                    LocalScanTarget {
                        domain,
                        client_id: target.client_id,
                    },
                );
            }
        }
    }

    let mut registrations = Vec::with_capacity(expanded.len());
    for target in expanded {
        let summary = match source.expected_after_write(target) {
            Ok(summary) if summary.target == target => summary,
            Ok(_) => {
                log::warn!(
                    "本地写入摘要登记失败: target={}/{}, kind=target_mismatch",
                    target.domain.as_str(),
                    target.client_id.as_str()
                );
                continue;
            }
            Err(failure) => {
                log::warn!(
                    "本地写入摘要登记失败: target={}/{}, kind={:?}",
                    target.domain.as_str(),
                    target.client_id.as_str(),
                    failure.kind
                );
                continue;
            }
        };
        match tracker.record_expected(&summary) {
            Ok(write_generation) => registrations.push(LocalScanWriteRegistration {
                target,
                write_generation,
            }),
            Err(_) => log::warn!(
                "本地写入摘要登记失败: target={}/{}, kind=invalid_summary",
                target.domain.as_str(),
                target.client_id.as_str()
            ),
        }
    }
    registrations
}

pub fn record_runtime_local_writes(
    tracker: &LocalScanWriteTracker,
    targets: impl IntoIterator<Item = LocalScanTarget>,
) -> Vec<LocalScanWriteRegistration> {
    record_local_writes(tracker, &FixedLocalScanSummaryAdapter::runtime(), targets)
}

/// DB-aware post-write registration for callers that may include Skill targets.
/// Existing non-Skill callers can keep the fixed helper; Skill and synchronized
/// local-apply paths should pass their application database here.
pub fn record_database_local_writes(
    tracker: &LocalScanWriteTracker,
    database: Arc<Database>,
    targets: impl IntoIterator<Item = LocalScanTarget>,
) -> Vec<LocalScanWriteRegistration> {
    let (source, _) = DatabaseLocalScanSummaryAdapter::runtime(database);
    record_local_writes(tracker, &source, targets)
}

fn push_unique_target(
    targets: &mut Vec<LocalScanTarget>,
    seen: &mut HashSet<LocalScanTarget>,
    target: LocalScanTarget,
) {
    if seen.insert(target) {
        targets.push(target);
    }
}

/// Serializes summary observations and compares each target with its last read.
pub struct LocalScanCoordinator {
    source: Arc<dyn LocalScanSummaryPort>,
    parser: Arc<dyn LocalScanParserPort>,
    writes: Arc<LocalScanWriteTracker>,
    observe_gates: Mutex<HashMap<LocalScanTarget, Arc<Mutex<()>>>>,
    restart_requested: Mutex<HashSet<LocalScanTarget>>,
    previous: Mutex<HashMap<LocalScanTarget, LocalScanSummary>>,
    pending: Mutex<HashMap<LocalScanTarget, LocalScanParsedChange>>,
    pending_failures: Mutex<HashMap<LocalScanTarget, LocalScanEvent>>,
}

impl LocalScanCoordinator {
    pub fn new(
        source: Arc<dyn LocalScanSummaryPort>,
        parser: Arc<dyn LocalScanParserPort>,
        writes: Arc<LocalScanWriteTracker>,
    ) -> Self {
        Self {
            source,
            parser,
            writes,
            observe_gates: Mutex::new(HashMap::new()),
            restart_requested: Mutex::new(HashSet::new()),
            previous: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            pending_failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn pending_change(&self, target: LocalScanTarget) -> Option<LocalScanPendingChange> {
        if let Some(failure) = self
            .pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&target)
            .cloned()
        {
            return Some(LocalScanPendingChange::Failed(failure));
        }
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&target)
            .cloned()
            .map(LocalScanPendingChange::Parsed)
    }

    pub fn take_pending_change(&self, target: LocalScanTarget) -> Option<LocalScanPendingChange> {
        let failure = self
            .pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
        let parsed = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
        failure
            .map(LocalScanPendingChange::Failed)
            .or_else(|| parsed.map(LocalScanPendingChange::Parsed))
    }

    pub fn classify_pending(
        &self,
        target: LocalScanTarget,
        baseline: Option<LocalReconciliationSnapshot>,
        local: LocalReconciliationSnapshot,
    ) -> Result<Option<LocalReconciliationBatch>, DomainError> {
        self.pending_change(target)
            .map(|change| change.classify_against(baseline, local))
            .transpose()
    }

    pub fn classify_pending_from(
        &self,
        states: &dyn LocalReconciliationStatePort,
        target: LocalScanTarget,
    ) -> Result<Option<LocalReconciliationBatch>, DomainError> {
        let Some(change) = self.pending_change(target) else {
            return Ok(None);
        };
        let state = states
            .read_reconciliation_state(target)
            .map_err(|failure| {
                DomainError::new(
                    DomainErrorCode::InvalidRecord,
                    "failed to read local reconciliation state",
                )
                .with_context("failureKind", format!("{:?}", failure.kind))
            })?;
        if state.target != target {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "local reconciliation state source returned the wrong target",
            ));
        }
        change
            .classify_against(state.baseline, state.local)
            .map(Some)
    }

    pub fn rescan_target(&self, target: LocalScanTarget) -> LocalScanEvent {
        self.observe(target)
    }

    /// Rebuild one target from its persisted database baseline after an explicit
    /// authoritative refresh. A busy target is reset and queued instead of
    /// waiting behind a potentially wedged UNC read. The active observation
    /// consumes that queue before releasing its gate.
    pub fn restart_target_observation(&self, target: LocalScanTarget) -> LocalScanEvent {
        let cutoff = self.writes.last_generation();
        self.writes.discard_through(target, cutoff);
        self.restart_requested
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target);

        let gate = self.observation_gate(target);
        let outcome = match gate.try_lock() {
            Ok(observation) => self.observe_with_queued_restarts(target, observation),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                self.observe_with_queued_restarts(target, poisoned.into_inner())
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                failed_event(target, LocalScanFailureKind::ReadFailed, None)
            }
        };
        outcome
    }

    fn reset_target_observation(&self, target: LocalScanTarget) {
        self.previous
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
        self.pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
    }

    fn observation_gate(&self, target: LocalScanTarget) -> Arc<Mutex<()>> {
        self.observe_gates
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(target)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn observe(&self, target: LocalScanTarget) -> LocalScanEvent {
        let gate = self.observation_gate(target);
        let observation = gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.observe_with_queued_restarts(target, observation)
    }

    fn observe_with_queued_restarts<'a>(
        &self,
        target: LocalScanTarget,
        observation: MutexGuard<'a, ()>,
    ) -> LocalScanEvent {
        let mut event = None;
        loop {
            let mut restarts = self
                .restart_requested
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if restarts.remove(&target) {
                drop(restarts);
                self.reset_target_observation(target);
                event = Some(self.observe_locked(target));
                continue;
            }
            if let Some(event) = event {
                drop(observation);
                drop(restarts);
                return event;
            }
            drop(restarts);
            event = Some(self.observe_locked(target));
        }
    }

    fn observe_locked(&self, target: LocalScanTarget) -> LocalScanEvent {
        let prior = self
            .previous
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&target)
            .cloned();
        let first = if prior.is_none() {
            match self.source.scan_first_observation(target) {
                Ok(first) => Some(first),
                Err(failure) => {
                    let event = failed_event(target, failure.kind, failure.record_id);
                    self.remember_failed(event.clone());
                    return event;
                }
            }
        } else {
            None
        };
        let current = match first.as_ref() {
            Some(first) => first.current.clone(),
            None => match self.source.scan_summary(target) {
                Ok(summary) => summary,
                Err(failure) => {
                    let event = failed_event(target, failure.kind, failure.record_id);
                    self.remember_failed(event.clone());
                    return event;
                }
            },
        };

        let event = match prior.as_ref().or_else(|| first.as_ref()?.baseline.as_ref()) {
            Some(prior) => compare_local_scan_summaries(prior, &current)
                .unwrap_or_else(|_| failed_event(target, LocalScanFailureKind::DigestFailed, None)),
            None => LocalScanEvent::Unchanged {
                target,
                scope_digest: current.scope_digest.clone(),
            },
        };
        let must_parse_first = first.as_ref().is_some_and(|first| first.requires_parse);
        let event = if must_parse_first && matches!(event, LocalScanEvent::Unchanged { .. }) {
            forced_changed_event(&current)
        } else {
            event
        };

        match event {
            LocalScanEvent::Unchanged { .. } => {
                if let Some(write_generation) = self.writes.consume_matching_unchanged(&current) {
                    self.pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&target);
                    self.pending_failures
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&target);
                    self.remember(current.clone());
                    LocalScanEvent::SelfWriteSuppressed {
                        target,
                        scope_digest: current.scope_digest,
                        write_generation,
                    }
                } else {
                    self.clear_recovered_failure(target);
                    self.remember(current);
                    event
                }
            }
            LocalScanEvent::Changed { .. } => {
                self.pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&target);
                self.pending_failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&target);
                if let Some(write_generation) = self.writes.resolve_changed(&current) {
                    self.remember(current.clone());
                    return LocalScanEvent::SelfWriteSuppressed {
                        target,
                        scope_digest: current.scope_digest,
                        write_generation,
                    };
                }

                let snapshot = match self.parser.parse_changed(target) {
                    Ok(snapshot) if snapshot.target == target => snapshot,
                    Ok(_) => {
                        let failed = failed_event(target, LocalScanFailureKind::ParseFailed, None);
                        self.remember_failed(failed.clone());
                        return failed;
                    }
                    Err(failure) => {
                        let failed = failed_event(target, failure.kind, failure.record_id);
                        self.remember_failed(failed.clone());
                        return failed;
                    }
                };
                self.pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        target,
                        LocalScanParsedChange {
                            event: event.clone(),
                            snapshot,
                        },
                    );
                self.remember(current);
                event
            }
            LocalScanEvent::SelfWriteSuppressed { .. } => {
                unreachable!("summary comparison never emits suppression")
            }
            LocalScanEvent::Failed { .. } => {
                self.remember_failed(event.clone());
                event
            }
        }
    }

    fn remember_failed(&self, event: LocalScanEvent) {
        let LocalScanEvent::Failed { target, .. } = event else {
            return;
        };
        self.pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target, event);
    }

    fn clear_recovered_failure(&self, target: LocalScanTarget) {
        self.pending_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&target);
    }

    fn remember(&self, summary: LocalScanSummary) {
        self.previous
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(summary.target, summary);
    }
}

fn forced_changed_event(current: &LocalScanSummary) -> LocalScanEvent {
    LocalScanEvent::Changed {
        target: current.target,
        previous_scope_digest: "0".repeat(64),
        current_scope_digest: current.scope_digest.clone(),
        records: current
            .entries
            .iter()
            .cloned()
            .map(|current| LocalScanRecordChange::Added { current })
            .collect(),
    }
}

fn failed_event(
    target: LocalScanTarget,
    kind: LocalScanFailureKind,
    record_id: Option<String>,
) -> LocalScanEvent {
    LocalScanEvent::failed(target, kind, record_id)
        .or_else(|_| LocalScanEvent::failed(target, kind, None))
        .expect("a failure without a record id is always valid")
}

impl LocalScanExecutor for LocalScanCoordinator {
    fn scan_domains(&self, domains: &[LocalScanDomain]) -> Vec<LocalScanEvent> {
        let mut events = Vec::with_capacity(domains.len() * ManagedClientId::ALL.len());
        for domain in domains {
            for client_id in ManagedClientId::ALL {
                events.push(self.observe(LocalScanTarget {
                    domain: *domain,
                    client_id,
                }));
            }
        }
        events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalScanCadence {
    pub foreground: Duration,
    pub background: Duration,
}

impl LocalScanCadence {
    pub const fn production() -> Self {
        Self {
            foreground: Duration::from_secs(5),
            background: Duration::from_secs(30),
        }
    }

    fn validate(self) -> Result<Self, LocalScanSchedulerError> {
        if self.foreground.is_zero() || self.background.is_zero() {
            return Err(LocalScanSchedulerError::InvalidCadence);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalScanSchedulerError {
    InvalidCadence,
    Stopped,
}

impl std::fmt::Display for LocalScanSchedulerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidCadence => "local scan cadence must be greater than zero",
            Self::Stopped => "local scan scheduler has stopped",
        })
    }
}

impl std::error::Error for LocalScanSchedulerError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalScanMode {
    Foreground,
    Background,
}

enum LocalScanCommand {
    EnterPage(LocalScanDomain),
    WindowRestored,
    SetBackground,
    Cancel,
}

/// Non-blocking command handle managed by Tauri state and UI commands.
pub struct LocalScanScheduler {
    commands: mpsc::UnboundedSender<LocalScanCommand>,
}

/// Tauri-managed handle for lifecycle and page-trigger commands.
pub struct LocalScanRuntimeState {
    scheduler: LocalScanScheduler,
}

impl LocalScanRuntimeState {
    pub fn new(scheduler: LocalScanScheduler) -> Self {
        Self { scheduler }
    }

    pub fn enter_page(&self, domain: LocalScanDomain) -> Result<(), LocalScanSchedulerError> {
        self.scheduler.enter_page(domain)
    }

    pub fn window_restored(&self) -> Result<(), LocalScanSchedulerError> {
        self.scheduler.window_restored()
    }

    pub fn set_background(&self) -> Result<(), LocalScanSchedulerError> {
        self.scheduler.set_background()
    }

    pub fn cancel(&self) -> Result<(), LocalScanSchedulerError> {
        self.scheduler.cancel()
    }
}

impl LocalScanScheduler {
    pub fn new(
        executor: Arc<dyn LocalScanExecutor>,
        cadence: LocalScanCadence,
        start_in_background: bool,
    ) -> (Self, LocalScanWorker) {
        let (commands, receiver) = mpsc::unbounded_channel();
        let worker = LocalScanWorker {
            executor,
            cadence,
            mode: if start_in_background {
                LocalScanMode::Background
            } else {
                LocalScanMode::Foreground
            },
            degraded: false,
            last_duration: Duration::ZERO,
            scan_in_flight: Arc::new(AtomicBool::new(false)),
            commands: receiver,
        };
        (Self { commands }, worker)
    }

    pub fn enter_page(&self, domain: LocalScanDomain) -> Result<(), LocalScanSchedulerError> {
        self.send(LocalScanCommand::EnterPage(domain))
    }

    pub fn window_restored(&self) -> Result<(), LocalScanSchedulerError> {
        self.send(LocalScanCommand::WindowRestored)
    }

    pub fn set_background(&self) -> Result<(), LocalScanSchedulerError> {
        self.send(LocalScanCommand::SetBackground)
    }

    pub fn cancel(&self) -> Result<(), LocalScanSchedulerError> {
        self.send(LocalScanCommand::Cancel)
    }

    fn send(&self, command: LocalScanCommand) -> Result<(), LocalScanSchedulerError> {
        self.commands
            .send(command)
            .map_err(|_| LocalScanSchedulerError::Stopped)
    }
}

struct ScanInFlightReset(Arc<AtomicBool>);

impl Drop for ScanInFlightReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct LocalScanWorker {
    executor: Arc<dyn LocalScanExecutor>,
    cadence: LocalScanCadence,
    mode: LocalScanMode,
    /// Set while scan targets keep failing (e.g. degraded WSL UNC access) so
    /// the scheduler rests at the background cadence instead of re-hammering.
    degraded: bool,
    /// Duration of the most recent scan cycle. The next rest is never shorter
    /// than this, so slow UNC reads cannot keep the bridge saturated.
    last_duration: Duration,
    /// Remains true until the blocking read actually exits, even if the async
    /// deadline already returned. Timed-out cycles never enqueue another reader.
    scan_in_flight: Arc<AtomicBool>,
    commands: mpsc::UnboundedReceiver<LocalScanCommand>,
}

impl LocalScanWorker {
    pub async fn run(mut self) {
        let Ok(cadence) = self.cadence.validate() else {
            log::error!("本地扫描调度器未启动：周期必须大于 0");
            return;
        };
        self.cadence = cadence;
        self.degraded = !self.execute(LocalScanDomain::ALL.to_vec()).await;

        loop {
            let interval = match (self.mode, self.degraded) {
                (LocalScanMode::Foreground, false) => {
                    self.cadence.foreground.max(self.last_duration)
                }
                // Background mode or degraded targets rest at the slower cadence.
                _ => self.cadence.background.max(self.last_duration),
            };
            let delay = tokio::time::sleep(interval);
            tokio::pin!(delay);
            tokio::select! {
                command = self.commands.recv() => match command {
                    Some(LocalScanCommand::EnterPage(domain)) => {
                        self.execute(vec![domain]).await;
                        // Rest after the work so a slow UNC read cannot chain
                        // the next scan immediately after the current one.
                        delay
                            .as_mut()
                            .reset(tokio::time::Instant::now() + interval);
                    }
                    Some(LocalScanCommand::WindowRestored) => {
                        self.mode = LocalScanMode::Foreground;
                        self.degraded = false;
                        self.execute(LocalScanDomain::ALL.to_vec()).await;
                        delay
                            .as_mut()
                            .reset(
                                tokio::time::Instant::now()
                                    + self.cadence.foreground.max(self.last_duration),
                            );
                    }
                    Some(LocalScanCommand::SetBackground) => {
                        self.mode = LocalScanMode::Background;
                        delay
                            .as_mut()
                            .reset(
                                tokio::time::Instant::now()
                                    + self.cadence.background.max(self.last_duration),
                            );
                    }
                    Some(LocalScanCommand::Cancel) | None => return,
                },
                () = &mut delay => {
                    self.degraded = !self.execute(LocalScanDomain::ALL.to_vec()).await;
                }
            }
        }
    }

    /// Runs one scan cycle and reports whether every target succeeded.
    ///
    /// A wedged WSL UNC channel can park a blocking read forever. The async
    /// deadline keeps scheduler commands responsive, while `scan_in_flight`
    /// prevents later cycles from accumulating more uncancellable readers.
    async fn execute(&mut self, domains: Vec<LocalScanDomain>) -> bool {
        let deadline = self.cadence.background * 4;
        if self
            .scan_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            log::warn!("上一轮本地摘要扫描仍在运行，跳过本轮以避免积压");
            self.last_duration = Duration::ZERO;
            return false;
        }
        let executor = self.executor.clone();
        let in_flight = self.scan_in_flight.clone();
        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            deadline,
            tokio::task::spawn_blocking(move || {
                let _reset = ScanInFlightReset(in_flight);
                executor.scan_domains(&domains)
            }),
        )
        .await;
        self.last_duration = started.elapsed();
        match outcome {
            Ok(Ok(events)) => {
                let failed = events
                    .iter()
                    .filter(|event| matches!(event, LocalScanEvent::Failed { .. }))
                    .count();
                if failed > 0 {
                    log::warn!("本地摘要扫描完成，失败目标数：{failed}");
                } else {
                    log::debug!("本地摘要扫描完成，目标数：{}", events.len());
                }
                failed == 0
            }
            Ok(Err(_)) => {
                log::error!("本地摘要扫描 worker 意外终止");
                false
            }
            Err(_) => {
                log::error!(
                    "本地摘要扫描超过 {deadline:?} 未返回，已放弃本轮并降速（WSL 文件通道可能已无响应）"
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> LocalScanTarget {
        LocalScanTarget {
            domain: LocalScanDomain::Skill,
            client_id: ManagedClientId::Claude,
        }
    }

    fn summary(generation: u64) -> LocalScanSummary {
        LocalScanSummary::new(
            target(),
            format!(
                "{:x}",
                Sha256::digest(format!("scope-{generation}").as_bytes())
            ),
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn restart_cutoff_never_discards_a_newer_expectation() {
        let tracker = LocalScanWriteTracker::default();
        assert_eq!(tracker.record_expected(&summary(1)).unwrap(), 1);
        let cutoff = tracker.last_generation();
        assert_eq!(tracker.record_expected(&summary(2)).unwrap(), 2);

        tracker.discard_through(target(), cutoff);

        assert_eq!(tracker.pending_count(), 1);
        assert_eq!(tracker.resolve_changed(&summary(2)), Some(2));
    }
}
