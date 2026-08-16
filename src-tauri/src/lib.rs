pub mod adapters;
mod app_config;
mod auto_launch;
mod claude_mcp;
mod codex_config;
mod codex_state_db;
mod commands;
mod config;
mod database;
pub mod domain;
mod error;
mod init_status;
mod mcp;
mod opencode_config;
mod panic_hook;
pub mod ports;
mod prompt;
mod provider;
mod services;
mod session_manager;
mod settings;
mod store;
mod tray;

use std::sync::Arc;

use tauri::tray::TrayIconBuilder;
use tauri::{Manager, RunEvent};

pub use adapters::RuntimeSyncLocalAdapter;
pub use app_config::{LegacyAppType, McpApps, McpServer, MultiAppConfig};
pub use codex_config::{
    get_codex_auth_path, get_codex_config_path, read_codex_live_settings, write_codex_live_atomic,
};
pub use commands::*;
pub use config::{get_claude_mcp_path, get_claude_settings_path, read_json_file};
pub use database::Database;
pub use error::AppError;
pub use mcp::{
    import_from_claude, import_from_codex, remove_server_from_claude, remove_server_from_codex,
    sync_enabled_to_claude, sync_enabled_to_codex, sync_single_server_to_claude,
    sync_single_server_to_codex,
};
pub use prompt::Prompt;
pub use provider::{Provider, ProviderMeta};
pub use services::{
    apply_committed_sync_batch, default_local_actions, list_conflict_center_items,
    local_reconciliation_items, reconciliation_snapshot_from_parsed, record_local_writes,
    record_runtime_local_writes, resolve_conflict_center_item, sync_manifest_remote_path,
    sync_record_remote_path, CommonSnippetService, ConflictCenterRuntimeState,
    InMemoryLocalReconciliationBaselines, LocalScanCadence, LocalScanConflictSource,
    LocalScanCoordinator, LocalScanExecutor, LocalScanParsedChange, LocalScanRuntimeState,
    LocalScanScheduler, LocalScanSchedulerError, LocalScanWorker, LocalScanWriteRegistration,
    LocalScanWriteTracker, LocalSkillService, McpService, PromptService, ProviderService,
    SyncDeviceRetireRequest, SyncFirstSyncConfirmRequest, SyncFirstSyncPreviewRequest,
    SyncRunError, SyncRunErrorCode, SyncRunRequest, SyncRunResult, SyncV3Orchestrator,
    WebDavConflictSource,
};
pub use settings::{update_settings, AppSettings};
pub use store::AppState;

#[cfg(target_os = "windows")]
fn set_windows_app_user_model_id(app: &tauri::AppHandle) {
    let app_id = app.config().identifier.clone();
    let wide_app_id = app_id
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(wide_app_id.as_ptr())
    };
    if result < 0 {
        log::warn!("设置 Windows AppUserModelID 失败: 0x{result:08X}");
    }
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    if let Some(scan) = app.try_state::<services::LocalScanRuntimeState>() {
        if let Err(error) = scan.window_restored() {
            log::warn!("恢复窗口时触发本地扫描失败: {error}");
        }
    }
}

fn initialize_database(app: &tauri::App) -> Result<Option<Arc<Database>>, AppError> {
    let database_path = config::get_app_config_dir().join("cc-switch.db");
    if let Some(version) = Database::stored_user_version_exceeds_supported(&database_path)? {
        init_status::set_init_error(init_status::InitErrorPayload {
            path: database_path.display().to_string(),
            error: format!(
                "数据库版本 {version} 高于当前支持版本 {}",
                database::SCHEMA_VERSION
            ),
            kind: Some("db_version_too_new".to_string()),
            db_version: Some(version),
            supported_version: Some(database::SCHEMA_VERSION),
        });
        show_main_window(app.handle());
        return Ok(None);
    }

    let database = match Database::init() {
        Ok(database) => Arc::new(database),
        Err(error) => {
            init_status::set_init_error(init_status::InitErrorPayload {
                path: database_path.display().to_string(),
                error: error.to_string(),
                kind: Some("database_init_failed".to_string()),
                db_version: None,
                supported_version: Some(database::SCHEMA_VERSION),
            });
            show_main_window(app.handle());
            return Ok(None);
        }
    };

    #[cfg(target_os = "windows")]
    {
        let now_ms = chrono::Utc::now().timestamp_millis();
        if let Some(report) =
            services::retained_migration::migrate_retained_data_runtime(&database, now_ms)?
        {
            log::info!(
                "保留数据迁移完成: source={:?}, records={}",
                report.source,
                report.retained.total()
            );
            init_status::set_migration_success();
            settings::reload_settings()?;
        }
    }

    Ok(Some(database))
}

