use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::domain::{
    validate_local_scan_record_id, DomainError, DomainErrorCode, LocalReconciliationSnapshot,
    LocalScanFailureKind, LocalScanSummary, LocalScanTarget,
};

/// Redacted failure returned by a local summary source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanReadFailure {
    pub kind: LocalScanFailureKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
}

/// Read-only source of content-free summaries for fixed WSL live targets.
pub trait LocalScanSummaryPort: Send + Sync {
    fn scan_summary(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure>;
}

/// Content-free application state used for three-way local reconciliation.
/// `baseline` is absent until an explicit write or user confirmation establishes
/// historical evidence; callers must never substitute `local` for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalReconciliationState {
    pub target: LocalScanTarget,
    pub baseline: Option<LocalReconciliationSnapshot>,
    pub local: LocalReconciliationSnapshot,
}

impl LocalReconciliationState {
    pub fn new(
        target: LocalScanTarget,
        baseline: Option<LocalReconciliationSnapshot>,
        local: LocalReconciliationSnapshot,
    ) -> Result<Self, DomainError> {
        local.validate()?;
        if local.target != target {
            return Err(reconciliation_target_mismatch());
        }
        if let Some(baseline) = &baseline {
            baseline.validate()?;
            if baseline.target != target {
                return Err(reconciliation_target_mismatch());
            }
        }
        Ok(Self {
            target,
            baseline,
            local,
        })
    }
}

fn reconciliation_target_mismatch() -> DomainError {
    DomainError::new(
        DomainErrorCode::InvalidRecord,
        "local reconciliation state must describe its requested target",
    )
}

/// Read-only boundary for the last confirmed baseline and current application
/// database projection. Mutation and live-file access deliberately do not exist
/// on this port.
pub trait LocalReconciliationStatePort: Send + Sync {
    fn read_reconciliation_state(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalReconciliationState, LocalScanReadFailure>;
}

/// Replaceable baseline storage. Confirming one record must not infer or
/// overwrite baselines for unrelated records in the same live file.
pub trait LocalReconciliationBaselinePort: Send + Sync {
    fn read_baseline(&self, target: LocalScanTarget) -> Option<LocalReconciliationSnapshot>;

    fn confirm_record(
        &self,
        target: LocalScanTarget,
        record_id: &str,
        content_digest: Option<&str>,
    ) -> Result<(), DomainError>;
}

/// One normalized record produced by a domain-specific full parser. Values may
/// contain sensitive live configuration and therefore deliberately do not
/// implement Serialize or Display.
#[derive(Clone, PartialEq)]
pub struct LocalScanParsedRecord {
    pub record_id: String,
    pub value: Value,
}

impl std::fmt::Debug for LocalScanParsedRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalScanParsedRecord")
            .field("record_id", &self.record_id)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

impl LocalScanParsedRecord {
    pub fn new(record_id: impl Into<String>, value: Value) -> Result<Self, DomainError> {
        let record = Self {
            record_id: record_id.into(),
            value,
        };
        validate_local_scan_record_id(&record.record_id)?;
        Ok(record)
    }
}

/// In-memory normalized output for one changed target. The constructor gives
/// downstream reconciliation a stable identity order without exposing content
/// across IPC or logs.
#[derive(Clone, PartialEq)]
pub struct LocalScanParsedSnapshot {
    pub target: LocalScanTarget,
    pub records: Vec<LocalScanParsedRecord>,
}

impl std::fmt::Debug for LocalScanParsedSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let record_ids: Vec<_> = self
            .records
            .iter()
            .map(|record| record.record_id.as_str())
            .collect();
        formatter
            .debug_struct("LocalScanParsedSnapshot")
            .field("target", &self.target)
            .field("record_ids", &record_ids)
            .field("values", &"[REDACTED]")
            .finish()
    }
}

impl LocalScanParsedSnapshot {
    pub fn new(
        target: LocalScanTarget,
        mut records: Vec<LocalScanParsedRecord>,
    ) -> Result<Self, DomainError> {
        records.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        if records
            .windows(2)
            .any(|pair| pair[0].record_id == pair[1].record_id)
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "parsed local scan records must have unique ids",
            ));
        }
        Ok(Self { target, records })
    }
}

/// Full parsing remains independent per domain and runs only after a summary
/// change has been established.
pub trait LocalScanParserPort: Send + Sync {
    fn parse_changed(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure>;
}
