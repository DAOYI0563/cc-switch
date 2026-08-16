//! 模型列表获取命令
//!
//! 提供 Tauri 命令，供前端在供应商表单中获取可用模型列表。

use crate::services::model_fetch::{self, FetchedModel};
use serde::Serialize;
use std::collections::BTreeSet;
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeModelRef {
    pub provider_id: String,
    pub model_id: String,
}

const OPENCODE_MODELS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// 获取 OpenCode 当前运行时可用的模型。
///
/// 在固定的 WSL Ubuntu 环境中执行只读的 `opencode models` 查询。
#[tauri::command]
pub async fn get_opencode_models() -> Result<Vec<OpenCodeModelRef>, String> {
    let config_dir = crate::opencode_config::get_opencode_dir();
    let config_dir_env = config_dir.to_string_lossy().into_owned();

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = tokio::process::Command::new("wsl.exe");
        command.args([
            "-d",
            "Ubuntu",
            "-u",
            "zhldm",
            "--",
            "env",
            &format!("OPENCODE_CONFIG_DIR={config_dir_env}"),
            "OPENCODE_DISABLE_PROJECT_CONFIG=true",
            "opencode",
            "models",
        ]);
        command
    };

    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = tokio::process::Command::new("opencode");
        command
            .arg("models")
            .env("OPENCODE_CONFIG_DIR", &config_dir_env)
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "true")
            .current_dir(&config_dir);
        command
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = tokio::time::timeout(OPENCODE_MODELS_TIMEOUT, command.output())
        .await
        .map_err(|_| "OpenCode model discovery timed out".to_string())?
        .map_err(|error| format!("Failed to start OpenCode: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(if detail.is_empty() {
            "Failed to load OpenCode models".to_string()
        } else {
            format!("Failed to load OpenCode models: {detail}")
        });
    }

    Ok(parse_opencode_models(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_opencode_models(output: &str) -> Vec<OpenCodeModelRef> {
    output
        .lines()
        .filter_map(|line| {
            let (provider_id, model_id) = line.trim().split_once('/')?;
            if provider_id.is_empty()
                || model_id.is_empty()
                || !provider_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                || model_id
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
            {
                return None;
            }
            Some((provider_id.to_string(), model_id.to_string()))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|(provider_id, model_id)| OpenCodeModelRef {
            provider_id,
            model_id,
        })
        .collect()
}

/// 获取供应商的可用模型列表
///
/// 使用 OpenAI 兼容的 GET /models 端点。优先使用 `models_url` 精确覆写；
/// 否则从当前 Base URL 唯一确定一个目标，不尝试其他地址。
#[tauri::command(rename_all = "camelCase")]
pub async fn fetch_models_for_config(
    base_url: String,
    api_key: String,
    is_full_url: Option<bool>,
    models_url: Option<String>,
    custom_user_agent: Option<String>,
) -> Result<Vec<FetchedModel>, String> {
    let user_agent = crate::provider::parse_custom_user_agent(custom_user_agent.as_deref())
        .ok()
        .flatten();
    model_fetch::fetch_models(
        &base_url,
        &api_key,
        is_full_url.unwrap_or(false),
        models_url.as_deref(),
        user_agent,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{parse_opencode_models, OpenCodeModelRef};

    #[test]
    fn parses_sorts_and_deduplicates_models() {
        assert_eq!(
            parse_opencode_models(
                "openrouter/vendor/model\nopencode/free-model\ninvalid\nopencode/free-model\n"
            ),
            vec![
                OpenCodeModelRef {
                    provider_id: "opencode".to_string(),
                    model_id: "free-model".to_string(),
                },
                OpenCodeModelRef {
                    provider_id: "openrouter".to_string(),
                    model_id: "vendor/model".to_string(),
                },
            ]
        );
    }

    #[test]
    fn skips_malformed_output_lines() {
        assert!(parse_opencode_models(
            "notice: loading models\n/model\nprovider/\nbad provider/model\nprovider/bad model\nprovider/bad\u{1b}[0m\n"
        )
        .is_empty());
    }
}
