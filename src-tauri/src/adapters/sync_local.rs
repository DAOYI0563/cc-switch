use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::app_config::LegacyAppType;
use crate::database::DailyBriefRecord;
use crate::domain::{
    content_hash, validate_html, validate_skill_cloud_total, DailyBriefStatus, LocalSkill,
    ManagedClientApps, ManagedClientId, McpServer, PortableDomain, PortablePayload,
    PortableRecordId, PromptVersion, SyncDeviceId, SyncLocalCommitPlan, SyncLocalSnapshot,
    SyncMergeSideAction, SyncRecord, SyncRecordBaseline,
};
use crate::ports::{
    ConflictCenterError, ConflictCenterErrorCode, LocalSkillFile, LocalSkillTree,
    LocalSkillTreePort, SyncLocalApplyPort,
};
use crate::provider::Provider;
use crate::services::{
    daily_brief, CommonSnippetService, LocalSkillService, McpService, PromptService,
    ProviderService,
};
use crate::store::AppState;

use super::local_skill_tree::LocalSkillTreeAdapter;

pub struct RuntimeSyncLocalAdapter<'a> {
    state: &'a AppState,
}

impl<'a> RuntimeSyncLocalAdapter<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn snapshot(
        &self,
        device_id: &SyncDeviceId,
        now_ms: i64,
    ) -> Result<SyncLocalSnapshot, ConflictCenterError> {
        if now_ms < 0 {
            return Err(invalid_input("sync snapshot time must not be negative"));
        }
        let identity = self
            .state
            .db
            .load_sync_identity()
            .map_err(|_| read_error("failed to read fixed sync identity"))?;
        if identity
            .as_ref()
            .is_some_and(|identity| &identity.device_id != device_id)
        {
            return Err(invalid_input(
                "requested sync device does not match the fixed local identity",
            ));
        }
        let mut baselines = self
            .state
            .db
            .load_sync_baselines()
            .map_err(|_| read_error("failed to read sync baselines"))?;
        baselines.sort_by(|left, right| left.record.id.cmp(&right.record.id));
        let (payloads, protected) = self.capture_portable_payloads(now_ms)?;
        let local_records =
            build_local_records(device_id, now_ms, &baselines, payloads, &protected)?;
        let snapshot = SyncLocalSnapshot {
            identity,
            baselines,
            local_records,
        };
        snapshot
            .validate_for(device_id)
            .map_err(|_| read_error("local sync snapshot is invalid"))?;
        Ok(snapshot)
    }

    fn capture_portable_payloads(
        &self,
        now_ms: i64,
    ) -> Result<PortableCapture, ConflictCenterError> {
        let mut payloads = BTreeMap::new();
        let mut protected = BTreeSet::new();
        self.capture_providers(now_ms, &mut payloads)?;
        self.capture_mcp(&mut payloads)?;
        self.capture_prompts(&mut payloads)?;
        self.capture_skills(&mut payloads, &mut protected)?;
        self.capture_common_snippets(now_ms, &mut payloads)?;
        self.capture_daily_briefs(&mut payloads, &mut protected)?;
        Ok((payloads, protected))
    }

    fn capture_providers(
        &self,
        now_ms: i64,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
    ) -> Result<(), ConflictCenterError> {
        for client in ManagedClientId::ALL {
            let providers = ProviderService::list_managed(self.state, client)
                .map_err(|_| read_error("failed to read managed providers"))?;
            for provider in providers.values() {
                let key = format!("{}:{}", client.as_str(), provider.id);
                let id = PortableRecordId::new(PortableDomain::Provider, key)
                    .map_err(|_| read_error("managed provider has an invalid sync identity"))?;
                let content = json!({
                    "clientId": client,
                    "providerId": provider.id,
                    "kind": provider.category.as_deref().unwrap_or("custom"),
                    "name": provider.name,
                    "portableConfig": provider_portable_config(client, &provider.settings_config),
                    "sortIndex": provider.sort_index.unwrap_or(0),
                    "notes": provider.notes,
                    "icon": provider.icon,
                    "iconColor": provider.icon_color,
                    "createdAtMs": provider.created_at.unwrap_or(now_ms),
                    "updatedAtMs": now_ms,
                });
                payloads.insert(id, portable_payload(PortableDomain::Provider, content)?);
            }
        }
        Ok(())
    }

    fn capture_mcp(
        &self,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
    ) -> Result<(), ConflictCenterError> {
        let servers = McpService::get_all_servers(self.state)
            .map_err(|_| read_error("failed to read MCP servers"))?;
        for server in servers.values() {
            let id = PortableRecordId::new(PortableDomain::Mcp, server.id.clone())
                .map_err(|_| read_error("MCP server has an invalid sync identity"))?;
            let content = json!({
                "id": server.id,
                "name": server.name,
                "serverConfig": sanitize_portable_value(&server.server),
                "description": server.description,
                "homepage": server.homepage,
                "docs": server.docs,
                "tags": server.tags,
                "apps": server.apps,
            });
            payloads.insert(id, portable_payload(PortableDomain::Mcp, content)?);
        }
        Ok(())
    }

    fn capture_prompts(
        &self,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
    ) -> Result<(), ConflictCenterError> {
        for client in ManagedClientId::ALL {
            let prompts = PromptService::get_prompts(self.state, client)
                .map_err(|_| read_error("failed to read Prompt versions"))?;
            for prompt in prompts.values() {
                let id = PortableRecordId::new(
                    PortableDomain::Prompt,
                    format!("{}:{}", client.as_str(), prompt.id),
                )
                .map_err(|_| read_error("Prompt has an invalid sync identity"))?;
                let content = json!({
                    "id": prompt.id,
                    "clientId": client,
                    "name": prompt.name,
                    "version": prompt.version,
                    "content": prompt.content,
                    "description": prompt.description,
                    "isActive": prompt.enabled,
                    "createdAtMs": prompt.created_at,
                    "updatedAtMs": prompt.updated_at,
                });
                payloads.insert(id, portable_payload(PortableDomain::Prompt, content)?);
            }
        }
        Ok(())
    }

    fn capture_skills(
        &self,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
        protected: &mut BTreeSet<PortableRecordId>,
    ) -> Result<(), ConflictCenterError> {
        let skills = LocalSkillService::get_all(self.state)
            .map_err(|_| read_error("failed to read managed Skills"))?;
        validate_skill_cloud_total(&skills)
            .map_err(|_| read_error("managed Skills exceed the cloud sync limit"))?;
        let trees = LocalSkillTreeAdapter::runtime();
        for skill in skills {
            let id = PortableRecordId::new(PortableDomain::Skill, skill.id.clone())
                .map_err(|_| read_error("Skill has an invalid sync identity"))?;
            if !skill.cloud_eligible {
                protected.insert(id);
                continue;
            }
            let tree = skill
                .apps
                .enabled_clients()
                .find_map(|client| trees.capture(client, &skill.directory).ok()?.tree)
                .filter(LocalSkillTree::is_cloud_eligible);
            let Some(tree) = tree else {
                protected.insert(id);
                continue;
            };
            if tree.content_hash != skill.content_hash.as_deref().unwrap_or_default() {
                protected.insert(id);
                continue;
            }
            let files = SkillFilesPayload::from_tree(&tree);
            let content = json!({
                "id": skill.id,
                "name": skill.name,
                "description": skill.description,
                "directory": skill.directory,
                "contentHash": skill.content_hash,
                "totalSizeBytes": skill.total_size_bytes,
                "fileCount": skill.file_count,
                "apps": skill.apps,
                "cloudEligible": true,
                "files": files,
                "createdAtMs": skill.created_at_ms,
                "updatedAtMs": skill.updated_at_ms,
            });
            payloads.insert(id, portable_payload(PortableDomain::Skill, content)?);
        }
        Ok(())
    }

    fn capture_common_snippets(
        &self,
        now_ms: i64,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
    ) -> Result<(), ConflictCenterError> {
        for client in [ManagedClientId::Claude, ManagedClientId::Codex] {
            let Some(content) = CommonSnippetService::get(self.state, client)
                .map_err(|_| read_error("failed to read common configuration snippet"))?
            else {
                continue;
            };
            let id = PortableRecordId::new(PortableDomain::CommonSnippet, client.as_str())
                .map_err(|_| read_error("common snippet has an invalid sync identity"))?;
            payloads.insert(
                id,
                portable_payload(
                    PortableDomain::CommonSnippet,
                    json!({
                        "id": format!("{}-common", client.as_str()),
                        "clientId": client,
                        "name": "Common",
                        "content": content,
                        "enabled": true,
                        "createdAtMs": now_ms,
                        "updatedAtMs": now_ms,
                    }),
                )?,
            );
        }
        Ok(())
    }

    fn capture_daily_briefs(
        &self,
        payloads: &mut BTreeMap<PortableRecordId, PortablePayload>,
        protected: &mut BTreeSet<PortableRecordId>,
    ) -> Result<(), ConflictCenterError> {
        let records = self
            .state
            .db
            .list_daily_briefs()
            .map_err(|_| read_error("failed to read daily briefs"))?;
        for record in records {
            let key = format!("{}:{}", record.date, record.device_id);
            let id = PortableRecordId::new(PortableDomain::DailyBrief, key)
                .map_err(|_| read_error("daily brief has an invalid sync identity"))?;
            if record.status != DailyBriefStatus::Complete.as_str() {
                protected.insert(id);
                continue;
            }
            let Some(path) = record.local_path.as_deref() else {
                // A remotely sourced brief remains represented by its confirmed baseline.
                // Its DPAPI cache must never create a new local revision or tombstone.
                protected.insert(id);
                continue;
            };
            let Some(expected_hash) = record.content_hash.as_deref() else {
                protected.insert(id);
                continue;
            };
            let Ok(html) = std::fs::read_to_string(path) else {
                protected.insert(id);
                continue;
            };
            if validate_html(&html).is_err() || content_hash(&html) != expected_hash {
                protected.insert(id);
                continue;
            }
            let content = json!({
                "date": record.date,
                "status": DailyBriefStatus::Complete.as_str(),
                "html": html,
                "contentHash": expected_hash,
                "sourceFingerprint": record.source_fingerprint,
                "templateVersion": record.template_version,
                "promptVersion": record.prompt_version,
                "generatedAtMs": record.generated_at_ms,
                "updatedAtMs": record.updated_at_ms,
            });
            payloads.insert(id, portable_payload(PortableDomain::DailyBrief, content)?);
        }
        Ok(())
    }

    fn apply_records(&self, plan: &SyncLocalCommitPlan) -> Result<(), ConflictCenterError> {
        let mut provider_clients = HashSet::new();
        let mut prompt_clients = HashSet::new();
        let mut mcp_changed = false;
        for resolution in &plan.merge_batch.resolved {
            if resolution.local_action != SyncMergeSideAction::ApplyMerged {
                continue;
            }
            match resolution.record.id.domain {
                PortableDomain::Provider => {
                    provider_clients.insert(self.apply_provider(&resolution.record)?);
                }
                PortableDomain::Mcp => {
                    mcp_changed |= self.apply_mcp(&resolution.record)?;
                }
                PortableDomain::Prompt => {
                    prompt_clients.insert(self.apply_prompt(&resolution.record)?);
                }
                PortableDomain::Skill => self.apply_skill(&resolution.record)?,
                PortableDomain::CommonSnippet => {
                    self.apply_common_snippet(&resolution.record)?;
                }
                PortableDomain::DailyBrief => self.apply_daily_brief(&resolution.record)?,
                PortableDomain::PortableSetting => {
                    return Err(apply_error(
                        "sync record domain is not connected to a local runtime adapter",
                    ));
                }
            }
        }

        for client in provider_clients {
            ProviderService::sync_current_provider_for_app(self.state, LegacyAppType::from(client))
                .map_err(|_| {
                    apply_error("failed to project synchronized provider to live config")
                })?;
        }
        if mcp_changed {
            McpService::sync_all_enabled(self.state)
                .map_err(|_| apply_error("failed to project synchronized MCP servers"))?;
        }
        for client in prompt_clients {
            PromptService::sync_to_live(self.state, client)
                .map_err(|_| apply_error("failed to project synchronized Prompt"))?;
        }
        Ok(())
    }

    fn apply_provider(&self, record: &SyncRecord) -> Result<ManagedClientId, ConflictCenterError> {
        let (client_text, provider_id) = split_scoped_key(&record.id.key)?;
        let client = ManagedClientId::from_str(client_text)
            .map_err(|_| apply_error("synchronized provider client is invalid"))?;
        let app_type = LegacyAppType::from(client);
        if record.payload.is_none() {
            if self
                .state
                .db
                .get_provider_by_id(provider_id, app_type.as_str())
                .map_err(|_| apply_error("failed to read synchronized provider"))?
                .is_some()
            {
                self.state
                    .db
                    .delete_provider(app_type.as_str(), provider_id)
                    .map_err(|_| apply_error("failed to delete synchronized provider"))?;
            }
            return Ok(client);
        }
        let content = payload_content(record, PortableDomain::Provider)?;
        require_equal_string(&content, "clientId", client.as_str())?;
        require_equal_string(&content, "providerId", provider_id)?;
        let existing = self
            .state
            .db
            .get_provider_by_id(provider_id, app_type.as_str())
            .map_err(|_| apply_error("failed to read synchronized provider"))?;
        let mut provider = existing.unwrap_or_else(|| {
            Provider::with_id(
                provider_id.to_string(),
                required_string(&content, "name").unwrap_or_else(|_| provider_id.to_string()),
                Value::Object(Map::new()),
                None,
            )
        });
        provider.name = required_string(&content, "name")?;
        provider.category = Some(required_string(&content, "kind")?);
        provider.sort_index = optional_u64(&content, "sortIndex")?
            .map(|value| {
                usize::try_from(value).map_err(|_| apply_error("provider sort index is too large"))
            })
            .transpose()?;
        provider.notes = optional_string(&content, "notes")?;
        provider.icon = optional_string(&content, "icon")?;
        provider.icon_color = optional_string(&content, "iconColor")?;
        if provider.created_at.is_none() {
            provider.created_at = optional_i64(&content, "createdAtMs")?;
        }
        let portable_config = content
            .get("portableConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        apply_provider_portable_config(client, &mut provider.settings_config, &portable_config)?;
        self.state
            .db
            .save_provider(app_type.as_str(), &provider)
            .map_err(|_| apply_error("failed to save synchronized provider"))?;
        Ok(client)
    }

    fn apply_mcp(&self, record: &SyncRecord) -> Result<bool, ConflictCenterError> {
        let existing = self
            .state
            .db
            .get_all_mcp_servers()
            .map_err(|_| apply_error("failed to read synchronized MCP server"))?
            .shift_remove(&record.id.key);
        let affected_live = existing
            .as_ref()
            .is_some_and(|server| !server.apps.is_empty());
        let Some(_) = &record.payload else {
            self.state
                .db
                .delete_mcp_server(&record.id.key)
                .map_err(|_| apply_error("failed to delete synchronized MCP server"))?;
            return Ok(affected_live);
        };
        let content = payload_content(record, PortableDomain::Mcp)?;
        require_equal_string(&content, "id", &record.id.key)?;
        let incoming = content
            .get("serverConfig")
            .cloned()
            .ok_or_else(|| apply_error("synchronized MCP server is missing its config"))?;
        let server = McpServer {
            id: record.id.key.clone(),
            name: required_string(&content, "name")?,
            server: merge_local_only_fields(
                existing.as_ref().map(|server| &server.server),
                incoming,
            ),
            apps: content
                .get("apps")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| apply_error("synchronized MCP apps are invalid"))?
                .unwrap_or_default(),
            description: optional_string(&content, "description")?,
            homepage: optional_string(&content, "homepage")?,
            docs: optional_string(&content, "docs")?,
            tags: content
                .get("tags")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|_| apply_error("synchronized MCP tags are invalid"))?
                .unwrap_or_default(),
        };
        let affected_live = affected_live || !server.apps.is_empty();
        self.state
            .db
            .save_mcp_server(&server)
            .map_err(|_| apply_error("failed to save synchronized MCP server"))?;
        Ok(affected_live)
    }

    fn apply_prompt(&self, record: &SyncRecord) -> Result<ManagedClientId, ConflictCenterError> {
        let (client_text, prompt_id) = split_scoped_key(&record.id.key)?;
        let client = ManagedClientId::from_str(client_text)
            .map_err(|_| apply_error("synchronized Prompt client is invalid"))?;
        let Some(_) = &record.payload else {
            self.state
                .db
                .delete_prompt_version(client, prompt_id)
                .map_err(|_| apply_error("failed to delete synchronized Prompt"))?;
            return Ok(client);
        };
        let content = payload_content(record, PortableDomain::Prompt)?;
        require_equal_string(&content, "id", prompt_id)?;
        require_equal_string(&content, "clientId", client.as_str())?;
        let prompt = PromptVersion {
            id: prompt_id.to_string(),
            name: required_string(&content, "name")?,
            version: required_i64(&content, "version")?,
            content: required_string(&content, "content")?,
            description: optional_string(&content, "description")?,
            enabled: content
                .get("isActive")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            created_at: optional_i64(&content, "createdAtMs")?,
            updated_at: optional_i64(&content, "updatedAtMs")?,
        };
        self.state
            .db
            .save_prompt_version(client, &prompt)
            .map_err(|_| apply_error("failed to save synchronized Prompt"))?;
        Ok(client)
    }

    fn apply_skill(&self, record: &SyncRecord) -> Result<(), ConflictCenterError> {
        let trees = LocalSkillTreeAdapter::runtime();
        let existing = self
            .state
            .db
            .get_core_skill(&record.id.key)
            .map_err(|_| apply_error("failed to read synchronized Skill"))?;
        let Some(_) = &record.payload else {
            if let Some(existing) = existing {
                for client in existing.apps.enabled_clients() {
                    trees
                        .remove(client, &existing.directory)
                        .map_err(|_| apply_error("failed to remove synchronized Skill tree"))?;
                }
                self.state
                    .db
                    .delete_core_skill(&record.id.key)
                    .map_err(|_| apply_error("failed to delete synchronized Skill"))?;
            }
            return Ok(());
        };
        let content = payload_content(record, PortableDomain::Skill)?;
        require_equal_string(&content, "id", &record.id.key)?;
        let skill = local_skill_from_payload(&content)?;
        let files: SkillFilesPayload = content
            .get("files")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| apply_error("synchronized Skill files are invalid"))?
            .ok_or_else(|| apply_error("synchronized Skill files are missing"))?;
        let tree = files.into_tree(&skill)?;
        if let Some(existing) = &existing {
            for client in existing.apps.enabled_clients() {
                if !skill.apps.is_enabled_for(client) || existing.directory != skill.directory {
                    trees
                        .remove(client, &existing.directory)
                        .map_err(|_| apply_error("failed to remove obsolete Skill tree"))?;
                }
            }
        }
        for client in skill.apps.enabled_clients() {
            trees
                .replace(client, &skill.directory, &tree)
                .map_err(|_| apply_error("failed to replace synchronized Skill tree"))?;
        }
        self.state
            .db
            .save_core_skills(&[skill])
            .map_err(|_| apply_error("failed to save synchronized Skill"))
    }

    fn apply_common_snippet(&self, record: &SyncRecord) -> Result<(), ConflictCenterError> {
        let client = ManagedClientId::from_str(&record.id.key)
            .map_err(|_| apply_error("synchronized common snippet client is invalid"))?;
        let content = record
            .payload
            .as_ref()
            .map(|_| payload_content(record, PortableDomain::CommonSnippet))
            .transpose()?
            .map(|content| required_string(&content, "content"))
            .transpose()?
            .unwrap_or_default();
        CommonSnippetService::set(self.state, client, content)
            .map_err(|_| apply_error("failed to apply synchronized common snippet"))
    }

    fn apply_daily_brief(&self, record: &SyncRecord) -> Result<(), ConflictCenterError> {
        let (date, device_id) = split_scoped_key(&record.id.key)?;
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map_err(|_| apply_error("synchronized daily brief date is invalid"))?;
        let existing = self
            .state
            .db
            .list_daily_briefs()
            .map_err(|_| apply_error("failed to read synchronized daily brief"))?
            .into_iter()
            .find(|item| item.date == date && item.device_id == device_id);
        let Some(_) = &record.payload else {
            if let Some(existing) = existing {
                daily_brief::delete_record(&self.state.db, &existing.date, &existing.device_id)
                    .map_err(|_| apply_error("failed to delete synchronized daily brief"))?;
            } else {
                daily_brief::remove_synced_brief_cache(date, device_id)
                    .map_err(|_| apply_error("failed to delete synchronized daily brief cache"))?;
            }
            return Ok(());
        };
        let content = payload_content(record, PortableDomain::DailyBrief)?;
        require_equal_string(&content, "date", date)?;
        require_equal_string(&content, "status", DailyBriefStatus::Complete.as_str())?;
        let html = required_string(&content, "html")?;
        let expected_hash = required_string(&content, "contentHash")?;
        validate_html(&html)
            .map_err(|_| apply_error("synchronized daily brief HTML is invalid"))?;
        if content_hash(&html) != expected_hash {
            return Err(apply_error(
                "synchronized daily brief content hash does not match",
            ));
        }
        let local_path = existing
            .as_ref()
            .and_then(|item| item.local_path.clone())
            .filter(|path| {
                std::fs::read_to_string(path).is_ok_and(|local_html| {
                    validate_html(&local_html).is_ok() && content_hash(&local_html) == expected_hash
                })
            });
        if local_path.is_none() {
            daily_brief::store_synced_brief_cache(date, device_id, &html, &expected_hash)
                .map_err(|_| apply_error("failed to protect synchronized daily brief cache"))?;
        }
        let brief = DailyBriefRecord {
            date: date.to_string(),
            device_id: device_id.to_string(),
            status: DailyBriefStatus::Complete.as_str().to_string(),
            source_fingerprint: optional_string(&content, "sourceFingerprint")?,
            content_hash: Some(expected_hash),
            local_path,
            source_state: "present".to_string(),
            model_name: existing.and_then(|item| item.model_name),
            template_version: optional_string(&content, "templateVersion")?,
            prompt_version: optional_string(&content, "promptVersion")?,
            generated_at_ms: optional_i64(&content, "generatedAtMs")?,
            updated_at_ms: required_i64(&content, "updatedAtMs")?,
        };
        self.state
            .db
            .upsert_daily_brief(&brief)
            .map_err(|_| apply_error("failed to save synchronized daily brief"))
    }

    fn validate_committed_metadata(
        &self,
        plan: &SyncLocalCommitPlan,
    ) -> Result<(), ConflictCenterError> {
        let baselines = self
            .state
            .db
            .load_sync_baselines()
            .map_err(|_| validation_error("failed to validate committed sync baselines"))?;
        let indexed = baselines
            .iter()
            .map(|baseline| (&baseline.record.id, baseline))
            .collect::<BTreeMap<_, _>>();
        for resolution in &plan.merge_batch.resolved {
            let Some(baseline) = indexed.get(&resolution.record.id) else {
                return Err(validation_error("committed sync baseline is missing"));
            };
            if baseline.confirmed_generation != plan.committed_generation
                || baseline.record != resolution.record
            {
                return Err(validation_error("committed sync baseline does not match"));
            }
        }
        let devices = self
            .state
            .db
            .load_sync_devices()
            .map_err(|_| validation_error("failed to validate committed sync devices"))?;
        if devices != plan.devices {
            return Err(validation_error(
                "committed sync device registry does not match",
            ));
        }
        if let Some(expected) = &plan.fixed_identity {
            let actual = self
                .state
                .db
                .load_sync_identity()
                .map_err(|_| validation_error("failed to validate fixed sync identity"))?;
            if actual.as_ref() != Some(expected) {
                return Err(validation_error("fixed sync identity was not committed"));
            }
        }
        Ok(())
    }
}

