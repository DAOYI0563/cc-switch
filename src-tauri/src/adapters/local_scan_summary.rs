use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::database::Database;
use crate::domain::{
    prompt_live_filename, LocalScanDomain, LocalScanEntrySummary, LocalScanFailureKind,
    LocalScanSummary, LocalScanTarget, LocalSkill, ManagedClientId,
};
use crate::ports::{
    LocalScanFirstObservation, LocalScanReadFailure, LocalScanSummaryPort, LocalSkillTreeError,
    LocalSkillTreeErrorCode, ManagedSkillInventoryPort, WslFileError, WslFileErrorCode,
    WslFileSystem, WslPathScope,
};

use super::wsl_files::WslFileAdapter;

/// Database-backed, read-only managed Skill inventory. A per-client snapshot is
/// shared by the summary and parser adapters so one observation cannot mix two
/// different database reads. Summary scans explicitly refresh it first.
pub struct DatabaseManagedSkillInventory {
    database: Arc<Database>,
    cached: Mutex<HashMap<ManagedClientId, Vec<LocalSkill>>>,
}

impl DatabaseManagedSkillInventory {
    pub fn new(database: Arc<Database>) -> Self {
        Self {
            database,
            cached: Mutex::new(HashMap::new()),
        }
    }

    fn read_database(&self) -> Result<Vec<LocalSkill>, LocalScanReadFailure> {
        self.database
            .list_core_skills()
            .map_err(|_| read_failure(None))
    }
}

impl ManagedSkillInventoryPort for DatabaseManagedSkillInventory {
    fn list_managed_skills(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkill>, LocalScanReadFailure> {
        self.cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&client)
            .cloned()
            .ok_or_else(|| read_failure(None))
    }

    fn refresh_managed_skills(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkill>, LocalScanReadFailure> {
        let skills = self.read_database()?;
        self.cached
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(client, skills.clone());
        Ok(skills)
    }
}

/// Reads content-free summaries from the one supported WSL environment.
#[derive(Debug, Clone, Default)]
pub struct FixedLocalScanSummaryAdapter {
    files: WslFileAdapter,
    skills: LocalSkillTreeAdapter,
}

impl FixedLocalScanSummaryAdapter {
    pub fn runtime() -> Self {
        Self {
            files: WslFileAdapter::runtime(),
            skills: LocalSkillTreeAdapter::runtime(),
        }
    }

    fn scan_files(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let mut entries = Vec::new();
        for spec in file_specs(target) {
            let contents = self
                .files
                .read_optional(spec.scope, spec.relative)
                .map_err(map_file_error)?;
            if let Some(contents) = contents {
                entries.push(
                    LocalScanEntrySummary::new(
                        spec.record_id,
                        sha256(&contents),
                        contents.len() as u64,
                        None,
                    )
                    .map_err(|_| digest_failure(Some(spec.record_id)))?,
                );
            }
        }
        build_summary(target, entries)
    }

    fn scan_skills(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let candidates = self
            .skills
            .scan_strict(target.client_id)
            .map_err(map_skill_error)?;
        let mut entries = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            entries.push(
                LocalScanEntrySummary::new(
                    candidate.directory.as_str(),
                    candidate.tree.content_hash.as_str(),
                    candidate.tree.total_size_bytes,
                    None,
                )
                .map_err(|_| digest_failure(Some(candidate.directory.as_str())))?,
            );
        }
        build_summary(target, entries)
    }
}

impl LocalScanSummaryPort for FixedLocalScanSummaryAdapter {
    fn scan_summary(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        match target.domain {
            LocalScanDomain::Skill => self.scan_skills(target),
            LocalScanDomain::Provider | LocalScanDomain::Mcp | LocalScanDomain::Prompt => {
                self.scan_files(target)
            }
        }
    }
}

