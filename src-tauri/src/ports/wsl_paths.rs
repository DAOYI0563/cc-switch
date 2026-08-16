use std::path::PathBuf;

use crate::domain::ManagedClientId;
use serde::{Deserialize, Serialize};

/// The same managed location as seen by Windows and by a process inside WSL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslPathPair {
    pub windows: PathBuf,
    pub wsl: String,
}

/// Resolves every product-owned path into the one supported WSL environment.
pub trait WslPathResolver {
    fn client_config_root(&self, client: ManagedClientId) -> WslPathPair;
    fn claude_state_file(&self) -> WslPathPair;
    fn opencode_session_data_root(&self) -> WslPathPair;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslPathScope {
    ClientConfig(ManagedClientId),
    ClaudeStateFile,
    OpencodeSessionData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WslPathAccess {
    Read,
    Write,
}

pub trait WslPathGuard {
    fn resolve(
        &self,
        scope: WslPathScope,
        relative: &str,
        access: WslPathAccess,
    ) -> Result<PathBuf, WslPathError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WslPathErrorCode {
    InvalidRelativePath,
    ScopeIsFile,
    ReadOnlyScope,
    LinkNotAllowed,
    PathEscape,
    InspectionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslPathError {
    pub code: WslPathErrorCode,
    pub message: String,
}

impl WslPathError {
    pub fn new(code: WslPathErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WslPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WslPathError {}
