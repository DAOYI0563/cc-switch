use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::adapters::live_provider_config::runtime_adapter;
use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::adapters::mcp_live_files::McpLiveFileAdapter;
use crate::adapters::prompt_live_file::PromptLiveFileAdapter;
use crate::app_config::MultiAppConfig;
use crate::domain::{
    validate_server_spec, LocalScanDomain, LocalScanFailureKind, LocalScanTarget, ManagedClientId,
};
use crate::ports::{
    LiveProviderConfigErrorCode, LocalScanParsedRecord, LocalScanParsedSnapshot,
    LocalScanParserPort, LocalScanReadFailure, LocalSkillTree,
};

/// Dispatches to four independent full parsers after the summary layer proves a
/// target changed. Parsed values remain in memory and are never logged here.
#[derive(Debug, Clone, Default)]
pub struct FixedLocalScanParserAdapter;

impl FixedLocalScanParserAdapter {
    pub const fn runtime() -> Self {
        Self
    }

    fn parse_provider(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        let records = match runtime_adapter(target.client_id).read() {
            Ok(snapshot) if target.client_id == ManagedClientId::Opencode => snapshot
                .settings
                .get("provider")
                .and_then(Value::as_object)
                .map(|providers| {
                    providers
                        .iter()
                        .map(|(id, value)| parsed_record(id, value.clone()))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default(),
            Ok(snapshot) => vec![parsed_record("live", snapshot.settings)?],
            Err(error) if error.code == LiveProviderConfigErrorCode::Missing => Vec::new(),
            Err(error) => {
                return Err(LocalScanReadFailure {
                    kind: if error.code == LiveProviderConfigErrorCode::Io {
                        LocalScanFailureKind::ReadFailed
                    } else {
                        LocalScanFailureKind::ParseFailed
                    },
                    record_id: None,
                })
            }
        };
        parsed_snapshot(target, records)
    }

    fn parse_mcp(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        let bytes = McpLiveFileAdapter::runtime()
            .read_optional(target.client_id)
            .map_err(|_| read_failure(None))?;
        let Some(bytes) = bytes else {
            return parsed_snapshot(target, Vec::new());
        };
        if bytes.is_empty() {
            return parsed_snapshot(target, Vec::new());
        }

        if target.client_id == ManagedClientId::Claude {
            return parse_claude_mcp(target, &bytes);
        }

        let source_ids = match target.client_id {
            ManagedClientId::Codex => strict_codex_mcp_ids(&bytes)?,
            ManagedClientId::Opencode => strict_opencode_mcp_ids(&bytes)?,
            ManagedClientId::Claude => unreachable!("Claude returned above"),
        };
        let mut config = MultiAppConfig::default();
        match target.client_id {
            ManagedClientId::Codex => crate::mcp::import_from_codex(&mut config),
            ManagedClientId::Opencode => crate::mcp::import_from_opencode(&mut config),
            ManagedClientId::Claude => unreachable!("Claude returned above"),
        }
        .map_err(|_| parse_failure(None))?;

        let servers = config.mcp.servers.unwrap_or_default();
        let parsed_ids: BTreeSet<_> = servers.keys().cloned().collect();
        if parsed_ids != source_ids {
            return Err(parse_failure(None));
        }
        let mut records = Vec::with_capacity(servers.len());
        for (id, server) in servers {
            server
                .validate()
                .map_err(|_| parse_failure(Some(id.as_str())))?;
            records.push(parsed_record(&id, server.server)?);
        }
        parsed_snapshot(target, records)
    }

    fn parse_prompt(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        let records = PromptLiveFileAdapter::runtime()
            .read_text(target.client_id)
            .map_err(|_| parse_failure(Some("prompt-live")))?
            .map(|content| parsed_record("prompt-live", json!({ "content": content })))
            .transpose()?
            .into_iter()
            .collect();
        parsed_snapshot(target, records)
    }

    fn parse_skills(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        let candidates = LocalSkillTreeAdapter::runtime()
            .scan_strict(target.client_id)
            .map_err(|_| parse_failure(None))?;
        let mut records = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let (name, description) =
                parse_skill_metadata(&candidate.tree, &candidate.directory)
                    .map_err(|_| parse_failure(Some(candidate.directory.as_str())))?;
            records.push(parsed_record(
                &candidate.directory,
                json!({
                    "name": name,
                    "description": description,
                    "contentHash": candidate.tree.content_hash,
                    "totalSizeBytes": candidate.tree.total_size_bytes,
                    "fileCount": candidate.tree.file_count,
                    "cloudEligible": candidate.tree.is_cloud_eligible(),
                }),
            )?);
        }
        parsed_snapshot(target, records)
    }
}

impl LocalScanParserPort for FixedLocalScanParserAdapter {
    fn parse_changed(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        match target.domain {
            LocalScanDomain::Provider => self.parse_provider(target),
            LocalScanDomain::Mcp => self.parse_mcp(target),
            LocalScanDomain::Prompt => self.parse_prompt(target),
            LocalScanDomain::Skill => self.parse_skills(target),
        }
    }
}

fn parse_claude_mcp(
    target: LocalScanTarget,
    bytes: &[u8],
) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| parse_failure(None))?;
    let object = root.as_object().ok_or_else(|| parse_failure(None))?;
    let Some(servers) = object.get("mcpServers") else {
        return parsed_snapshot(target, Vec::new());
    };
    let servers = servers.as_object().ok_or_else(|| parse_failure(None))?;
    let mut records = Vec::with_capacity(servers.len());
    for (id, spec) in servers {
        validate_server_spec(spec).map_err(|_| parse_failure(Some(id)))?;
        records.push(parsed_record(id, spec.clone())?);
    }
    parsed_snapshot(target, records)
}

