use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{DomainError, DomainErrorCode};

const MAX_ID_BYTES: usize = 256;
const MAX_DEVICE_NAME_BYTES: usize = 128;
const MAX_PAYLOAD_DEPTH: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum SyncProtocolVersion {
    V3,
}

impl TryFrom<u32> for SyncProtocolVersion {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            3 => Ok(Self::V3),
            _ => Err(format!("unsupported sync protocol version {value}")),
        }
    }
}

impl From<SyncProtocolVersion> for u32 {
    fn from(value: SyncProtocolVersion) -> Self {
        match value {
            SyncProtocolVersion::V3 => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub enum SyncSchemaVersion {
    V1,
}

impl TryFrom<u32> for SyncSchemaVersion {
    type Error = String;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::V1),
            _ => Err(format!("unsupported sync schema version {value}")),
        }
    }
}

impl From<SyncSchemaVersion> for u32 {
    fn from(value: SyncSchemaVersion) -> Self {
        match value {
            SyncSchemaVersion::V1 => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SyncDeviceId(String);

impl SyncDeviceId {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        validate_identifier("sync device id", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SyncDeviceId {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SyncDeviceId> for String {
    fn from(value: SyncDeviceId) -> Self {
        value.0
    }
}

impl fmt::Display for SyncDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let valid = value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !valid {
            return Err(DomainError::new(
                DomainErrorCode::InvalidHash,
                "sync digest must contain 64 lowercase hexadecimal characters",
            ));
        }
        Ok(Self(value))
    }

    pub fn of_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(encoded)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Sha256Digest {
    type Error = DomainError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Sha256Digest> for String {
    fn from(value: Sha256Digest) -> Self {
        value.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Portable domains are the only user-data classes accepted by sync-v3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortableDomain {
    Provider,
    Mcp,
    Prompt,
    Skill,
    CommonSnippet,
    DailyBrief,
    PortableSetting,
}

impl PortableDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Mcp => "mcp",
            Self::Prompt => "prompt",
            Self::Skill => "skill",
            Self::CommonSnippet => "common_snippet",
            Self::DailyBrief => "daily_brief",
            Self::PortableSetting => "portable_setting",
        }
    }
}

impl PartialOrd for PortableDomain {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PortableDomain {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "PortableRecordIdWire"
)]
pub struct PortableRecordId {
    pub domain: PortableDomain,
    pub key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableRecordIdWire {
    domain: PortableDomain,
    key: String,
}

impl PortableRecordId {
    pub fn new(domain: PortableDomain, key: impl Into<String>) -> Result<Self, DomainError> {
        let key = key.into();
        validate_identifier("record key", &key)?;
        Ok(Self { domain, key })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        validate_identifier("record key", &self.key)
    }
}

impl TryFrom<PortableRecordIdWire> for PortableRecordId {
    type Error = DomainError;

    fn try_from(value: PortableRecordIdWire) -> Result<Self, Self::Error> {
        Self::new(value.domain, value.key)
    }
}

impl PartialOrd for PortableRecordId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PortableRecordId {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.domain, self.key.as_str()).cmp(&(other.domain, other.key.as_str()))
    }
}

impl fmt::Display for PortableRecordId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.domain.as_str(), self.key)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PortableJsonObject(BTreeMap<String, Value>);

impl PortableJsonObject {
    fn new(value: Value) -> Result<Self, DomainError> {
        let Value::Object(object) = value else {
            return Err(invalid_record(
                "portable payload content must be a JSON object",
            ));
        };
        let entries = object
            .into_iter()
            .map(|(key, value)| (key, canonicalize_json(value)))
            .collect();
        let payload = Self(entries);
        payload.validate()?;
        Ok(payload)
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self.0.is_empty() {
            return Err(invalid_record("portable payload content must not be empty"));
        }
        validate_payload_object(&self.0, 0, "content")
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    fn as_value(&self) -> Value {
        canonical_object_value(&self.0)
    }
}

impl Serialize for PortableJsonObject {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.as_value().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PortableJsonObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

/// A versioned payload whose domain is repeated and checked against its record ID.
/// The content object is canonicalized and rejects local-only or secret-bearing fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortablePayload {
    pub schema_version: SyncSchemaVersion,
    pub domain: PortableDomain,
    content: PortableJsonObject,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortablePayloadWire {
    schema_version: SyncSchemaVersion,
    domain: PortableDomain,
    content: PortableJsonObject,
}

impl PortablePayload {
    pub fn new(domain: PortableDomain, content: Value) -> Result<Self, DomainError> {
        let payload = Self {
            schema_version: SyncSchemaVersion::V1,
            domain,
            content: PortableJsonObject::new(content)?,
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn content(&self) -> Value {
        self.content.as_value()
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        self.content.validate()?;
        let allowed = allowed_top_level_fields(self.domain);
        if let Some(field) = self.content.keys().find(|field| !allowed.contains(field)) {
            return Err(invalid_record(format!(
                "field '{field}' is not part of the {} portable payload schema",
                self.domain.as_str()
            ))
            .with_context("field", field));
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for PortablePayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PortablePayloadWire::deserialize(deserializer)?;
        let payload = Self {
            schema_version: wire.schema_version,
            domain: wire.domain,
            content: wire.content,
        };
        payload.validate().map_err(D::Error::custom)?;
        Ok(payload)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordRevision {
    pub schema_version: SyncSchemaVersion,
    pub device_id: SyncDeviceId,
    pub counter: u64,
    pub content_hash: Sha256Digest,
    pub updated_at_ms: i64,
}

impl RecordRevision {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.counter == 0 {
            return Err(invalid_record(
                "record revision counter must be greater than zero",
            ));
        }
        validate_timestamp("record revision update time", self.updated_at_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneRetention {
    Permanent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermanentTombstone {
    pub schema_version: SyncSchemaVersion,
    pub deleted_at_ms: i64,
    pub deleted_by_device_id: SyncDeviceId,
    pub introduced_generation: u64,
    pub retention: TombstoneRetention,
}

impl PermanentTombstone {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_timestamp("tombstone deletion time", self.deleted_at_ms)?;
        if self.introduced_generation == 0 {
            return Err(invalid_record(
                "tombstone introduction generation must be greater than zero",
            ));
        }
        Ok(())
    }

    pub fn can_compact_after<I>(&self, acknowledged_generations: I) -> bool
    where
        I: IntoIterator<Item = u64>,
    {
        let mut saw_device = false;
        for generation in acknowledged_generations {
            saw_device = true;
            if generation < self.introduced_generation {
                return false;
            }
        }
        saw_device
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRecordState {
    Live,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRecord {
    pub schema_version: SyncSchemaVersion,
    pub id: PortableRecordId,
    pub revision: RecordRevision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<PortablePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tombstone: Option<PermanentTombstone>,
}

impl SyncRecord {
    pub fn live(
        id: PortableRecordId,
        device_id: SyncDeviceId,
        counter: u64,
        updated_at_ms: i64,
        payload: PortablePayload,
    ) -> Result<Self, DomainError> {
        if payload.domain != id.domain {
            return Err(invalid_record("record ID and payload domains must match"));
        }
        payload.validate()?;
        let content_hash = hash_json(&payload)?;
        let record = Self {
            schema_version: SyncSchemaVersion::V1,
            id,
            revision: RecordRevision {
                schema_version: SyncSchemaVersion::V1,
                device_id,
                counter,
                content_hash,
                updated_at_ms,
            },
            payload: Some(payload),
            tombstone: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn deleted(
        id: PortableRecordId,
        device_id: SyncDeviceId,
        counter: u64,
        deleted_at_ms: i64,
        introduced_generation: u64,
    ) -> Result<Self, DomainError> {
        let tombstone = PermanentTombstone {
            schema_version: SyncSchemaVersion::V1,
            deleted_at_ms,
            deleted_by_device_id: device_id.clone(),
            introduced_generation,
            retention: TombstoneRetention::Permanent,
        };
        tombstone.validate()?;
        let content_hash = hash_json(&tombstone)?;
        let record = Self {
            schema_version: SyncSchemaVersion::V1,
            id,
            revision: RecordRevision {
                schema_version: SyncSchemaVersion::V1,
                device_id,
                counter,
                content_hash,
                updated_at_ms: deleted_at_ms,
            },
            payload: None,
            tombstone: Some(tombstone),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn state(&self) -> SyncRecordState {
        if self.tombstone.is_some() {
            SyncRecordState::Deleted
        } else {
            SyncRecordState::Live
        }
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        self.id.validate()?;
        self.revision.validate()?;
        match (&self.payload, &self.tombstone) {
            (Some(payload), None) => {
                payload.validate()?;
                if payload.domain != self.id.domain {
                    return Err(invalid_record("record ID and payload domains must match"));
                }
                let expected_hash = hash_json(payload)?;
                if self.revision.content_hash != expected_hash {
                    return Err(invalid_record(
                        "live record content hash does not match payload",
                    ));
                }
            }
            (None, Some(tombstone)) => {
                tombstone.validate()?;
                if tombstone.deleted_by_device_id != self.revision.device_id {
                    return Err(invalid_record(
                        "tombstone author must own the deletion revision",
                    ));
                }
                if tombstone.deleted_at_ms != self.revision.updated_at_ms {
                    return Err(invalid_record(
                        "tombstone deletion time must match the deletion revision",
                    ));
                }
                let expected_hash = hash_json(tombstone)?;
                if self.revision.content_hash != expected_hash {
                    return Err(invalid_record(
                        "deleted record content hash does not match tombstone",
                    ));
                }
            }
            _ => {
                return Err(invalid_record(
                    "sync record must contain exactly one live payload or permanent tombstone",
                ));
            }
        }
        Ok(())
    }

    pub fn to_canonical_json_bytes(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        encode_json(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, DomainError> {
        let record: Self = decode_json(bytes, "sync record")?;
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRecordBaseline {
    pub schema_version: SyncSchemaVersion,
    pub confirmed_generation: u64,
    pub record: SyncRecord,
}

impl SyncRecordBaseline {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.confirmed_generation == 0 {
            return Err(invalid_record(
                "baseline confirmation generation must be greater than zero",
            ));
        }
        self.record.validate()?;
        if let Some(tombstone) = &self.record.tombstone {
            if tombstone.introduced_generation > self.confirmed_generation {
                return Err(invalid_record(
                    "baseline cannot confirm a tombstone from a future generation",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRecordIndexEntry {
    pub schema_version: SyncSchemaVersion,
    pub id: PortableRecordId,
    pub revision: RecordRevision,
    pub state: SyncRecordState,
    pub record_sha256: Sha256Digest,
}

impl SyncRecordIndexEntry {
    pub fn from_record(record: &SyncRecord) -> Result<Self, DomainError> {
        let bytes = record.to_canonical_json_bytes()?;
        Ok(Self {
            schema_version: SyncSchemaVersion::V1,
            id: record.id.clone(),
            revision: record.revision.clone(),
            state: record.state(),
            record_sha256: Sha256Digest::of_bytes(&bytes),
        })
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        self.id.validate()?;
        self.revision.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncDeviceStatus {
    Active,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDevice {
    pub schema_version: SyncSchemaVersion,
    pub device_id: SyncDeviceId,
    pub display_name: String,
    pub acknowledged_generation: u64,
    pub registered_at_ms: i64,
    pub last_seen_at_ms: i64,
    pub status: SyncDeviceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired_at_ms: Option<i64>,
}

impl SyncDevice {
    pub fn validate(&self) -> Result<(), DomainError> {
        let display_name = self.display_name.trim();
        if display_name.is_empty()
            || display_name.len() > MAX_DEVICE_NAME_BYTES
            || display_name.chars().any(char::is_control)
        {
            return Err(invalid_record("sync device display name is invalid"));
        }
        validate_timestamp("device registration time", self.registered_at_ms)?;
        validate_timestamp("device last-seen time", self.last_seen_at_ms)?;
        if self.last_seen_at_ms < self.registered_at_ms {
            return Err(invalid_record(
                "device last-seen time must not precede registration",
            ));
        }
        match (self.status, self.retired_at_ms) {
            (SyncDeviceStatus::Active, None) => Ok(()),
            (SyncDeviceStatus::Retired, Some(retired_at_ms))
                if retired_at_ms >= self.last_seen_at_ms =>
            {
                Ok(())
            }
            _ => Err(invalid_record(
                "device status and retirement time are inconsistent",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncV3Manifest {
    pub protocol_version: SyncProtocolVersion,
    pub schema_version: SyncSchemaVersion,
    pub generation: u64,
    pub generated_at_ms: i64,
    pub generated_by_device_id: SyncDeviceId,
    pub records: Vec<SyncRecordIndexEntry>,
    pub devices: Vec<SyncDevice>,
}

impl SyncV3Manifest {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.generation == 0 {
            return Err(invalid_record(
                "manifest generation must be greater than zero",
            ));
        }
        validate_timestamp("manifest generation time", self.generated_at_ms)?;
        validate_strict_order(
            &self.records,
            |entry| &entry.id,
            "manifest record index must be strictly sorted without duplicates",
        )?;
        validate_strict_order(
            &self.devices,
            |device| &device.device_id,
            "manifest device index must be strictly sorted without duplicates",
        )?;
        if self.devices.is_empty() {
            return Err(invalid_record("manifest must register at least one device"));
        }

        for device in &self.devices {
            device.validate()?;
            if device.acknowledged_generation > self.generation {
                return Err(invalid_record(
                    "device cannot acknowledge a future manifest generation",
                ));
            }
            if device.registered_at_ms > self.generated_at_ms
                || device.last_seen_at_ms > self.generated_at_ms
            {
                return Err(invalid_record(
                    "device timestamps cannot be newer than the manifest",
                ));
            }
        }

        let writer = self
            .devices
            .iter()
            .find(|device| device.device_id == self.generated_by_device_id)
            .ok_or_else(|| invalid_record("manifest writer must be a registered device"))?;
        if writer.status != SyncDeviceStatus::Active {
            return Err(invalid_record("a retired device cannot write a manifest"));
        }

        for entry in &self.records {
            entry.validate()?;
            if entry.revision.updated_at_ms > self.generated_at_ms {
                return Err(invalid_record(
                    "record revision cannot be newer than the manifest",
                ));
            }
            if !self
                .devices
                .iter()
                .any(|device| device.device_id == entry.revision.device_id)
            {
                return Err(invalid_record(
                    "record revision owner must be present in the device registry",
                ));
            }
        }
        Ok(())
    }

    pub fn to_canonical_json_bytes(&self) -> Result<Vec<u8>, DomainError> {
        self.validate()?;
        encode_json(self)
    }

    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, DomainError> {
        let manifest: Self = decode_json(bytes, "sync manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), DomainError> {
    let valid = !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err(
            DomainError::new(DomainErrorCode::InvalidId, format!("invalid {label}"))
                .with_context("value", value),
        )
    }
}

fn validate_timestamp(label: &str, value: i64) -> Result<(), DomainError> {
    if value >= 0 {
        Ok(())
    } else {
        Err(invalid_record(format!("{label} must not be negative")))
    }
}

fn invalid_record(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}

fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    serde_json::to_vec(value).map_err(|_| invalid_record("failed to encode canonical sync-v3 JSON"))
}

fn decode_json<T>(bytes: &[u8], label: &str) -> Result<T, DomainError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_slice(bytes).map_err(|error| {
        invalid_record(format!("invalid {label} JSON: {error}")).with_context("objectType", label)
    })
}

fn hash_json<T: Serialize>(value: &T) -> Result<Sha256Digest, DomainError> {
    Ok(Sha256Digest::of_bytes(&encode_json(value)?))
}

fn validate_strict_order<T, K, F>(values: &[T], key: F, message: &str) -> Result<(), DomainError>
where
    K: Ord + ?Sized,
    F: Fn(&T) -> &K,
{
    if values
        .windows(2)
        .any(|window| key(&window[0]) >= key(&window[1]))
    {
        Err(invalid_record(message))
    } else {
        Ok(())
    }
}

fn canonicalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let ordered = object
                .into_iter()
                .map(|(key, value)| (key, canonicalize_json(value)))
                .collect::<BTreeMap<_, _>>();
            canonical_object_value(&ordered)
        }
        scalar => scalar,
    }
}

fn canonical_object_value(object: &BTreeMap<String, Value>) -> Value {
    let mut canonical = serde_json::Map::with_capacity(object.len());
    for (key, value) in object {
        canonical.insert(key.clone(), canonicalize_json(value.clone()));
    }
    Value::Object(canonical)
}

fn validate_payload_object(
    object: &BTreeMap<String, Value>,
    depth: usize,
    path: &str,
) -> Result<(), DomainError> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(invalid_record("portable payload nesting is too deep"));
    }
    for (key, value) in object {
        validate_payload_field_name(key, path)?;
        validate_payload_value(value, depth + 1, &format!("{path}.{key}"))?;
    }
    Ok(())
}

fn validate_payload_value(value: &Value, depth: usize, path: &str) -> Result<(), DomainError> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(invalid_record("portable payload nesting is too deep"));
    }
    match value {
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_payload_value(value, depth + 1, &format!("{path}[{index}]"))?;
            }
        }
        Value::Object(object) => {
            let ordered = object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<BTreeMap<_, _>>();
            validate_payload_object(&ordered, depth, path)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_payload_field_name(field: &str, path: &str) -> Result<(), DomainError> {
    if field.is_empty() || field.len() > MAX_ID_BYTES || field.chars().any(char::is_control) {
        return Err(invalid_record("portable payload field name is invalid"));
    }
    let normalized = field
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect::<String>();

    let local_only = matches!(
        normalized.as_str(),
        "currentprovider"
            | "currentproviderid"
            | "activeprovider"
            | "selectedprovider"
            | "deviceid"
            | "devicename"
            | "fixedwslpath"
            | "wslpath"
            | "localpath"
            | "webdavurl"
            | "webdavusername"
            | "webdavpassword"
            | "syncpassphrase"
            | "syncpassword"
            | "briefmodel"
            | "briefmodelid"
            | "briefprovider"
            | "briefproviderid"
            | "briefapikey"
            | "runtimestate"
            | "processid"
            | "rawsession"
            | "sessionevents"
            | "sessionmessages"
            | "transcript"
            | "conversationhistory"
            | "searchindex"
            | "sessionindex"
            | "restorecommand"
            | "officialtoken"
            | "officialauthtoken"
            | "env"
            | "headers"
            | "authorization"
            | "credentials"
    );
    let credential = [
        "apikey",
        "accesstoken",
        "refreshtoken",
        "password",
        "passphrase",
        "secret",
        "cookie",
        "privatekey",
        "credential",
        "authorization",
    ]
    .iter()
    .any(|suffix| normalized == *suffix || normalized.ends_with(suffix));

    if local_only || credential {
        return Err(invalid_record(format!(
            "local-only or credential field '{path}.{field}' is forbidden in sync-v3 payloads"
        ))
        .with_context("field", field));
    }
    Ok(())
}

fn allowed_top_level_fields(domain: PortableDomain) -> &'static [&'static str] {
    match domain {
        PortableDomain::Provider => &[
            "clientId",
            "providerId",
            "kind",
            "name",
            "portableConfig",
            "sortIndex",
            "notes",
            "icon",
            "iconColor",
            "createdAtMs",
            "updatedAtMs",
        ],
        PortableDomain::Mcp => &[
            "id",
            "name",
            "serverConfig",
            "description",
            "homepage",
            "docs",
            "tags",
            "apps",
            "createdAtMs",
            "updatedAtMs",
        ],
        PortableDomain::Prompt => &[
            "id",
            "clientId",
            "name",
            "version",
            "content",
            "description",
            "isActive",
            "createdAtMs",
            "updatedAtMs",
        ],
        PortableDomain::Skill => &[
            "id",
            "name",
            "description",
            "directory",
            "contentHash",
            "totalSizeBytes",
            "fileCount",
            "apps",
            "cloudEligible",
            "files",
            "createdAtMs",
            "updatedAtMs",
        ],
        PortableDomain::CommonSnippet => &[
            "id",
            "clientId",
            "name",
            "content",
            "providerId",
            "enabled",
            "createdAtMs",
            "updatedAtMs",
        ],
        PortableDomain::DailyBrief => &[
            "date",
            "status",
            "html",
            "contentHash",
            "sourceFingerprint",
            "templateVersion",
            "promptVersion",
            "generatedAtMs",
            "updatedAtMs",
        ],
        PortableDomain::PortableSetting => &["key", "value", "updatedAtMs"],
    }
}
