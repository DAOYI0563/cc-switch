use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;

use crate::adapters::prompt_live_file::{PromptLiveFileAdapter, PromptLiveFileSnapshot};
use crate::domain::{
    prompt_live_filename, LocalScanDomain, LocalScanTarget, ManagedClientId, PromptVersion,
};
use crate::error::AppError;
use crate::store::AppState;

pub struct PromptService;

fn prompt_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn rollback_error(primary: AppError, rollback: Result<(), AppError>) -> AppError {
    match rollback {
        Ok(()) => primary,
        Err(error) => AppError::Message(format!(
            "{primary}; Prompt live rollback also failed: {error}"
        )),
    }
}

impl PromptService {
    pub fn get_prompts(
        state: &AppState,
        client: ManagedClientId,
    ) -> Result<IndexMap<String, PromptVersion>, AppError> {
        state.db.get_prompt_versions(client)
    }

    pub fn upsert_prompt(
        state: &AppState,
        client: ManagedClientId,
        id: &str,
        prompt: PromptVersion,
    ) -> Result<PromptVersion, AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        if id != prompt.id {
            return Err(AppError::InvalidInput(
                "Prompt 路径 ID 与记录 ID 不一致".to_string(),
            ));
        }
        let existing = state.db.get_prompt_versions(client)?;
        let target = state.db.prepare_prompt_version(client, prompt)?;
        let current_active = existing.values().find(|version| version.enabled);
        let target_was_active = existing.get(id).is_some_and(|version| version.enabled);
        let desired_live = if target.enabled {
            Some(target.content.as_str())
        } else if target_was_active {
            Some("")
        } else {
            None
        };