/// Production composite: non-Skill domains retain their fixed adapters while
/// Skill scans are constrained to the database-known directory inventory.
pub struct DatabaseLocalScanSummaryAdapter {
    fixed: FixedLocalScanSummaryAdapter,
    inventory: Arc<dyn ManagedSkillInventoryPort>,
    skills: LocalSkillTreeAdapter,
}

impl DatabaseLocalScanSummaryAdapter {
    pub fn new(inventory: Arc<dyn ManagedSkillInventoryPort>) -> Self {
        Self {
            fixed: FixedLocalScanSummaryAdapter::runtime(),
            inventory,
            skills: LocalSkillTreeAdapter::runtime(),
        }
    }

    pub fn runtime(database: Arc<Database>) -> (Self, Arc<dyn ManagedSkillInventoryPort>) {
        let inventory: Arc<dyn ManagedSkillInventoryPort> =
            Arc::new(DatabaseManagedSkillInventory::new(database));
        (Self::new(inventory.clone()), inventory)
    }

    fn scan_managed_skills(
        &self,
        target: LocalScanTarget,
        records: &[LocalSkill],
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let directories: BTreeSet<_> = records
            .iter()
            .map(|skill| skill.directory.clone())
            .collect();
        if directories.len() != records.len() {
            return Err(read_failure(None));
        }
        let candidates = self
            .skills
            .scan_managed(target.client_id, directories)
            .map_err(map_skill_error)?;
        let entries = candidates
            .into_iter()
            .map(|candidate| {
                LocalScanEntrySummary::new(
                    candidate.directory.as_str(),
                    candidate.tree.content_hash.as_str(),
                    candidate.tree.total_size_bytes,
                    None,
                )
                .map_err(|_| digest_failure(Some(candidate.directory.as_str())))
            })
            .collect::<Result<Vec<_>, _>>()?;
        build_summary(target, entries)
    }

