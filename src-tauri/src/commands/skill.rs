//! Local-only Skill command boundary.

use crate::commands::parse_managed_client_id;
use crate::domain::{LocalSkill, LocalSkillImport, UnmanagedLocalSkill};
use crate::services::LocalSkillService;
use crate::store::AppState;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub fn get_installed_skills(app_state: State<'_, AppState>) -> Result<Vec<LocalSkill>, String> {
    LocalSkillService::get_all(&app_state).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn uninstall_skill_unified(
    id: String,
    app_state: State<'_, AppState>,
) -> Result<bool, String> {
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::remove_managed(&app_state, &id).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Skill 删除任务执行失败: {error}"))?
}

#[tauri::command]
pub async fn toggle_skill_app(
    id: String,
    app: String,
    source_app: String,
    enabled: bool,
    app_state: State<'_, AppState>,
) -> Result<LocalSkill, String> {
    let app = parse_managed_client_id(&app)?;
    let source_app = parse_managed_client_id(&source_app)?;
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::toggle_app(&app_state, &id, source_app, app, enabled)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Skill 客户端切换任务执行失败: {error}"))?
}

#[tauri::command]
pub async fn sync_skill_from_live(
    id: String,
    source_app: String,
    app_state: State<'_, AppState>,
) -> Result<LocalSkill, String> {
    let source_app = parse_managed_client_id(&source_app)?;
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::sync_from_live(&app_state, &id, source_app)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Skill 同步任务执行失败: {error}"))?
}

#[tauri::command]
pub async fn read_skill_document(
    id: String,
    app_state: State<'_, AppState>,
) -> Result<crate::services::local_skill::SkillDocumentRead, String> {
    let app_state = app_state.inner().clone();
    let read = tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::read_skill_markdown(&app_state, &id).map_err(|error| error.to_string())
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), read)
        .await
        .map_err(|_| "WSL 文件通道无响应：读取 SKILL.md 已超时".to_string())?
        .map_err(|error| format!("读取 SKILL.md 失败: {error}"))?
}

#[tauri::command]
pub async fn open_skill_directory(
    id: String,
    app: String,
    handle: tauri::AppHandle,
    app_state: State<'_, AppState>,
) -> Result<bool, String> {
    let client = parse_managed_client_id(&app)?;
    let app_state = app_state.inner().clone();
    let path = tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::skill_directory_windows_path(&app_state, &id, client)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("解析 Skill 目录失败: {error}"))??;
    handle
        .opener()
        .open_path(path, None::<String>)
        .map_err(|error| format!("打开目录失败: {error}"))?;
    Ok(true)
}

#[tauri::command]
pub async fn scan_unmanaged_skills(
    app_state: State<'_, AppState>,
) -> Result<Vec<UnmanagedLocalSkill>, String> {
    let app_state = app_state.inner().clone();
    let scan = tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::scan_unmanaged(&app_state).map_err(|error| error.to_string())
    });
    tokio::time::timeout(std::time::Duration::from_secs(60), scan)
        .await
        .map_err(|_| "WSL 文件通道无响应：扫描已超时，请检查 WSL 状态后重试".to_string())?
        .map_err(|error| format!("Skill 扫描任务执行失败: {error}"))?
}

#[tauri::command]
pub async fn import_skills_from_apps(
    imports: Vec<LocalSkillImport>,
    app_state: State<'_, AppState>,
) -> Result<Vec<LocalSkill>, String> {
    let app_state = app_state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        LocalSkillService::import_from_live(&app_state, imports).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Skill 导入任务执行失败: {error}"))?
}
