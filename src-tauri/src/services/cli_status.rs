use std::process::Command;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};

const WSL_DISTRIBUTION: &str = "Ubuntu";
const WSL_USER: &str = "zhldm";

#[derive(Debug, Clone, Copy)]
struct CliSpec {
    id: &'static str,
    display_name: &'static str,
    executable: &'static str,
    npm_package: &'static str,
    registry_latest_url: &'static str,
    source_url: &'static str,
}

const CLI_SPECS: [CliSpec; 3] = [
    CliSpec {
        id: "claude",
        display_name: "Claude Code",
        executable: "claude",
        npm_package: "@anthropic-ai/claude-code",
        registry_latest_url: "https://registry.npmjs.org/@anthropic-ai%2fclaude-code/latest",
        source_url: "https://www.npmjs.com/package/@anthropic-ai/claude-code",
    },
    CliSpec {
        id: "codex",
        display_name: "Codex",
        executable: "codex",
        npm_package: "@openai/codex",
        registry_latest_url: "https://registry.npmjs.org/@openai%2fcodex/latest",
        source_url: "https://www.npmjs.com/package/@openai/codex",
    },
    CliSpec {
        id: "opencode",
        display_name: "OpenCode",
        executable: "opencode",
        npm_package: "opencode-ai",
        registry_latest_url: "https://registry.npmjs.org/opencode-ai/latest",
        source_url: "https://www.npmjs.com/package/opencode-ai",
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub id: String,
    pub display_name: String,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub installation_channel: String,
    pub executable_path: Option<String>,
    pub latest_source_url: String,
    pub wsl_command: String,
    pub powershell_command: String,
    pub state: String,
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NpmLatest {
    version: String,
}

#[derive(Debug, Clone)]
struct CurrentInstallation {
    version: Option<String>,
    channel: String,
    executable_path: Option<String>,
    detail: Option<String>,
}

pub async fn load_cli_statuses() -> Vec<CliStatus> {
    let current = tauri::async_runtime::spawn_blocking(|| {
        CLI_SPECS
            .iter()
            .map(detect_current_installation)
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_else(|error| {
        CLI_SPECS
            .iter()
            .map(|_| CurrentInstallation {
                version: None,
                channel: "unknown".to_string(),
                executable_path: None,
                detail: Some(format!("Current version detection failed: {error}")),
            })
            .collect()
    });

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .ok();

    let latest = match client {
        Some(client) => {
            let (claude, codex, opencode) = tokio::join!(
                fetch_latest(&client, &CLI_SPECS[0]),
                fetch_latest(&client, &CLI_SPECS[1]),
                fetch_latest(&client, &CLI_SPECS[2]),
            );
            vec![claude, codex, opencode]
        }
        None => vec![
            Err("Latest-version client initialization failed".to_string()),
            Err("Latest-version client initialization failed".to_string()),
            Err("Latest-version client initialization failed".to_string()),
        ],
    };

    CLI_SPECS
        .iter()
        .zip(current)
        .zip(latest)
        .map(|((spec, current), latest)| build_status(spec, current, latest))
        .collect()
}

async fn fetch_latest(client: &reqwest::Client, spec: &CliSpec) -> Result<String, String> {
    let response = client
        .get(spec.registry_latest_url)
        .send()
        .await
        .map_err(|_| "Official release source is unavailable".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Official release source returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let latest = response
        .json::<NpmLatest>()
        .await
        .map_err(|_| "Official release metadata is invalid".to_string())?;
    let version = latest.version.trim();
    if version.is_empty() || version.len() > 64 {
        return Err("Official release version is invalid".to_string());
    }
    Ok(version.to_string())
}

fn detect_current_installation(spec: &CliSpec) -> CurrentInstallation {
    if !cfg!(target_os = "windows") {
        return CurrentInstallation {
            version: None,
            channel: "unknown".to_string(),
            executable_path: None,
            detail: Some("CLI detection is only available on Windows".to_string()),
        };
    }

    let executable_path = run_wsl(["/usr/bin/env", "which", spec.executable])
        .ok()
        .and_then(|output| first_non_empty_line(&output));
    let Some(path) = executable_path else {
        return CurrentInstallation {
            version: None,
            channel: "notInstalled".to_string(),
            executable_path: None,
            detail: Some("CLI is not installed or is not on PATH".to_string()),
        };
    };
    let version = run_wsl([spec.executable, "--version"])
        .ok()
        .and_then(|output| extract_version(&output));
    CurrentInstallation {
        channel: installation_channel(&path).to_string(),
        executable_path: Some(path),
        detail: version
            .is_none()
            .then(|| "CLI was found but its version could not be parsed".to_string()),
        version,
    }
}

fn run_wsl<const N: usize>(command: [&str; N]) -> Result<String, String> {
    let output = Command::new("wsl.exe")
        .args(["-d", WSL_DISTRIBUTION, "-u", WSL_USER, "--"])
        .args(command)
        .output()
        .map_err(|error| format!("Failed to start wsl.exe: {error}"))?;
    if !output.status.success() {
        return Err("WSL command failed".to_string());
    }
    String::from_utf8(output.stdout).map_err(|_| "WSL command returned invalid UTF-8".to_string())
}

fn first_non_empty_line(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

fn extract_version(output: &str) -> Option<String> {
    let regex = Regex::new(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?").expect("version regex");
    regex.find(output).map(|value| value.as_str().to_string())
}

fn installation_channel(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    if path.contains("pnpm") {
        "pnpm"
    } else if path.contains(".bun/") || path.contains("/bun/") {
        "bun"
    } else if path.contains("node_modules")
        || path.contains("/.npm")
        || path.contains("/nvm/")
        || path.contains("/.nvm/")
    {
        "npm"
    } else if path.contains("homebrew") || path.contains("linuxbrew") {
        "homebrew"
    } else {
        "standalone"
    }
}

fn build_status(
    spec: &CliSpec,
    current: CurrentInstallation,
    latest: Result<String, String>,
) -> CliStatus {
    let (latest_version, latest_error) = match latest {
        Ok(version) => (Some(version), None),
        Err(error) => (None, Some(error)),
    };
    let wsl_command = upgrade_command(spec);
    let powershell_command = format!(
        "wsl.exe -d {WSL_DISTRIBUTION} -u {WSL_USER} -- bash -lc '{}'",
        wsl_command.replace('\'', "'\\''")
    );
    let state = if current.version.is_none() && current.channel == "notInstalled" {
        "notInstalled"
    } else if latest_version.is_none() {
        "latestUnavailable"
    } else if current.version.is_none() {
        "currentUnavailable"
    } else {
        "ok"
    };
    CliStatus {
        id: spec.id.to_string(),
        display_name: spec.display_name.to_string(),
        current_version: current.version,
        latest_version,
        installation_channel: current.channel,
        executable_path: current.executable_path,
        latest_source_url: spec.source_url.to_string(),
        wsl_command,
        powershell_command,
        state: state.to_string(),
        detail: latest_error.or(current.detail),
    }
}

/// Copy-only upgrade/install hint. npm is the single canonical channel for
/// every managed CLI, so the command no longer varies by detected channel.
fn upgrade_command(spec: &CliSpec) -> String {
    format!("npm install -g {}@latest", spec.npm_package)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_and_installation_channels() {
        assert_eq!(
            extract_version("claude-code 2.1.161"),
            Some("2.1.161".to_string())
        );
        assert_eq!(
            installation_channel("/home/u/.nvm/versions/node/bin/codex"),
            "npm"
        );
        assert_eq!(
            installation_channel("/home/u/.local/share/pnpm/opencode"),
            "pnpm"
        );
        assert_eq!(installation_channel("/home/u/.bun/bin/claude"), "bun");
        assert_eq!(
            installation_channel("/home/u/.local/bin/codex"),
            "standalone"
        );
    }

    #[test]
    fn commands_are_fixed_copy_only_npm_commands_for_three_official_packages() {
        assert_eq!(
            upgrade_command(&CLI_SPECS[0]),
            "npm install -g @anthropic-ai/claude-code@latest"
        );
        assert_eq!(
            upgrade_command(&CLI_SPECS[1]),
            "npm install -g @openai/codex@latest"
        );
        assert_eq!(
            upgrade_command(&CLI_SPECS[2]),
            "npm install -g opencode-ai@latest"
        );
        let source = include_str!("cli_status.rs");
        let production = source.split_once("#[cfg(test)]").unwrap().0;
        assert!(!production.contains("run_upgrade"));
        assert!(!production.contains("run_tool_lifecycle"));
        assert!(!production.contains("install.sh"));
        assert!(!production.contains("curl -fsSL"));
    }
}
