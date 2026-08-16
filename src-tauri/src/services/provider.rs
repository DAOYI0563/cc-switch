use std::sync::{Mutex, MutexGuard, OnceLock};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, TableLike};

use crate::app_config::LegacyAppType;
use crate::domain::{LocalScanDomain, LocalScanTarget, ManagedClientId};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};
use crate::store::AppState;

pub struct ProviderService;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SwitchResult {
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderSortUpdate {
    pub id: String,
    #[serde(rename = "sortIndex")]
    pub sort_index: usize,
}

pub(crate) fn operation_lock() -> Result<MutexGuard<'static, ()>, AppError> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|error| AppError::Message(format!("供应商操作锁已损坏: {error}")))
}

fn validate_provider(client: ManagedClientId, provider: &Provider) -> Result<(), AppError> {
    if provider.id.trim().is_empty()
        || !provider.id.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(AppError::InvalidInput(
            "供应商 ID 只能包含字母、数字、点、横线和下划线".to_string(),
        ));
    }
    if provider.name.trim().is_empty() {
        return Err(AppError::InvalidInput("供应商名称不能为空".to_string()));
    }
    if !provider.settings_config.is_object() {
        return Err(AppError::InvalidInput("供应商配置必须是对象".to_string()));
    }
    match client {
        ManagedClientId::Claude => {
            if provider
                .settings_config
                .get("env")
                .is_some_and(|value| !value.is_object())
            {
                return Err(AppError::InvalidInput("Claude env 必须是对象".to_string()));
            }
        }
        ManagedClientId::Codex => {
            let config = provider
                .settings_config
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !config.trim().is_empty() {
                config.parse::<DocumentMut>().map_err(|error| {
                    AppError::InvalidInput(format!("Codex config.toml 无效: {error}"))
                })?;
            }
            if provider
                .settings_config
                .get("auth")
                .is_some_and(|value| !value.is_object())
            {
                return Err(AppError::InvalidInput("Codex auth 必须是对象".to_string()));
            }
        }
        ManagedClientId::Opencode => {
            if provider.category.as_deref() == Some("official") {
                return Err(AppError::InvalidInput(
                    "OpenCode 不创建伪官方供应商".to_string(),
                ));
            }
        }
    }
    Ok(())
}

fn canonical_provider(
    client: ManagedClientId,
    mut provider: Provider,
) -> Result<Provider, AppError> {
    provider.id = provider.id.trim().to_string();
    provider.name = provider.name.trim().to_string();
    provider.category = Some(
        if client == ManagedClientId::Opencode {
            "custom"
        } else if provider.category.as_deref() == Some("official") {
            "official"
        } else {
            "custom"
        }
        .to_string(),
    );
    validate_provider(client, &provider)?;
    Ok(provider)
}

fn deep_merge_json(target: &mut Value, source: &Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                if let Some(existing) = target.get_mut(key) {
                    deep_merge_json(existing, value);
                } else {
                    target.insert(key.clone(), value.clone());
                }
            }
        }
        (target, source) => *target = source.clone(),
    }
}

fn merge_toml_item(target: &mut Item, source: &Item) {
    if let (Some(target_table), Some(source_table)) =
        (target.as_table_like_mut(), source.as_table_like())
    {
        merge_toml_table(target_table, source_table);
    } else {
        *target = source.clone();
    }
}

fn merge_toml_table(target: &mut dyn TableLike, source: &dyn TableLike) {
    for (key, source_item) in source.iter() {
        if let Some(target_item) = target.get_mut(key) {
            merge_toml_item(target_item, source_item);
        } else {
            target.insert(key, source_item.clone());
        }
    }
}

fn remove_toml_item(target: &mut Item, source: &Item) {
    if let (Some(target_table), Some(source_table)) =
        (target.as_table_like_mut(), source.as_table_like())
    {
        remove_toml_table(target_table, source_table);
        if target_table.is_empty() {
            *target = Item::None;
        }
    } else if target.as_value().is_some()
        && source.as_value().is_some()
        && target.to_string() == source.to_string()
    {
        *target = Item::None;
    }
}