        if let Some(desired_live) = desired_live {
            Self::commit_with_live_change(
                state,
                client,
                current_active.map(|version| version.content.as_str()),
                desired_live,
                || state.db.save_prompt_version(client, &target),
            )?;
        } else {
            state.db.save_prompt_version(client, &target)?;
        }
        Ok(target)
    }

    pub fn delete_prompt(
        state: &AppState,
        client: ManagedClientId,
        id: &str,
    ) -> Result<(), AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        let versions = state.db.get_prompt_versions(client)?;
        if versions.get(id).is_some_and(|version| version.enabled) {
            return Err(AppError::InvalidInput(
                "无法删除已启用的 Prompt 版本；请先切换或禁用".to_string(),
            ));
        }
        state.db.delete_prompt_version(client, id)?;
        Ok(())
    }

    pub fn enable_prompt(
        state: &AppState,
        client: ManagedClientId,
        id: &str,
    ) -> Result<(), AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        let versions = state.db.get_prompt_versions(client)?;
        let current_active = versions.values().find(|version| version.enabled);
        let mut target = versions
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::InvalidInput(format!("Prompt 版本 {id} 不存在")))?;
        if target.enabled {
            return Ok(());
        }
        target.enabled = true;
        target = state.db.prepare_prompt_version(client, target)?;

        Self::commit_with_live_change(
            state,
            client,
            current_active.map(|version| version.content.as_str()),
            &target.content,
            || state.db.save_prompt_version(client, &target),
        )
    }

    pub fn import_from_file(state: &AppState, client: ManagedClientId) -> Result<String, AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        let files = PromptLiveFileAdapter::runtime();
        let content = files
            .read_text(client)?
            .ok_or_else(|| AppError::InvalidInput("Prompt live 文件不存在".to_string()))?;
        if content.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "Prompt live 文件内容为空，未创建版本".to_string(),
            ));
        }
        let id = format!("prompt-{}", uuid::Uuid::new_v4().simple());
        let prompt = PromptVersion {
            id: id.clone(),
            name: format!("从 {} 导入", prompt_live_filename(client)),
            version: 0,
            content,
            description: Some("从 WSL live 文件手动提取".to_string()),
            enabled: false,
            created_at: None,
            updated_at: None,
        };
        let prompt = state.db.prepare_prompt_version(client, prompt)?;
        state.db.save_prompt_version(client, &prompt)?;
        Ok(id)
    }

    pub fn get_current_file_content(client: ManagedClientId) -> Result<Option<String>, AppError> {
        PromptLiveFileAdapter::runtime().read_text(client)
    }

    /// Explicit user-confirmed projection. Unlike enable/edit, this operation may
    /// replace externally changed bytes because the UI presents it as a live-file
    /// overwrite command.
    pub fn sync_to_live(state: &AppState, client: ManagedClientId) -> Result<(), AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        let versions = state.db.get_prompt_versions(client)?;
        let desired = versions
            .values()
            .find(|version| version.enabled)
            .map(|version| version.content.as_str())
            .unwrap_or("");
        let files = PromptLiveFileAdapter::runtime();
        let snapshot = files.capture(client)?;
        if let Err(primary) = files.write_text(client, desired) {
            return Err(rollback_error(primary, files.restore(&snapshot)));
        }
        record_prompt_write(state, client);
        Ok(())
    }

    pub fn import_from_file_on_first_launch(
        state: &AppState,
        client: ManagedClientId,
    ) -> Result<usize, AppError> {
        let _guard = prompt_mutation_lock().lock()?;
        if !state.db.get_prompt_versions(client)?.is_empty() {
            return Ok(0);
        }
        let Some(content) = PromptLiveFileAdapter::runtime().read_text(client)? else {
            return Ok(0);
        };
        if content.trim().is_empty() {
            return Ok(0);
        }
        let id = format!("prompt-{}", uuid::Uuid::new_v4().simple());
        let prompt = PromptVersion {
            id,
            name: prompt_live_filename(client).to_string(),
            version: 0,
            content,
            description: Some("首次启动时从 WSL live 文件导入".to_string()),
            enabled: true,
            created_at: None,
            updated_at: None,
        };
        let prompt = state.db.prepare_prompt_version(client, prompt)?;
        state.db.save_prompt_version(client, &prompt)?;
        Ok(1)
    }

    fn commit_with_live_change(
        state: &AppState,
        client: ManagedClientId,
        expected_active: Option<&str>,
        desired_live: &str,
        commit_database: impl FnOnce() -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let files = PromptLiveFileAdapter::runtime();
        let snapshot = files.capture(client)?;
        Self::ensure_live_is_managed_or_desired(&snapshot, expected_active, desired_live)?;
        files.write_text(client, desired_live)?;
        if let Err(primary) = commit_database() {
            return Err(rollback_error(primary, files.restore(&snapshot)));
        }
        record_prompt_write(state, client);
        Ok(())
    }

    fn ensure_live_is_managed_or_desired(
        snapshot: &PromptLiveFileSnapshot,
        expected_active: Option<&str>,
        desired_live: &str,
    ) -> Result<(), AppError> {
        let actual = snapshot.contents.as_deref();
        let expected = expected_active.map(str::as_bytes);
        let desired = desired_live.as_bytes();
        let empty_or_missing = actual.is_none_or(|value| value.is_empty());
        let expected_empty = expected_active.is_none_or(str::is_empty);
        if actual == expected || actual == Some(desired) || (expected_empty && empty_or_missing) {
            return Ok(());
        }
        Err(AppError::InvalidInput(format!(
            "{} 已被外部修改；请先导入 live 内容为新版本，或使用明确的“同步到 live”覆盖",
            prompt_live_filename(snapshot.client)
        )))
    }
}

