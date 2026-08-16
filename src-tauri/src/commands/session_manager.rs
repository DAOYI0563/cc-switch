#![allow(non_snake_case)]

use crate::commands::parse_managed_client_id;
use crate::session_manager;

#[tauri::command]
pub async fn list_sessions() -> Result<Vec<session_manager::SessionMeta>, String> {
    tauri::async_runtime::spawn_blocking(session_manager::scan_sessions)
        .await
        .map_err(|error| format!("Failed to scan sessions: {error}"))
}

#[tauri::command]
pub async fn search_sessions(
    request: session_manager::SessionSearchRequest,
) -> Result<session_manager::SessionPage<session_manager::SessionMeta>, String> {
    if let Some(provider_id) = request.provider_id.as_deref() {
        if provider_id != "all" {
            parse_managed_client_id(provider_id)?;
        }
    }
    tauri::async_runtime::spawn_blocking(move || session_manager::search_sessions(&request))
        .await
        .map_err(|error| format!("Failed to search sessions: {error}"))?
}

#[tauri::command]
pub async fn get_session_messages(
    providerId: String,
    sessionId: String,
    offset: usize,
    limit: Option<usize>,
) -> Result<session_manager::SessionPage<session_manager::NormalizedSessionEvent>, String> {
    let provider_id = parse_managed_client_id(&providerId)?.as_str().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        session_manager::load_messages_page(&provider_id, &sessionId, offset, limit)
    })
    .await
    .map_err(|error| format!("Failed to load session messages: {error}"))?
}

#[tauri::command]
pub async fn launch_session_terminal(
    providerId: String,
    sessionId: String,
) -> Result<bool, String> {
    let provider_id = parse_managed_client_id(&providerId)?.as_str().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let session = session_manager::find_session(&provider_id, &sessionId)?;
        session_manager::terminal::launch_session(&session)
    })
    .await
    .map_err(|error| format!("Failed to launch terminal: {error}"))??;
    Ok(true)
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_surface_is_read_only_and_does_not_accept_paths_or_commands() {
        let source = include_str!("session_manager.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .map(|(production, _)| production)
            .expect("command test boundary");

        for forbidden in [
            "delete_session",
            "delete_sessions",
            "sourcePath",
            "command: String",
            "custom_config",
        ] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
        for required in [
            "list_sessions",
            "search_sessions",
            "get_session_messages",
            "launch_session_terminal",
        ] {
            assert!(production.contains(required), "missing {required}");
        }
    }
}