type PortableCapture = (
    BTreeMap<PortableRecordId, PortablePayload>,
    BTreeSet<PortableRecordId>,
);

impl SyncLocalApplyPort for RuntimeSyncLocalAdapter<'_> {
    fn capture_rollback(&self, plan: &SyncLocalCommitPlan) -> Result<Vec<u8>, ConflictCenterError> {
        plan.validate()
            .map_err(|_| invalid_input("sync local commit plan is invalid"))?;
        let database_sql = self
            .state
            .db
            .export_sql_string()
            .map_err(|_| read_error("failed to capture encrypted rollback database input"))?;
        serde_json::to_vec(&SyncRollbackPayload {
            schema_version: 1,
            database_sql,
        })
        .map_err(|_| read_error("failed to encode sync rollback payload"))
    }

    fn apply_and_validate(&self, plan: &SyncLocalCommitPlan) -> Result<(), ConflictCenterError> {
        plan.validate()
            .map_err(|_| invalid_input("sync local commit plan is invalid"))?;
        self.apply_records(plan)?;
        self.state
            .db
            .commit_sync_metadata(plan)
            .map_err(|_| apply_error("failed to commit local sync metadata"))?;
        self.validate_committed_metadata(plan)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncRollbackPayload {
    schema_version: u32,
    database_sql: String,
}

fn build_local_records(
    device_id: &SyncDeviceId,
    now_ms: i64,
    baselines: &[SyncRecordBaseline],
    mut payloads: BTreeMap<PortableRecordId, PortablePayload>,
    protected: &BTreeSet<PortableRecordId>,
) -> Result<Vec<SyncRecord>, ConflictCenterError> {
    let baseline_map = baselines
        .iter()
        .map(|baseline| (baseline.record.id.clone(), baseline))
        .collect::<BTreeMap<_, _>>();
    let mut ids = baseline_map.keys().cloned().collect::<BTreeSet<_>>();
    ids.extend(payloads.keys().cloned());
    let mut records = Vec::with_capacity(ids.len());
    for id in ids {
        let baseline = baseline_map.get(&id).copied();
        let payload = payloads.remove(&id);
        let record = match (baseline, payload) {
            (Some(baseline), Some(payload))
                if baseline.record.payload.as_ref() == Some(&payload) =>
            {
                baseline.record.clone()
            }
            (Some(baseline), Some(payload)) => SyncRecord::live(
                id,
                device_id.clone(),
                next_counter(baseline.record.revision.counter)?,
                now_ms,
                payload,
            )
            .map_err(|_| read_error("failed to revise local sync record"))?,
            (None, Some(payload)) => SyncRecord::live(id, device_id.clone(), 1, now_ms, payload)
                .map_err(|_| read_error("failed to create local sync record"))?,
            (Some(baseline), None) if protected.contains(&id) => baseline.record.clone(),
            (Some(baseline), None) if baseline.record.tombstone.is_some() => {
                baseline.record.clone()
            }
            (Some(baseline), None) => SyncRecord::deleted(
                id,
                device_id.clone(),
                next_counter(baseline.record.revision.counter)?,
                now_ms,
                baseline
                    .confirmed_generation
                    .checked_add(1)
                    .ok_or_else(|| read_error("sync generation overflow"))?,
            )
            .map_err(|_| read_error("failed to create local sync tombstone"))?,
            (None, None) => continue,
        };
        records.push(record);
    }
    Ok(records)
}

fn next_counter(value: u64) -> Result<u64, ConflictCenterError> {
    value
        .checked_add(1)
        .ok_or_else(|| read_error("sync record revision overflow"))
}

fn portable_payload(
    domain: PortableDomain,
    mut content: Value,
) -> Result<PortablePayload, ConflictCenterError> {
    remove_null_fields(&mut content);
    PortablePayload::new(domain, content)
        .map_err(|_| read_error("local portable payload is invalid"))
}

fn remove_null_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, value| !value.is_null());
            for value in object.values_mut() {
                remove_null_fields(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_null_fields(value);
            }
        }
        _ => {}
    }
}

