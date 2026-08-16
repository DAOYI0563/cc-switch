use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::init_status::InitErrorPayload;
use crate::services::ProviderService;
use crate::store::AppState;

#[tauri::command]
pub async fn open_external(app: AppHandle, url: String) -> Result<bool, String> {
    let parsed = url::Url::parse(&url).map_err(|_| "链接格式无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("只允许打开 HTTP/HTTPS 链接".to_string());
    }
    app.opener()
        .open_url(parsed.as_str(), None::<String>)
        .map_err(|error| format!("打开链接失败: {error}"))?;
    Ok(true)
}

#[tauri::command]
pub async fn copy_text_to_clipboard(text: String) -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|error| format!("访问系统剪贴板失败: {error}"))?;
        clipboard
            .set_text(text)
            .map_err(|error| format!("写入系统剪贴板失败: {error}"))?;
        Ok(true)
    })
    .await
    .map_err(|error| format!("剪贴板任务执行失败: {error}"))?
}

#[tauri::command]
pub async fn is_portable_mode() -> Result<bool, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("获取可执行路径失败: {error}"))?;
    Ok(executable
        .parent()
        .is_some_and(|directory| directory.join("portable.ini").is_file()))
}

#[tauri::command]
pub async fn get_init_error() -> Result<Option<InitErrorPayload>, String> {
    Ok(crate::init_status::get_init_error())
}

#[tauri::command]
pub async fn get_migration_result() -> Result<bool, String> {
    Ok(crate::init_status::take_migration_success())
}

fn valid_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn provider_environment(provider: &crate::provider::Provider) -> Vec<(String, String)> {
    provider
        .settings_config
        .get("env")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            (valid_environment_name(name))
                .then(|| {
                    value
                        .as_str()
                        .map(|value| (name.clone(), value.to_string()))
                })
                .flatten()
        })
        .collect()
}

fn wsl_cwd(raw: Option<String>) -> Result<Option<String>, String> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    if raw
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        return Err("工作目录包含非法字符".to_string());
    }
    let normalized = raw.replace('/', "\\");
    let parts = normalized
        .split('\\')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 3
        || !matches!(
            parts[0].to_ascii_lowercase().as_str(),
            "wsl.localhost" | "wsl$"
        )
        || !parts[1].eq_ignore_ascii_case("Ubuntu")
    {
        return Err("工作目录必须位于 WSL Ubuntu 中".to_string());
    }
    Ok(Some(format!("/{}", parts[2..].join("/"))))
}

#[allow(non_snake_case)]
#[tauri::command]
pub async fn open_provider_terminal(
    state: State<'_, AppState>,
    app: String,
    providerId: String,
    cwd: Option<String>,
) -> Result<bool, String> {
    let client = crate::commands::parse_managed_client_id(&app)?;
    let providers = ProviderService::list_managed(state.inner(), client)
        .map_err(|error| format!("获取供应商列表失败: {error}"))?;
    let provider = providers
        .get(&providerId)
        .ok_or_else(|| format!("供应商 {providerId} 不存在"))?;
    let environment = provider_environment(provider);
    let cwd = wsl_cwd(cwd)?;

    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("wt.exe");
        command.args([
            "new-tab",
            "--title",
            &format!("{} - {}", client.as_str(), provider.name),
            "wsl.exe",
            "-d",
            "Ubuntu",
            "-u",
            "zhldm",
        ]);
        if let Some(cwd) = cwd {
            command.args(["--cd", &cwd]);
        }
        command.arg("--").arg("env");
        for (name, value) in environment {
            command.arg(format!("{name}={value}"));
        }
        command.arg(client.as_str());
        command
            .spawn()
            .map_err(|error| format!("启动 Windows Terminal 失败: {error}"))?;
        Ok(true)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = (client, environment, cwd);
        Err("仅支持 Windows 便携版".to_string())
    }
}

#[tauri::command]
pub async fn set_window_theme(window: tauri::Window, theme: String) -> Result<(), String> {
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        "system" => None,
        _ => return Err("未知主题".to_string()),
    };
    window.set_theme(theme).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{valid_environment_name, wsl_cwd};

    #[test]
    fn terminal_arguments_accept_only_environment_names() {
        assert!(valid_environment_name("ANTHROPIC_API_KEY"));
        assert!(!valid_environment_name("BAD-NAME"));
        assert!(!valid_environment_name("1BAD"));
    }

    #[test]
    fn terminal_cwd_is_scoped_to_fixed_wsl_distribution() {
        assert_eq!(
            wsl_cwd(Some(
                r"\\wsl.localhost\Ubuntu\home\zhldm\project".to_string()
            ))
            .unwrap()
            .as_deref(),
            Some("/home/zhldm/project")
        );
        assert!(wsl_cwd(Some(r"C:\temp".to_string())).is_err());
    }
}
