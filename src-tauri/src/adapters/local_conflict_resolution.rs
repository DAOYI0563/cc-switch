use std::sync::Arc;

use serde::Serialize;
use serde_json::{json, Value};

use crate::adapters::live_provider_config::runtime_adapter;
use crate::adapters::local_reconciliation_state::DatabaseLocalReconciliationStateAdapter;
use crate::adapters::local_scan_parser::{
    DatabaseLocalScanParserAdapter, FixedLocalScanParserAdapter,
};
use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::app_config::{LegacyAppType, McpApps, McpServer};
use crate::domain::{
    ConflictCenterDisposition, ConflictCenterItem, ConflictCenterSource, ConflictResolutionAction,
    ConflictResolutionRequest, LocalConflictKind, LocalScanDomain, LocalScanTarget,
    LocalSkillImport, ManagedClientApps, ManagedClientId, PortableDomain, PromptVersion,
};
use crate::ports::{
    ConflictCenterError, ConflictCenterErrorCode, ConflictCenterResolutionPort,
    LiveProviderConfigPort, LiveProviderRecord, LocalReconciliationBaselinePort,
    LocalReconciliationStatePort, LocalScanParserPort, LocalSkillTreePort,
};
use crate::provider::{Provider, ProviderMeta};
use crate::services::{
    reconciliation_snapshot_from_parsed, record_database_local_writes, record_runtime_local_writes,
    LocalScanCoordinator, LocalSkillService, McpService, PromptService, ProviderService,
};
use crate::store::AppState;

pub struct RuntimeLocalConflictResolution<'a> {
    app_state: &'a AppState,
    coordinator: Arc<LocalScanCoordinator>,
    baselines: Arc<dyn LocalReconciliationBaselinePort>,
}

impl<'a> RuntimeLocalConflictResolution<'a> {
    pub fn new(
        app_state: &'a AppState,
        coordinator: Arc<LocalScanCoordinator>,
        baselines: Arc<dyn LocalReconciliationBaselinePort>,
    ) -> Self {
        Self {
            app_state,
            coordinator,
            baselines,
        }
    }

    fn state_adapter(&self) -> DatabaseLocalReconciliationStateAdapter {
        DatabaseLocalReconciliationStateAdapter::new(
            self.app_state.db.clone(),
            self.baselines.clone(),
        )
    }

    fn local_value(
        &self,
        target: LocalScanTarget,
        record_id: &str,
    ) -> Result<Option<Value>, ConflictCenterError> {
        self.state_adapter()
            .read_parsed_local(target)
            .map_err(|_| read_error("read application projection"))
            .map(|snapshot| {
                snapshot
                    .records
                    .into_iter()
                    .find(|record| record.record_id == record_id)
                    .map(|record| record.value)
            })
    }

    fn external_value(
        &self,
        target: LocalScanTarget,
        record_id: &str,
    ) -> Result<Option<Value>, ConflictCenterError> {
        let pending = self.coordinator.pending_change(target).ok_or_else(|| {
            ConflictCenterError::new(
                ConflictCenterErrorCode::StaleItem,
                "local scan change is no longer pending",
            )
        })?;
        Ok(pending.parsed_snapshot().and_then(|snapshot| {
            snapshot
                .records
                .iter()
                .find(|record| record.record_id == record_id)
                .map(|record| record.value.clone())
        }))
    }

    fn accept_external(
        &self,
        target: LocalScanTarget,
        record_id: &str,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        match target.domain {
            LocalScanDomain::Provider => {
                self.accept_provider_external(target.client_id, record_id, external)
            }
            LocalScanDomain::Mcp => self.accept_mcp_external(target.client_id, record_id, external),
            LocalScanDomain::Prompt => {
                self.accept_prompt_external(target.client_id, record_id, external)
            }
            LocalScanDomain::Skill => {
                self.accept_skill_external(target.client_id, record_id, external)
            }
        }
    }

