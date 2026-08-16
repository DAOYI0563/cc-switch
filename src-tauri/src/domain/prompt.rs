use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorCode, ManagedClientId};

pub const MAX_PROMPT_VERSIONS_PER_NAME: i64 = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptVersion {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: i64,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl PromptVersion {
    pub fn validate_input(&self) -> Result<(), DomainError> {
        validate_text_id("prompt version id", &self.id)?;
        if self.name.trim().is_empty() || self.name.chars().count() > 256 {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "prompt version name must contain 1 to 256 characters",
            ));
        }
        if self.content.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "prompt version content must not be empty",
            ));
        }
        if self
            .description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 2_000)
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "prompt version description must not exceed 2000 characters",
            ));
        }
        Ok(())
    }

    pub fn validate_stored(&self) -> Result<(), DomainError> {
        self.validate_input()?;
        if !(1..=MAX_PROMPT_VERSIONS_PER_NAME).contains(&self.version) {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                format!("prompt version must be between 1 and {MAX_PROMPT_VERSIONS_PER_NAME}"),
            ));
        }
        if self.created_at.is_some_and(|value| value < 0)
            || self.updated_at.is_some_and(|value| value < 0)
            || matches!((self.created_at, self.updated_at), (Some(created), Some(updated)) if updated < created)
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "prompt version timestamps are invalid",
            ));
        }
        Ok(())
    }
}

pub const fn prompt_live_filename(client: ManagedClientId) -> &'static str {
    match client {
        ManagedClientId::Claude => "CLAUDE.md",
        ManagedClientId::Codex | ManagedClientId::Opencode => "AGENTS.md",
    }
}

fn validate_text_id(label: &str, value: &str) -> Result<(), DomainError> {
    let valid = !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorCode::InvalidId,
            format!("invalid {label}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_filenames_are_fixed_for_exactly_three_clients() {
        assert_eq!(prompt_live_filename(ManagedClientId::Claude), "CLAUDE.md");
        assert_eq!(prompt_live_filename(ManagedClientId::Codex), "AGENTS.md");
        assert_eq!(prompt_live_filename(ManagedClientId::Opencode), "AGENTS.md");
    }

    #[test]
    fn stored_version_requires_valid_identity_content_and_range() {
        let mut version = PromptVersion {
            id: "version-1".to_string(),
            name: "Default".to_string(),
            version: 1,
            content: "Use focused changes.".to_string(),
            description: None,
            enabled: false,
            created_at: Some(1),
            updated_at: Some(1),
        };
        version.validate_stored().expect("valid version");

        version.version = 21;
        assert!(version.validate_stored().is_err());
        version.version = 1;
        version.content = " \n".to_string();
        assert!(version.validate_stored().is_err());
    }
}
