use indexmap::IndexMap;
use tauri::{Manager, State};

use crate::commands::{parse_managed_app_type, parse_managed_client_id};
use crate::provider::Provider;
use crate::services::{ProviderService, ProviderSortUpdate, SwitchResult};
use crate::store::AppState;

#[tauri::command]
pub fn get_providers(
    state: State<'_, AppState>,
    app: String,
) -> Result<IndexMap<String, Provider>, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::list_managed(state.inner(), client).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_current_provider(state: State<'_, AppState>, app: String) -> Result<String, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::current_managed(state.inner(), client).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn add_provider(
    state: State<'_, AppState>,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] addToLive: Option<bool>,
) -> Result<bool, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::add_managed(state.inner(), client, provider, addToLive.unwrap_or(true))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_provider(
    state: State<'_, AppState>,
    app: String,
    provider: Provider,
    #[allow(non_snake_case)] originalId: Option<String>,
) -> Result<bool, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::update_managed(state.inner(), client, originalId.as_deref(), provider)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_provider(
    state: State<'_, AppState>,
    app: String,
    id: String,
) -> Result<bool, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::delete_managed(state.inner(), client, &id)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_provider_from_live_config(
    state: State<'_, AppState>,
    app: String,
    id: String,
) -> Result<bool, String> {
    let client = parse_managed_client_id(&app)?;
    ProviderService::remove_managed_from_live(state.inner(), client, &id)
        .map(|_| true)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn switch_provider(
    app_handle: tauri::AppHandle,
    app: String,
    id: String,
) -> Result<SwitchResult, String> {
    let client = parse_managed_client_id(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;
        ProviderService::switch_managed(state.inner(), client, &id)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("供应商切换任务执行失败: {error}"))?
}

#[tauri::command]
pub fn import_default_config(state: State<'_, AppState>, app: String) -> Result<bool, String> {
    let app_type = parse_managed_app_type(&app)?;
    ProviderService::import_default_config(state.inner(), app_type)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_providers_sort_order(
    state: State<'_, AppState>,
    app: String,
    updates: Vec<ProviderSortUpdate>,
) -> Result<bool, String> {
    let app_type = parse_managed_app_type(&app)?;
    ProviderService::update_sort_order(state.inner(), app_type, updates)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn import_opencode_providers_from_live(state: State<'_, AppState>) -> Result<usize, String> {
    crate::services::provider::import_opencode_providers_from_live(state.inner())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_opencode_live_provider_ids() -> Result<Vec<String>, String> {
    crate::services::provider::opencode_live_provider_ids().map_err(|error| error.to_string())
}
