use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorCode, ManagedClientApps, ManagedClientId};

const MEBIBYTE: u64 = 1024 * 1024;

pub const MAX_SKILL_FILE_SIZE_BYTES: u64 = 10 * MEBIBYTE;
pub const MAX_SKILL_TOTAL_SIZE_BYTES: u64 = 20 * MEBIBYTE;
pub const MAX_SKILL_FILE_COUNT: u64 = 500;
pub const MAX_SKILLS_CLOUD_TOTAL_SIZE_BYTES: u64 = 200 * MEBIBYTE;

/// Client-neutral metadata for one locally managed Skill.
///
/// Skill contents remain in the selected clients' live directories. The
/// database stores only identity, the last confirmed content digest, measured
/// size, and client enablement state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkill {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub total_size_bytes: u64,
    pub file_count: u64,
    pub apps: ManagedClientApps,
    pub cloud_eligible: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl LocalSkill {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_skill_id(&self.id)?;
        validate_skill_directory(&self.directory)?;
        validate_skill_name(&self.name)?;
        if self
            .description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 4_000)
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "Skill 描述不得超过 4000 个字符",
            ));
        }
        if let Some(hash) = &self.content_hash {
            validate_sha256(hash)?;
        }
        if self.apps.is_empty() {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "Skill 必须至少关联一个客户端",
            ));
        }
        if self.cloud_eligible
            && (self.content_hash.is_none()
                || self.total_size_bytes > MAX_SKILL_TOTAL_SIZE_BYTES
                || self.file_count > MAX_SKILL_FILE_COUNT)
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "可云同步 Skill 的哈希、大小或文件数无效",
            ));
        }
        if self.created_at_ms < 0 || self.updated_at_ms < self.created_at_ms {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "Skill 时间戳无效",
            ));
        }
        Ok(())
    }
}

pub fn skill_tree_is_cloud_eligible(
    total_size_bytes: u64,
    file_count: u64,
    largest_file_size_bytes: u64,
) -> bool {
    total_size_bytes <= MAX_SKILL_TOTAL_SIZE_BYTES
        && file_count <= MAX_SKILL_FILE_COUNT
        && largest_file_size_bytes <= MAX_SKILL_FILE_SIZE_BYTES
}

/// Validate the aggregate payload before any Skill record is built for WebDAV.
pub fn validate_skill_cloud_total(skills: &[LocalSkill]) -> Result<u64, DomainError> {
    let mut total = 0_u64;
    for skill in skills.iter().filter(|skill| skill.cloud_eligible) {
        if skill.content_hash.is_none()
            || skill.total_size_bytes > MAX_SKILL_TOTAL_SIZE_BYTES
            || skill.file_count > MAX_SKILL_FILE_COUNT
        {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "Skill 云同步资格与资源计量不一致",
            )
            .with_context("skillId", &skill.id));
        }
        total = total.checked_add(skill.total_size_bytes).ok_or_else(|| {
            DomainError::new(DomainErrorCode::InvalidRecord, "Skills 云同步总量溢出")
        })?;
        if total > MAX_SKILLS_CLOUD_TOTAL_SIZE_BYTES {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "Skills 云同步总量不得超过 200 MB",
            )
            .with_context("totalSizeBytes", total.to_string()));
        }
    }
    Ok(total)
}

/// Explicit request to adopt one Skill from a selected live client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillImport {
    pub directory: String,
    pub source_client: ManagedClientId,
    pub apps: ManagedClientApps,
}

impl LocalSkillImport {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_skill_directory(&self.directory)?;
        if self.apps.is_empty() {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "导入 Skill 时必须至少选择一个客户端",
            ));
        }
        if !self.apps.is_enabled_for(self.source_client) {
            return Err(DomainError::new(
                DomainErrorCode::InvalidRecord,
                "导入目标必须包含明确选择的来源客户端",
            ));
        }
        Ok(())
    }
}

/// One safe, importable live Skill found outside the managed database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedLocalSkill {
    pub directory: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub found_in: Vec<ManagedClientId>,
}

/// A live-copy problem that prevented one managed Skill from being refreshed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSkillScanIssueKind {
    DivergentCopies,
    InvalidCopy,
    CaseCollision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillScanIssue {
    pub directory: String,
    pub clients: Vec<ManagedClientId>,
    pub kind: LocalSkillScanIssueKind,
}

/// Combined local-authority refresh and unmanaged discovery result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillScanResult {
    pub installed: Vec<LocalSkill>,
    pub unmanaged: Vec<UnmanagedLocalSkill>,
    pub issues: Vec<LocalSkillScanIssue>,
    pub updated_count: u64,
    pub removed_count: u64,
}

pub fn validate_skill_directory(value: &str) -> Result<(), DomainError> {
    let invalid_windows_name = {
        let stem = value
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit()
                && stem.as_bytes()[3] != b'0')
    };
    let valid = !value.is_empty()
        && value.chars().count() <= 255
        && value != "."
        && value != ".."
        && !value.starts_with('.')
        && !value.ends_with([' ', '.'])
        && !invalid_windows_name
        && !value.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'
                )
        });
    if valid {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorCode::InvalidRecord,
            "Skill 目录必须是安全的单段相对目录名",
        )
        .with_context("directory", value))
    }
}