    fn accept_provider_external(
        &self,
        client: ManagedClientId,
        record_id: &str,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        let app_type = LegacyAppType::from(client);
        if client == ManagedClientId::Opencode {
            let existing = self
                .app_state
                .db
                .get_provider_by_id(record_id, app_type.as_str())
                .map_err(apply_error)?;
            let Some(settings) = external else {
                if let Some(mut provider) = existing {
                    let meta = provider.meta.get_or_insert_with(ProviderMeta::default);
                    meta.live_config_managed = Some(false);
                    self.app_state
                        .db
                        .save_provider(app_type.as_str(), &provider)
                        .map_err(apply_error)?;
                }
                return Ok(());
            };
            let mut provider = existing.unwrap_or_else(|| empty_provider(record_id));
            provider.settings_config = settings;
            provider
                .meta
                .get_or_insert_with(ProviderMeta::default)
                .live_config_managed = Some(true);
            self.app_state
                .db
                .save_provider(app_type.as_str(), &provider)
                .map_err(apply_error)?;
            return Ok(());
        }

        let Some(settings) = external else {
            self.app_state
                .db
                .set_current_provider_optional(app_type.as_str(), None)
                .map_err(apply_error)?;
            return Ok(());
        };
        let current = self
            .app_state
            .db
            .get_current_provider(app_type.as_str())
            .map_err(apply_error)?;
        let id = current.unwrap_or_else(|| "wsl-live".to_string());
        let mut provider = self
            .app_state
            .db
            .get_provider_by_id(&id, app_type.as_str())
            .map_err(apply_error)?
            .unwrap_or_else(|| empty_provider(&id));
        provider.settings_config = settings;
        self.app_state
            .db
            .save_provider(app_type.as_str(), &provider)
            .and_then(|()| {
                self.app_state
                    .db
                    .set_current_provider(app_type.as_str(), &id)
            })
            .map_err(apply_error)
    }

    fn accept_mcp_external(
        &self,
        client: ManagedClientId,
        record_id: &str,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        let existing = self
            .app_state
            .db
            .get_all_mcp_servers()
            .map_err(apply_error)?
            .get(record_id)
            .cloned();
        let Some(server_value) = external else {
            if let Some(mut server) = existing {
                server.apps.set_enabled_for(client, false);
                self.app_state
                    .db
                    .save_mcp_server(&server)
                    .map_err(apply_error)?;
            }
            return Ok(());
        };
        let mut server = existing.unwrap_or_else(|| McpServer {
            id: record_id.to_string(),
            name: record_id.to_string(),
            server: Value::Null,
            apps: McpApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        });
        server.server = server_value;
        server.apps.set_enabled_for(client, true);
        self.app_state
            .db
            .save_mcp_server(&server)
            .map_err(apply_error)
    }

