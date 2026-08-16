use sha2::{Digest, Sha256};

use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::domain::{
    prompt_live_filename, LocalScanDomain, LocalScanEntrySummary, LocalScanFailureKind,
    LocalScanSummary, LocalScanTarget, ManagedClientId,
};
use crate::ports::{
    LocalScanReadFailure, LocalScanSummaryPort, LocalSkillTreeError, LocalSkillTreeErrorCode,
    WslFileError, WslFileErrorCode, WslFileSystem, WslPathScope,
};

use super::wsl_files::WslFileAdapter;

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
