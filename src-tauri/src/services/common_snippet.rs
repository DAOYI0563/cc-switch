use indexmap::IndexMap;

use crate::app_config::LegacyAppType;
use crate::domain::ManagedClientId;
use crate::error::AppError;
use crate::provider::Provider;
use crate::store::AppState;

use super::provider::{operation_lock, ProviderService};

/// Claude/Codex 通用配置片段的应用用例边界。
pub struct CommonSnippetService;

impl CommonSnippetService {
    pub fn get(state: &AppState, client_id: ManagedClientId) -> Result<Option<String>, AppError> {
        let app_type = supported_app_type(client_id)?;
        let Some(snippet) = state.db.get_config_snippet(app_type.as_str())? else {
            return Ok(None);
        };
        crate::domain::sanitize_common_snippet(client_id, &snippet)
            .map(|sanitized| Some(sanitized.text))
            .map_err(AppError::Message)
    }

    pub fn set(
        state: &AppState,
        client_id: ManagedClientId,
        snippet: String,
    ) -> Result<(), AppError> {
        crate::domain::validate_common_snippet(client_id, &snippet)
            .map_err(AppError::InvalidInput)?;
        let app_type = supported_app_type(client_id)?;
        let _guard = operation_lock()?;
        let app_name = app_type.as_str();
        let old_snippet = state.db.get_config_snippet(app_name)?;
        let old_cleared = state.db.is_config_snippet_cleared(app_name)?;
        let providers_before = state.db.get_all_providers(app_name)?;

        let is_cleared = snippet.trim().is_empty();
        let new_snippet = (!is_cleared).then_some(snippet.as_str());
        if let Err(primary) = state
            .db
            .set_config_snippet_state(app_name, new_snippet, is_cleared)
        {
            return Err(with_provider_rollback(
                state,
                &app_type,
                &providers_before,
                primary,
            ));
        }

        if let Err(primary) =
            ProviderService::sync_current_provider_for_app(state, app_type.clone())
        {
            let mut failures = Vec::new();
            if let Err(error) =
                state
                    .db
                    .set_config_snippet_state(app_name, old_snippet.as_deref(), old_cleared)
            {
                failures.push(format!("通用配置片段数据库状态: {error}"));
            }
            restore_providers(state, &app_type, &providers_before, &mut failures);

            // 运行时适配器承诺失败零写或内部回滚；这里再按旧数据库状态重投影，
            // 同时覆盖未来替换适配器后可能出现的部分写入。
            if failures.is_empty() {
                if let Err(error) =
                    ProviderService::sync_current_provider_for_app(state, app_type.clone())
                {
                    failures.push(format!("live 配置: {error}"));
                }
            }
            return Err(rollback_error(primary, failures));
        }

        Ok(())
    }
}

fn supported_app_type(client_id: ManagedClientId) -> Result<LegacyAppType, AppError> {
    match client_id {
        ManagedClientId::Claude | ManagedClientId::Codex => Ok(LegacyAppType::from(client_id)),
        ManagedClientId::Opencode => Err(AppError::InvalidInput(
            "通用配置片段仅支持 Claude 和 Codex".to_string(),
        )),
    }
}

fn with_provider_rollback(
    state: &AppState,
    app_type: &LegacyAppType,
    providers_before: &IndexMap<String, Provider>,
    primary: AppError,
) -> AppError {
    let mut failures = Vec::new();
    restore_providers(state, app_type, providers_before, &mut failures);
    rollback_error(primary, failures)
}

fn restore_providers(
    state: &AppState,
    app_type: &LegacyAppType,
    providers_before: &IndexMap<String, Provider>,
    failures: &mut Vec<String>,
) {
    for provider in providers_before.values() {
        if let Err(error) = state.db.save_provider(app_type.as_str(), provider) {
            failures.push(format!("供应商 {}: {error}", provider.id));
        }
    }
}

fn rollback_error(primary: AppError, failures: Vec<String>) -> AppError {
    if failures.is_empty() {
        primary
    } else {
        AppError::Message(format!(
            "{primary}; additionally failed to roll back: {}",
            failures.join("; ")
        ))
    }
}