fn sanitize_portable_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !is_local_only_field(key))
                .map(|(key, value)| (key.clone(), sanitize_portable_value(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_portable_value).collect()),
        scalar => scalar.clone(),
    }
}

fn merge_local_only_fields(existing: Option<&Value>, mut incoming: Value) -> Value {
    let (Some(Value::Object(existing)), Value::Object(incoming_object)) = (existing, &mut incoming)
    else {
        return incoming;
    };
    for (key, existing_value) in existing {
        if is_local_only_field(key) {
            incoming_object.insert(key.clone(), existing_value.clone());
        } else if let Some(incoming_value) = incoming_object.get_mut(key) {
            *incoming_value = merge_local_only_fields(Some(existing_value), incoming_value.clone());
        }
    }
    incoming
}

fn is_local_only_field(field: &str) -> bool {
    let normalized = field
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "env"
            | "auth"
            | "headers"
            | "authorization"
            | "credentials"
            | "config"
            | "password"
            | "passphrase"
            | "cookie"
            | "privatekey"
            | "accesstoken"
            | "refreshtoken"
    ) || [
        "apikey",
        "token",
        "password",
        "passphrase",
        "secret",
        "cookie",
        "privatekey",
        "credential",
        "authorization",
    ]
    .iter()
    .any(|suffix| normalized == *suffix || normalized.ends_with(suffix))
}

