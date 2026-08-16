use std::process::Command;

use crate::session_manager::SessionMeta;

const WSL_DISTRIBUTION: &str = "Ubuntu";
const WSL_USER: &str = "zhldm";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub program: String,
    pub args: Vec<String>,
}

pub fn build_launch(session: &SessionMeta) -> Result<TerminalLaunch, String> {
    let session_id = session.session_id.trim();
    if session_id.is_empty() {
        return Err("Session ID is empty".to_string());
    }
    let (cli, resume_args): (&str, Vec<String>) = match session.provider_id.as_str() {
        "claude" => (
            "claude",
            vec!["--resume".to_string(), session_id.to_string()],
        ),
        "codex" => ("codex", vec!["resume".to_string(), session_id.to_string()]),
        "opencode" => ("opencode", vec!["-s".to_string(), session_id.to_string()]),
        provider_id => return Err(format!("Unsupported provider: {provider_id}")),
    };

    let mut args = vec![
        "wsl.exe".to_string(),
        "-d".to_string(),
        WSL_DISTRIBUTION.to_string(),
        "-u".to_string(),
        WSL_USER.to_string(),
    ];
    if let Some(project_dir) = session
        .project_dir
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push("--cd".to_string());
        args.push(project_dir.to_string());
    }
    args.push(cli.to_string());
    args.extend(resume_args);

    Ok(TerminalLaunch {
        program: "wt.exe".to_string(),
        args,
    })
}

pub fn launch_session(session: &SessionMeta) -> Result<(), String> {
    let launch = build_launch(session)?;
    if !cfg!(target_os = "windows") {
        return Err("Windows Terminal resume is only supported on Windows".to_string());
    }
    Command::new(&launch.program)
        .args(&launch.args)
        .spawn()
        .map_err(|error| format!("Failed to launch Windows Terminal: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(provider_id: &str, session_id: &str, project_dir: &str) -> SessionMeta {
        SessionMeta {
            provider_id: provider_id.to_string(),
            session_id: session_id.to_string(),
            title: None,
            summary: None,
            project_dir: Some(project_dir.to_string()),
            created_at: None,
            last_active_at: None,
            source_path: None,
            resume_command: None,
        }
    }

    #[test]
    fn builds_fixed_windows_terminal_wsl_argument_arrays_for_three_clients() {
        let cases = [
            ("claude", vec!["claude", "--resume", "session-1"]),
            ("codex", vec!["codex", "resume", "session-1"]),
            ("opencode", vec!["opencode", "-s", "session-1"]),
        ];
        for (provider_id, tail) in cases {
            let launch = build_launch(&session(provider_id, "session-1", "/work/project"))
                .expect("build launch");
            assert_eq!(launch.program, "wt.exe");
            assert_eq!(
                &launch.args[..8],
                [
                    "wsl.exe",
                    "-d",
                    "Ubuntu",
                    "-u",
                    "zhldm",
                    "--cd",
                    "/work/project",
                    tail[0]
                ]
            );
            assert_eq!(&launch.args[8..], &tail[1..]);
        }
    }

    #[test]
    fn keeps_untrusted_values_in_individual_arguments() {
        let launch = build_launch(&session(
            "claude",
            "session; touch /tmp/injected",
            "/work/$(touch injected)",
        ))
        .expect("build launch");

        assert_eq!(launch.args[6], "/work/$(touch injected)");
        assert_eq!(launch.args[9], "session; touch /tmp/injected");
        assert!(!launch.args.iter().any(|arg| arg == "sh" || arg == "-c"));
    }
}
