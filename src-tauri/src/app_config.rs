use std::collections::HashMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub use crate::domain::McpServer;
use crate::domain::{DomainError, ManagedClientId};
use crate::error::AppError;

pub type McpApps = crate::domain::ManagedClientApps;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRoot {
    #[serde(default)]
    pub servers: Option<HashMap<String, McpServer>>,
    #[serde(default)]
    pub claude: McpConfig,
    #[serde(default)]
    pub codex: McpConfig,
    #[serde(default)]
    pub opencode: McpConfig,
}

impl Default for McpRoot {
    fn default() -> Self {
        Self {
            servers: Some(HashMap::new()),
            claude: McpConfig::default(),
            codex: McpConfig::default(),
            opencode: McpConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LegacyAppType {
    Claude,
    Codex,
    OpenCode,
}

impl LegacyAppType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }

    pub fn is_additive_mode(&self) -> bool {
        *self == Self::OpenCode
    }
}

impl FromStr for LegacyAppType {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "opencode" => Ok(Self::OpenCode),
            other => Err(AppError::InvalidInput(format!(
                "不支持的客户端 '{other}'，仅允许 claude、codex、opencode"
            ))),
        }
    }
}

impl From<ManagedClientId> for LegacyAppType {
    fn from(client: ManagedClientId) -> Self {
        match client {
            ManagedClientId::Claude => Self::Claude,
            ManagedClientId::Codex => Self::Codex,
            ManagedClientId::Opencode => Self::OpenCode,
        }
    }
}

impl TryFrom<&LegacyAppType> for ManagedClientId {
    type Error = DomainError;

    fn try_from(app: &LegacyAppType) -> Result<Self, Self::Error> {
        match app {
            LegacyAppType::Claude => Ok(Self::Claude),
            LegacyAppType::Codex => Ok(Self::Codex),
            LegacyAppType::OpenCode => Ok(Self::Opencode),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MultiAppConfig {
    #[serde(default)]
    pub mcp: McpRoot,
}