fn provider_portable_config(client: ManagedClientId, settings: &Value) -> Value {
    let mut result = Map::new();
    match client {
        ManagedClientId::Claude => {
            let env = settings.get("env").and_then(Value::as_object);
            copy_string(env, "ANTHROPIC_BASE_URL", &mut result, "baseUrl");
            copy_string(env, "ANTHROPIC_MODEL", &mut result, "model");
            copy_string(env, "ANTHROPIC_SMALL_FAST_MODEL", &mut result, "smallModel");
        }
        ManagedClientId::Codex => {
            let config = settings.get("config").and_then(Value::as_str);
            if let Some(base_url) = config.and_then(crate::codex_config::extract_codex_base_url) {
                result.insert("baseUrl".to_string(), Value::String(base_url));
            }
            if let Some(model) = config
                .and_then(|value| toml::from_str::<toml::Value>(value).ok())
                .and_then(|value| {
                    value
                        .get("model")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                })
            {
                result.insert("model".to_string(), Value::String(model));
            }
        }
        ManagedClientId::Opencode => {
            let options = settings.get("options").and_then(Value::as_object);
            copy_string(options, "baseURL", &mut result, "baseUrl");
            copy_string(options, "model", &mut result, "model");
        }
    }
    Value::Object(result)
}

fn copy_string(
    source: Option<&Map<String, Value>>,
    source_key: &str,
    target: &mut Map<String, Value>,
    target_key: &str,
) {
    if let Some(value) = source
        .and_then(|source| source.get(source_key))
        .and_then(Value::as_str)
    {
        if !value.trim().is_empty() {
            target.insert(target_key.to_string(), Value::String(value.to_string()));
        }
    }
}