    fn accept_prompt_external(
        &self,
        client: ManagedClientId,
        record_id: &str,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        if record_id != "prompt-live" {
            return Err(invalid_input("unexpected Prompt live record id"));
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let Some(content) = external
            .as_ref()
            .and_then(|value| value.get("content"))
            .and_then(Value::as_str)
        else {
            if external.is_some() {
                return Err(invalid_input("Prompt external value is invalid"));
            }
            return self
                .app_state
                .db
                .disable_all_prompt_versions(client, now_ms)
                .map_err(apply_error);
        };
        let prompt = self
            .app_state
            .db
            .prepare_prompt_version(
                client,
                PromptVersion {
                    id: format!("prompt-{}", uuid::Uuid::new_v4().simple()),
                    name: "Accepted from WSL live".to_string(),
                    version: 0,
                    content: content.to_string(),
                    description: Some("Confirmed in conflict center".to_string()),
                    enabled: true,
                    created_at: None,
                    updated_at: None,
                },
            )
            .map_err(apply_error)?;
        self.app_state
            .db
            .save_prompt_version(client, &prompt)
            .map_err(apply_error)
    }

    fn accept_skill_external(
        &self,
        client: ManagedClientId,
        directory: &str,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        let existing = self
            .app_state
            .db
            .list_core_skills()
            .map_err(apply_error)?
            .into_iter()
            .find(|skill| skill.directory == directory);
        let Some(_) = external else {
            if let Some(mut skill) = existing {
                skill.apps.set_enabled_for(client, false);
                if skill.apps.is_empty() {
                    self.app_state
                        .db
                        .delete_core_skill(&skill.id)
                        .map_err(apply_error)?;
                } else {
                    self.app_state
                        .db
                        .save_core_skills(&[skill])
                        .map_err(apply_error)?;
                }
            }
            return Ok(());
        };

        if let Some(original) = existing {
            let mut enabled = original.clone();
            enabled.apps.set_enabled_for(client, true);
            if enabled != original {
                self.app_state
                    .db
                    .save_core_skills(&[enabled.clone()])
                    .map_err(apply_error)?;
            }
            match LocalSkillService::sync_from_live(self.app_state, &enabled.id, client) {
                Ok(_) => Ok(()),
                Err(primary) => {
                    if enabled != original {
                        let _ = self.app_state.db.save_core_skills(&[original]);
                    }
                    Err(apply_error(primary))
                }
            }
        } else {
            let mut apps = ManagedClientApps::default();
            apps.set_enabled_for(client, true);
            LocalSkillService::import_from_live(
                self.app_state,
                vec![LocalSkillImport {
                    directory: directory.to_string(),
                    source_client: client,
                    apps,
                }],
            )
            .map(|_| ())
            .map_err(apply_error)
        }
    }

    fn keep_local(
        &self,
        target: LocalScanTarget,
        record_id: &str,
        local: Option<Value>,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        match target.domain {
            LocalScanDomain::Provider => {
                self.keep_local_provider(target.client_id, record_id, local, external)
            }
            LocalScanDomain::Mcp => {
                McpService::sync_enabled_for_app(self.app_state, target.client_id)
                    .map_err(apply_error)
            }
            LocalScanDomain::Prompt => {
                PromptService::sync_to_live(self.app_state, target.client_id).map_err(apply_error)
            }
            LocalScanDomain::Skill => self.keep_local_skill(target.client_id, record_id, local),
        }
    }

    fn keep_local_provider(
        &self,
        client: ManagedClientId,
        record_id: &str,
        local: Option<Value>,
        external: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        let app_type = LegacyAppType::from(client);
        if client != ManagedClientId::Opencode {
            let current = self
                .app_state
                .db
                .get_current_provider(app_type.as_str())
                .map_err(apply_error)?
                .ok_or_else(|| invalid_input("no local current provider is available"))?;
            return ProviderService::switch_managed(self.app_state, client, &current)
                .map(|_| ())
                .map_err(apply_error);
        }

        let Some(settings) = local else {
            runtime_adapter(client)
                .remove(record_id)
                .map_err(|_| apply_failed())?;
            record_runtime_local_writes(
                &self.app_state.local_scan_writes,
                [LocalScanTarget {
                    domain: LocalScanDomain::Provider,
                    client_id: client,
                }],
            );
            return Ok(());
        };
        let mut provider = self
            .app_state
            .db
            .get_provider_by_id(record_id, app_type.as_str())
            .map_err(apply_error)?
            .ok_or_else(|| invalid_input("local OpenCode provider was not found"))?;
        let adapter = runtime_adapter(client);
        let record = live_provider_record(client, &provider, settings);
        adapter.write(&record).map_err(|_| apply_failed())?;
        provider
            .meta
            .get_or_insert_with(ProviderMeta::default)
            .live_config_managed = Some(true);
        if let Err(primary) = self
            .app_state
            .db
            .save_provider(app_type.as_str(), &provider)
        {
            restore_opencode_external(adapter.as_ref(), record_id, external, &provider);
            return Err(apply_error(primary));
        }
        record_runtime_local_writes(
            &self.app_state.local_scan_writes,
            [LocalScanTarget {
                domain: LocalScanDomain::Provider,
                client_id: client,
            }],
        );
        Ok(())
    }

    fn keep_local_skill(
        &self,
        target_client: ManagedClientId,
        directory: &str,
        local: Option<Value>,
    ) -> Result<(), ConflictCenterError> {
        let trees = LocalSkillTreeAdapter::runtime();
        let target_before = trees
            .capture(target_client, directory)
            .map_err(|_| apply_failed())?;
        if local.is_none() {
            trees
                .remove(target_client, directory)
                .map_err(|_| apply_failed())?;
            record_database_local_writes(
                &self.app_state.local_scan_writes,
                self.app_state.db.clone(),
                [LocalScanTarget {
                    domain: LocalScanDomain::Skill,
                    client_id: target_client,
                }],
            );
            return Ok(());
        }

        let skill = self
            .app_state
            .db
            .list_core_skills()
            .map_err(apply_error)?
            .into_iter()
            .find(|skill| skill.directory == directory)
            .ok_or_else(|| invalid_input("local Skill metadata was not found"))?;
        let baseline = skill
            .content_hash
            .as_deref()
            .ok_or_else(|| invalid_input("local Skill has no confirmed content baseline"))?;
        let mut source_tree = None;
        for client in skill
            .apps
            .enabled_clients()
            .filter(|client| *client != target_client)
        {
            let snapshot = trees
                .capture(client, directory)
                .map_err(|_| apply_failed())?;
            if snapshot
                .tree
                .as_ref()
                .is_some_and(|tree| tree.content_hash == baseline)
            {
                source_tree = snapshot.tree;
                break;
            }
        }
        let source_tree = source_tree.ok_or_else(|| {
            invalid_input("no enabled client retains the confirmed local Skill tree")
        })?;
        if let Err(primary) = trees.replace(target_client, directory, &source_tree) {
            let _ = trees.restore(&target_before);
            return Err(ConflictCenterError::new(
                ConflictCenterErrorCode::Apply,
                "failed to restore local Skill tree",
            )
            .with_context("treeCode", format!("{:?}", primary.code)));
        }
        record_database_local_writes(
            &self.app_state.local_scan_writes,
            self.app_state.db.clone(),
            [LocalScanTarget {
                domain: LocalScanDomain::Skill,
                client_id: target_client,
            }],
        );
        Ok(())
    }

    fn validate_expected(
        &self,
        target: LocalScanTarget,
        record_id: &str,
        expected: Option<&str>,
        live: bool,
    ) -> Result<(), ConflictCenterError> {
        let parsed = if live && target.domain == LocalScanDomain::Skill {
            DatabaseLocalScanParserAdapter::runtime(self.app_state.db.clone())
                .parse_changed(target)
                .map_err(|_| validation_failed())?
        } else if live {
            FixedLocalScanParserAdapter::runtime()
                .parse_changed(target)
                .map_err(|_| validation_failed())?
        } else {
            self.state_adapter()
                .read_parsed_local(target)
                .map_err(|_| validation_failed())?
        };
        let snapshot =
            reconciliation_snapshot_from_parsed(&parsed).map_err(|_| validation_failed())?;
        let actual = snapshot
            .records
            .iter()
            .find(|record| record.record_id == record_id)
            .map(|record| record.content_digest.as_str());
        if actual == expected {
            Ok(())
        } else {
            Err(validation_failed())
        }
    }

    fn clear_pending_when_no_record_work_remains(
        &self,
        target: LocalScanTarget,
    ) -> Result<(), ConflictCenterError> {
        let Some(pending) = self.coordinator.pending_change(target) else {
            return Ok(());
        };
        if pending.parsed_snapshot().is_none() {
            return Ok(());
        }
        let state = self
            .state_adapter()
            .read_reconciliation_state(target)
            .map_err(|_| validation_failed())?;
        let batch = pending
            .classify_against(state.baseline, state.local)
            .map_err(|_| validation_failed())?;
        let has_record_work = !batch.differences.is_empty()
            || batch
                .conflicts
                .iter()
                .any(|conflict| conflict.record_id.is_some());
        if !has_record_work {
            self.coordinator.take_pending_change(target);
        }
        Ok(())
    }
}

impl ConflictCenterResolutionPort for RuntimeLocalConflictResolution<'_> {
    fn supported_actions(
        &self,
        item: &ConflictCenterItem,
    ) -> Result<Vec<ConflictResolutionAction>, ConflictCenterError> {
        if item.source != ConflictCenterSource::LocalScan {
            return Ok(Vec::new());
        }
        if matches!(
            item.disposition,
            ConflictCenterDisposition::Conflict(
                LocalConflictKind::ParseFailed | LocalConflictKind::IntegrityMismatch
            )
        ) {
            return Ok(vec![ConflictResolutionAction::Retry]);
        }
        let target = local_target(item)?;
        let record_id = item
            .record_id
            .as_deref()
            .ok_or_else(|| invalid_input("local conflict item has no record id"))?;
        let local = self.local_value(target, record_id)?;
        let mut actions = vec![ConflictResolutionAction::AcceptExternal];
        let keep_local_supported = match target.domain {
            LocalScanDomain::Provider => {
                target.client_id == ManagedClientId::Opencode || local.is_some()
            }
            LocalScanDomain::Skill if local.is_some() => self
                .app_state
                .db
                .list_core_skills()
                .map_err(apply_error)?
                .into_iter()
                .find(|skill| skill.directory == record_id)
                .is_some_and(|skill| {
                    skill
                        .apps
                        .enabled_clients()
                        .any(|client| client != target.client_id)
                }),
            LocalScanDomain::Mcp | LocalScanDomain::Prompt | LocalScanDomain::Skill => true,
        };
        if keep_local_supported {
            actions.push(ConflictResolutionAction::KeepLocal);
        }
        Ok(actions)
    }

