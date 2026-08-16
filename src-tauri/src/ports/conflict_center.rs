use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::{
    ConflictCenterItem, ConflictResolutionAction, ConflictResolutionRequest, SyncLocalCommitPlan,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictCenterErrorCode {
    InvalidInput,
    StaleItem,
    UnsupportedAction,
    Read,
    Rollback,
    Apply,
    Validation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictCenterError {
    pub code: ConflictCenterErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl ConflictCenterError {
    pub fn new(code: ConflictCenterErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: BTreeMap::new(),
        }
    }

    pub fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

impl fmt::Display for ConflictCenterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ConflictCenterError {}

/// One replaceable, read-only feed. P5 provides the local-scan feed and P6
/// adds the WebDAV feed behind the same contract.
pub trait ConflictCenterSourcePort: Send + Sync {
    fn list_pending(&self) -> Result<Vec<ConflictCenterItem>, ConflictCenterError>;
}

/// Atomic domain-specific resolution. Implementations own compensation and
/// post-write validation; the orchestrator owns the encrypted rollback point.
pub trait ConflictCenterResolutionPort: Send + Sync {
    fn supported_actions(
        &self,
        item: &ConflictCenterItem,
    ) -> Result<Vec<ConflictResolutionAction>, ConflictCenterError>;

    fn capture_rollback(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<Vec<u8>, ConflictCenterError>;

    fn apply_and_validate(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<(), ConflictCenterError>;
}

/// Applies only the local actions from a remotely committed sync-v3 merge.
/// The caller owns the encrypted rollback-point lifecycle.
pub trait SyncLocalApplyPort: Send + Sync {
    fn capture_rollback(&self, plan: &SyncLocalCommitPlan) -> Result<Vec<u8>, ConflictCenterError>;

    fn apply_and_validate(&self, plan: &SyncLocalCommitPlan) -> Result<(), ConflictCenterError>;
}