fn strict_codex_mcp_ids(bytes: &[u8]) -> Result<BTreeSet<String>, LocalScanReadFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| parse_failure(None))?;
    let root: toml::Table = toml::from_str(text).map_err(|_| parse_failure(None))?;
    let mut ids = BTreeSet::new();

    if let Some(value) = root.get("mcp_servers") {
        collect_toml_server_ids(value, &mut ids)?;
    }
    if let Some(mcp) = root.get("mcp") {
        let mcp = mcp.as_table().ok_or_else(|| parse_failure(None))?;
        if let Some(servers) = mcp.get("servers") {
            collect_toml_server_ids(servers, &mut ids)?;
        }
    }
    Ok(ids)
}

fn collect_toml_server_ids(
    value: &toml::Value,
    ids: &mut BTreeSet<String>,
) -> Result<(), LocalScanReadFailure> {
    let table = value.as_table().ok_or_else(|| parse_failure(None))?;
    for (id, entry) in table {
        if !entry.is_table() || !ids.insert(id.clone()) {
            return Err(parse_failure(Some(id)));
        }
    }
    Ok(())
}

fn strict_opencode_mcp_ids(bytes: &[u8]) -> Result<BTreeSet<String>, LocalScanReadFailure> {
    let root: Value = serde_json::from_slice(bytes).map_err(|_| parse_failure(None))?;
    let root = root.as_object().ok_or_else(|| parse_failure(None))?;
    let Some(mcp) = root.get("mcp") else {
        return Ok(BTreeSet::new());
    };
    let mcp = mcp.as_object().ok_or_else(|| parse_failure(None))?;
    let mut ids = BTreeSet::new();
    for (id, entry) in mcp {
        if !entry.is_object() || !ids.insert(id.clone()) {
            return Err(parse_failure(Some(id)));
        }
    }
    Ok(ids)
}

#[derive(Debug, Deserialize)]
struct SkillFrontMatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_skill_metadata(
    tree: &LocalSkillTree,
    fallback_name: &str,
) -> Result<(String, Option<String>), ()> {
    let contents = tree.file("SKILL.md").ok_or(())?;
    let text = std::str::from_utf8(contents).map_err(|_| ())?;
    let front_matter = text
        .strip_prefix("---")
        .and_then(|tail| tail.strip_prefix(['\r', '\n']))
        .and_then(|tail| tail.split_once("\n---"))
        .map(|(yaml, _)| yaml.trim());
    let metadata = front_matter
        .map(serde_yaml::from_str::<SkillFrontMatter>)
        .transpose()
        .map_err(|_| ())?;
    let name = metadata
        .as_ref()
        .and_then(|metadata| metadata.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let description = metadata
        .and_then(|metadata| metadata.description)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok((name, description))
}

fn parsed_record(
    record_id: &str,
    value: Value,
) -> Result<LocalScanParsedRecord, LocalScanReadFailure> {
    LocalScanParsedRecord::new(record_id, value).map_err(|_| parse_failure(Some(record_id)))
}

fn parsed_snapshot(
    target: LocalScanTarget,
    records: Vec<LocalScanParsedRecord>,
) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
    LocalScanParsedSnapshot::new(target, records).map_err(|_| parse_failure(None))
}

fn read_failure(record_id: Option<&str>) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: LocalScanFailureKind::ReadFailed,
        record_id: record_id.map(ToOwned::to_owned),
    }
}

fn parse_failure(record_id: Option<&str>) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: LocalScanFailureKind::ParseFailed,
        record_id: record_id.map(ToOwned::to_owned),
    }
}
