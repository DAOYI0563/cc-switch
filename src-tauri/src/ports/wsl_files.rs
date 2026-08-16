use crate::ports::{WslPathError, WslPathScope};
use serde::{Deserialize, Serialize};

pub trait WslFileSystem {
    fn read(&self, scope: WslPathScope, relative: &str) -> Result<Vec<u8>, WslFileError>;

    fn read_optional(
        &self,
        scope: WslPathScope,
        relative: &str,
    ) -> Result<Option<Vec<u8>>, WslFileError>;

    fn atomic_write(
        &self,
        scope: WslPathScope,
        relative: &str,
        contents: &[u8],
    ) -> Result<(), WslFileError>;

    fn remove_file(&self, scope: WslPathScope, relative: &str) -> Result<(), WslFileError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WslFileErrorCode {
    InvalidPath,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WslFileError {
    pub code: WslFileErrorCode,
    pub message: String,
}

impl WslFileError {
    pub fn from_path(error: WslPathError) -> Self {
        Self {
            code: WslFileErrorCode::InvalidPath,
            message: error.to_string(),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self {
            code: WslFileErrorCode::Io,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WslFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WslFileError {}
