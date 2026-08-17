use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::domain::{
    ConflictCenterDisposition, ConflictCenterItem, ConflictCenterSource, ConflictResolutionAction,
    ConflictResolutionRequest, LocalConflict, LocalConflictKind, LocalDifference,
    LocalReconciliationBatch, LocalReconciliationRecord, LocalReconciliationSnapshot,
    LocalScanDomain, LocalScanEvent, LocalScanRecordChange, LocalScanTarget, PortableDomain,
    RollbackPointPurpose, SyncLocalCommitPlan, SyncMergeBatch, SyncMergeConflict,
    SyncMergeConflictKind,
};
use crate::ports::{
    ConflictCenterError, ConflictCenterErrorCode, ConflictCenterResolutionPort,
    ConflictCenterSourcePort, LocalReconciliationBaselinePort, LocalReconciliationStatePort,
    SyncLocalApplyPort, TemporaryRollbackStore,
};

use super::local_scan::LocalScanCoordinator;

#[derive(Default)]
pub struct InMemoryLocalReconciliationBaselines {
    snapshots: Mutex<HashMap<LocalScanTarget, LocalReconciliationSnapshot>>,
}

pub struct ConflictCenterRuntimeState {
    coordinator: Arc<LocalScanCoordinator>,
    baselines: Arc<InMemoryLocalReconciliationBaselines>,
    webdav: Arc<WebDavConflictSource>,
    resolution_gate: Mutex<()>,
}

impl ConflictCenterRuntimeState {
    pub fn new(
        coordinator: Arc<LocalScanCoordinator>,
        baselines: Arc<InMemoryLocalReconciliationBaselines>,
    ) -> Self {
        Self {
            coordinator,
            baselines,
            webdav: Arc::new(WebDavConflictSource::default()),
            resolution_gate: Mutex::new(()),
        }
    }

    pub fn coordinator(&self) -> Arc<LocalScanCoordinator> {
        self.coordinator.clone()
    }

    pub fn baselines(&self) -> Arc<InMemoryLocalReconciliationBaselines> {
        self.baselines.clone()
    }

    pub fn webdav(&self) -> Arc<WebDavConflictSource> {
        self.webdav.clone()
    }

    pub fn lock_resolution(&self) -> std::sync::MutexGuard<'_, ()> {
        self.resolution_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl LocalReconciliationBaselinePort for InMemoryLocalReconciliationBaselines {
    fn read_baseline(&self, target: LocalScanTarget) -> Option<LocalReconciliationSnapshot> {
        self.snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&target)
            .cloned()
    }

    fn confirm_record(
        &self,
        target: LocalScanTarget,
        record_id: &str,
        content_digest: Option<&str>,
    ) -> Result<(), crate::domain::DomainError> {
        crate::domain::validate_local_scan_record_id(record_id)?;
        let mut snapshots = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut records = snapshots
            .remove(&target)
            .map(|snapshot| snapshot.records)
            .unwrap_or_default();
        records.retain(|record| record.record_id != record_id);
        if let Some(digest) = content_digest {
            records.push(LocalReconciliationRecord::new(record_id, digest)?);
        }
        snapshots.insert(target, LocalReconciliationSnapshot::new(target, records)?);
        Ok(())
    }
}

pub struct LocalScanConflictSource {
    coordinator: Arc<LocalScanCoordinator>,
    states: Arc<dyn LocalReconciliationStatePort>,
}

impl LocalScanConflictSource {
    pub fn new(
        coordinator: Arc<LocalScanCoordinator>,
        states: Arc<dyn LocalReconciliationStatePort>,
    ) -> Self {
        Self {
            coordinator,
            states,
        }
    }
}

impl ConflictCenterSourcePort for LocalScanConflictSource {
    fn list_pending(&self) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
        let mut items = Vec::new();
        for domain in LocalScanDomain::ALL {
            for client_id in crate::domain::ManagedClientId::ALL {
                let target = LocalScanTarget { domain, client_id };
                let Some(pending) = self.coordinator.pending_change(target) else {
                    continue;
                };
                let batch = match self.states.read_reconciliation_state(target) {
                    Ok(state) => pending
                        .classify_against(state.baseline, state.local)
                        .map_err(domain_error)?,
                    Err(_) => LocalReconciliationBatch {
                        target,
                        differences: Vec::new(),
                        conflicts: vec![LocalConflict {
                            record_id: None,
                            kind: LocalConflictKind::IntegrityMismatch,
                            baseline_digest: None,
                            local_digest: None,
                            external_digest: None,
                            failure_kind: None,
                        }],
                    },
                };
                items.extend(local_reconciliation_items(&batch, pending.event())?);
            }
        }
        Ok(items)
    }
}

