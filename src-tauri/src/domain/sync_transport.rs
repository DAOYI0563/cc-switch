use std::fmt;

use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorCode};

const MAX_REMOTE_PATH_SEGMENTS: usize = 64;
const MAX_REMOTE_PATH_SEGMENT_BYTES: usize = 255;
const MAX_REMOTE_PATH_BYTES: usize = 4096;
const MAX_ETAG_BYTES: usize = 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct SyncRemotePath(Vec<String>);

impl SyncRemotePath {
    pub fn new<I, S>(segments: I) -> Result<Self, DomainError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::try_from(segments.into_iter().map(Into::into).collect::<Vec<_>>())
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }
}

impl fmt::Debug for SyncRemotePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncRemotePath")
            .field("segments", &self.0.len())
            .finish()
    }
}

impl TryFrom<Vec<String>> for SyncRemotePath {
    type Error = DomainError;

    fn try_from(segments: Vec<String>) -> Result<Self, Self::Error> {
        if segments.is_empty() || segments.len() > MAX_REMOTE_PATH_SEGMENTS {
            return Err(invalid_transport("sync remote path has an invalid depth"));
        }
        let mut total_bytes = 0_usize;
        for segment in &segments {
            let invalid = segment.is_empty()
                || matches!(segment.as_str(), "." | "..")
                || segment.len() > MAX_REMOTE_PATH_SEGMENT_BYTES
                || segment
                    .chars()
                    .any(|character| character.is_control() || matches!(character, '/' | '\\'));
            if invalid {
                return Err(invalid_transport(
                    "sync remote path contains an invalid segment",
                ));
            }
            total_bytes = total_bytes.saturating_add(segment.len());
        }
        if total_bytes > MAX_REMOTE_PATH_BYTES {
            return Err(invalid_transport("sync remote path exceeds its size limit"));
        }
        Ok(Self(segments))
    }
}

impl From<SyncRemotePath> for Vec<String> {
    fn from(path: SyncRemotePath) -> Self {
        path.0
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SyncEtag(String);

impl SyncEtag {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        Self::try_from(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SyncEtag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SyncEtag([redacted])")
    }
}

impl TryFrom<String> for SyncEtag {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() || value.len() > MAX_ETAG_BYTES {
            return Err(invalid_transport("sync ETag has an invalid length"));
        }
        let opaque = value.strip_prefix("W/").unwrap_or(&value);
        if opaque.len() < 2 || !opaque.starts_with('"') || !opaque.ends_with('"') {
            return Err(invalid_transport("sync ETag has an invalid format"));
        }
        let contents = &opaque.as_bytes()[1..opaque.len() - 1];
        if !contents
            .iter()
            .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte) || *byte >= 0x80)
        {
            return Err(invalid_transport("sync ETag has an invalid format"));
        }
        Ok(Self(value))
    }
}

impl From<SyncEtag> for String {
    fn from(etag: SyncEtag) -> Self {
        etag.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncWriteCondition {
    Match(SyncEtag),
    CreateOnly,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncRemoteObject {
    bytes: Vec<u8>,
    etag: Option<SyncEtag>,
}

impl SyncRemoteObject {
    pub fn new(bytes: Vec<u8>, etag: Option<SyncEtag>) -> Self {
        Self { bytes, etag }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn etag(&self) -> Option<&SyncEtag> {
        self.etag.as_ref()
    }
}

impl fmt::Debug for SyncRemoteObject {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncRemoteObject")
            .field("bytes", &self.bytes.len())
            .field("has_etag", &self.etag.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncWriteReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<SyncEtag>,
}

impl SyncWriteReceipt {
    pub fn new(etag: Option<SyncEtag>) -> Self {
        Self { etag }
    }

    pub fn etag(&self) -> Option<&SyncEtag> {
        self.etag.as_ref()
    }
}

fn invalid_transport(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