fn remove_toml_table(target: &mut dyn TableLike, source: &dyn TableLike) {
    let keys = source
        .iter()
        .map(|(key, _)| key.to_string())
        .collect::<Vec<_>>();
    for key in keys {
        let mut empty = false;
        if let (Some(target_item), Some(source_item)) = (target.get_mut(&key), source.get(&key)) {
            remove_toml_item(target_item, source_item);
            empty = target_item.is_none()
                || target_item
                    .as_table_like()
                    .is_some_and(|table| table.is_empty());
        }
        if empty {
            target.remove(&key);
        }
    }
}

pub fn update_toml_common_config_snippet(
    config_toml: &str,
    snippet_toml: &str,
    enabled: bool,
) -> Result<String, AppError> {
    if enabled {
        crate::domain::validate_common_snippet(ManagedClientId::Codex, snippet_toml)
            .map_err(AppError::InvalidInput)?;
    }
    let snippet = crate::domain::sanitize_common_snippet(ManagedClientId::Codex, snippet_toml)
        .map_err(AppError::InvalidInput)?
        .text;
    if snippet.trim().is_empty() {
        return Ok(config_toml.to_string());
    }
    let mut target = if config_toml.trim().is_empty() {
        DocumentMut::new()
    } else {
        config_toml
            .parse::<DocumentMut>()
            .map_err(|error| AppError::InvalidInput(format!("Codex config.toml 无效: {error}")))?
    };
    let source = snippet
        .parse::<DocumentMut>()
        .map_err(|error| AppError::InvalidInput(format!("Codex 通用配置无效: {error}")))?;
    if enabled {
        merge_toml_table(target.as_table_mut(), source.as_table());
    } else {
        remove_toml_table(target.as_table_mut(), source.as_table());
    }
    Ok(target.to_string())
}

fn effective_settings(
    state: &AppState,
    client: ManagedClientId,
    provider: &Provider,
) -> Result<Value, AppError> {
    let mut settings = provider.settings_config.clone();
    if provider
        .meta
        .as_ref()
        .and_then(|meta| meta.common_config_enabled)
        != Some(true)
    {
        return Ok(settings);
    }
    let Some(snippet) = state.db.get_config_snippet(client.as_str())? else {
        return Ok(settings);
    };
    let sanitized = crate::domain::sanitize_common_snippet(client, &snippet)
        .map_err(AppError::InvalidInput)?
        .text;
    if sanitized.trim().is_empty() {
        return Ok(settings);
    }
    match client {
        ManagedClientId::Claude => {
            let value = serde_json::from_str::<Value>(&sanitized)
                .map_err(|error| AppError::InvalidInput(format!("Claude 通用配置无效: {error}")))?;
            deep_merge_json(&mut settings, &value);
        }
        ManagedClientId::Codex => {
            let config = settings
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let merged = update_toml_common_config_snippet(config, &sanitized, true)?;
            settings
                .as_object_mut()
                .expect("validated provider settings")
                .insert("config".to_string(), Value::String(merged));
        }
        ManagedClientId::Opencode => {}
    }
    Ok(settings)
}

fn read_live(client: ManagedClientId) -> Result<Value, AppError> {
    match client {
        ManagedClientId::Claude => {
            let path = crate::config::get_claude_settings_path();
            if path.exists() {
                crate::config::read_json_file(&path)
            } else {
                Ok(Value::Object(Map::new()))
            }
        }
        ManagedClientId::Codex => crate::codex_config::read_codex_live_settings(),
        ManagedClientId::Opencode => Ok(Value::Object(Map::new())),
    }
}

fn write_live(
    state: &AppState,
    client: ManagedClientId,
    provider: &Provider,
) -> Result<(), AppError> {
    let settings = effective_settings(state, client, provider)?;
    match client {
        ManagedClientId::Claude => {
            crate::config::write_json_file(&crate::config::get_claude_settings_path(), &settings)?;
        }
        ManagedClientId::Codex => {
            let current = crate::codex_config::read_codex_live_settings()
                .unwrap_or_else(|_| serde_json::json!({ "auth": {}, "config": "" }));
            let configured_auth = settings
                .get("auth")
                .filter(|value| value.as_object().is_some_and(|object| !object.is_empty()));
            let auth = configured_auth
                .or_else(|| current.get("auth"))
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
            let config = settings
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            crate::codex_config::write_codex_live_atomic(&auth, Some(config))?;
        }
        ManagedClientId::Opencode => {
            crate::opencode_config::set_provider(&provider.id, settings)?;
        }
    }
    if let Err(error) = crate::services::McpService::sync_enabled_for_app(state, client) {
        log::warn!("供应商写入后重投影 {client} MCP 失败: {error}");
    }
    crate::services::record_runtime_local_writes(
        &state.local_scan_writes,
        [LocalScanTarget {
            domain: LocalScanDomain::Provider,
            client_id: client,
        }],
    );
    Ok(())
}