#[derive(Default)]
pub struct WebDavConflictSource {
    pending: Mutex<Vec<ConflictCenterItem>>,
}

impl WebDavConflictSource {
    pub fn replace_from_merge(&self, batch: &SyncMergeBatch) -> Result<(), ConflictCenterError> {
        let mut items = batch
            .conflicts
            .iter()
            .map(webdav_conflict_item)
            .collect::<Result<Vec<_>, _>>()?;
        items.sort_by(|left, right| item_sort_key(left).cmp(&item_sort_key(right)));
        *self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = items;
        Ok(())
    }

    pub fn clear(&self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

impl ConflictCenterSourcePort for WebDavConflictSource {
    fn list_pending(&self) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
        Ok(self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }
}

pub fn list_conflict_center_items(
    sources: &[&dyn ConflictCenterSourcePort],
    resolver: &dyn ConflictCenterResolutionPort,
) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
    let mut items = Vec::new();
    for source in sources {
        for mut item in source.list_pending()? {
            item.actions = normalize_actions(resolver.supported_actions(&item)?);
            item.validate().map_err(domain_error)?;
            items.push(item);
        }
    }
    items.sort_by(|left, right| item_sort_key(left).cmp(&item_sort_key(right)));
    if items
        .windows(2)
        .any(|pair| pair[0].item_id == pair[1].item_id)
    {
        return Err(ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "conflict-center sources returned duplicate item ids",
        ));
    }
    Ok(items)
}

pub fn resolve_conflict_center_item(
    sources: &[&dyn ConflictCenterSourcePort],
    resolver: &dyn ConflictCenterResolutionPort,
    rollback_store: &dyn TemporaryRollbackStore,
    now_ms: i64,
    request: &ConflictResolutionRequest,
) -> Result<(), ConflictCenterError> {
    if now_ms < 0 {
        return Err(ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "conflict resolution time must not be negative",
        ));
    }
    request.validate().map_err(domain_error)?;
    let current = list_conflict_center_items(sources, resolver)?;
    let item = current
        .iter()
        .find(|item| item.item_id == request.item_id)
        .ok_or_else(|| {
            ConflictCenterError::new(
                ConflictCenterErrorCode::StaleItem,
                "conflict-center item is stale or no longer pending",
            )
        })?;
    if !item.actions.contains(&request.action) {
        return Err(ConflictCenterError::new(
            ConflictCenterErrorCode::UnsupportedAction,
            "resolution action is not available for this item",
        ));
    }

    let payload = resolver.capture_rollback(item, request)?;
    let rollback = rollback_store
        .create(RollbackPointPurpose::ConflictResolution, now_ms, &payload)
        .map_err(|error| rollback_error("create", error))?;

    match resolver.apply_and_validate(item, request) {
        Ok(()) => {
            cleanup_rollback_after_success(rollback_store, &rollback.id, now_ms);
            Ok(())
        }
        Err(primary) => {
            retain_rollback_after_failure(rollback_store, &rollback.id, now_ms);
            Err(primary)
        }
    }
}

pub fn apply_committed_sync_batch(
    applier: &dyn SyncLocalApplyPort,
    rollback_store: &dyn TemporaryRollbackStore,
    now_ms: i64,
    plan: &SyncLocalCommitPlan,
) -> Result<(), ConflictCenterError> {
    if now_ms < 0 {
        return Err(ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "sync local-apply time must not be negative",
        ));
    }
    plan.validate().map_err(domain_error)?;
    if !plan.requires_local_write() {
        return Ok(());
    }

    let payload = applier.capture_rollback(plan)?;
    let rollback = rollback_store
        .create(RollbackPointPurpose::WebdavSync, now_ms, &payload)
        .map_err(|error| rollback_error("create", error))?;

    match applier.apply_and_validate(plan) {
        Ok(()) => {
            cleanup_rollback_after_success(rollback_store, &rollback.id, now_ms);
            Ok(())
        }
        Err(primary) => {
            retain_rollback_after_failure(rollback_store, &rollback.id, now_ms);
            Err(primary)
        }
    }
}

pub fn local_reconciliation_items(
    batch: &LocalReconciliationBatch,
    event: &LocalScanEvent,
) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
    let target = event_target(event);
    if target != batch.target {
        return Err(ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "local reconciliation batch and event targets differ",
        ));
    }
    let mut items = Vec::with_capacity(batch.differences.len() + batch.conflicts.len());
    for difference in &batch.differences {
        items.push(local_difference_item(target, event, difference)?);
    }
    for conflict in &batch.conflicts {
        items.push(local_conflict_item(target, event, conflict)?);
    }
    Ok(items)
}

