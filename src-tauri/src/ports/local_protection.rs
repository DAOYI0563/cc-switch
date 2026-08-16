use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable entropy domains prevent one kind of protected local data from being
/// accepted in another context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalProtectionPurpose {
    TemporaryRollback,
    DailyBriefCheckpoint,
    DailyBriefCache,
}

impl LocalProtectionPurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TemporaryRollback => "temporary-rollback",
            Self::DailyBriefCheckpoint => "daily-brief-checkpoint",
            Self::DailyBriefCache => "daily-brief-cache",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalProtectionErrorCode {
    InvalidInput,
    UnsupportedPlatform,
    ProtectFailed,
    UnprotectFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProtectionError {
    pub code: LocalProtectionErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl LocalProtectionError {
    pub fn new(code: LocalProtectionErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for LocalProtectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalProtectionError {}

pub trait LocalProtector: Send + Sync {
    fn protect(
        &self,
        purpose: LocalProtectionPurpose,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError>;

    fn unprotect(
        &self,
        purpose: LocalProtectionPurpose,
        protected: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError>;
}
