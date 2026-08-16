use crate::domain::ManagedClientId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A provider-shaped projection ready to be written to one client's live files.
///
/// This contract deliberately excludes the database `Provider` aggregate and all
/// infrastructure details. Application policy must be resolved before crossing
/// this boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProviderRecord {
    pub client_id: ManagedClientId,
    pub provider_id: String,
    pub category: Option<String>,
    pub settings: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProviderSnapshot {
    pub client_id: ManagedClientId,
    pub settings: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveProviderConfigOperation {
    Read,
    Write,
    Contains,
    Remove,
}

impl std::fmt::Display for LiveProviderConfigOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Contains => "contains",
            Self::Remove => "remove",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveProviderConfigErrorCode {
    Missing,
    InvalidInput,
    Parse,
    Io,
    UnsupportedOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProviderConfigError {
    pub code: LiveProviderConfigErrorCode,
    pub client_id: ManagedClientId,
    pub operation: LiveProviderConfigOperation,
    pub message: String,
}

impl LiveProviderConfigError {
    pub fn new(
        code: LiveProviderConfigErrorCode,
        client_id: ManagedClientId,
        operation: LiveProviderConfigOperation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            client_id,
            operation,
            message: message.into(),
        }
    }

    pub fn unsupported(client_id: ManagedClientId, operation: LiveProviderConfigOperation) -> Self {
        Self::new(
            LiveProviderConfigErrorCode::UnsupportedOperation,
            client_id,
            operation,
            format!("{client_id} live configuration does not support {operation}"),
        )
    }
}

impl std::fmt::Display for LiveProviderConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} live configuration {} failed: {}",
            self.client_id, self.operation, self.message
        )
    }
}

impl std::error::Error for LiveProviderConfigError {}

pub trait LiveProviderConfigPort {
    fn client_id(&self) -> ManagedClientId;

    fn read(&self) -> Result<LiveProviderSnapshot, LiveProviderConfigError>;

    fn write(&self, provider: &LiveProviderRecord) -> Result<(), LiveProviderConfigError>;

    fn contains(&self, provider_id: &str) -> Result<bool, LiveProviderConfigError>;

    fn remove(&self, provider_id: &str) -> Result<(), LiveProviderConfigError>;
}
