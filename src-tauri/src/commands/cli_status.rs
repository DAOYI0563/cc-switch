use crate::services::cli_status::{self, CliStatus};

#[tauri::command]
pub async fn get_cli_statuses() -> Result<Vec<CliStatus>, String> {
    Ok(cli_status::load_cli_statuses().await)
}
