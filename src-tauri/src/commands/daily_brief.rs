use chrono::NaiveDate;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::database::DailyBriefRecord;
use crate::services::daily_brief::{
    self, DailyBriefRuntimeState, DailyBriefSettingsView, SaveDailyBriefSettingsRequest,
};

fn parse_date(date: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map_err(|_| "日期必须使用 YYYY-MM-DD 格式".to_string())
}

#[tauri::command]
pub async fn get_daily_brief_settings(
    state: State<'_, DailyBriefRuntimeState>,
) -> Result<DailyBriefSettingsView, String> {
    daily_brief::get_settings_view(state.db().as_ref())
}

#[tauri::command]
pub async fn save_daily_brief_settings_command(
    state: State<'_, DailyBriefRuntimeState>,
    request: SaveDailyBriefSettingsRequest,
) -> Result<DailyBriefSettingsView, String> {
    daily_brief::save_settings(state.db().as_ref(), request)
}

#[tauri::command]
pub async fn test_daily_brief_connection(
    state: State<'_, DailyBriefRuntimeState>,
) -> Result<DailyBriefSettingsView, String> {
    daily_brief::test_connection(state.db().as_ref()).await
}

#[tauri::command]
pub async fn list_daily_briefs(
    state: State<'_, DailyBriefRuntimeState>,
    query: Option<String>,
) -> Result<Vec<DailyBriefRecord>, String> {
    daily_brief::search_records(state.db().as_ref(), query.as_deref().unwrap_or_default())
}

#[tauri::command]
pub async fn generate_daily_brief(
    state: State<'_, DailyBriefRuntimeState>,
    date: String,
    regenerate: Option<bool>,
) -> Result<DailyBriefRecord, String> {
    state
        .generate(parse_date(&date)?, regenerate.unwrap_or(false))
        .await
}

#[tauri::command]
pub async fn delete_daily_brief(
    state: State<'_, DailyBriefRuntimeState>,
    date: String,
    device_id: String,
) -> Result<(), String> {
    parse_date(&date)?;
    daily_brief::delete_record(state.db().as_ref(), &date, &device_id)
}

#[tauri::command]
pub async fn open_daily_brief(
    app: AppHandle,
    state: State<'_, DailyBriefRuntimeState>,
    date: String,
    device_id: String,
) -> Result<(), String> {
    parse_date(&date)?;
    let materialized = daily_brief::validated_record_path(state.db().as_ref(), &date, &device_id)?;
    app.opener()
        .open_path(
            materialized.path.to_string_lossy().to_string(),
            None::<String>,
        )
        .map_err(|_| "无法打开每日简报".to_string())?;
    if materialized.transient {
        let path = materialized.path;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(10 * 60)).await;
            let _ = std::fs::remove_file(path);
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn open_daily_brief_directory(app: AppHandle) -> Result<(), String> {
    let directory = daily_brief::brief_directory();
    std::fs::create_dir_all(&directory).map_err(|_| "无法创建每日简报目录".to_string())?;
    app.opener()
        .open_path(directory.to_string_lossy().to_string(), None::<String>)
        .map_err(|_| "无法打开每日简报目录".to_string())
}
