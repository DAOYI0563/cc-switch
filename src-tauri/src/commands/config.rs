#![allow(non_snake_case)]

use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::app_config::LegacyAppType;
use crate::codex_config;
use crate::commands::{parse_managed_app_type, parse_managed_client_id};
use crate::config::{self, get_claude_settings_path, ConfigStatus};
use crate::domain::ManagedClientId;
use crate::settings;

#[tauri::command]
pub async fn get_claude_config_status() -> Result<ConfigStatus, String> {
    Ok(config::get_claude_config_status())
}

fn invalid_json_format_error(error: serde_json::Error) -> String {
    let lang = settings::get_settings()
        .language
        .unwrap_or_else(|| "zh".to_string());

    match lang.as_str() {
        "en" => format!("Invalid JSON format: {error}"),
        "ja" => format!("JSON形式が無効です: {error}"),
        _ => format!("无效的 JSON 格式: {error}"),
    }
}

fn invalid_toml_format_error(error: toml_edit::TomlError) -> String {
    let lang = settings::get_settings()
        .language
        .unwrap_or_else(|| "zh".to_string());

    match lang.as_str() {
        "en" => format!("Invalid TOML format: {error}"),
        "ja" => format!("TOML形式が無効です: {error}"),
        _ => format!("无效的 TOML 格式: {error}"),
    }
}

fn validate_common_config_snippet(app_type: &str, snippet: &str) -> Result<(), String> {
    let client = match app_type {
        "claude" => ManagedClientId::Claude,
        "codex" => ManagedClientId::Codex,
        _ => return Err("common config snippets are supported only for claude and codex".into()),
    };
    crate::domain::validate_common_snippet(client, snippet).map_err(|error| {
        if error.contains("JSON") {
            serde_json::from_str::<serde_json::Value>(snippet)
                .err()
                .map(invalid_json_format_error)
                .unwrap_or(error)
        } else if error.contains("TOML") {
            snippet
                .parse::<toml_edit::DocumentMut>()
                .err()
                .map(invalid_toml_format_error)
                .unwrap_or(error)
        } else {
            error
        }
    })
}

fn parse_common_config_app_type(app: &str) -> Result<LegacyAppType, String> {
    match parse_managed_app_type(app)? {
        app @ (LegacyAppType::Claude | LegacyAppType::Codex) => Ok(app),
        LegacyAppType::OpenCode => {
            Err("common config snippets are supported only for claude and codex".to_string())
        }
    }
}

#[tauri::command]
pub async fn get_config_status(app: String) -> Result<ConfigStatus, String> {
    match parse_managed_client_id(&app)? {
        ManagedClientId::Claude => Ok(config::get_claude_config_status()),
        ManagedClientId::Codex => {
            let auth_path = codex_config::get_codex_auth_path();
            let config_text = codex_config::read_codex_config_text().unwrap_or_default();
            let exists = auth_path.exists() || !config_text.trim().is_empty();
            let path = codex_config::get_codex_config_dir()
                .to_string_lossy()
                .to_string();
            Ok(ConfigStatus { exists, path })
        }
        ManagedClientId::Opencode => {
            let config_path = crate::opencode_config::get_opencode_config_path();
            let exists = config_path.exists();
            let path = crate::opencode_config::get_opencode_dir()
                .to_string_lossy()
                .to_string();
            Ok(ConfigStatus { exists, path })
        }
    }
}