fn apply_provider_portable_config(
    client: ManagedClientId,
    settings: &mut Value,
    portable: &Value,
) -> Result<(), ConflictCenterError> {
    let portable = portable
        .as_object()
        .ok_or_else(|| apply_error("synchronized provider portable config is invalid"))?;
    let settings_object = ensure_object(settings)?;
    match client {
        ManagedClientId::Claude => {
            let env = ensure_object_entry(settings_object, "env")?;
            apply_optional_string(portable, "baseUrl", env, "ANTHROPIC_BASE_URL")?;
            apply_optional_string(portable, "model", env, "ANTHROPIC_MODEL")?;
            apply_optional_string(portable, "smallModel", env, "ANTHROPIC_SMALL_FAST_MODEL")?;
        }
        ManagedClientId::Codex => {
            let current = settings_object
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut config = toml::from_str::<toml::Value>(current)
                .unwrap_or_else(|_| toml::Value::Table(Default::default()));
            let table = config
                .as_table_mut()
                .ok_or_else(|| apply_error("local Codex provider config is invalid"))?;
            if let Some(model) = portable.get("model").and_then(Value::as_str) {
                table.insert("model".to_string(), toml::Value::String(model.to_string()));
            }
            if let Some(base_url) = portable.get("baseUrl").and_then(Value::as_str) {
                table.insert(
                    "model_provider".to_string(),
                    toml::Value::String("sync".to_string()),
                );
                let providers = table
                    .entry("model_providers".to_string())
                    .or_insert_with(|| toml::Value::Table(Default::default()))
                    .as_table_mut()
                    .ok_or_else(|| apply_error("local Codex provider table is invalid"))?;
                let sync = providers
                    .entry("sync".to_string())
                    .or_insert_with(|| toml::Value::Table(Default::default()))
                    .as_table_mut()
                    .ok_or_else(|| apply_error("local Codex sync provider is invalid"))?;
                sync.insert("name".to_string(), toml::Value::String("Sync".to_string()));
                sync.insert(
                    "base_url".to_string(),
                    toml::Value::String(base_url.to_string()),
                );
                sync.insert(
                    "wire_api".to_string(),
                    toml::Value::String("responses".to_string()),
                );
            }
            settings_object.insert(
                "config".to_string(),
                Value::String(toml::to_string(&config).map_err(|_| {
                    apply_error("failed to encode synchronized Codex provider config")
                })?),
            );
        }
        ManagedClientId::Opencode => {
            let options = ensure_object_entry(settings_object, "options")?;
            apply_optional_string(portable, "baseUrl", options, "baseURL")?;
            apply_optional_string(portable, "model", options, "model")?;
        }
    }
    Ok(())
}

