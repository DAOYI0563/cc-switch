use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorCode, ManagedClientId};

/// Local configuration domains that participate in read-only live scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LocalScanDomain {
    Provider,
    Mcp,
    Prompt,
    Skill,
}

impl LocalScanDomain {
    pub const ALL: [Self; 4] = [Self::Provider, Self::Mcp, Self::Prompt, Self::Skill];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Mcp => "mcp",
            Self::Prompt => "prompt",
            Self::Skill => "skill",
        }
    }
}

impl FromStr for LocalScanDomain {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "provider" => Ok(Self::Provider),
            "mcp" => Ok(Self::Mcp),
            "prompt" => Ok(Self::Prompt),
            "skill" => Ok(Self::Skill),
            _ => Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "unsupported local scan domain",
            )),
        }
    }
}

/// One independently scanned client and configuration domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanTarget {
    pub domain: LocalScanDomain,
    pub client_id: ManagedClientId,
}

/// Content-free summary of one logical record in a live configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanEntrySummary {
    pub record_id: String,
    pub content_digest: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<i64>,
}

impl LocalScanEntrySummary {
    pub fn new(
        record_id: impl Into<String>,
        content_digest: impl AsRef<str>,
        size_bytes: u64,
        modified_at_ms: Option<i64>,
    ) -> Result<Self, DomainError> {
        let summary = Self {
            record_id: record_id.into(),
            content_digest: normalize_sha256(content_digest.as_ref())?,
            size_bytes,
            modified_at_ms,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_local_scan_record_id(&self.record_id)?;
        validate_normalized_sha256(&self.content_digest)?;
        if self.modified_at_ms.is_some_and(|value| value < 0) {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "local scan modification time must not be negative",
            ));
        }
        Ok(())
    }
}

/// Complete content-free summary of one live configuration scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanSummary {
    pub schema_version: u32,
    pub target: LocalScanTarget,
    pub scope_digest: String,
    pub entries: Vec<LocalScanEntrySummary>,
}

impl LocalScanSummary {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(
        target: LocalScanTarget,
        scope_digest: impl AsRef<str>,
        mut entries: Vec<LocalScanEntrySummary>,
    ) -> Result<Self, DomainError> {
        entries.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        let summary = Self {
            schema_version: Self::SCHEMA_VERSION,
            target,
            scope_digest: normalize_sha256(scope_digest.as_ref())?,
            entries,
        };
        summary.validate()?;
        Ok(summary)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "unsupported local scan summary version",
            ));
        }
        validate_normalized_sha256(&self.scope_digest)?;

        let mut previous_id: Option<&str> = None;
        for entry in &self.entries {
            entry.validate()?;
            if previous_id.is_some_and(|value| value >= entry.record_id.as_str()) {
                return Err(DomainError::new(
                    DomainErrorCode::InvalidRecord,
                    "local scan entries must have unique, ascending record ids",
                ));
            }
            previous_id = Some(&entry.record_id);
        }
        Ok(())
    }
}

/// Stable, redacted failure categories exposed across the scan boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalScanFailureKind {
    NotFound,
    PermissionDenied,
    InvalidPath,
    PathOutsideRoot,
    LinkOrReparsePoint,
    PathCycle,
    PathResolutionFailed,
    ReadFailed,
    DigestFailed,
    ParseFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalScanFailure {
    pub kind: LocalScanFailureKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
}

/// Per-record changes emitted only after a complete scope digest changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum LocalScanRecordChange {
    Added {
        current: LocalScanEntrySummary,
    },
    Modified {
        previous: LocalScanEntrySummary,
        current: LocalScanEntrySummary,
    },
    Deleted {
        previous: LocalScanEntrySummary,
    },
}

/// Result of comparing or attempting to read one local scan target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LocalScanEvent {
    Unchanged {
        target: LocalScanTarget,
        #[serde(rename = "scopeDigest")]
        scope_digest: String,
    },
    Changed {
        target: LocalScanTarget,
        #[serde(rename = "previousScopeDigest")]
        previous_scope_digest: String,
        #[serde(rename = "currentScopeDigest")]
        current_scope_digest: String,
        records: Vec<LocalScanRecordChange>,
    },
    SelfWriteSuppressed {
        target: LocalScanTarget,
        #[serde(rename = "scopeDigest")]
        scope_digest: String,
        #[serde(rename = "writeGeneration")]
        write_generation: u64,
    },
    Failed {
        target: LocalScanTarget,
        failure: LocalScanFailure,
    },
}

impl LocalScanEvent {
    pub fn failed(
        target: LocalScanTarget,
        kind: LocalScanFailureKind,
        record_id: Option<String>,
    ) -> Result<Self, DomainError> {
        if let Some(record_id) = record_id.as_deref() {
            validate_local_scan_record_id(record_id)?;
        }
        Ok(Self::Failed {
            target,
            failure: LocalScanFailure { kind, record_id },
        })
    }
}

pub fn compare_local_scan_summaries(
    previous: &LocalScanSummary,
    current: &LocalScanSummary,
) -> Result<LocalScanEvent, DomainError> {
    previous.validate()?;
    current.validate()?;
    if previous.target != current.target {
        return Err(DomainError::new(
            DomainErrorCode::InvalidRecord,
            "local scan summaries must describe the same target",
        ));
    }

    if previous.scope_digest == current.scope_digest {
        return Ok(LocalScanEvent::Unchanged {
            target: current.target,
            scope_digest: current.scope_digest.clone(),
        });
    }

    let previous_by_id: BTreeMap<_, _> = previous
        .entries
        .iter()
        .map(|entry| (entry.record_id.as_str(), entry))
        .collect();
    let current_by_id: BTreeMap<_, _> = current
        .entries
        .iter()
        .map(|entry| (entry.record_id.as_str(), entry))
        .collect();
    let record_ids: BTreeSet<_> = previous_by_id
        .keys()
        .chain(current_by_id.keys())
        .copied()
        .collect();

    let mut records = Vec::new();
    for record_id in record_ids {
        match (previous_by_id.get(record_id), current_by_id.get(record_id)) {
            (None, Some(current)) => records.push(LocalScanRecordChange::Added {
                current: (*current).clone(),
            }),
            (Some(previous), None) => records.push(LocalScanRecordChange::Deleted {
                previous: (*previous).clone(),
            }),
            (Some(previous), Some(current))
                if previous.content_digest != current.content_digest =>
            {
                records.push(LocalScanRecordChange::Modified {
                    previous: (*previous).clone(),
                    current: (*current).clone(),
                });
            }
            (Some(_), Some(_)) => {}
            (None, None) => unreachable!("record id originated from one of the summary maps"),
        }
    }

    Ok(LocalScanEvent::Changed {
        target: current.target,
        previous_scope_digest: previous.scope_digest.clone(),
        current_scope_digest: current.scope_digest.clone(),
        records,
    })
}

pub(crate) fn validate_local_scan_record_id(value: &str) -> Result<(), DomainError> {
    let valid = !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control);
    if valid {
        return Ok(());
    }
    Err(DomainError::new(
        DomainErrorCode::InvalidRecord,
        "invalid local scan record id",
    ))
}

pub(crate) fn normalize_sha256(value: &str) -> Result<String, DomainError> {
    let normalized = value.to_ascii_lowercase();
    validate_normalized_sha256(&normalized)?;
    Ok(normalized)
}

pub(crate) fn validate_normalized_sha256(value: &str) -> Result<(), DomainError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(DomainError::new(
        DomainErrorCode::InvalidHash,
        "SHA-256 digest must contain 64 lowercase hexadecimal characters",
    ))
}
