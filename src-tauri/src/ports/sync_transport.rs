use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::domain::{SyncRemoteObject, SyncRemotePath, SyncWriteCondition, SyncWriteReceipt};

pub const MAX_SYNC_REMOTE_OBJECT_BYTES: usize = 24 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTransportErrorCode {
    InvalidConfiguration,
    InvalidInput,
    AuthenticationFailed,
    PreconditionFailed,
    LimitExceeded,
    Timeout,
    ConnectionFailed,
    HttpStatus,
    InvalidResponse,
    TransportFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncTransportError {
    pub code: SyncTransportErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl SyncTransportError {
    pub fn new(code: SyncTransportErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for SyncTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncTransportError {}

pub type SyncTransportFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, SyncTransportError>> + Send + 'a>>;

pub trait SyncTransportPort: Send + Sync {
    fn ensure_directories<'a>(&'a self, path: &'a SyncRemotePath) -> SyncTransportFuture<'a, ()>;

    fn read<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        max_bytes: usize,
    ) -> SyncTransportFuture<'a, Option<SyncRemoteObject>>;

    fn conditional_write<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        bytes: &'a [u8],
        condition: &'a SyncWriteCondition,
    ) -> SyncTransportFuture<'a, SyncWriteReceipt>;
}
