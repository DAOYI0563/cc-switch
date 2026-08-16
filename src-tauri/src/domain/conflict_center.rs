use serde::{Deserialize, Serialize};

use super::{
    normalize_sha256, validate_local_scan_record_id, DomainError, DomainErrorCode,
    LocalConflictKind, LocalDifferenceKind, LocalScanFailureKind, ManagedClientId, PortableDomain,
    PortableRecordId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictCenterSource {
    LocalScan,
    Webdav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "kind", rename_all = "snake_case")]
pub enum ConflictCenterDisposition {
    Difference(LocalDifferenceKind),
    Conflict(LocalConflictKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolutionAction {
    AcceptExternal,
    KeepLocal,
    KeepBoth,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictCenterItem {
    pub schema_version: u32,
    pub item_id: String,
    pub source: ConflictCenterSource,
    pub domain: PortableDomain,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<ManagedClientId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_id: Option<String>,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at_ms: Option<i64>,
    pub disposition: ConflictCenterDisposition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<LocalScanFailureKind>,
    pub actions: Vec<ConflictResolutionAction>,
}

impl ConflictCenterItem {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(invalid_item("unsupported conflict-center item version"));
        }
        validate_item_id(&self.item_id)?;
        if let Some(record_id) = self.record_id.as_deref() {
            match self.source {
                ConflictCenterSource::LocalScan => validate_local_scan_record_id(record_id)?,
                ConflictCenterSource::Webdav => {
                    PortableRecordId::new(self.domain, record_id.to_string())?;
                }
            }
        }
        if self.display_name.trim() != self.display_name
            || self.display_name.is_empty()
            || self.display_name.chars().count() > 512
            || self.display_name.chars().any(char::is_control)
        {
            return Err(invalid_item("invalid conflict-center display name"));
        }
        if self.modified_at_ms.is_some_and(|value| value < 0) {
            return Err(invalid_item(
                "conflict-center modification time must not be negative",
            ));
        }
        for digest in [
            self.baseline_digest.as_deref(),
            self.local_digest.as_deref(),
            self.external_digest.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if normalize_sha256(digest)? != digest {
                return Err(invalid_item("conflict-center digest must be normalized"));
            }
        }
        if self.source == ConflictCenterSource::LocalScan
            && (self.client_id.is_none()
                || !matches!(
                    self.domain,
                    PortableDomain::Provider
                        | PortableDomain::Mcp
                        | PortableDomain::Prompt
                        | PortableDomain::Skill
                ))
        {
            return Err(invalid_item(
                "local conflict-center items require one managed client and local domain",
            ));
        }
        if self.actions.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(invalid_item(
                "conflict-center actions must be unique and sorted",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolutionRequest {
    pub item_id: String,
    pub action: ConflictResolutionAction,
}

impl ConflictResolutionRequest {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_item_id(&self.item_id)
    }
}

fn validate_item_id(value: &str) -> Result<(), DomainError> {
    if !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(invalid_item("invalid conflict-center item id"))
    }
}

fn invalid_item(message: &str) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