    fn expected_skill_summary(
        &self,
        target: LocalScanTarget,
        records: &[LocalSkill],
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        let entries = records
            .iter()
            .filter(|skill| skill.apps.is_enabled_for(target.client_id))
            .filter_map(|skill| {
                skill.content_hash.as_deref().map(|content_hash| {
                    LocalScanEntrySummary::new(
                        skill.directory.as_str(),
                        content_hash,
                        skill.total_size_bytes,
                        None,
                    )
                    .map_err(|_| digest_failure(Some(skill.directory.as_str())))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        build_summary(target, entries)
    }
}

impl LocalScanSummaryPort for DatabaseLocalScanSummaryAdapter {
    fn scan_summary(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        if target.domain != LocalScanDomain::Skill {
            return self.fixed.scan_summary(target);
        }
        let records = self.inventory.refresh_managed_skills(target.client_id)?;
        self.scan_managed_skills(target, &records)
    }

    fn expected_after_write(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanSummary, LocalScanReadFailure> {
        if target.domain != LocalScanDomain::Skill {
            return self.fixed.scan_summary(target);
        }
        let records = self.inventory.refresh_managed_skills(target.client_id)?;
        self.expected_skill_summary(target, &records)
    }

    fn scan_first_observation(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanFirstObservation, LocalScanReadFailure> {
        if target.domain != LocalScanDomain::Skill {
            return Ok(LocalScanFirstObservation {
                current: self.fixed.scan_summary(target)?,
                baseline: None,
                requires_parse: false,
            });
        }
        let records = self.inventory.refresh_managed_skills(target.client_id)?;
        let current = self.scan_managed_skills(target, &records)?;
        let baseline = self.expected_skill_summary(target, &records)?;
        let requires_parse = records.iter().any(|skill| {
            skill.apps.is_enabled_for(target.client_id) && skill.content_hash.is_none()
        });
        Ok(LocalScanFirstObservation {
            current,
            baseline: Some(baseline),
            requires_parse,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct FileSpec {
    record_id: &'static str,
    scope: WslPathScope,
    relative: &'static str,
}

fn file_specs(target: LocalScanTarget) -> Vec<FileSpec> {
    let client_scope = WslPathScope::ClientConfig(target.client_id);
    match (target.domain, target.client_id) {
        (LocalScanDomain::Provider, ManagedClientId::Claude) => vec![FileSpec {
            record_id: "settings",
            scope: client_scope,
            relative: "settings.json",
        }],
        (LocalScanDomain::Provider, ManagedClientId::Codex) => vec![
            FileSpec {
                record_id: "auth",
                scope: client_scope,
                relative: "auth.json",
            },
            FileSpec {
                record_id: "config",
                scope: client_scope,
                relative: "config.toml",
            },
        ],
        (LocalScanDomain::Provider, ManagedClientId::Opencode) => vec![FileSpec {
            record_id: "config",
            scope: client_scope,
            relative: "opencode.json",
        }],
        (LocalScanDomain::Mcp, ManagedClientId::Claude) => vec![FileSpec {
            record_id: "config",
            scope: WslPathScope::ClaudeStateFile,
            relative: "",
        }],
        (LocalScanDomain::Mcp, ManagedClientId::Codex) => vec![FileSpec {
            record_id: "config",
            scope: client_scope,
            relative: "config.toml",
        }],
        (LocalScanDomain::Mcp, ManagedClientId::Opencode) => vec![FileSpec {
            record_id: "config",
            scope: client_scope,
            relative: "opencode.json",
        }],
        (LocalScanDomain::Prompt, client_id) => vec![FileSpec {
            record_id: "prompt-live",
            scope: client_scope,
            relative: prompt_live_filename(client_id),
        }],
        (LocalScanDomain::Skill, _) => Vec::new(),
    }
}

fn build_summary(
    target: LocalScanTarget,
    mut entries: Vec<LocalScanEntrySummary>,
) -> Result<LocalScanSummary, LocalScanReadFailure> {
    entries.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    let mut hasher = Sha256::new();
    hasher.update(b"wsl-code-switch-local-scan-v1\0");
    hasher.update(target.domain.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(target.client_id.as_str().as_bytes());
    for entry in &entries {
        hasher.update(b"\0record\0");
        hasher.update((entry.record_id.len() as u64).to_le_bytes());
        hasher.update(entry.record_id.as_bytes());
        hasher.update(entry.content_digest.as_bytes());
        hasher.update(entry.size_bytes.to_le_bytes());
    }
    LocalScanSummary::new(target, format!("{:x}", hasher.finalize()), entries)
        .map_err(|_| digest_failure(None))
}

fn sha256(contents: &[u8]) -> String {
    format!("{:x}", Sha256::digest(contents))
}

fn digest_failure(record_id: Option<&str>) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: LocalScanFailureKind::DigestFailed,
        record_id: record_id.map(ToOwned::to_owned),
    }
}

fn read_failure(record_id: Option<&str>) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: LocalScanFailureKind::ReadFailed,
        record_id: record_id.map(ToOwned::to_owned),
    }
}

fn map_file_error(error: WslFileError) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: match error.code {
            WslFileErrorCode::InvalidPath => LocalScanFailureKind::InvalidPath,
            WslFileErrorCode::Io => LocalScanFailureKind::ReadFailed,
        },
        record_id: None,
    }
}

fn map_skill_error(error: LocalSkillTreeError) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: match error.code {
            LocalSkillTreeErrorCode::InvalidPath => LocalScanFailureKind::InvalidPath,
            LocalSkillTreeErrorCode::LinkNotAllowed => LocalScanFailureKind::LinkOrReparsePoint,
            LocalSkillTreeErrorCode::NotFound => LocalScanFailureKind::NotFound,
            LocalSkillTreeErrorCode::InvalidTree | LocalSkillTreeErrorCode::Io => {
                LocalScanFailureKind::ReadFailed
            }
        },
        record_id: None,
    }
}
