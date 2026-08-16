use std::fmt;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{DomainError, DomainErrorCode, PortableRecordId, SyncProtocolVersion};

pub const SYNC_KDF_SALT_BYTES: usize = 16;
pub const SYNC_GCM_NONCE_BYTES: usize = 12;
pub const SYNC_GCM_TAG_BYTES: usize = 16;
pub const MAX_SYNC_PLAINTEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_SYNC_CIPHERTEXT_BYTES: usize = MAX_SYNC_PLAINTEXT_BYTES + SYNC_GCM_TAG_BYTES;
const AAD_PREFIX: &[u8] = b"com.zhldm.wsl-code-switch\0sync-v3-aad\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum SyncKdfVersion {
    V1,
}

impl TryFrom<u32> for SyncKdfVersion {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            _ => Err(format!("unsupported sync KDF version {value}")),
        }
    }
}

impl From<SyncKdfVersion> for u32 {
    fn from(value: SyncKdfVersion) -> Self {
        match value {
            SyncKdfVersion::V1 => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum SyncEnvelopeVersion {
    V1,
}

impl TryFrom<u32> for SyncEnvelopeVersion {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            _ => Err(format!("unsupported sync envelope version {value}")),
        }
    }
}

impl From<SyncEnvelopeVersion> for u32 {
    fn from(value: SyncEnvelopeVersion) -> Self {
        match value {
            SyncEnvelopeVersion::V1 => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum SyncAadVersion {
    V1,
}

impl TryFrom<u32> for SyncAadVersion {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            _ => Err(format!("unsupported sync AAD version {value}")),
        }
    }
}

impl From<SyncAadVersion> for u32 {
    fn from(value: SyncAadVersion) -> Self {
        match value {
            SyncAadVersion::V1 => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncKdfAlgorithm {
    Argon2id,
}

impl SyncKdfAlgorithm {
    const fn code(self) -> u8 {
        match self {
            Self::Argon2id => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCipher {
    #[serde(rename = "aes_256_gcm")]
    Aes256Gcm,
}

impl SyncCipher {
    const fn code(self) -> u8 {
        match self {
            Self::Aes256Gcm => 1,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncKdfSalt([u8; SYNC_KDF_SALT_BYTES]);

impl SyncKdfSalt {
    pub fn new(bytes: [u8; SYNC_KDF_SALT_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; SYNC_KDF_SALT_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SyncKdfSalt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SyncKdfSalt([16 bytes])")
    }
}

impl Serialize for SyncKdfSalt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_base64())
    }
}

impl<'de> Deserialize<'de> for SyncKdfSalt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = BASE64.decode(&encoded).map_err(D::Error::custom)?;
        let bytes: [u8; SYNC_KDF_SALT_BYTES] = decoded.try_into().map_err(|_| {
            D::Error::custom(format!(
                "sync KDF salt must contain exactly {SYNC_KDF_SALT_BYTES} bytes"
            ))
        })?;
        let salt = Self::new(bytes);
        if salt.as_base64() != encoded {
            return Err(D::Error::custom("sync KDF salt must use canonical base64"));
        }
        Ok(salt)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "SyncKdfProfileWire"
)]
pub struct SyncKdfProfile {
    kdf_version: SyncKdfVersion,
    algorithm: SyncKdfAlgorithm,
    argon2_version: u32,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_length: u32,
    salt: SyncKdfSalt,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncKdfProfileWire {
    kdf_version: SyncKdfVersion,
    algorithm: SyncKdfAlgorithm,
    argon2_version: u32,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    output_length: u32,
    salt: SyncKdfSalt,
}

impl SyncKdfProfile {
    pub const ARGON2_VERSION: u32 = 0x13;
    pub const MEMORY_KIB: u32 = 65_536;
    pub const ITERATIONS: u32 = 3;
    pub const PARALLELISM: u32 = 1;
    pub const OUTPUT_LENGTH: u32 = 32;

    pub fn recommended(salt: SyncKdfSalt) -> Self {
        Self {
            kdf_version: SyncKdfVersion::V1,
            algorithm: SyncKdfAlgorithm::Argon2id,
            argon2_version: Self::ARGON2_VERSION,
            memory_kib: Self::MEMORY_KIB,
            iterations: Self::ITERATIONS,
            parallelism: Self::PARALLELISM,
            output_length: Self::OUTPUT_LENGTH,
            salt,
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let supported = self.kdf_version == SyncKdfVersion::V1
            && self.algorithm == SyncKdfAlgorithm::Argon2id
            && self.argon2_version == Self::ARGON2_VERSION
            && self.memory_kib == Self::MEMORY_KIB
            && self.iterations == Self::ITERATIONS
            && self.parallelism == Self::PARALLELISM
            && self.output_length == Self::OUTPUT_LENGTH;
        if supported {
            Ok(())
        } else {
            Err(invalid_crypto("unsupported sync KDF profile"))
        }
    }

    pub fn version_number(&self) -> u32 {
        self.kdf_version.into()
    }

    pub const fn algorithm(&self) -> SyncKdfAlgorithm {
        self.algorithm
    }

    pub const fn argon2_version(&self) -> u32 {
        self.argon2_version
    }

    pub const fn memory_kib(&self) -> u32 {
        self.memory_kib
    }

    pub const fn iterations(&self) -> u32 {
        self.iterations
    }

    pub const fn parallelism(&self) -> u32 {
        self.parallelism
    }

    pub const fn output_length(&self) -> u32 {
        self.output_length
    }

    pub fn salt_base64(&self) -> String {
        self.salt.as_base64()
    }

    pub(crate) fn salt(&self) -> &SyncKdfSalt {
        &self.salt
    }
}

impl TryFrom<SyncKdfProfileWire> for SyncKdfProfile {
    type Error = DomainError;

    fn try_from(wire: SyncKdfProfileWire) -> Result<Self, Self::Error> {
        let profile = Self {
            kdf_version: wire.kdf_version,
            algorithm: wire.algorithm,
            argon2_version: wire.argon2_version,
            memory_kib: wire.memory_kib,
            iterations: wire.iterations,
            parallelism: wire.parallelism,
            output_length: wire.output_length,
            salt: wire.salt,
        };
        profile.validate()?;
        Ok(profile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncObjectType {
    Manifest,
    Record,
}

impl SyncObjectType {
    const fn code(self) -> u8 {
        match self {
            Self::Manifest => 1,
            Self::Record => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "SyncObjectIdentityWire"
)]
pub struct SyncObjectIdentity {
    aad_version: SyncAadVersion,
    protocol_version: SyncProtocolVersion,
    object_type: SyncObjectType,
    #[serde(skip_serializing_if = "Option::is_none")]
    record_id: Option<PortableRecordId>,
    object_version: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncObjectIdentityWire {
    aad_version: SyncAadVersion,
    protocol_version: SyncProtocolVersion,
    object_type: SyncObjectType,
    #[serde(default)]
    record_id: Option<PortableRecordId>,
    object_version: u64,
}

impl SyncObjectIdentity {
    pub fn manifest(object_version: u64) -> Result<Self, DomainError> {
        Self::new(SyncObjectType::Manifest, None, object_version)
    }

    pub fn record(record_id: PortableRecordId, object_version: u64) -> Result<Self, DomainError> {
        Self::new(SyncObjectType::Record, Some(record_id), object_version)
    }

    fn new(
        object_type: SyncObjectType,
        record_id: Option<PortableRecordId>,
        object_version: u64,
    ) -> Result<Self, DomainError> {
        let identity = Self {
            aad_version: SyncAadVersion::V1,
            protocol_version: SyncProtocolVersion::V3,
            object_type,
            record_id,
            object_version,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.aad_version != SyncAadVersion::V1
            || self.protocol_version != SyncProtocolVersion::V3
        {
            return Err(invalid_crypto("unsupported sync object identity version"));
        }
        if self.object_version == 0 {
            return Err(invalid_crypto("sync object version must be positive"));
        }
        match (self.object_type, &self.record_id) {
            (SyncObjectType::Manifest, None) | (SyncObjectType::Record, Some(_)) => Ok(()),
            (SyncObjectType::Manifest, Some(_)) => Err(invalid_crypto(
                "manifest identity must not contain a record id",
            )),
            (SyncObjectType::Record, None) => {
                Err(invalid_crypto("record identity requires a record id"))
            }
        }
    }

    pub const fn object_type(&self) -> SyncObjectType {
        self.object_type
    }

    pub const fn object_version(&self) -> u64 {
        self.object_version
    }

    pub fn record_id(&self) -> Option<&PortableRecordId> {
        self.record_id.as_ref()
    }
}

impl TryFrom<SyncObjectIdentityWire> for SyncObjectIdentity {
    type Error = DomainError;

    fn try_from(wire: SyncObjectIdentityWire) -> Result<Self, Self::Error> {
        let identity = Self {
            aad_version: wire.aad_version,
            protocol_version: wire.protocol_version,
            object_type: wire.object_type,
            record_id: wire.record_id,
            object_version: wire.object_version,
        };
        identity.validate()?;
        Ok(identity)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncNonce([u8; SYNC_GCM_NONCE_BYTES]);

impl SyncNonce {
    pub fn new(bytes: [u8; SYNC_GCM_NONCE_BYTES]) -> Self {
        Self(bytes)
    }

    pub fn as_base64(&self) -> String {
        BASE64.encode(self.0)
    }

    pub(crate) fn as_bytes(&self) -> &[u8; SYNC_GCM_NONCE_BYTES] {
        &self.0
    }
}

impl fmt::Debug for SyncNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SyncNonce([12 bytes])")
    }
}

impl Serialize for SyncNonce {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_base64())
    }
}

impl<'de> Deserialize<'de> for SyncNonce {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = BASE64.decode(&encoded).map_err(D::Error::custom)?;
        let bytes: [u8; SYNC_GCM_NONCE_BYTES] = decoded.try_into().map_err(|_| {
            D::Error::custom(format!(
                "sync nonce must contain exactly {SYNC_GCM_NONCE_BYTES} bytes"
            ))
        })?;
        let nonce = Self::new(bytes);
        if nonce.as_base64() != encoded {
            return Err(D::Error::custom("sync nonce must use canonical base64"));
        }
        Ok(nonce)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SyncCiphertext(Vec<u8>);

impl SyncCiphertext {
    pub fn new(bytes: Vec<u8>) -> Result<Self, DomainError> {
        if bytes.len() < SYNC_GCM_TAG_BYTES || bytes.len() > MAX_SYNC_CIPHERTEXT_BYTES {
            return Err(invalid_crypto("sync ciphertext has an invalid length"));
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_base64(&self) -> String {
        BASE64.encode(&self.0)
    }
}

impl fmt::Debug for SyncCiphertext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncCiphertext")
            .field("bytes", &self.0.len())
            .finish()
    }
}

impl Serialize for SyncCiphertext {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_base64())
    }
}

impl<'de> Deserialize<'de> for SyncCiphertext {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = BASE64.decode(&encoded).map_err(D::Error::custom)?;
        let ciphertext = Self::new(decoded).map_err(D::Error::custom)?;
        if ciphertext.as_base64() != encoded {
            return Err(D::Error::custom(
                "sync ciphertext must use canonical base64",
            ));
        }
        Ok(ciphertext)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "SyncEncryptedEnvelopeWire"
)]
pub struct SyncEncryptedEnvelope {
    envelope_version: SyncEnvelopeVersion,
    cipher: SyncCipher,
    kdf: SyncKdfProfile,
    identity: SyncObjectIdentity,
    nonce: SyncNonce,
    ciphertext: SyncCiphertext,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncEncryptedEnvelopeWire {
    envelope_version: SyncEnvelopeVersion,
    cipher: SyncCipher,
    kdf: SyncKdfProfile,
    identity: SyncObjectIdentity,
    nonce: SyncNonce,
    ciphertext: SyncCiphertext,
}

impl SyncEncryptedEnvelope {
    pub(crate) fn new(
        kdf: SyncKdfProfile,
        identity: SyncObjectIdentity,
        nonce: SyncNonce,
        ciphertext: SyncCiphertext,
    ) -> Result<Self, DomainError> {
        let envelope = Self {
            envelope_version: SyncEnvelopeVersion::V1,
            cipher: SyncCipher::Aes256Gcm,
            kdf,
            identity,
            nonce,
            ciphertext,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.envelope_version != SyncEnvelopeVersion::V1 || self.cipher != SyncCipher::Aes256Gcm
        {
            return Err(invalid_crypto("unsupported sync encryption envelope"));
        }
        self.kdf.validate()?;
        self.identity.validate()?;
        Ok(())
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| invalid_crypto("failed to encode sync envelope"))
    }

    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, DomainError> {
        let envelope: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid_crypto("invalid sync encryption envelope"))?;
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn identity(&self) -> &SyncObjectIdentity {
        &self.identity
    }

    pub fn kdf(&self) -> &SyncKdfProfile {
        &self.kdf
    }

    pub fn ciphertext(&self) -> &SyncCiphertext {
        &self.ciphertext
    }

    pub fn nonce_base64(&self) -> String {
        self.nonce.as_base64()
    }

    pub fn ciphertext_base64(&self) -> String {
        self.ciphertext.as_base64()
    }

    pub fn with_ciphertext(&self, ciphertext: SyncCiphertext) -> Result<Self, DomainError> {
        Self::new(
            self.kdf.clone(),
            self.identity.clone(),
            self.nonce.clone(),
            ciphertext,
        )
    }

    pub(crate) fn nonce(&self) -> &SyncNonce {
        &self.nonce
    }

    pub(crate) fn authenticated_metadata_bytes(
        kdf: &SyncKdfProfile,
        identity: &SyncObjectIdentity,
    ) -> Result<Vec<u8>, DomainError> {
        kdf.validate()?;
        identity.validate()?;
        let mut bytes = Vec::with_capacity(160);
        bytes.extend_from_slice(AAD_PREFIX);
        push_u32(&mut bytes, u32::from(SyncEnvelopeVersion::V1));
        bytes.push(SyncCipher::Aes256Gcm.code());
        push_u32(&mut bytes, kdf.version_number());
        bytes.push(kdf.algorithm.code());
        push_u32(&mut bytes, kdf.argon2_version);
        push_u32(&mut bytes, kdf.memory_kib);
        push_u32(&mut bytes, kdf.iterations);
        push_u32(&mut bytes, kdf.parallelism);
        push_u32(&mut bytes, kdf.output_length);
        push_bytes(&mut bytes, kdf.salt.as_bytes())?;
        push_u32(&mut bytes, u32::from(identity.aad_version));
        push_u32(&mut bytes, u32::from(identity.protocol_version));
        bytes.push(identity.object_type.code());
        push_u64(&mut bytes, identity.object_version);
        match &identity.record_id {
            Some(record_id) => {
                push_bytes(&mut bytes, record_id.domain.as_str().as_bytes())?;
                push_bytes(&mut bytes, record_id.key.as_bytes())?;
            }
            None => {
                push_bytes(&mut bytes, &[])?;
                push_bytes(&mut bytes, &[])?;
            }
        }
        Ok(bytes)
    }
}

impl TryFrom<SyncEncryptedEnvelopeWire> for SyncEncryptedEnvelope {
    type Error = DomainError;

    fn try_from(wire: SyncEncryptedEnvelopeWire) -> Result<Self, Self::Error> {
        let envelope = Self {
            envelope_version: wire.envelope_version,
            cipher: wire.cipher,
            kdf: wire.kdf,
            identity: wire.identity,
            nonce: wire.nonce,
            ciphertext: wire.ciphertext,
        };
        envelope.validate()?;
        Ok(envelope)
    }
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn push_bytes(target: &mut Vec<u8>, value: &[u8]) -> Result<(), DomainError> {
    let length = u32::try_from(value.len())
        .map_err(|_| invalid_crypto("sync AAD field exceeds its length limit"))?;
    push_u32(target, length);
    target.extend_from_slice(value);
    Ok(())
}

fn invalid_crypto(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
