use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    DomainError, DomainErrorCode, PortableRecordId, RecordRevision, SyncRecord, SyncRecordBaseline,
    SyncRecordState, SyncSchemaVersion,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncMergeInput {
    pub schema_version: SyncSchemaVersion,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
    pub remote_records: Vec<SyncRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMergeSideAction {
    Unchanged,
    ApplyMerged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncMergeResolution {
    pub schema_version: SyncSchemaVersion,
    pub record: SyncRecord,
    pub local_action: SyncMergeSideAction,
    pub remote_action: SyncMergeSideAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMergeConflictKind {
    ConcurrentUpdate,
    UpdateDelete,
    UntrackedRemoval,
}

/// Content-free record identity retained for conflict presentation and later
/// resolution. Portable payloads remain behind the merge boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncMergeRecordSummary {
    pub schema_version: SyncSchemaVersion,
    pub id: PortableRecordId,
    pub state: SyncRecordState,
    pub revision: RecordRevision,
}

impl SyncMergeRecordSummary {
    fn from_record(record: &SyncRecord) -> Self {
        Self {
            schema_version: SyncSchemaVersion::V1,
            id: record.id.clone(),
            state: record.state(),
            revision: record.revision.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncMergeConflict {
    pub schema_version: SyncSchemaVersion,
    pub id: PortableRecordId,
    pub kind: SyncMergeConflictKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_confirmed_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<SyncMergeRecordSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local: Option<SyncMergeRecordSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<SyncMergeRecordSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncMergeBatch {
    pub schema_version: SyncSchemaVersion,
    pub resolved: Vec<SyncMergeResolution>,
    pub conflicts: Vec<SyncMergeConflict>,
}

/// Pure record-by-record three-way merge. The result describes explicit side
/// actions but cannot perform persistence or transport writes.
pub fn merge_sync_records(input: SyncMergeInput) -> Result<SyncMergeBatch, DomainError> {
    validate_input(&input)?;

    let baselines = input
        .baselines
        .iter()
        .map(|baseline| (baseline.record.id.clone(), baseline))
        .collect::<BTreeMap<_, _>>();
    let local_records = input
        .local_records
        .iter()
        .map(|record| (record.id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let remote_records = input
        .remote_records
        .iter()
        .map(|record| (record.id.clone(), record))
        .collect::<BTreeMap<_, _>>();

    validate_revision_consistency(&baselines, &local_records, &remote_records)?;

    let ids = baselines
        .keys()
        .chain(local_records.keys())
        .chain(remote_records.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut resolved = Vec::new();
    let mut conflicts = Vec::new();

    for id in ids {
        let baseline = baselines.get(&id).copied();
        let local = local_records.get(&id).copied();
        let remote = remote_records.get(&id).copied();

        match (baseline, local, remote) {
            (None, Some(local), None) => {
                resolved.push(resolve_record(local, Some(local), None));
            }
            (None, None, Some(remote)) => {
                resolved.push(resolve_record(remote, None, Some(remote)));
            }
            (None, Some(local), Some(remote)) => {
                if same_logical_value(local, remote) {
                    resolved.push(resolve_equivalent(local, remote));
                } else {
                    conflicts.push(conflict(
                        id,
                        divergent_kind(local, remote),
                        None,
                        Some(local),
                        Some(remote),
                    ));
                }
            }
            (Some(baseline), Some(local), Some(remote)) => {
                if same_logical_value(local, remote) {
                    resolved.push(resolve_equivalent(local, remote));
                    continue;
                }

                let local_changed = !same_logical_value(&baseline.record, local);
                let remote_changed = !same_logical_value(&baseline.record, remote);
                match (local_changed, remote_changed) {
                    (true, false) => {
                        resolved.push(resolve_record(local, Some(local), Some(remote)));
                    }
                    (false, true) => {
                        resolved.push(resolve_record(remote, Some(local), Some(remote)));
                    }
                    (true, true) => conflicts.push(conflict(
                        id,
                        divergent_kind(local, remote),
                        Some(baseline),
                        Some(local),
                        Some(remote),
                    )),
                    (false, false) => {
                        return Err(invalid_merge(
                            "logically unchanged records cannot diverge from each other",
                            &id,
                        ));
                    }
                }
            }
            (Some(baseline), local, remote) => conflicts.push(conflict(
                id,
                SyncMergeConflictKind::UntrackedRemoval,
                Some(baseline),
                local,
                remote,
            )),
            (None, None, None) => {
                return Err(invalid_merge(
                    "merge record id is absent from every input snapshot",
                    &id,
                ));
            }
        }
    }

    Ok(SyncMergeBatch {
        schema_version: SyncSchemaVersion::V1,
        resolved,
        conflicts,
    })
}

fn validate_input(input: &SyncMergeInput) -> Result<(), DomainError> {
    validate_baselines(&input.baselines)?;
    validate_records(&input.local_records, "local")?;
    validate_records(&input.remote_records, "remote")
}

fn validate_baselines(baselines: &[SyncRecordBaseline]) -> Result<(), DomainError> {
    let mut previous: Option<&PortableRecordId> = None;
    for baseline in baselines {
        baseline.validate()?;
        if previous.is_some_and(|id| id >= &baseline.record.id) {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "sync merge baselines must have unique, ascending record ids",
            ));
        }
        previous = Some(&baseline.record.id);
    }
    Ok(())
}

fn validate_records(records: &[SyncRecord], side: &str) -> Result<(), DomainError> {
    let mut previous: Option<&PortableRecordId> = None;
    for record in records {
        record.validate()?;
        if previous.is_some_and(|id| id >= &record.id) {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                format!("sync merge {side} records must have unique, ascending record ids"),
            ));
        }
        previous = Some(&record.id);
    }
    Ok(())
}

fn validate_revision_consistency(
    baselines: &BTreeMap<PortableRecordId, &SyncRecordBaseline>,
    local_records: &BTreeMap<PortableRecordId, &SyncRecord>,
    remote_records: &BTreeMap<PortableRecordId, &SyncRecord>,
) -> Result<(), DomainError> {
    let ids = baselines
        .keys()
        .chain(local_records.keys())
        .chain(remote_records.keys())
        .collect::<BTreeSet<_>>();

    for id in ids {
        let baseline = baselines.get(id).map(|value| &value.record);
        let local = local_records.get(id).copied();
        let remote = remote_records.get(id).copied();

        if let (Some(baseline), Some(local)) = (baseline, local) {
            validate_not_stale(id, baseline, local)?;
            validate_revision_pair(id, baseline, local)?;
        }
        if let (Some(baseline), Some(remote)) = (baseline, remote) {
            validate_not_stale(id, baseline, remote)?;
            validate_revision_pair(id, baseline, remote)?;
        }
        if let (Some(local), Some(remote)) = (local, remote) {
            validate_revision_pair(id, local, remote)?;
        }
    }
    Ok(())
}

fn validate_not_stale(
    id: &PortableRecordId,
    baseline: &SyncRecord,
    current: &SyncRecord,
) -> Result<(), DomainError> {
    if current.revision.device_id == baseline.revision.device_id
        && current.revision.counter < baseline.revision.counter
    {
        return Err(invalid_merge(
            "record revision counter is older than its confirmed baseline",
            id,
        ));
    }
    Ok(())
}

fn validate_revision_pair(
    id: &PortableRecordId,
    left: &SyncRecord,
    right: &SyncRecord,
) -> Result<(), DomainError> {
    if left.revision.device_id == right.revision.device_id
        && left.revision.counter == right.revision.counter
        && left != right
    {
        return Err(invalid_merge(
            "one device revision cannot identify different record contents",
            id,
        ));
    }
    Ok(())
}

fn same_logical_value(left: &SyncRecord, right: &SyncRecord) -> bool {
    match (left.state(), right.state()) {
        (SyncRecordState::Live, SyncRecordState::Live) => {
            left.revision.content_hash == right.revision.content_hash
        }
        (SyncRecordState::Deleted, SyncRecordState::Deleted) => true,
        _ => false,
    }
}

fn resolve_equivalent(local: &SyncRecord, remote: &SyncRecord) -> SyncMergeResolution {
    let selected = if preference_key(local) >= preference_key(remote) {
        local
    } else {
        remote
    };
    resolve_record(selected, Some(local), Some(remote))
}

fn preference_key(record: &SyncRecord) -> (u64, u64, i64, &str, &str) {
    let tombstone_generation = record
        .tombstone
        .as_ref()
        .map(|tombstone| tombstone.introduced_generation)
        .unwrap_or(0);
    (
        tombstone_generation,
        record.revision.counter,
        record.revision.updated_at_ms,
        record.revision.device_id.as_str(),
        record.revision.content_hash.as_str(),
    )
}

fn resolve_record(
    selected: &SyncRecord,
    local: Option<&SyncRecord>,
    remote: Option<&SyncRecord>,
) -> SyncMergeResolution {
    SyncMergeResolution {
        schema_version: SyncSchemaVersion::V1,
        record: selected.clone(),
        local_action: side_action(local, selected),
        remote_action: side_action(remote, selected),
    }
}

fn side_action(current: Option<&SyncRecord>, selected: &SyncRecord) -> SyncMergeSideAction {
    if current == Some(selected) {
        SyncMergeSideAction::Unchanged
    } else {
        SyncMergeSideAction::ApplyMerged
    }
}

fn divergent_kind(local: &SyncRecord, remote: &SyncRecord) -> SyncMergeConflictKind {
    if local.state() == remote.state() {
        SyncMergeConflictKind::ConcurrentUpdate
    } else {
        SyncMergeConflictKind::UpdateDelete
    }
}

fn conflict(
    id: PortableRecordId,
    kind: SyncMergeConflictKind,
    baseline: Option<&SyncRecordBaseline>,
    local: Option<&SyncRecord>,
    remote: Option<&SyncRecord>,
) -> SyncMergeConflict {
    SyncMergeConflict {
        schema_version: SyncSchemaVersion::V1,
        id,
        kind,
        baseline_confirmed_generation: baseline.map(|value| value.confirmed_generation),
        baseline: baseline.map(|value| SyncMergeRecordSummary::from_record(&value.record)),
        local: local.map(SyncMergeRecordSummary::from_record),
        remote: remote.map(SyncMergeRecordSummary::from_record),
    }
}

fn invalid_merge(message: &str, id: &PortableRecordId) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
        .with_context("recordId", id.to_string())
}
