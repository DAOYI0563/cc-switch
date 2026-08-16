use tauri::menu::{CheckMenuItem, Menu, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};

use crate::app_config::LegacyAppType;
use crate::domain::ManagedClientId;
use crate::error::AppError;
use crate::store::AppState;

pub const TRAY_ID: &str = "cc-switch";

struct TraySection {
    client: ManagedClientId,
    prefix: &'static str,
    label: &'static str,
}

const SECTIONS: [TraySection; 3] = [
    TraySection {
        client: ManagedClientId::Claude,
        prefix: "claude_",
        label: "Claude Code",
    },
    TraySection {
        client: ManagedClientId::Codex,
        prefix: "codex_",
        label: "Codex",
    },
    TraySection {
        client: ManagedClientId::Opencode,
        prefix: "opencode_",
        label: "OpenCode",
    },
];

pub fn create_tray_menu(
    app: &tauri::AppHandle,
    state: &AppState,
) -> Result<Menu<tauri::Wry>, AppError> {
    let open = MenuItem::with_id(app, "show_main", "打开 WSL Code Switch", true, None::<&str>)
        .map_err(|error| AppError::Message(format!("创建托盘打开菜单失败: {error}")))?;
    let mut menu = MenuBuilder::new(app).item(&open).separator();

    for section in &SECTIONS {
        let app_type = LegacyAppType::from(section.client);
        let providers = state.db.get_all_providers(section.client.as_str())?;
        let current = crate::settings::get_effective_current_provider(&state.db, &app_type)?
            .unwrap_or_default();
        let label = providers.get(&current).map_or_else(
            || section.label.to_string(),
            |provider| format!("{} · {}", section.label, provider.name),
        );
        let mut submenu =
            SubmenuBuilder::with_id(app, format!("{}_providers", section.client.as_str()), label);
        if providers.is_empty() {
            let empty = MenuItem::with_id(
                app,
                format!("{}_empty", section.client.as_str()),
                "暂无供应商",
                false,
                None::<&str>,
            )
            .map_err(|error| AppError::Message(format!("创建托盘空状态失败: {error}")))?;
            submenu = submenu.item(&empty);
        } else {
            let mut sorted = providers.iter().collect::<Vec<_>>();
            sorted.sort_by(|(_, left), (_, right)| {
                left.sort_index
                    .cmp(&right.sort_index)
                    .then_with(|| left.name.cmp(&right.name))
            });
            for (id, provider) in sorted {
                let item = CheckMenuItem::with_id(
                    app,
                    format!("{}{}", section.prefix, id),
                    &provider.name,
                    true,
                    section.client != ManagedClientId::Opencode && current == *id,
                    None::<&str>,
                )
                .map_err(|error| AppError::Message(format!("创建托盘供应商菜单失败: {error}")))?;
                submenu = submenu.item(&item);
            }
        }
        let submenu = submenu
            .build()
            .map_err(|error| AppError::Message(format!("构建托盘供应商菜单失败: {error}")))?;
        menu = menu.item(&submenu);
    }

    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)
        .map_err(|error| AppError::Message(format!("创建托盘退出菜单失败: {error}")))?;
    menu.separator()
        .item(&quit)
        .build()
        .map_err(|error| AppError::Message(format!("构建托盘菜单失败: {error}")))
}

pub fn refresh_tray_menu(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let (Ok(menu), Some(tray)) = (
        create_tray_menu(app, state.inner()),
        app.tray_by_id(TRAY_ID),
    ) {
        if let Err(error) = tray.set_menu(Some(menu)) {
            log::warn!("刷新托盘菜单失败: {error}");
        }
    }
}

pub fn handle_tray_menu_event(app: &tauri::AppHandle, event_id: &str) {
    match event_id {
        "show_main" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_skip_taskbar(false);
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
                if let Some(scan) =
                    app.try_state::<crate::services::local_scan::LocalScanRuntimeState>()
                {
                    if let Err(error) = scan.window_restored() {
                        log::warn!("恢复窗口时触发本地扫描失败: {error}");
                    }
                }
            }
        }
        "quit" => app.exit(0),
        _ => {
            if !handle_provider_event(app, event_id) {
                log::warn!("忽略未知托盘事件: {event_id}");
            }
        }
    }
}

fn handle_provider_event(app: &tauri::AppHandle, event_id: &str) -> bool {
    let Some(section) = SECTIONS
        .iter()
        .find(|section| event_id.starts_with(section.prefix))
    else {
        return false;
    };
    let provider_id = event_id[section.prefix.len()..].to_string();
    if provider_id.is_empty() {
        return false;
    }
    let app = app.clone();
    let client = section.client;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        match crate::services::ProviderService::switch_managed(state.inner(), client, &provider_id)
        {
            Ok(_) => {
                refresh_tray_menu(&app);
                let _ = app.emit(
                    "provider-switched",
                    serde_json::json!({
                        "appType": client.as_str(),
                        "providerId": provider_id,
                    }),
                );
            }
            Err(error) => log::error!("托盘切换供应商失败: {error}"),
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_is_fixed_to_three_clients_and_chinese_commands() {
        assert_eq!(TRAY_ID, "cc-switch");
        assert_eq!(SECTIONS.len(), 3);
        assert_eq!(SECTIONS[0].client, ManagedClientId::Claude);
        assert_eq!(SECTIONS[1].client, ManagedClientId::Codex);
        assert_eq!(SECTIONS[2].client, ManagedClientId::Opencode);
        assert_eq!(
            SECTIONS
                .iter()
                .map(|section| (section.prefix, section.label))
                .collect::<Vec<_>>(),
            vec![
                ("claude_", "Claude Code"),
                ("codex_", "Codex"),
                ("opencode_", "OpenCode"),
            ]
        );
    }
}