fn restore_live(client: ManagedClientId, settings: &Value) -> Result<(), AppError> {
    match client {
        ManagedClientId::Claude => {
            crate::config::write_json_file(&crate::config::get_claude_settings_path(), settings)
        }
        ManagedClientId::Codex => {
            let auth = settings
                .get("auth")
                .cloned()
                .unwrap_or_else(|| Value::Object(Map::new()));
            let config = settings
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            crate::codex_config::write_codex_live_atomic(&auth, Some(config))
        }
        ManagedClientId::Opencode => Ok(()),
    }
}

fn mark_live(provider: &mut Provider, managed: bool) {
    provider
        .meta
        .get_or_insert_with(ProviderMeta::default)
        .live_config_managed = Some(managed);
}

impl ProviderService {
    pub fn list_managed(
        state: &AppState,
        client: ManagedClientId,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let mut providers = state.db.get_all_providers(client.as_str())?;
        providers.retain(|_, provider| {
            canonical_provider(client, provider.clone()).is_ok()
                && !(client == ManagedClientId::Opencode
                    && provider.category.as_deref() == Some("official"))
        });
        Ok(providers)
    }

    pub fn current_managed(state: &AppState, client: ManagedClientId) -> Result<String, AppError> {
        if client == ManagedClientId::Opencode {
            return Ok(String::new());
        }
        crate::settings::get_effective_current_provider(
            state.db.as_ref(),
            &LegacyAppType::from(client),
        )
        .map(|value| value.unwrap_or_default())
    }

    pub fn add_managed(
        state: &AppState,
        client: ManagedClientId,
        provider: Provider,
        add_to_live: bool,
    ) -> Result<bool, AppError> {
        let _guard = operation_lock()?;
        let mut provider = canonical_provider(client, provider)?;
        if state
            .db
            .get_provider_by_id(&provider.id, client.as_str())?
            .is_some()
        {
            return Err(AppError::InvalidInput(format!(
                "供应商 {} 已存在",
                provider.id
            )));
        }
        if client == ManagedClientId::Opencode {
            mark_live(&mut provider, add_to_live);
            if add_to_live {
                write_live(state, client, &provider)?;
            }
            state.db.save_provider(client.as_str(), &provider)?;
            return Ok(true);
        }

        let current = Self::current_managed(state, client)?;
        state.db.save_provider(client.as_str(), &provider)?;
        if current.is_empty() {
            let old_live = read_live(client)?;
            if let Err(error) = write_live(state, client, &provider) {
                let _ = state.db.delete_provider(client.as_str(), &provider.id);
                return Err(error);
            }
            if let Err(error) = state.db.set_current_provider(client.as_str(), &provider.id) {
                let _ = restore_live(client, &old_live);
                let _ = state.db.delete_provider(client.as_str(), &provider.id);
                return Err(error);
            }
        }
        Ok(true)
    }

    pub fn update_managed(
        state: &AppState,
        client: ManagedClientId,
        original_id: Option<&str>,
        provider: Provider,
    ) -> Result<bool, AppError> {
        let _guard = operation_lock()?;
        let mut provider = canonical_provider(client, provider)?;
        let original_id = original_id.unwrap_or(&provider.id).to_string();
        let original = state
            .db
            .get_provider_by_id(&original_id, client.as_str())?
            .ok_or_else(|| AppError::InvalidInput(format!("供应商 {original_id} 不存在")))?;
        if original_id != provider.id {
            if client != ManagedClientId::Opencode
                || original
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.live_config_managed)
                    == Some(true)
            {
                return Err(AppError::InvalidInput(
                    "写入 live 后不能修改供应商 ID".to_string(),
                ));
            }
            if state
                .db
                .get_provider_by_id(&provider.id, client.as_str())?
                .is_some()
            {
                return Err(AppError::InvalidInput(format!(
                    "供应商 {} 已存在",
                    provider.id
                )));
            }
            mark_live(&mut provider, false);
            state.db.save_provider(client.as_str(), &provider)?;
            state.db.delete_provider(client.as_str(), &original_id)?;
            return Ok(true);
        }