fn local_difference_item(
    target: LocalScanTarget,
    event: &LocalScanEvent,
    difference: &LocalDifference,
) -> Result<ConflictCenterItem, ConflictCenterError> {
    build_local_item(
        target,
        event,
        Some(&difference.record_id),
        ConflictCenterDisposition::Difference(difference.kind),
        difference.baseline_digest.clone(),
        difference.baseline_digest.clone(),
        difference.external_digest.clone(),
        None,
    )
}

fn local_conflict_item(
    target: LocalScanTarget,
    event: &LocalScanEvent,
    conflict: &LocalConflict,
) -> Result<ConflictCenterItem, ConflictCenterError> {
    build_local_item(
        target,
        event,
        conflict.record_id.as_deref(),
        ConflictCenterDisposition::Conflict(conflict.kind),
        conflict.baseline_digest.clone(),
        conflict.local_digest.clone(),
        conflict.external_digest.clone(),
        conflict.failure_kind,
    )
}

fn webdav_conflict_item(
    conflict: &SyncMergeConflict,
) -> Result<ConflictCenterItem, ConflictCenterError> {
    let baseline_digest = conflict
        .baseline
        .as_ref()
        .map(|summary| summary.revision.content_hash.to_string());
    let local_digest = conflict
        .local
        .as_ref()
        .map(|summary| summary.revision.content_hash.to_string());
    let external_digest = conflict
        .remote
        .as_ref()
        .map(|summary| summary.revision.content_hash.to_string());
    let identity = serde_json::to_vec(&(
        ConflictCenterSource::Webdav,
        &conflict.id,
        conflict.kind,
        &baseline_digest,
        &local_digest,
        &external_digest,
    ))
    .map_err(|_| {
        ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "failed to encode WebDAV conflict identity",
        )
    })?;
    let item = ConflictCenterItem {
        schema_version: ConflictCenterItem::SCHEMA_VERSION,
        item_id: format!("webdav_{}", hex_sha256(&identity)),
        source: ConflictCenterSource::Webdav,
        domain: conflict.id.domain,
        client_id: None,
        record_id: Some(conflict.id.key.clone()),
        display_name: conflict.id.key.clone(),
        modified_at_ms: conflict
            .local
            .iter()
            .chain(conflict.remote.iter())
            .map(|summary| summary.revision.updated_at_ms)
            .max(),
        disposition: ConflictCenterDisposition::Conflict(match conflict.kind {
            SyncMergeConflictKind::ConcurrentUpdate => LocalConflictKind::ConcurrentUpdate,
            SyncMergeConflictKind::UpdateDelete => LocalConflictKind::UpdateDelete,
            SyncMergeConflictKind::UntrackedRemoval => LocalConflictKind::DeleteWithoutBaseline,
        }),
        baseline_digest,
        local_digest,
        external_digest,
        failure_kind: None,
        actions: Vec::new(),
    };
    item.validate().map_err(domain_error)?;
    Ok(item)
}

#[allow(clippy::too_many_arguments)]
fn build_local_item(
    target: LocalScanTarget,
    event: &LocalScanEvent,
    record_id: Option<&str>,
    disposition: ConflictCenterDisposition,
    baseline_digest: Option<String>,
    local_digest: Option<String>,
    external_digest: Option<String>,
    failure_kind: Option<crate::domain::LocalScanFailureKind>,
) -> Result<ConflictCenterItem, ConflictCenterError> {
    let display_name = record_id.unwrap_or("configuration scope").to_string();
    let identity = serde_json::to_vec(&(
        ConflictCenterSource::LocalScan,
        target,
        record_id,
        disposition,
        &baseline_digest,
        &local_digest,
        &external_digest,
        failure_kind,
    ))
    .map_err(|_| {
        ConflictCenterError::new(
            ConflictCenterErrorCode::InvalidInput,
            "failed to encode conflict-center identity",
        )
    })?;
    let item = ConflictCenterItem {
        schema_version: ConflictCenterItem::SCHEMA_VERSION,
        item_id: format!("local_{}", hex_sha256(&identity)),
        source: ConflictCenterSource::LocalScan,
        domain: portable_domain(target.domain),
        client_id: Some(target.client_id),
        record_id: record_id.map(ToOwned::to_owned),
        display_name,
        modified_at_ms: event_modified_at(event, record_id),
        disposition,
        baseline_digest,
        local_digest,
        external_digest,
        failure_kind,
        actions: Vec::new(),
    };
    item.validate().map_err(domain_error)?;
    Ok(item)
}

