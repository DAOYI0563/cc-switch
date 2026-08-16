use std::fmt;

use crate::domain::{LegacyRetainedSnapshot, RetainedMigrationReport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedMigrationTargetError {
    pub message: String,
}

impl RetainedMigrationTargetError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for RetainedMigrationTargetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RetainedMigrationTargetError {}

pub trait RetainedMigrationTarget: Send + Sync {
    fn apply_retained(
        &self,
        snapshot: &LegacyRetainedSnapshot,
        completed_at_ms: i64,
    ) -> Result<RetainedMigrationReport, RetainedMigrationTargetError>;

    fn rollback_retained(
        &self,
        source_fingerprint: &str,
    ) -> Result<(), RetainedMigrationTargetError>;

    fn retained_resources_complete(&self) -> Result<bool, RetainedMigrationTargetError>;

    fn mark_retained_resources_complete(
        &self,
        source_fingerprint: &str,
        completed_at_ms: i64,
    ) -> Result<(), RetainedMigrationTargetError>;
}
