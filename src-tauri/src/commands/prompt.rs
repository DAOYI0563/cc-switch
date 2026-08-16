use indexmap::IndexMap;
use tauri::State;

use crate::commands::parse_managed_client_id;
use crate::prompt::Prompt;
use crate::services::PromptService;
use crate::store::AppState;

fn upsert_prompt_for_state(
    state: &AppState,
    app: &str,
    id: &str,
    prompt: Prompt,
) -> Result<Prompt, String> {
    let client = parse_managed_client_id(app)?;
    PromptService::upsert_prompt(state, client, id, prompt).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_prompts(
    app: String,
    state: State<'_, AppState>,
) -> Result<IndexMap<String, Prompt>, String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::get_prompts(&state, client).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn upsert_prompt(
    app: String,
    id: String,
    prompt: Prompt,
    state: State<'_, AppState>,
) -> Result<Prompt, String> {
    upsert_prompt_for_state(&state, &app, &id, prompt)
}

#[tauri::command]
pub async fn delete_prompt(
    app: String,
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::delete_prompt(&state, client, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn enable_prompt(
    app: String,
    id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::enable_prompt(&state, client, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn import_prompt_from_file(
    app: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::import_from_file(&state, client).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_current_prompt_file_content(app: String) -> Result<Option<String>, String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::get_current_file_content(client).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sync_prompt_to_live(app: String, state: State<'_, AppState>) -> Result<(), String> {
    let client = parse_managed_client_id(&app)?;
    PromptService::sync_to_live(&state, client).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use serial_test::serial;

    use super::*;
    use crate::database::Database;
    use crate::domain::ManagedClientId;

    struct TestHomeGuard(Option<OsString>);

    impl TestHomeGuard {
        #[allow(deprecated)]
        fn set(home: &Path) -> Self {
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home);
            Self(previous)
        }
    }

    impl Drop for TestHomeGuard {
        #[allow(deprecated)]
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    fn prompt(id: &str, name: &str, content: &str) -> Prompt {
        Prompt {
            id: id.to_string(),
            name: name.to_string(),
            version: 0,
            content: content.to_string(),
            description: None,
            enabled: true,
            created_at: None,
            updated_at: None,
        }
    }

    fn live_paths(home: &Path) -> [PathBuf; 3] {
        [
            home.join(".claude/CLAUDE.md"),
            home.join(".codex/AGENTS.md"),
            home.join(".config/opencode/AGENTS.md"),
        ]
    }

    fn assert_zero_writes(state: &AppState, home: &Path) {
        for client in ManagedClientId::ALL {
            assert!(state.db.get_prompt_versions(client).unwrap().is_empty());
        }
        for path in live_paths(home) {
            assert!(!path.exists(), "invalid command created {}", path.display());
        }
    }

    #[test]
    #[serial]
    fn command_rejects_unknown_client_mismatched_id_and_invalid_payload_without_writes() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let state = AppState::new(Arc::new(Database::memory().unwrap()));

        let error = upsert_prompt_for_state(
            &state,
            "gemini",
            "prompt-one",
            prompt("prompt-one", "Default", "content"),
        )
        .expect_err("a non-target client must be rejected");
        assert!(error.contains("gemini") || error.contains("managed client"));
        assert_zero_writes(&state, temp.path());

        let error = upsert_prompt_for_state(
            &state,
            "claude",
            "route-id",
            prompt("payload-id", "Default", "content"),
        )
        .expect_err("route and payload ids must match");
        assert!(error.contains("ID"));
        assert_zero_writes(&state, temp.path());

        let error = upsert_prompt_for_state(
            &state,
            "claude",
            "invalid-prompt",
            prompt("invalid-prompt", "   ", ""),
        )
        .expect_err("invalid prompt content must be rejected");
        assert!(error.contains("name") || error.contains("content"));
        assert_zero_writes(&state, temp.path());
    }
}