fn ensure_object(value: &mut Value) -> Result<&mut Map<String, Value>, ConflictCenterError> {
    if value.is_null() {
        *value = Value::Object(Map::new());
    }
    value
        .as_object_mut()
        .ok_or_else(|| apply_error("local provider settings are invalid"))
}

fn ensure_object_entry<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>, ConflictCenterError> {
    object
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| apply_error("local provider settings section is invalid"))
}

fn apply_optional_string(
    source: &Map<String, Value>,
    source_key: &str,
    target: &mut Map<String, Value>,
    target_key: &str,
) -> Result<(), ConflictCenterError> {
    if let Some(value) = source.get(source_key) {
        let value = value
            .as_str()
            .ok_or_else(|| apply_error("synchronized provider config field is invalid"))?;
        target.insert(target_key.to_string(), Value::String(value.to_string()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillFilesPayload {
    directories: Vec<String>,
    entries: Vec<SkillFilePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SkillFilePayload {
    path: String,
    content_base64: String,
}

impl SkillFilesPayload {
    fn from_tree(tree: &LocalSkillTree) -> Self {
        Self {
            directories: tree.directories.clone(),
            entries: tree
                .files
                .iter()
                .map(|file| SkillFilePayload {
                    path: file.relative_path.clone(),
                    content_base64: BASE64.encode(&file.contents),
                })
                .collect(),
        }
    }

    fn into_tree(self, skill: &LocalSkill) -> Result<LocalSkillTree, ConflictCenterError> {
        let files = self
            .entries
            .into_iter()
            .map(|entry| {
                Ok(LocalSkillFile {
                    relative_path: entry.path,
                    contents: BASE64
                        .decode(entry.content_base64)
                        .map_err(|_| apply_error("synchronized Skill file is not valid base64"))?,
                })
            })
            .collect::<Result<Vec<_>, ConflictCenterError>>()?;
        Ok(LocalSkillTree {
            directories: self.directories,
            files,
            content_hash: skill
                .content_hash
                .clone()
                .ok_or_else(|| apply_error("synchronized Skill content hash is missing"))?,
            total_size_bytes: skill.total_size_bytes,
            file_count: skill.file_count,
        })
    }
}

fn local_skill_from_payload(content: &Value) -> Result<LocalSkill, ConflictCenterError> {
    let apps: ManagedClientApps = content
        .get("apps")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| apply_error("synchronized Skill apps are invalid"))?
        .unwrap_or_default();
    let skill = LocalSkill {
        id: required_string(content, "id")?,
        name: required_string(content, "name")?,
        description: optional_string(content, "description")?,
        directory: required_string(content, "directory")?,
        content_hash: optional_string(content, "contentHash")?,
        total_size_bytes: required_u64(content, "totalSizeBytes")?,
        file_count: required_u64(content, "fileCount")?,
        apps,
        cloud_eligible: content
            .get("cloudEligible")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        created_at_ms: required_i64(content, "createdAtMs")?,
        updated_at_ms: required_i64(content, "updatedAtMs")?,
    };
    skill
        .validate()
        .map_err(|_| apply_error("synchronized Skill metadata is invalid"))?;
    Ok(skill)
}

fn payload_content(
    record: &SyncRecord,
    expected_domain: PortableDomain,
) -> Result<Value, ConflictCenterError> {
    let payload = record
        .payload
        .as_ref()
        .ok_or_else(|| apply_error("synchronized live record has no payload"))?;
    if payload.domain != expected_domain || record.id.domain != expected_domain {
        return Err(apply_error("synchronized record domain does not match"));
    }
    Ok(payload.content())
}

fn split_scoped_key(value: &str) -> Result<(&str, &str), ConflictCenterError> {
    let (scope, id) = value
        .split_once(':')
        .ok_or_else(|| apply_error("synchronized scoped record key is invalid"))?;
    if scope.is_empty() || id.is_empty() {
        return Err(apply_error("synchronized scoped record key is invalid"));
    }
    Ok((scope, id))
}

fn require_equal_string(
    content: &Value,
    key: &str,
    expected: &str,
) -> Result<(), ConflictCenterError> {
    if content.get(key).and_then(Value::as_str) == Some(expected) {
        Ok(())
    } else {
        Err(apply_error("synchronized record identity does not match"))
    }
}

fn required_string(content: &Value, key: &str) -> Result<String, ConflictCenterError> {
    content
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| apply_error("synchronized record is missing a required string"))
}

fn optional_string(content: &Value, key: &str) -> Result<Option<String>, ConflictCenterError> {
    content
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| apply_error("synchronized optional string is invalid"))
        })
        .transpose()
}

fn required_u64(content: &Value, key: &str) -> Result<u64, ConflictCenterError> {
    content
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| apply_error("synchronized record is missing a required integer"))
}

fn optional_u64(content: &Value, key: &str) -> Result<Option<u64>, ConflictCenterError> {
    content
        .get(key)
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| apply_error("synchronized optional integer is invalid"))
        })
        .transpose()
}

fn required_i64(content: &Value, key: &str) -> Result<i64, ConflictCenterError> {
    content
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| apply_error("synchronized record is missing a required integer"))
}

fn optional_i64(content: &Value, key: &str) -> Result<Option<i64>, ConflictCenterError> {
    content
        .get(key)
        .map(|value| {
            value
                .as_i64()
                .ok_or_else(|| apply_error("synchronized optional integer is invalid"))
        })
        .transpose()
}

fn invalid_input(message: impl Into<String>) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::InvalidInput, message)
}

fn read_error(message: impl Into<String>) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::Read, message)
}

fn apply_error(message: impl Into<String>) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::Apply, message)
}

fn validation_error(message: impl Into<String>) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::Validation, message)
}
