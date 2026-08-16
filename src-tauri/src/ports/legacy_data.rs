use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::{LegacyMigrationPreview, LegacyRetainedSnapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyDataErrorCode {
    InspectionFailed,
    LinkNotAllowed,
    PendingDatabaseChanges,
    UnsupportedVersion,
    InvalidDatabase,
    InvalidJson,
    SourceChanged,
    InvalidRecord,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDataError {
    pub code: LegacyDataErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl LegacyDataError {
    pub fn new(code: LegacyDataErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for LegacyDataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LegacyDataError {}

pub trait LegacyDataSource: Send + Sync {
    fn preview(&self) -> Result<LegacyMigrationPreview, LegacyDataError>;

    fn load_retained(
        &self,
        expected_fingerprint: &str,
    ) -> Result<LegacyRetainedSnapshot, LegacyDataError>;
}