fn import_retained_runtime(state: &AppState) {
    for client in domain::ManagedClientId::ALL {
        let app_type = LegacyAppType::from(client);
        if app_type.is_additive_mode() {
            continue;
        }
        match ProviderService::should_import_default_config_on_startup(state, &app_type) {
            Ok(true) => {
                if let Err(error) = ProviderService::import_default_config(state, app_type) {
                    log::debug!("未导入 {client} live 配置: {error}");
                }
            }
            Ok(false) => {}
            Err(error) => log::warn!("检查 {client} live 配置失败: {error}"),
        }
    }

    if let Err(error) = state.db.init_default_official_providers() {
        log::warn!("初始化官方供应商失败: {error}");
    }
    if let Err(error) = services::provider::import_opencode_providers_from_live(state) {
        log::warn!("导入 OpenCode live 配置失败: {error}");
    }

    if state.db.is_mcp_table_empty().unwrap_or(false) {
        for result in [
            McpService::import_from_claude(state),
            McpService::import_from_codex(state),
            McpService::import_from_opencode(state),
        ] {
            if let Err(error) = result {
                log::warn!("导入 MCP 配置失败: {error}");
            }
        }
    }

    if state.db.is_prompts_table_empty().unwrap_or(false) {
        for client in domain::ManagedClientId::ALL {
            if let Err(error) = PromptService::import_from_file_on_first_launch(state, client) {
                log::warn!("导入 {client} Prompt 失败: {error}");
            }
        }
    }
}

fn initialize_runtime_state(app: &mut tauri::App, state: AppState) -> Result<(), AppError> {
    import_retained_runtime(&state);

    let menu = tray::create_tray_menu(app.handle(), &state)?;
    let settings = settings::get_settings();
    let mut tray_builder = TrayIconBuilder::with_id(tray::TRAY_ID)
        .tooltip("WSL Code Switch")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| tray::handle_tray_menu_event(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        tray_builder = tray_builder.icon(icon.clone());
    }
    let tray_icon = tray_builder
        .build(app)
        .map_err(|error| AppError::Message(format!("创建托盘失败: {error}")))?;
    if !settings.show_in_tray {
        let _ = tray_icon.set_visible(false);
    }

    let local_scan_source =
        Arc::new(adapters::local_scan_summary::FixedLocalScanSummaryAdapter::runtime());
    let local_scan_parser =
        Arc::new(adapters::local_scan_parser::FixedLocalScanParserAdapter::runtime());
    let local_scan_coordinator = Arc::new(services::LocalScanCoordinator::new(
        local_scan_source,
        local_scan_parser,
        state.local_scan_writes.clone(),
    ));
    let local_reconciliation_baselines =
        Arc::new(services::InMemoryLocalReconciliationBaselines::default());
    let conflict_state = services::ConflictCenterRuntimeState::new(
        local_scan_coordinator.clone(),
        local_reconciliation_baselines,
    );
    let (local_scan_scheduler, local_scan_worker) = services::LocalScanScheduler::new(
        local_scan_coordinator,
        services::LocalScanCadence::production(),
        settings.silent_startup,
    );
    let daily_brief = services::DailyBriefRuntimeState::new(state.db.clone());

    app.manage(state);
    app.manage(conflict_state);
    app.manage(services::LocalScanRuntimeState::new(local_scan_scheduler));
    app.manage(daily_brief.clone());
    tauri::async_runtime::spawn(local_scan_worker.run());
    tauri::async_runtime::spawn(daily_brief.run_scheduler());

    if settings.silent_startup {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
            let _ = window.set_skip_taskbar(true);
        }
    } else {
        show_main_window(app.handle());
    }
    Ok(())
}