fn validate_skill_id(value: &str) -> Result<(), DomainError> {
    let valid = !value.is_empty()
        && value.len() <= 512
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        });
    if valid {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorCode::InvalidId,
            "Skill ID 无效",
        ))
    }
}

fn validate_skill_name(value: &str) -> Result<(), DomainError> {
    let trimmed = value.trim();
    if !trimmed.is_empty()
        && trimmed.chars().count() <= 512
        && !trimmed.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorCode::InvalidRecord,
            "Skill 名称必须包含 1 到 512 个可见字符",
        ))
    }
}

fn validate_sha256(value: &str) -> Result<(), DomainError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorCode::InvalidHash,
            "Skill 内容哈希必须是 64 位十六进制 SHA-256",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud_skill(id: &str, total_size_bytes: u64, cloud_eligible: bool) -> LocalSkill {
        LocalSkill {
            id: id.to_string(),
            name: id.to_string(),
            description: None,
            directory: id.to_string(),
            content_hash: Some("a".repeat(64)),
            total_size_bytes,
            file_count: 1,
            apps: ManagedClientApps::only(ManagedClientId::Claude),
            cloud_eligible,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn valid_import() -> LocalSkillImport {
        LocalSkillImport {
            directory: "local-skill".to_string(),
            source_client: ManagedClientId::Claude,
            apps: ManagedClientApps::only(ManagedClientId::Claude),
        }
    }

    #[test]
    fn accepts_only_safe_single_directory_segments() {
        for valid in ["local-skill", "skill_v2", "本地技能"] {
            validate_skill_directory(valid).expect(valid);
        }
        for invalid in [
            "",
            ".hidden",
            "..",
            "../escape",
            "nested/skill",
            r"nested\skill",
            "CON",
            "com1.txt",
            "trailing.",
            "bad:name",
        ] {
            assert!(validate_skill_directory(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn import_requires_the_explicit_source_to_remain_selected() {
        valid_import().validate().expect("valid import");
        let mut invalid = valid_import();
        invalid.apps = ManagedClientApps::only(ManagedClientId::Codex);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn local_scan_contract_uses_camel_case_fields_and_stable_snake_case_issue_kinds() {
        let result = LocalSkillScanResult {
            installed: Vec::new(),
            unmanaged: vec![UnmanagedLocalSkill {
                directory: "unmanaged".to_string(),
                name: "Unmanaged".to_string(),
                description: None,
                found_in: vec![ManagedClientId::Codex],
            }],
            issues: vec![LocalSkillScanIssue {
                directory: "fixture".to_string(),
                clients: vec![ManagedClientId::Claude],
                kind: LocalSkillScanIssueKind::DivergentCopies,
            }],
            updated_count: 2,
            removed_count: 1,
        };

        assert_eq!(
            serde_json::to_value(result).expect("serialize scan result"),
            serde_json::json!({
                "installed": [],
                "unmanaged": [{
                    "directory": "unmanaged",
                    "name": "Unmanaged",
                    "foundIn": ["codex"]
                }],
                "issues": [{
                    "directory": "fixture",
                    "clients": ["claude"],
                    "kind": "divergent_copies"
                }],
                "updatedCount": 2,
                "removedCount": 1
            })
        );
    }

    #[test]
    fn cloud_eligibility_accepts_each_exact_limit_and_rejects_one_over() {
        assert!(skill_tree_is_cloud_eligible(
            MAX_SKILL_TOTAL_SIZE_BYTES,
            MAX_SKILL_FILE_COUNT,
            MAX_SKILL_FILE_SIZE_BYTES,
        ));
        assert!(!skill_tree_is_cloud_eligible(
            MAX_SKILL_TOTAL_SIZE_BYTES + 1,
            MAX_SKILL_FILE_COUNT,
            MAX_SKILL_FILE_SIZE_BYTES,
        ));
        assert!(!skill_tree_is_cloud_eligible(
            MAX_SKILL_TOTAL_SIZE_BYTES,
            MAX_SKILL_FILE_COUNT + 1,
            MAX_SKILL_FILE_SIZE_BYTES,
        ));
        assert!(!skill_tree_is_cloud_eligible(
            MAX_SKILL_TOTAL_SIZE_BYTES,
            MAX_SKILL_FILE_COUNT,
            MAX_SKILL_FILE_SIZE_BYTES + 1,
        ));
    }

    #[test]
    fn cloud_total_accepts_200_mb_and_rejects_the_next_byte() {
        let at_limit: Vec<_> = (0..10)
            .map(|index| cloud_skill(&format!("skill-{index}"), MAX_SKILL_TOTAL_SIZE_BYTES, true))
            .collect();
        assert_eq!(
            validate_skill_cloud_total(&at_limit).expect("exact aggregate limit"),
            MAX_SKILLS_CLOUD_TOTAL_SIZE_BYTES
        );

        let mut over_limit = at_limit;
        over_limit.push(cloud_skill("one-more-byte", 1, true));
        assert!(validate_skill_cloud_total(&over_limit).is_err());

        over_limit.last_mut().unwrap().cloud_eligible = false;
        assert_eq!(
            validate_skill_cloud_total(&over_limit).expect("ineligible Skills are excluded"),
            MAX_SKILLS_CLOUD_TOTAL_SIZE_BYTES
        );
    }
}
