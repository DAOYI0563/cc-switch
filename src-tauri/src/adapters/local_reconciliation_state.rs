use std::sync::Arc;

use serde_json::{json, Value};

use crate::app_config::LegacyAppType;
use crate::database::Database;
use crate::domain::{LocalScanDomain, LocalScanFailureKind, LocalScanTarget, ManagedClientId};
use crate::ports::{
    LocalReconciliationBaselinePort, LocalReconciliationState, LocalReconciliationStatePort,
    LocalScanParsedRecord, LocalScanParsedSnapshot, LocalScanReadFailure,
};
use crate::services::reconciliation_snapshot_from_parsed;

pub struct DatabaseLocalReconciliationStateAdapter {
    database: Arc<Database>,
    baselines: Arc<dyn LocalReconciliationBaselinePort>,
}

impl DatabaseLocalReconciliationStateAdapter {
    pub fn new(
        database: Arc<Database>,
        baselines: Arc<dyn LocalReconciliationBaselinePort>,
    ) -> Self {
        Self {
            database,
            baselines,
        }
    }

    pub fn read_parsed_local(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalScanParsedSnapshot, LocalScanReadFailure> {
        let records = match target.domain {
            LocalScanDomain::Provider => self.provider_records(target.client_id)?,
            LocalScanDomain::Mcp => self.mcp_records(target.client_id)?,
            LocalScanDomain::Prompt => self.prompt_records(target.client_id)?,
            LocalScanDomain::Skill => self.skill_records(target.client_id)?,
        };
        LocalScanParsedSnapshot::new(target, records).map_err(|_| read_failure(None))
    }

    fn provider_records(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalScanParsedRecord>, LocalScanReadFailure> {
        let app_type = LegacyAppType::from(client);
        let providers = self
            .database
            .get_all_providers(app_type.as_str())
            .map_err(|_| read_failure(None))?;
        if client == ManagedClientId::Opencode {
            return providers
                .into_values()
                .filter(|provider| {
                    provider
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.live_config_managed)
                        == Some(true)
                })
                .map(|provider| {
                    let value = opencode_provider_fragment(&provider.id, &provider.settings_config)
                        .ok_or_else(|| read_failure(Some(&provider.id)))?;
                    LocalScanParsedRecord::new(provider.id, value).map_err(|_| read_failure(None))
                })
                .collect();
        }

        let current = self
            .database
            .get_current_provider(app_type.as_str())
            .map_err(|_| read_failure(None))?;
        let Some(current) = current else {
            return Ok(Vec::new());
        };
        let Some(provider) = providers.get(&current) else {
            return Err(read_failure(Some("live")));
        };
        let mut value = provider.settings_config.clone();
        if client == ManagedClientId::Claude {
            strip_claude_private_fields(&mut value);
        }
        LocalScanParsedRecord::new("live", value)
            .map(|record| vec![record])
            .map_err(|_| read_failure(Some("live")))
    }

    fn mcp_records(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalScanParsedRecord>, LocalScanReadFailure> {
        self.database
            .get_all_mcp_servers()
            .map_err(|_| read_failure(None))?
            .into_values()
            .filter(|server| server.apps.is_enabled_for(client))
            .map(|server| {
                let id = server.id.clone();
                LocalScanParsedRecord::new(id.clone(), server.server)
                    .map_err(|_| read_failure(Some(&id)))
            })
            .collect()
    }

    fn prompt_records(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalScanParsedRecord>, LocalScanReadFailure> {
        let active = self
            .database
            .get_prompt_versions(client)
            .map_err(|_| read_failure(None))?
            .into_values()
            .find(|version| version.enabled);
        active
            .map(|version| {
                LocalScanParsedRecord::new("prompt-live", json!({ "content": version.content }))
                    .map_err(|_| read_failure(Some("prompt-live")))
            })
            .transpose()
            .map(|record| record.into_iter().collect())
    }

    fn skill_records(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalScanParsedRecord>, LocalScanReadFailure> {
        self.database
            .list_core_skills()
            .map_err(|_| read_failure(None))?
            .into_iter()
            .filter(|skill| skill.apps.is_enabled_for(client))
            .map(|skill| {
                let directory = skill.directory.clone();
                LocalScanParsedRecord::new(
                    directory.clone(),
                    json!({
                        "name": skill.name,
                        "description": skill.description,
                        "contentHash": skill.content_hash,
                        "totalSizeBytes": skill.total_size_bytes,
                        "fileCount": skill.file_count,
                        "cloudEligible": skill.cloud_eligible,
                    }),
                )
                .map_err(|_| read_failure(Some(&directory)))
            })
            .collect()
    }
}

impl LocalReconciliationStatePort for DatabaseLocalReconciliationStateAdapter {
    fn read_reconciliation_state(
        &self,
        target: LocalScanTarget,
    ) -> Result<LocalReconciliationState, LocalScanReadFailure> {
        let local = reconciliation_snapshot_from_parsed(&self.read_parsed_local(target)?)
            .map_err(|_| read_failure(None))?;
        LocalReconciliationState::new(target, self.baselines.read_baseline(target), local)
            .map_err(|_| read_failure(None))
    }
}

fn opencode_provider_fragment(provider_id: &str, settings: &Value) -> Option<Value> {
    let object = settings.as_object()?;
    if object.contains_key("$schema") || object.contains_key("provider") {
        object.get("provider")?.get(provider_id).cloned()
    } else {
        Some(settings.clone())
    }
}

fn strip_claude_private_fields(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in [
        "api_format",
        "apiFormat",
        "openrouter_compat_mode",
        "openrouterCompatMode",
    ] {
        object.remove(key);
    }
}

fn read_failure(record_id: Option<&str>) -> LocalScanReadFailure {
    LocalScanReadFailure {
        kind: LocalScanFailureKind::ReadFailed,
        record_id: record_id.map(ToOwned::to_owned),
    }
}
