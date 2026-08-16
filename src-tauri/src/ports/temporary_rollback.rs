use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::{RollbackPointMetadata, RollbackPointPurpose};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporaryRollbackErrorCode {
    InvalidId,
    InvalidState,
    NotFound,
    LinkNotAllowed,
    Io,
    Serialization,
    Integrity,
    Protection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporaryRollbackError {
    pub code: TemporaryRollbackErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl TemporaryRollbackError {
    pub fn new(code: TemporaryRollbackErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for TemporaryRollbackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TemporaryRollbackError {}

pub trait TemporaryRollbackStore: Send + Sync {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError>;

    fn restore(&self, id: &str) -> Result<Vec<u8>, TemporaryRollbackError>;

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError>;

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError>;

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError>;
}