        let is_live = if client == ManagedClientId::Opencode {
            original
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed)
                == Some(true)
        } else {
            Self::current_managed(state, client)? == provider.id
        };
        if client == ManagedClientId::Opencode {
            mark_live(&mut provider, is_live);
        }
        state.db.save_provider(client.as_str(), &provider)?;
        if is_live {
            if let Err(error) = write_live(state, client, &provider) {
                let _ = state.db.save_provider(client.as_str(), &original);
                return Err(error);
            }
        }
        Ok(true)
    }

    pub fn delete_managed(
        state: &AppState,
        client: ManagedClientId,
        provider_id: &str,
    ) -> Result<(), AppError> {
        let _guard = operation_lock()?;
        let Some(provider) = state.db.get_provider_by_id(provider_id, client.as_str())? else {
            return Ok(());
        };
        if client != ManagedClientId::Opencode {
            if Self::current_managed(state, client)? == provider_id {
                return Err(AppError::InvalidInput(
                    "无法删除当前正在使用的供应商".to_string(),
                ));
            }
        } else if provider
            .meta
            .as_ref()
            .and_then(|meta| meta.live_config_managed)
            == Some(true)
        {
            crate::opencode_config::remove_provider(provider_id)?;
        }
        state.db.delete_provider(client.as_str(), provider_id)
    }

    pub fn remove_managed_from_live(
        state: &AppState,
        client: ManagedClientId,
        provider_id: &str,
    ) -> Result<(), AppError> {
        let _guard = operation_lock()?;
        if client != ManagedClientId::Opencode {
            return Err(AppError::InvalidInput(
                "只有 OpenCode 支持从 live 配置移除供应商".to_string(),
            ));
        }
        let mut provider = state
            .db
            .get_provider_by_id(provider_id, client.as_str())?
            .ok_or_else(|| AppError::InvalidInput(format!("供应商 {provider_id} 不存在")))?;
        crate::opencode_config::remove_provider(provider_id)?;
        mark_live(&mut provider, false);
        state.db.save_provider(client.as_str(), &provider)
    }

    pub fn switch_managed(
        state: &AppState,
        client: ManagedClientId,
        provider_id: &str,
    ) -> Result<SwitchResult, AppError> {
        let _guard = operation_lock()?;
        let mut target = state
            .db
            .get_provider_by_id(provider_id, client.as_str())?
            .ok_or_else(|| AppError::InvalidInput(format!("供应商 {provider_id} 不存在")))?;
        target = canonical_provider(client, target)?;
        if client == ManagedClientId::Opencode {
            write_live(state, client, &target)?;
            mark_live(&mut target, true);
            state.db.save_provider(client.as_str(), &target)?;
            return Ok(SwitchResult::default());
        }

        let old_live = read_live(client)?;
        let old_current = Self::current_managed(state, client)?;
        if !old_current.is_empty() && old_current != provider_id {
            if let Some(mut outgoing) =
                state.db.get_provider_by_id(&old_current, client.as_str())?
            {
                outgoing.settings_config = old_live.clone();
                state.db.save_provider(client.as_str(), &outgoing)?;
            }
        }
        write_live(state, client, &target)?;
        if let Err(error) = state.db.set_current_provider(client.as_str(), provider_id) {
            let _ = restore_live(client, &old_live);
            if !old_current.is_empty() {
                let _ = state.db.set_current_provider(client.as_str(), &old_current);
            }
            return Err(error);
        }
        Ok(SwitchResult::default())
    }

    pub fn sync_current_provider_for_app(
        state: &AppState,
        app_type: LegacyAppType,
    ) -> Result<(), AppError> {
        let client = ManagedClientId::try_from(&app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        if client == ManagedClientId::Opencode {
            for provider in Self::list_managed(state, client)?
                .values()
                .filter(|provider| {
                    provider
                        .meta
                        .as_ref()
                        .and_then(|meta| meta.live_config_managed)
                        == Some(true)
                })
            {
                write_live(state, client, provider)?;
            }
            return Ok(());
        }
        let current = Self::current_managed(state, client)?;
        if current.is_empty() {
            return Ok(());
        }
        let provider = state
            .db
            .get_provider_by_id(&current, client.as_str())?
            .ok_or_else(|| AppError::InvalidInput(format!("供应商 {current} 不存在")))?;
        write_live(state, client, &provider)
    }

    pub fn import_default_config(
        state: &AppState,
        app_type: LegacyAppType,
    ) -> Result<bool, AppError> {
        let client = ManagedClientId::try_from(&app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        if client == ManagedClientId::Opencode {
            return import_opencode_providers_from_live(state).map(|count| count > 0);
        }
        let settings = read_live(client)?;
        let meaningful = match client {
            ManagedClientId::Claude => settings
                .as_object()
                .is_some_and(|object| !object.is_empty()),
            ManagedClientId::Codex => {
                settings
                    .get("auth")
                    .and_then(Value::as_object)
                    .is_some_and(|object| !object.is_empty())
                    || settings
                        .get("config")
                        .and_then(Value::as_str)
                        .is_some_and(|value| !value.trim().is_empty())
            }
            ManagedClientId::Opencode => false,
        };
        if !meaningful {
            return Ok(false);
        }
        let provider = Provider {
            id: "default".to_string(),
            name: "导入的本地配置".to_string(),
            settings_config: settings,
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: Some(0),
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
        };
        state.db.save_provider(client.as_str(), &provider)?;
        state
            .db
            .set_current_provider(client.as_str(), &provider.id)?;
        Ok(true)
    }

    pub fn should_import_default_config_on_startup(
        state: &AppState,
        app_type: &LegacyAppType,
    ) -> Result<bool, AppError> {
        let client = ManagedClientId::try_from(app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        Ok(client != ManagedClientId::Opencode
            && state.db.get_all_providers(client.as_str())?.is_empty())
    }

    pub fn update_sort_order(
        state: &AppState,
        app_type: LegacyAppType,
        updates: Vec<ProviderSortUpdate>,
    ) -> Result<bool, AppError> {
        let client = ManagedClientId::try_from(&app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        for update in updates {
            state
                .db
                .update_provider_sort_index(client.as_str(), &update.id, update.sort_index)?;
        }
        Ok(true)
    }

    pub fn extract_common_config_snippet(
        _state: &AppState,
        app_type: LegacyAppType,
    ) -> Result<String, AppError> {
        let client = ManagedClientId::try_from(&app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        Self::extract_common_config_snippet_from_settings(app_type, &read_live(client)?)
    }

    pub fn extract_common_config_snippet_from_settings(
        app_type: LegacyAppType,
        settings: &Value,
    ) -> Result<String, AppError> {
        let client = ManagedClientId::try_from(&app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        crate::domain::extract_common_snippet(client, settings).map_err(AppError::InvalidInput)
    }
}

pub fn import_opencode_providers_from_live(state: &AppState) -> Result<usize, AppError> {
    let live = crate::opencode_config::get_providers()?;
    let mut changed = 0;
    for (id, settings_config) in live {
        let existing = state.db.get_provider_by_id(&id, "opencode")?;
        let mut provider = existing.unwrap_or_else(|| Provider {
            id: id.clone(),
            name: id.clone(),
            settings_config: Value::Null,
            website_url: None,
            category: Some("custom".to_string()),
            created_at: Some(chrono::Utc::now().timestamp_millis()),
            sort_index: None,
            notes: None,
            meta: None,
            icon: None,
            icon_color: None,
        });
        if provider.settings_config != settings_config
            || provider
                .meta
                .as_ref()
                .and_then(|meta| meta.live_config_managed)
                != Some(true)
        {
            provider.settings_config = settings_config;
            provider.category = Some("custom".to_string());
            mark_live(&mut provider, true);
            state.db.save_provider("opencode", &provider)?;
            changed += 1;
        }
    }
    Ok(changed)
}

pub fn opencode_live_provider_ids() -> Result<Vec<String>, AppError> {
    let mut ids = crate::opencode_config::get_providers()?
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    ids.sort();
    Ok(ids)
}