#[tauri::command]
pub async fn get_claude_code_config_path() -> Result<String, String> {
    Ok(get_claude_settings_path().to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_config_dir(app: String) -> Result<String, String> {
    let dir = match parse_managed_client_id(&app)? {
        ManagedClientId::Claude => config::get_claude_config_dir(),
        ManagedClientId::Codex => codex_config::get_codex_config_dir(),
        ManagedClientId::Opencode => crate::opencode_config::get_opencode_dir(),
    };

    Ok(dir.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_config_folder(handle: AppHandle, app: String) -> Result<bool, String> {
    let config_dir = match parse_managed_client_id(&app)? {
        ManagedClientId::Claude => config::get_claude_config_dir(),
        ManagedClientId::Codex => codex_config::get_codex_config_dir(),
        ManagedClientId::Opencode => crate::opencode_config::get_opencode_dir(),
    };

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    handle
        .opener()
        .open_path(config_dir.to_string_lossy().to_string(), None::<String>)
        .map_err(|e| format!("打开文件夹失败: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn pick_directory(
    app: AppHandle,
    #[allow(non_snake_case)] defaultPath: Option<String>,
) -> Result<Option<String>, String> {
    let initial = defaultPath
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());

    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut builder = app.dialog().file();
        if let Some(path) = initial {
            builder = builder.set_directory(path);
        }
        builder.blocking_pick_folder()
    })
    .await
    .map_err(|e| format!("弹出目录选择器失败: {e}"))?;

    match result {
        Some(file_path) => {
            let resolved = file_path
                .simplified()
                .into_path()
                .map_err(|e| format!("解析选择的目录失败: {e}"))?;
            Ok(Some(resolved.to_string_lossy().to_string()))
        }
        None => Ok(None),
    }
}

#[tauri::command]
pub async fn get_app_config_path() -> Result<String, String> {
    let config_path = config::get_app_config_path();
    Ok(config_path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn open_app_config_folder(handle: AppHandle) -> Result<bool, String> {
    let config_dir = config::get_app_config_dir();

    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }

    handle
        .opener()
        .open_path(config_dir.to_string_lossy().to_string(), None::<String>)
        .map_err(|e| format!("打开文件夹失败: {e}"))?;

    Ok(true)
}

#[tauri::command]
pub async fn get_common_config_snippet(
    app_type: String,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<Option<String>, String> {
    let app = parse_common_config_app_type(&app_type)?;
    let client = ManagedClientId::try_from(&app).map_err(|error| error.to_string())?;
    crate::services::CommonSnippetService::get(state.inner(), client).map_err(|e| e.to_string())
}

/// 对前端编辑器里的 config.toml 文本做通用配置片段的合并/剥离。
/// 放后端是为了走 toml_edit（保注释、保键序）；前端 smol-toml 的
/// 整文档重序列化会破坏用户手写格式。
#[tauri::command]
pub async fn update_toml_common_config_snippet(
    config_toml: String,
    snippet_toml: String,
    enabled: bool,
) -> Result<String, String> {
    crate::services::provider::update_toml_common_config_snippet(
        &config_toml,
        &snippet_toml,
        enabled,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_common_config_snippet(
    app_type: String,
    snippet: String,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<(), String> {
    let app = parse_common_config_app_type(&app_type)?;
    validate_common_config_snippet(app.as_str(), &snippet)?;
    let client = ManagedClientId::try_from(&app).map_err(|error| error.to_string())?;
    crate::services::CommonSnippetService::set(state.inner(), client, snippet)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::validate_common_config_snippet;

    #[test]
    fn validate_common_config_snippet_accepts_comment_only_codex_snippet() {
        validate_common_config_snippet("codex", "# comment only\n")
            .expect("comment-only codex snippet should be valid");
    }

    #[test]
    fn validate_common_config_snippet_rejects_invalid_codex_snippet() {
        let err = validate_common_config_snippet("codex", "[broken")
            .expect_err("invalid codex snippet should be rejected");
        assert!(
            err.contains("TOML") || err.contains("toml") || err.contains("格式"),
            "expected TOML validation error, got {err}"
        );
    }

    #[test]
    fn validate_common_config_snippet_rejects_claude_managed_and_sensitive_fields() {
        for snippet in [
            r#"{"env":{"ANTHROPIC_API_KEY":"secret"}}"#,
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://provider.example"}}"#,
            r#"{"mcpServers":{"echo":{"command":"echo"}}}"#,
            r#"{"prompts":{"daily":"managed"}}"#,
            r#"{"skills":{"review":"managed"}}"#,
            r#"{"nested":{"client_secret":"secret"}}"#,
        ] {
            let error = validate_common_config_snippet("claude", snippet)
                .expect_err("forbidden Claude fields must be rejected");
            assert!(
                error.contains("forbidden") || error.contains("禁止"),
                "expected a forbidden-field error for {snippet}, got {error}"
            );
        }
    }

    #[test]
    fn validate_common_config_snippet_rejects_codex_managed_and_sensitive_fields() {
        for snippet in [
            "model_provider = \"custom\"\n",
            "[model_providers.custom]\nbase_url = \"https://provider.example\"\n",
            "[mcp_servers.echo]\ncommand = \"echo\"\n",
            "[prompts.daily]\ncontent = \"managed\"\n",
            "[skills.review]\nenabled = true\n",
            "[nested]\napi_key = \"secret\"\n",
        ] {
            let error = validate_common_config_snippet("codex", snippet)
                .expect_err("forbidden Codex fields must be rejected");
            assert!(
                error.contains("forbidden") || error.contains("禁止"),
                "expected a forbidden-field error for {snippet}, got {error}"
            );
        }
    }
}

#[tauri::command]
pub async fn extract_common_config_snippet(
    appType: String,
    settingsConfig: Option<String>,
    state: tauri::State<'_, crate::store::AppState>,
) -> Result<String, String> {
    let app = parse_common_config_app_type(&appType)?;

    if let Some(settings_config) = settingsConfig.filter(|s| !s.trim().is_empty()) {
        let settings: serde_json::Value =
            serde_json::from_str(&settings_config).map_err(invalid_json_format_error)?;

        return crate::services::provider::ProviderService::extract_common_config_snippet_from_settings(
            app,
            &settings,
        )
        .map_err(|e| e.to_string());
    }

    crate::services::provider::ProviderService::extract_common_config_snippet(&state, app)
        .map_err(|e| e.to_string())
}
