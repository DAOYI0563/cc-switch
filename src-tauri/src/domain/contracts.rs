use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{PortableRecordId, RecordRevision};

/// The complete set of managed command-line clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManagedClientId {
    Claude,
    Codex,
    Opencode,
}

impl ManagedClientId {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Opencode];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
        }
    }
}

impl FromStr for ManagedClientId {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::Opencode),
            unsupported => Err(DomainError::new(
                DomainErrorCode::UnsupportedClient,
                format!(
                    "unsupported managed client '{unsupported}'; allowed: claude, codex, opencode"
                ),
            )
            .with_context("clientId", unsupported)),
        }
    }
}

impl fmt::Display for ManagedClientId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Enablement state shared by every resource that can target managed clients.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedClientApps {
    #[serde(default)]
    pub claude: bool,
    #[serde(default)]
    pub codex: bool,
    #[serde(default)]
    pub opencode: bool,
}

impl ManagedClientApps {
    pub const fn is_enabled_for(&self, client: ManagedClientId) -> bool {
        match client {
            ManagedClientId::Claude => self.claude,
            ManagedClientId::Codex => self.codex,
            ManagedClientId::Opencode => self.opencode,
        }
    }

    pub fn set_enabled_for(&mut self, client: ManagedClientId, enabled: bool) {
        match client {
            ManagedClientId::Claude => self.claude = enabled,
            ManagedClientId::Codex => self.codex = enabled,
            ManagedClientId::Opencode => self.opencode = enabled,
        }
    }

    pub fn enabled_clients(&self) -> impl Iterator<Item = ManagedClientId> + '_ {
        ManagedClientId::ALL
            .into_iter()
            .filter(|client| self.is_enabled_for(*client))
    }

    pub const fn is_empty(&self) -> bool {
        !self.claude && !self.codex && !self.opencode
    }

    pub fn only(client: ManagedClientId) -> Self {
        let mut apps = Self::default();
        apps.set_enabled_for(client, true);
        apps
    }

    pub fn from_labels(labels: &[String]) -> Self {
        let mut apps = Self::default();
        for label in labels {
            if let Ok(client) = label.parse::<ManagedClientId>() {
                apps.set_enabled_for(client, true);
            }
        }
        apps
    }
}

