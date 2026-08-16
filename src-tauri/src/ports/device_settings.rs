use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSettingsErrorCode {
    LinkNotAllowed,
    TooLarge,
    ReadFailed,
    WriteFailed,
    DeleteFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSettingsError {
    pub code: DeviceSettingsErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl DeviceSettingsError {
    pub fn new(code: DeviceSettingsErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for DeviceSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DeviceSettingsError {}

/// Exact-byte access used by cross-resource migration rollback.
pub trait DeviceSettingsStore: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, DeviceSettingsError>;

    fn replace(&self, contents: &[u8]) -> Result<(), DeviceSettingsError>;

    fn delete(&self) -> Result<(), DeviceSettingsError>;
}