fn event_target(event: &LocalScanEvent) -> LocalScanTarget {
    match event {
        LocalScanEvent::Unchanged { target, .. }
        | LocalScanEvent::Changed { target, .. }
        | LocalScanEvent::SelfWriteSuppressed { target, .. }
        | LocalScanEvent::Failed { target, .. } => *target,
    }
}

fn event_modified_at(event: &LocalScanEvent, record_id: Option<&str>) -> Option<i64> {
    let LocalScanEvent::Changed { records, .. } = event else {
        return None;
    };
    let exact = record_id.and_then(|record_id| {
        records.iter().find_map(|change| match change {
            LocalScanRecordChange::Added { current } if current.record_id == record_id => {
                current.modified_at_ms
            }
            LocalScanRecordChange::Modified { current, .. } if current.record_id == record_id => {
                current.modified_at_ms
            }
            LocalScanRecordChange::Deleted { previous } if previous.record_id == record_id => {
                previous.modified_at_ms
            }
            _ => None,
        })
    });
    exact.or_else(|| {
        records
            .iter()
            .filter_map(|change| match change {
                LocalScanRecordChange::Added { current }
                | LocalScanRecordChange::Modified { current, .. } => current.modified_at_ms,
                LocalScanRecordChange::Deleted { previous } => previous.modified_at_ms,
            })
            .max()
    })
}

fn portable_domain(domain: LocalScanDomain) -> PortableDomain {
    match domain {
        LocalScanDomain::Provider => PortableDomain::Provider,
        LocalScanDomain::Mcp => PortableDomain::Mcp,
        LocalScanDomain::Prompt => PortableDomain::Prompt,
        LocalScanDomain::Skill => PortableDomain::Skill,
    }
}

fn item_sort_key(item: &ConflictCenterItem) -> (u8, &str, &str, &str) {
    (
        match item.source {
            ConflictCenterSource::LocalScan => 0,
            ConflictCenterSource::Webdav => 1,
        },
        item.domain.as_str(),
        item.client_id.map(|client| client.as_str()).unwrap_or(""),
        item.record_id.as_deref().unwrap_or(""),
    )
}

fn normalize_actions(mut actions: Vec<ConflictResolutionAction>) -> Vec<ConflictResolutionAction> {
    actions.sort();
    actions.dedup();
    actions
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn domain_error(error: crate::domain::DomainError) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::InvalidInput, error.to_string())
}

fn rollback_error(
    operation: &str,
    error: crate::ports::TemporaryRollbackError,
) -> ConflictCenterError {
    ConflictCenterError::new(
        ConflictCenterErrorCode::Rollback,
        format!("failed to {operation} conflict-resolution rollback point"),
    )
    .with_context("rollbackCode", format!("{:?}", error.code))
}

fn cleanup_rollback_after_success(
    rollback_store: &dyn TemporaryRollbackStore,
    rollback_id: &str,
    now_ms: i64,
) {
    if let Err(error) = rollback_store.delete_after_success(rollback_id) {
        log::warn!(
            "rollback_cleanup_delete_after_success_failed code={:?}",
            error.code
        );
        if let Err(error) = rollback_store.retain_after_failure(rollback_id, now_ms) {
            log::warn!(
                "rollback_cleanup_retain_after_delete_failure_failed code={:?}",
                error.code
            );
        }
    }
}

fn retain_rollback_after_failure(
    rollback_store: &dyn TemporaryRollbackStore,
    rollback_id: &str,
    now_ms: i64,
) {
    if let Err(error) = rollback_store.retain_after_failure(rollback_id, now_ms) {
        log::warn!(
            "rollback_retain_after_apply_failure_failed code={:?}",
            error.code
        );
    }
}

pub fn default_local_actions(item: &ConflictCenterItem) -> Vec<ConflictResolutionAction> {
    match item.disposition {
        ConflictCenterDisposition::Difference(_) => vec![
            ConflictResolutionAction::AcceptExternal,
            ConflictResolutionAction::KeepLocal,
        ],
        ConflictCenterDisposition::Conflict(
            LocalConflictKind::ParseFailed | LocalConflictKind::IntegrityMismatch,
        ) => vec![ConflictResolutionAction::Retry],
        ConflictCenterDisposition::Conflict(_) => vec![
            ConflictResolutionAction::AcceptExternal,
            ConflictResolutionAction::KeepLocal,
        ],
    }
}
