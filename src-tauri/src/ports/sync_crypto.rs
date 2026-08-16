use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::domain::{SyncEncryptedEnvelope, SyncKdfProfile, SyncObjectIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCryptoErrorCode {
    InvalidInput,
    RandomnessFailed,
    KeyDerivationFailed,
    EncryptionFailed,
    AuthenticationFailed,
    IdentityMismatch,
    ProfileMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncCryptoError {
    pub code: SyncCryptoErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl SyncCryptoError {
    pub fn new(code: SyncCryptoErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for SyncCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncCryptoError {}

pub struct SyncPlaintext(Zeroizing<Vec<u8>>);

impl SyncPlaintext {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SyncPlaintext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncPlaintext")
            .field("bytes", &self.0.len())
            .finish()
    }
}

pub trait SyncCryptoRandom: Send + Sync {
    fn fill_bytes(&self, destination: &mut [u8]) -> Result<(), SyncCryptoError>;
}

pub trait SyncCryptoSession: fmt::Debug + Send + Sync {
    fn seal(
        &self,
        identity: &SyncObjectIdentity,
        plaintext: &[u8],
    ) -> Result<SyncEncryptedEnvelope, SyncCryptoError>;

    fn open(
        &self,
        expected_identity: &SyncObjectIdentity,
        envelope: &SyncEncryptedEnvelope,
    ) -> Result<SyncPlaintext, SyncCryptoError>;
}

pub trait SyncCryptoPort: Send + Sync {
    fn create_profile(&self) -> Result<SyncKdfProfile, SyncCryptoError>;

    fn unlock(
        &self,
        passphrase: &[u8],
        profile: &SyncKdfProfile,
    ) -> Result<Box<dyn SyncCryptoSession>, SyncCryptoError>;
}