fn record_prompt_write(state: &AppState, client_id: ManagedClientId) {
    crate::services::record_runtime_local_writes(
        &state.local_scan_writes,
        [LocalScanTarget {
            domain: LocalScanDomain::Prompt,
            client_id,
        }],
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::database::Database;
    use serial_test::serial;

    struct TestHomeGuard(Option<std::ffi::OsString>);

    impl TestHomeGuard {
        #[allow(deprecated)]
        fn set(home: &std::path::Path) -> Self {
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

    fn prompt(id: &str, content: &str, enabled: bool) -> PromptVersion {
        PromptVersion {
            id: id.to_string(),
            name: id.to_string(),
            version: 0,
            content: content.to_string(),
            description: None,
            enabled,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    #[serial]
    fn database_failure_after_live_write_restores_exact_bytes_and_active_row() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let state = AppState::new(Arc::new(Database::memory().unwrap()));
        PromptService::upsert_prompt(
            &state,
            ManagedClientId::Claude,
            "first",
            prompt("first", "first\r\n", true),
        )
        .unwrap();
        PromptService::upsert_prompt(
            &state,
            ManagedClientId::Claude,
            "second",
            prompt("second", "second\n", false),
        )
        .unwrap();
        let path = temp.path().join(".claude/CLAUDE.md");
        let before = std::fs::read(&path).unwrap();
        {
            let conn = state.db.conn.lock().expect("database lock");
            conn.execute_batch(
                "CREATE TRIGGER fail_prompt_activation
                 BEFORE UPDATE ON core_prompt_versions
                 BEGIN SELECT RAISE(ABORT, 'forced prompt database failure'); END;",
            )
            .unwrap();
        }

        let error = PromptService::enable_prompt(&state, ManagedClientId::Claude, "second")
            .expect_err("database failure must surface");
        assert!(error.to_string().contains("forced prompt database failure"));
        assert_eq!(std::fs::read(path).unwrap(), before);
        let stored = state
            .db
            .get_prompt_versions(ManagedClientId::Claude)
            .unwrap();
        assert!(stored["first"].enabled);
        assert!(!stored["second"].enabled);
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::env;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::database::Database;

    #[test]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR and CC_SWITCH_TEST_HOME on isolated WSL2 UNC paths"]
    fn prompt_versions_import_sync_and_rollback_on_wsl_unc() {
        let root = PathBuf::from(
            env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = PathBuf::from(
            env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set"),
        );
        assert!(home.starts_with(&root));
        let state = AppState::new(Arc::new(Database::memory().unwrap()));

        for client in ManagedClientId::ALL {
            let id = format!("{}-native", client.as_str());
            let content = format!("{} native content\r\n", client.as_str());
            PromptService::upsert_prompt(
                &state,
                client,
                &id,
                PromptVersion {
                    id: id.clone(),
                    name: "Native".to_string(),
                    version: 0,
                    content: content.clone(),
                    description: None,
                    enabled: true,
                    created_at: None,
                    updated_at: None,
                },
            )
            .unwrap();
            assert_eq!(
                PromptService::get_current_file_content(client).unwrap(),
                Some(content)
            );
        }

        let claude_path = home.join(".claude/CLAUDE.md");
        std::fs::write(&claude_path, b"external native bytes\r\n").unwrap();
        let before = state
            .db
            .get_prompt_versions(ManagedClientId::Claude)
            .unwrap();
        let error = PromptService::upsert_prompt(
            &state,
            ManagedClientId::Claude,
            "blocked-native",
            PromptVersion {
                id: "blocked-native".to_string(),
                name: "Blocked".to_string(),
                version: 0,
                content: "replacement".to_string(),
                description: None,
                enabled: true,
                created_at: None,
                updated_at: None,
            },
        )
        .expect_err("external bytes must block implicit overwrite");
        assert!(error.to_string().contains("导入"));
        assert_eq!(
            std::fs::read(&claude_path).unwrap(),
            b"external native bytes\r\n"
        );
        assert_eq!(
            state
                .db
                .get_prompt_versions(ManagedClientId::Claude)
                .unwrap(),
            before
        );

        for client in ManagedClientId::ALL {
            let path = match client {
                ManagedClientId::Claude => home.join(".claude/CLAUDE.md"),
                ManagedClientId::Codex => home.join(".codex/AGENTS.md"),
                ManagedClientId::Opencode => home.join(".config/opencode/AGENTS.md"),
            };
            let _ = std::fs::remove_file(path);
        }
    }
}
