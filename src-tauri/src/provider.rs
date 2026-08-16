use reqwest::header::{HeaderValue, InvalidHeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(rename = "settingsConfig")]
    pub settings_config: Value,
    #[serde(rename = "websiteUrl", skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(rename = "createdAt", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(rename = "sortIndex", skip_serializing_if = "Option::is_none")]
    pub sort_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ProviderMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(rename = "iconColor", skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
}

impl Provider {
    pub fn with_id(
        id: String,
        name: String,
        settings_config: Value,
        website_url: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            settings_config,
            website_url,
            category: None,
            created_at: None,
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderMeta {
    #[serde(
        rename = "commonConfigEnabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub common_config_enabled: Option<bool>,
    #[serde(rename = "isFullUrl", skip_serializing_if = "Option::is_none")]
    pub is_full_url: Option<bool>,
    #[serde(rename = "customUserAgent", skip_serializing_if = "Option::is_none")]
    pub custom_user_agent: Option<String>,
    #[serde(rename = "liveConfigManaged", skip_serializing_if = "Option::is_none")]
    pub live_config_managed: Option<bool>,
}

impl ProviderMeta {
    pub fn custom_user_agent_header(&self) -> Result<Option<HeaderValue>, InvalidHeaderValue> {
        parse_custom_user_agent(self.custom_user_agent.as_deref())
    }
}

pub fn parse_custom_user_agent(
    raw: Option<&str>,
) -> Result<Option<HeaderValue>, InvalidHeaderValue> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => HeaderValue::from_str(value).map(Some),
        None => Ok(None),
    }
}
