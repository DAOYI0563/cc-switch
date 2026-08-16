use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// The complete set of device-local secrets owned by this application.
/// Callers cannot construct arbitrary Windows Credential Manager target names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSecretId {
    WebdavPassword,
    WebdavSyncPassphrase,
    DailyBriefApiKey,
}

impl DeviceSecretId {
    pub const ALL: [Self; 3] = [
        Self::WebdavPassword,
        Self::WebdavSyncPassphrase,
        Self::DailyBriefApiKey,
    ];

    pub const fn target_name(self) -> &'static str {
        match self {
            Self::WebdavPassword => "com.zhldm.wsl-code-switch/webdav-password",
            Self::WebdavSyncPassphrase => "com.zhldm.wsl-code-switch/webdav-sync-passphrase",
            Self::DailyBriefApiKey => "com.zhldm.wsl-code-switch/daily-brief-api-key",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStoreErrorCode {
    InvalidSecret,
    SecretTooLarge,
    UnsupportedPlatform,
    ReadFailed,
    WriteFailed,
    DeleteFailed,
    InvalidStoredValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStoreError {
    pub code: SecretStoreErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl SecretStoreError {
    pub fn new(code: SecretStoreErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SecretStoreError {}

pub trait SecretStore: Send + Sync {
    fn read(&self, id: DeviceSecretId) -> Result<Option<String>, SecretStoreError>;

    fn write(&self, id: DeviceSecretId, secret: &str) -> Result<(), SecretStoreError>;

    fn delete(&self, id: DeviceSecretId) -> Result<(), SecretStoreError>;
}