/// Result state of probing the fixed legacy `.cc-switch` directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyMigrationStatus {
    NotFound,
    Empty,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySourceKind {
    Sqlite,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyRetainedCounts {
    pub claude_providers: u64,
    pub codex_providers: u64,
    pub opencode_providers: u64,
    pub mcp_servers: u64,
    pub claude_prompts: u64,
    pub codex_prompts: u64,
    pub opencode_prompts: u64,
    pub skills: u64,
    pub common_snippets: u64,
}

impl LegacyRetainedCounts {
    pub fn total(&self) -> u64 {
        self.claude_providers
            + self.codex_providers
            + self.opencode_providers
            + self.mcp_servers
            + self.claude_prompts
            + self.codex_prompts
            + self.opencode_prompts
            + self.skills
            + self.common_snippets
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyIgnoredCounts {
    pub non_target_client_records: u64,
    pub profiles: u64,
    pub proxy_and_routing: u64,
    pub usage_and_pricing: u64,
    pub failover: u64,
    pub online_skill_repositories: u64,
}

impl LegacyIgnoredCounts {
    pub fn total(&self) -> u64 {
        self.non_target_client_records
            + self.profiles
            + self.proxy_and_routing
            + self.usage_and_pricing
            + self.failover
            + self.online_skill_repositories
    }
}

/// A content-free digest of one recognized legacy source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyFileSummary {
    pub name: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Read-only migration preview. Configuration bodies and credentials are
/// intentionally absent from this cross-module contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationPreview {
    pub status: LegacyMigrationStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<LegacySourceKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<u32>,
    pub retained: LegacyRetainedCounts,
    pub ignored: LegacyIgnoredCounts,
    pub files: Vec<LegacyFileSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory_fingerprint: Option<String>,
}

/// High-risk operation covered by a short-lived local rollback point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackPointPurpose {
    DataMigration,
    WebdavSync,
    ConflictResolution,
    SkillIndexRefresh,
    RestoreOperation,
    RemoteReset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackPointState {
    Pending,
    Failed,
}

/// Content-free metadata for an encrypted, temporary rollback point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackPointMetadata {
    pub schema_version: u32,
    pub id: String,
    pub purpose: RollbackPointPurpose,
    pub state: RollbackPointState,
    pub created_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at_ms: Option<i64>,
    pub payload_size_bytes: u64,
    pub payload_sha256: String,
}

impl RollbackPointMetadata {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn validate(&self) -> Result<(), DomainError> {
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRollbackPoint,
                "unsupported rollback point metadata version",
            ));
        }
        validate_id_component("rollback point id", &self.id)?;
        validate_hash(&self.payload_sha256)?;
        if self.created_at_ms < 0 {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRollbackPoint,
                "rollback point creation time must not be negative",
            ));
        }
        match (self.state, self.failed_at_ms) {
            (RollbackPointState::Pending, None) => Ok(()),
            (RollbackPointState::Failed, Some(failed_at_ms))
                if failed_at_ms >= self.created_at_ms =>
            {
                Ok(())
            }
            _ => Err(DomainError::new(
                DomainErrorCode::InvalidRollbackPoint,
                "rollback point state and failure time are inconsistent",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    ConcurrentUpdate,
    UpdateDelete,
    AmbiguousLocalMatch,
    IntegrityMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    Pending,
    ResolvedLocal,
    ResolvedExternal,
    KeptBoth,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictSide {
    pub revision: RecordRevision,
    pub summary_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    pub conflict_id: String,
    pub record_id: PortableRecordId,
    pub kind: ConflictKind,
    pub status: ConflictStatus,
    pub local: ConflictSide,
    pub external: ConflictSide,
    pub detected_at_ms: i64,
}

impl ConflictRecord {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_id_component("conflict id", &self.conflict_id)?;
        self.local.revision.validate()?;
        self.external.revision.validate()?;
        validate_hash(&self.local.summary_hash)?;
        validate_hash(&self.external.summary_hash)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionEventRole {
    User,
    Assistant,
    Tool,
}

/// Client-independent input to local search and daily-brief generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSessionEvent {
    pub client_id: ManagedClientId,
    pub session_id: String,
    pub event_id: String,
    pub role: SessionEventRole,
    pub occurred_at_ms: i64,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
}

impl NormalizedSessionEvent {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_id_component("session id", &self.session_id)?;
        validate_id_component("event id", &self.event_id)?;
        if self.content.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorCode::InvalidSessionEvent,
                "session event content must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyBriefStatus {
    Disabled,
    Pending,
    WaitingForStability,
    Running,
    PendingResume,
    Complete,
    Failed,
    NoSessions,
    IntegrityInvalid,
}

impl DailyBriefStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Pending => "pending",
            Self::WaitingForStability => "waiting_for_stability",
            Self::Running => "running",
            Self::PendingResume => "pending_resume",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::NoSessions => "no_sessions",
            Self::IntegrityInvalid => "integrity_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBriefState {
    /// Beijing calendar date in `YYYY-MM-DD` form.
    pub date: String,
    pub device_id: String,
    pub status: DailyBriefStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub updated_at_ms: i64,
}

impl DailyBriefState {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_date(&self.date)?;
        validate_id_component("device id", &self.device_id)?;
        if let Some(hash) = &self.source_fingerprint {
            validate_hash(hash)?;
        }
        if let Some(hash) = &self.content_hash {
            validate_hash(hash)?;
        }
        if self.status == DailyBriefStatus::Complete && self.content_hash.is_none() {
            return Err(DomainError::new(
                DomainErrorCode::InvalidBriefState,
                "a complete daily brief requires a content hash",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainErrorCode {
    UnsupportedClient,
    InvalidId,
    InvalidHash,
    InvalidRecord,
    InvalidSessionEvent,
    InvalidBriefState,
    InvalidRollbackPoint,
}

/// Serializable error returned by pure domain validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainError {
    pub code: DomainErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl DomainError {
    pub fn new(code: DomainErrorCode, message: impl Into<String>) -> Self {
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

impl fmt::Display for DomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DomainError {}

fn validate_id_component(label: &str, value: &str) -> Result<(), DomainError> {
    let valid = !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        return Ok(());
    }
    Err(
        DomainError::new(DomainErrorCode::InvalidId, format!("invalid {label}"))
            .with_context("value", value),
    )
}

fn validate_hash(value: &str) -> Result<(), DomainError> {
    let valid = value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if valid {
        return Ok(());
    }
    Err(DomainError::new(
        DomainErrorCode::InvalidHash,
        "hash must contain 64 hexadecimal characters",
    ))
}

fn validate_date(value: &str) -> Result<(), DomainError> {
    let bytes = value.as_bytes();
    let valid = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    if valid {
        return Ok(());
    }
    Err(DomainError::new(
        DomainErrorCode::InvalidBriefState,
        "daily brief date must use YYYY-MM-DD",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PortableDomain;
    use serde_json::json;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn managed_clients_are_exactly_the_three_target_clients() {
        assert_eq!(
            ManagedClientId::ALL.map(ManagedClientId::as_str),
            ["claude", "codex", "opencode"]
        );
    }

    #[test]
    fn managed_client_parser_rejects_every_legacy_client() {
        for unsupported in [
            "claude-desktop",
            "gemini",
            "grokbuild",
            "openclaw",
            "hermes",
        ] {
            let error = unsupported.parse::<ManagedClientId>().unwrap_err();
            assert_eq!(error.code, DomainErrorCode::UnsupportedClient);
            assert_eq!(
                error.context.get("clientId").map(String::as_str),
                Some(unsupported)
            );
        }

        assert_eq!(
            " OpenCode ".parse::<ManagedClientId>().unwrap(),
            ManagedClientId::Opencode
        );
    }

    #[test]
    fn managed_client_serde_is_canonical_and_rejects_legacy_values() {
        assert_eq!(
            serde_json::to_value(ManagedClientId::Opencode).unwrap(),
            json!("opencode")
        );
        for unsupported in [
            "claude-desktop",
            "gemini",
            "grokbuild",
            "openclaw",
            "hermes",
        ] {
            assert!(serde_json::from_value::<ManagedClientId>(json!(unsupported)).is_err());
        }
    }

    #[test]
    fn managed_client_apps_exposes_only_the_three_target_clients() {
        let apps = ManagedClientApps {
            claude: true,
            codex: false,
            opencode: true,
        };

        assert_eq!(
            apps.enabled_clients().collect::<Vec<_>>(),
            vec![ManagedClientId::Claude, ManagedClientId::Opencode]
        );
        assert_eq!(
            serde_json::to_value(&apps).unwrap(),
            json!({"claude": true, "codex": false, "opencode": true})
        );

        let imported = ManagedClientApps::from_labels(&[
            "claude".to_string(),
            "gemini".to_string(),
            "opencode".to_string(),
        ]);
        assert!(imported.claude);
        assert!(!imported.codex);
        assert!(imported.opencode);
    }

    #[test]
    fn rollback_metadata_requires_consistent_lifecycle_state() {
        let mut metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: "0123456789abcdef0123456789abcdef".to_string(),
            purpose: RollbackPointPurpose::DataMigration,
            state: RollbackPointState::Pending,
            created_at_ms: 1_700_000_000_000,
            failed_at_ms: None,
            payload_size_bytes: 7,
            payload_sha256: HASH_A.to_string(),
        };
        metadata.validate().unwrap();

        metadata.failed_at_ms = Some(metadata.created_at_ms + 1);
        assert_eq!(
            metadata.validate().unwrap_err().code,
            DomainErrorCode::InvalidRollbackPoint
        );
        metadata.state = RollbackPointState::Failed;
        metadata.validate().unwrap();
    }

    #[test]
    fn legacy_preview_roundtrip_exposes_only_counts_and_digests() {
        let preview = LegacyMigrationPreview {
            status: LegacyMigrationStatus::Ready,
            source: Some(LegacySourceKind::Sqlite),
            source_version: Some(16),
            retained: LegacyRetainedCounts {
                claude_providers: 1,
                codex_providers: 1,
                opencode_providers: 1,
                mcp_servers: 1,
                ..LegacyRetainedCounts::default()
            },
            ignored: LegacyIgnoredCounts {
                profiles: 1,
                ..LegacyIgnoredCounts::default()
            },
            files: vec![LegacyFileSummary {
                name: "cc-switch.db".to_string(),
                size_bytes: 4096,
                sha256: HASH_A.to_string(),
            }],
            directory_fingerprint: Some(HASH_A.to_string()),
        };

        assert_eq!(preview.retained.total(), 4);
        assert_eq!(preview.ignored.total(), 1);
        let value = serde_json::to_value(&preview).unwrap();
        assert_eq!(value["status"], "ready");
        assert_eq!(value["source"], "sqlite");
        assert!(value.get("content").is_none());
        assert_eq!(
            serde_json::from_value::<LegacyMigrationPreview>(value).unwrap(),
            preview
        );
    }

    #[test]
    fn session_event_and_brief_state_roundtrip() {
        let event = NormalizedSessionEvent {
            client_id: ManagedClientId::Codex,
            session_id: "session-a".to_string(),
            event_id: "event-a".to_string(),
            role: SessionEventRole::Assistant,
            occurred_at_ms: 1_700_000_000_000,
            content: "Fixture result".to_string(),
            project_dir: Some("/workspace/fixture".to_string()),
        };
        event.validate().unwrap();
        let event_json = serde_json::to_string(&event).unwrap();
        assert_eq!(
            serde_json::from_str::<NormalizedSessionEvent>(&event_json).unwrap(),
            event
        );

        let state = DailyBriefState {
            date: "2026-01-01".to_string(),
            device_id: "device-a".to_string(),
            status: DailyBriefStatus::Complete,
            source_fingerprint: Some(HASH_A.to_string()),
            content_hash: Some(HASH_A.to_string()),
            updated_at_ms: 1_700_000_000_000,
        };
        state.validate().unwrap();
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["status"], "complete");
        assert_eq!(
            serde_json::from_value::<DailyBriefState>(value).unwrap(),
            state
        );
    }

    #[test]
    fn domain_errors_are_stable_and_serializable() {
        let error = PortableRecordId::new(PortableDomain::Skill, "bad key").unwrap_err();
        assert_eq!(error.code, DomainErrorCode::InvalidId);
        let value = serde_json::to_value(error).unwrap();
        assert_eq!(value["code"], "invalid_id");
        assert_eq!(value["context"]["value"], "bad key");
    }
}