#[tauri::command]
fn update_tray_menu(app: tauri::AppHandle) -> Result<bool, String> {
    tray::refresh_tray_menu(&app);
    Ok(true)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    panic_hook::setup_panic_hook();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default();
    #[cfg(target_os = "windows")]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main_window(app);
        }));
    }

    let builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            panic_hook::init_app_config_dir(config::get_app_config_dir());
            let log_directory = panic_hook::get_log_dir();
            std::fs::create_dir_all(&log_directory)?;
            use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(log::LevelFilter::Info)
                    .targets([
                        Target::new(TargetKind::Stdout),
                        Target::new(TargetKind::Folder {
                            path: log_directory,
                            file_name: Some("wsl-code-switch".into()),
                        }),
                    ])
                    .rotation_strategy(RotationStrategy::KeepSome(3))
                    .max_file_size(10 * 1024 * 1024)
                    .timezone_strategy(TimezoneStrategy::UseLocal)
                    .build(),
            )?;
            log::info!("WSL Code Switch {} 启动", env!("CARGO_PKG_VERSION"));

            #[cfg(target_os = "windows")]
            set_windows_app_user_model_id(app.handle());

            if let Some(database) = initialize_database(app)? {
                initialize_runtime_state(app, AppState::new(database))?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_providers,
            commands::get_current_provider,
            commands::add_provider,
            commands::update_provider,
            commands::delete_provider,
            commands::remove_provider_from_live_config,
            commands::switch_provider,
            commands::import_default_config,
            commands::update_providers_sort_order,
            commands::import_opencode_providers_from_live,
            commands::get_opencode_live_provider_ids,
            commands::open_provider_terminal,
            commands::get_common_config_snippet,
            commands::set_common_config_snippet,
            commands::update_toml_common_config_snippet,
            commands::extract_common_config_snippet,
            commands::get_mcp_servers,
            commands::upsert_mcp_server,
            commands::delete_mcp_server,
            commands::toggle_mcp_app,
            commands::import_mcp_from_apps,
            commands::sync_mcp_to_apps,
            commands::validate_mcp_command,
            commands::get_prompts,
            commands::upsert_prompt,
            commands::delete_prompt,
            commands::enable_prompt,
            commands::import_prompt_from_file,
            commands::get_current_prompt_file_content,
            commands::sync_prompt_to_live,
            commands::get_installed_skills,
            commands::uninstall_skill_unified,
            commands::toggle_skill_app,
            commands::sync_skill_from_live,
            commands::scan_unmanaged_skills,
            commands::import_skills_from_apps,
            commands::read_skill_document,
            commands::open_skill_directory,
            commands::local_scan_enter_page,
            commands::list_conflict_center_items_command,
            commands::resolve_conflict_center_item_command,
            commands::list_sessions,
            commands::search_sessions,
            commands::get_session_messages,
            commands::launch_session_terminal,
            commands::get_cli_statuses,
            commands::get_daily_brief_settings,
            commands::save_daily_brief_settings_command,
            commands::test_daily_brief_connection,
            commands::list_daily_briefs,
            commands::generate_daily_brief,
            commands::delete_daily_brief,
            commands::open_daily_brief,
            commands::open_daily_brief_directory,
            commands::fetch_models_for_config,
            commands::get_opencode_models,
            commands::webdav_test_connection,
            commands::webdav_sync_save_settings,
            commands::webdav_sync_preview_first,
            commands::webdav_sync_confirm_first,
            commands::webdav_sync_now,
            commands::webdav_sync_list_devices,
            commands::webdav_sync_retire_device,
            commands::get_settings,
            commands::save_settings,
            commands::set_auto_launch,
            commands::get_auto_launch_status,
            commands::is_portable_mode,
            commands::get_init_error,
            commands::get_migration_result,
            commands::pick_directory,
            commands::open_app_config_folder,
            commands::open_external,
            commands::copy_text_to_clipboard,
            commands::set_window_theme,
            update_tray_menu,
        ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("无法构建 WSL Code Switch");
    app.run(|app, event| {
        if let RunEvent::ExitRequested { .. } = event {
            if let Some(scan) = app.try_state::<services::LocalScanRuntimeState>() {
                let _ = scan.cancel();
            }
        }
    });
}
