use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    normalize_sha256, validate_local_scan_record_id, DomainError, DomainErrorCode,
    LocalScanFailure, LocalScanFailureKind, LocalScanTarget,
};

/// Content-free state of one logical record in an agreed, application, or live
/// snapshot. Raw configuration stays in the in-memory parser boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReconciliationRecord {
    pub record_id: String,
    pub content_digest: String,
}

impl LocalReconciliationRecord {
    pub fn new(
        record_id: impl Into<String>,
        content_digest: impl AsRef<str>,
    ) -> Result<Self, DomainError> {
        let record = Self {
            record_id: record_id.into(),
            content_digest: normalize_sha256(content_digest.as_ref())?,
        };
        validate_local_scan_record_id(&record.record_id)?;
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReconciliationSnapshot {
    pub target: LocalScanTarget,
    pub records: Vec<LocalReconciliationRecord>,
}

impl LocalReconciliationSnapshot {
    pub fn new(
        target: LocalScanTarget,
        mut records: Vec<LocalReconciliationRecord>,
    ) -> Result<Self, DomainError> {
        records.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        let snapshot = Self { target, records };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let mut previous_id: Option<&str> = None;
        for record in &self.records {
            validate_local_scan_record_id(&record.record_id)?;
            if normalize_sha256(&record.content_digest)? != record.content_digest {
                return Err(DomainError::new(
                    DomainErrorCode::InvalidHash,
                    "local reconciliation digest must be normalized",
                ));
            }
            if previous_id.is_some_and(|value| value >= record.record_id.as_str()) {
                return Err(DomainError::new(
                    DomainErrorCode::InvalidRecord,
                    "local reconciliation records must have unique, ascending ids",
                ));
            }
            previous_id = Some(&record.record_id);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LocalReconciliationExternal {
    Parsed {
        snapshot: LocalReconciliationSnapshot,
        #[serde(rename = "scopeChanged")]
        scope_changed: bool,
    },
    Failed {
        failure: LocalScanFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReconciliationInput {
    pub target: LocalScanTarget,
    pub baseline: Option<LocalReconciliationSnapshot>,
    pub local: LocalReconciliationSnapshot,
    pub external: LocalReconciliationExternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalDifferenceKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDifference {
    pub record_id: String,
    pub kind: LocalDifferenceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalConflictKind {
    AmbiguousLocalMatch,
    ConcurrentUpdate,
    UpdateDelete,
    DeleteWithoutBaseline,
    ParseFailed,
    IntegrityMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalConflict {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    pub kind: LocalConflictKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<LocalScanFailureKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReconciliationBatch {
    pub target: LocalScanTarget,
    pub differences: Vec<LocalDifference>,
    pub conflicts: Vec<LocalConflict>,
}

/// Three-way, content-free classification. This function only describes work
/// for later confirmation; it cannot mutate either the application or live
/// configuration.
pub fn classify_local_reconciliation(
    input: LocalReconciliationInput,
) -> Result<LocalReconciliationBatch, DomainError> {
    validate_input_targets(&input)?;

    let LocalReconciliationExternal::Parsed {
        snapshot: external,
        scope_changed,
    } = input.external
    else {
        let LocalReconciliationExternal::Failed { failure } = input.external else {
            unreachable!("external state is exhaustive")
        };
        if let Some(record_id) = failure.record_id.as_deref() {
            validate_local_scan_record_id(record_id)?;
        }
        return Ok(LocalReconciliationBatch {
            target: input.target,
            differences: Vec::new(),
            conflicts: vec![LocalConflict {
                record_id: failure.record_id,
                kind: if failure.kind == LocalScanFailureKind::ParseFailed {
                    LocalConflictKind::ParseFailed
                } else {
                    LocalConflictKind::IntegrityMismatch
                },
                baseline_digest: None,
                local_digest: None,
                external_digest: None,
                failure_kind: Some(failure.kind),
            }],
        });
    };

    let baseline = input.baseline.as_ref().map(record_map);
    let local = record_map(&input.local);
    let external = record_map(&external);
    let ids: BTreeSet<_> = baseline
        .iter()
        .flat_map(|records| records.keys())
        .chain(local.keys())
        .chain(external.keys())
        .copied()
        .collect();
    let mut differences = Vec::new();
    let mut conflicts = Vec::new();

    for record_id in ids {
        let agreed = baseline
            .as_ref()
            .and_then(|records| records.get(record_id))
            .copied();
        let local_digest = local.get(record_id).copied();
        let external_digest = external.get(record_id).copied();

        if local_digest == external_digest {
            continue;
        }

        match agreed {
            None => classify_without_baseline(
                record_id,
                local_digest,
                external_digest,
                &mut differences,
                &mut conflicts,
            ),
            Some(agreed_digest) => classify_with_baseline(
                record_id,
                agreed_digest,
                local_digest,
                external_digest,
                &mut differences,
                &mut conflicts,
            ),
        }
    }

    if scope_changed && differences.is_empty() && conflicts.is_empty() {
        conflicts.push(LocalConflict {
            record_id: None,
            kind: LocalConflictKind::IntegrityMismatch,
            baseline_digest: None,
            local_digest: None,
            external_digest: None,
            failure_kind: None,
        });
    }

    Ok(LocalReconciliationBatch {
        target: input.target,
        differences,
        conflicts,
    })
}

fn validate_input_targets(input: &LocalReconciliationInput) -> Result<(), DomainError> {
    input.local.validate()?;
    if input.local.target != input.target {
        return Err(target_mismatch());
    }
    if let Some(baseline) = &input.baseline {
        baseline.validate()?;
        if baseline.target != input.target {
            return Err(target_mismatch());
        }
    }
    if let LocalReconciliationExternal::Parsed { snapshot, .. } = &input.external {
        snapshot.validate()?;
        if snapshot.target != input.target {
            return Err(target_mismatch());
        }
    }
    Ok(())
}

fn target_mismatch() -> DomainError {
    DomainError::new(
        DomainErrorCode::InvalidRecord,
        "local reconciliation snapshots must describe the input target",
    )
}

fn record_map(snapshot: &LocalReconciliationSnapshot) -> BTreeMap<&str, &str> {
    snapshot
        .records
        .iter()
        .map(|record| (record.record_id.as_str(), record.content_digest.as_str()))
        .collect()
}

fn classify_without_baseline(
    record_id: &str,
    local: Option<&str>,
    external: Option<&str>,
    differences: &mut Vec<LocalDifference>,
    conflicts: &mut Vec<LocalConflict>,
) {
    match (local, external) {
        (None, Some(external_digest)) => differences.push(LocalDifference {
            record_id: record_id.to_string(),
            kind: LocalDifferenceKind::Added,
            baseline_digest: None,
            external_digest: Some(external_digest.to_string()),
        }),
        (Some(local_digest), None) => conflicts.push(LocalConflict {
            record_id: Some(record_id.to_string()),
            kind: LocalConflictKind::DeleteWithoutBaseline,
            baseline_digest: None,
            local_digest: Some(local_digest.to_string()),
            external_digest: None,
            failure_kind: None,
        }),
        (Some(local_digest), Some(external_digest)) => conflicts.push(LocalConflict {
            record_id: Some(record_id.to_string()),
            kind: LocalConflictKind::AmbiguousLocalMatch,
            baseline_digest: None,
            local_digest: Some(local_digest.to_string()),
            external_digest: Some(external_digest.to_string()),
            failure_kind: None,
        }),
        (None, None) => {}
    }
}

fn classify_with_baseline(
    record_id: &str,
    baseline: &str,
    local: Option<&str>,
    external: Option<&str>,
    differences: &mut Vec<LocalDifference>,
    conflicts: &mut Vec<LocalConflict>,
) {
    if local == Some(baseline) {
        differences.push(LocalDifference {
            record_id: record_id.to_string(),
            kind: if external.is_some() {
                LocalDifferenceKind::Modified
            } else {
                LocalDifferenceKind::Deleted
            },
            baseline_digest: Some(baseline.to_string()),
            external_digest: external.map(ToOwned::to_owned),
        });
        return;
    }
    if external == Some(baseline) {
        return;
    }

    conflicts.push(LocalConflict {
        record_id: Some(record_id.to_string()),
        kind: if local.is_none() || external.is_none() {
            LocalConflictKind::UpdateDelete
        } else {
            LocalConflictKind::ConcurrentUpdate
        },
        baseline_digest: Some(baseline.to_string()),
        local_digest: local.map(ToOwned::to_owned),
        external_digest: external.map(ToOwned::to_owned),
        failure_kind: None,
    });
}
