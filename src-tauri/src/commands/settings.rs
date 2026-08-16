#[tauri::command]
pub async fn get_settings() -> Result<crate::settings::AppSettings, String> {
    Ok(crate::settings::get_settings_for_frontend())
}

#[tauri::command]
pub async fn save_settings(settings: crate::settings::AppSettings) -> Result<bool, String> {
    let mut next = crate::settings::get_settings();
    next.show_in_tray = settings.show_in_tray;
    next.use_app_window_controls = settings.use_app_window_controls;
    next.launch_on_startup = settings.launch_on_startup;
    next.silent_startup = settings.silent_startup;
    next.language = Some("zh".to_string());

    if let Some(mut incoming) = settings.webdav_sync {
        if incoming.password.is_empty() {
            incoming.password = next
                .webdav_sync
                .as_ref()
                .map(|value| value.password.clone())
                .unwrap_or_default();
        }
        next.webdav_sync = Some(incoming);
    }

    crate::settings::update_settings(next).map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub async fn set_auto_launch(enabled: bool) -> Result<bool, String> {
    if enabled {
        crate::auto_launch::enable_auto_launch()
            .map_err(|error| format!("启用开机自启失败: {error}"))?;
    } else {
        crate::auto_launch::disable_auto_launch()
            .map_err(|error| format!("禁用开机自启失败: {error}"))?;
    }
    Ok(true)
}

#[tauri::command]
pub async fn get_auto_launch_status() -> Result<bool, String> {
    crate::auto_launch::is_auto_launch_enabled().map_err(|error| error.to_string())
}
