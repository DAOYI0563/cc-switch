use serde::{Deserialize, Serialize};

use super::{LegacyRetainedCounts, LegacySourceKind};

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyProviderRecord {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub settings_config_json: String,
    pub website_url: Option<String>,
    pub category: Option<String>,
    pub created_at_ms: i64,
    pub sort_index: i64,
    pub notes: Option<String>,
    pub icon: Option<String>,
    pub icon_color: Option<String>,
    pub meta_json: String,
    pub is_current: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyMcpRecord {
    pub id: String,
    pub name: String,
    pub server_config_json: String,
    pub description: Option<String>,
    pub homepage: Option<String>,
    pub docs: Option<String>,
    pub tags_json: String,
    pub enabled_claude: bool,
    pub enabled_codex: bool,
    pub enabled_opencode: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyPromptRecord {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LegacySkillRecord {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub directory: String,
    pub content_hash: Option<String>,
    pub enabled_claude: bool,
    pub enabled_codex: bool,
    pub enabled_opencode: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LegacyCommonSnippetRecord {
    pub id: String,
    pub client_id: String,
    pub content: String,
}

/// Sensitive migration input. It deliberately has no `Debug` or serialization
/// implementation so configuration bodies cannot accidentally enter logs or IPC.
#[derive(Clone, PartialEq, Eq)]
pub struct LegacyRetainedSnapshot {
    pub source: LegacySourceKind,
    pub source_version: u32,
    pub source_fingerprint: String,
    pub providers: Vec<LegacyProviderRecord>,
    pub mcp_servers: Vec<LegacyMcpRecord>,
    pub prompts: Vec<LegacyPromptRecord>,
    pub skills: Vec<LegacySkillRecord>,
    pub common_snippets: Vec<LegacyCommonSnippetRecord>,
    pub legacy_settings_json: Option<String>,
}

impl LegacyRetainedSnapshot {
    pub fn counts(&self) -> LegacyRetainedCounts {
        let mut counts = LegacyRetainedCounts::default();
        for provider in &self.providers {
            match provider.client_id.as_str() {
                "claude" => counts.claude_providers += 1,
                "codex" => counts.codex_providers += 1,
                "opencode" => counts.opencode_providers += 1,
                _ => {}
            }
        }
        counts.mcp_servers = self.mcp_servers.len() as u64;
        for prompt in &self.prompts {
            match prompt.client_id.as_str() {
                "claude" => counts.claude_prompts += 1,
                "codex" => counts.codex_prompts += 1,
                "opencode" => counts.opencode_prompts += 1,
                _ => {}
            }
        }
        counts.skills = self.skills.len() as u64;
        counts.common_snippets = self.common_snippets.len() as u64;
        counts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetainedMigrationStatus {
    Applied,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RetainedMigrationReport {
    pub schema_version: u32,
    pub status: RetainedMigrationStatus,
    pub source: LegacySourceKind,
    pub source_version: u32,
    pub source_fingerprint: String,
    pub retained: LegacyRetainedCounts,
    pub content_sha256: String,
    pub completed_at_ms: i64,
}

impl RetainedMigrationReport {
    pub const SCHEMA_VERSION: u32 = 1;
}