    fn capture_rollback(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<Vec<u8>, ConflictCenterError> {
        let target = local_target(item)?;
        let record_id = item.record_id.as_deref();
        let local = record_id
            .map(|record_id| self.local_value(target, record_id))
            .transpose()?
            .flatten();
        let external = record_id
            .map(|record_id| self.external_value(target, record_id))
            .transpose()?
            .flatten();
        let skill_tree = if target.domain == LocalScanDomain::Skill {
            record_id
                .map(|directory| capture_skill_tree(target.client_id, directory))
                .transpose()?
        } else {
            None
        };
        serde_json::to_vec(&LocalResolutionRollbackPayload {
            schema_version: 1,
            item,
            action: request.action,
            local,
            external,
            skill_tree,
        })
        .map_err(|_| {
            ConflictCenterError::new(
                ConflictCenterErrorCode::Rollback,
                "failed to serialize conflict-resolution rollback payload",
            )
        })
    }

    fn apply_and_validate(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<(), ConflictCenterError> {
        let target = local_target(item)?;
        if request.action == ConflictResolutionAction::Retry {
            self.coordinator.rescan_target(target);
            return Ok(());
        }
        let record_id = item
            .record_id
            .as_deref()
            .ok_or_else(|| invalid_input("local conflict item has no record id"))?;
        let local = self.local_value(target, record_id)?;
        let external = self.external_value(target, record_id)?;
        let confirmed_digest = match request.action {
            ConflictResolutionAction::AcceptExternal => {
                self.accept_external(target, record_id, external)?;
                self.validate_expected(target, record_id, item.external_digest.as_deref(), false)?;
                item.external_digest.as_deref()
            }
            ConflictResolutionAction::KeepLocal => {
                self.keep_local(target, record_id, local, external)?;
                self.validate_expected(target, record_id, item.local_digest.as_deref(), true)?;
                self.coordinator.rescan_target(target);
                item.local_digest.as_deref()
            }
            ConflictResolutionAction::KeepBoth | ConflictResolutionAction::Retry => {
                return Err(ConflictCenterError::new(
                    ConflictCenterErrorCode::UnsupportedAction,
                    "resolution action is not implemented for local conflicts",
                ))
            }
        };
        self.baselines
            .confirm_record(target, record_id, confirmed_digest)
            .map_err(|_| validation_failed())?;
        self.clear_pending_when_no_record_work_remains(target)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalResolutionRollbackPayload<'a> {
    schema_version: u32,
    item: &'a ConflictCenterItem,
    action: ConflictResolutionAction,
    local: Option<Value>,
    external: Option<Value>,
    skill_tree: Option<SerializableSkillTree>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializableSkillTree {
    files: Vec<SerializableSkillFile>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SerializableSkillFile {
    relative_path: String,
    contents: Vec<u8>,
}

fn capture_skill_tree(
    client: ManagedClientId,
    directory: &str,
) -> Result<SerializableSkillTree, ConflictCenterError> {
    let snapshot = LocalSkillTreeAdapter::runtime()
        .capture(client, directory)
        .map_err(|_| read_error("capture Skill rollback tree"))?;
    Ok(SerializableSkillTree {
        files: snapshot
            .tree
            .map(|tree| {
                tree.files
                    .into_iter()
                    .map(|file| SerializableSkillFile {
                        relative_path: file.relative_path,
                        contents: file.contents,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn local_target(item: &ConflictCenterItem) -> Result<LocalScanTarget, ConflictCenterError> {
    if item.source != ConflictCenterSource::LocalScan {
        return Err(invalid_input("item is not a local-scan conflict"));
    }
    let domain = match item.domain {
        PortableDomain::Provider => LocalScanDomain::Provider,
        PortableDomain::Mcp => LocalScanDomain::Mcp,
        PortableDomain::Prompt => LocalScanDomain::Prompt,
        PortableDomain::Skill => LocalScanDomain::Skill,
        _ => return Err(invalid_input("item domain is not locally managed")),
    };
    Ok(LocalScanTarget {
        domain,
        client_id: item
            .client_id
            .ok_or_else(|| invalid_input("local item has no managed client"))?,
    })
}

fn empty_provider(id: &str) -> Provider {
    Provider {
        id: id.to_string(),
        name: id.to_string(),
        settings_config: json!({}),
        website_url: None,
        category: Some("custom".to_string()),
        created_at: Some(chrono::Utc::now().timestamp_millis()),
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            live_config_managed: Some(true),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
    }
}

fn live_provider_record(
    client: ManagedClientId,
    provider: &Provider,
    settings: Value,
) -> LiveProviderRecord {
    LiveProviderRecord {
        client_id: client,
        provider_id: provider.id.clone(),
        category: provider.category.clone(),
        settings,
    }
}

fn restore_opencode_external(
    adapter: &dyn LiveProviderConfigPort,
    record_id: &str,
    external: Option<Value>,
    provider: &Provider,
) {
    match external {
        Some(settings) => {
            let _ = adapter.write(&live_provider_record(
                ManagedClientId::Opencode,
                provider,
                settings,
            ));
        }
        None => {
            let _ = adapter.remove(record_id);
        }
    }
}

fn invalid_input(message: &str) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::InvalidInput, message)
}

fn read_error(stage: &str) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::Read, format!("failed to {stage}"))
}

fn apply_failed() -> ConflictCenterError {
    ConflictCenterError::new(
        ConflictCenterErrorCode::Apply,
        "failed to apply conflict resolution",
    )
}

fn apply_error(error: impl std::fmt::Display) -> ConflictCenterError {
    ConflictCenterError::new(ConflictCenterErrorCode::Apply, error.to_string())
}

fn validation_failed() -> ConflictCenterError {
    ConflictCenterError::new(
        ConflictCenterErrorCode::Validation,
        "conflict resolution did not produce the confirmed digest",
    )
}
