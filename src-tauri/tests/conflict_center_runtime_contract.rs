use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use serial_test::serial;
use wsl_code_switch_lib::adapters::live_provider_config::runtime_adapter;
use wsl_code_switch_lib::adapters::local_conflict_resolution::RuntimeLocalConflictResolution;
use wsl_code_switch_lib::adapters::local_reconciliation_state::DatabaseLocalReconciliationStateAdapter;
use wsl_code_switch_lib::adapters::local_scan_parser::FixedLocalScanParserAdapter;
use wsl_code_switch_lib::adapters::local_scan_summary::FixedLocalScanSummaryAdapter;
use wsl_code_switch_lib::domain::{
    ConflictCenterDisposition, ConflictResolutionAction, ConflictResolutionRequest,
    LocalConflictKind, LocalScanDomain, LocalScanTarget, LocalSkillImport, ManagedClientApps,
    ManagedClientId, RollbackPointMetadata, RollbackPointPurpose, RollbackPointState,
};
use wsl_code_switch_lib::ports::{
    ConflictCenterSourcePort, LiveProviderRecord, LocalScanParserPort, TemporaryRollbackError,
    TemporaryRollbackErrorCode, TemporaryRollbackStore,
};
use wsl_code_switch_lib::{
    list_conflict_center_items, resolve_conflict_center_item, AppState, Database,
    InMemoryLocalReconciliationBaselines, LocalScanConflictSource, LocalScanCoordinator,
    LocalSkillService, McpApps, McpServer, McpService, Provider, ProviderMeta,
};

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
            Some(value) => std::env::set_var("CC_SWITCH_TEST_HOME", value),
            None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
        }
    }
}

#[derive(Default)]
struct MemoryRollbacks {
    points: Mutex<HashMap<String, RollbackPointMetadata>>,
    deleted: Mutex<usize>,
}

impl TemporaryRollbackStore for MemoryRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        assert_eq!(purpose, RollbackPointPurpose::ConflictResolution);
        assert!(!payload.is_empty());
        let metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: "runtime-point".to_string(),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: "a".repeat(64),
        };
        self.points
            .lock()
            .unwrap()
            .insert(metadata.id.clone(), metadata.clone());
        Ok(metadata)
    }

    fn restore(&self, _id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        Err(not_found())
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        self.points
            .lock()
            .unwrap()
            .remove(id)
            .ok_or_else(not_found)?;
        *self.deleted.lock().unwrap() += 1;
        Ok(())
    }

    fn retain_after_failure(
        &self,
        _id: &str,
        _failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        Err(not_found())
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self.points.lock().unwrap().values().cloned().collect())
    }
}

fn not_found() -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::NotFound,
        "missing runtime point",
    )
}

fn target() -> LocalScanTarget {
    target_for(LocalScanDomain::Mcp, ManagedClientId::Claude)
}

fn target_for(domain: LocalScanDomain, client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget { domain, client_id }
}

fn runtime(
    app_state: &AppState,
) -> (
    Arc<LocalScanCoordinator>,
    Arc<InMemoryLocalReconciliationBaselines>,
) {
    let coordinator = Arc::new(LocalScanCoordinator::new(
        Arc::new(FixedLocalScanSummaryAdapter::runtime()),
        Arc::new(FixedLocalScanParserAdapter::runtime()),
        app_state.local_scan_writes.clone(),
    ));
    let baselines = Arc::new(InMemoryLocalReconciliationBaselines::default());
    (coordinator, baselines)
}

fn source_and_resolver<'a>(
    app_state: &'a AppState,
    coordinator: Arc<LocalScanCoordinator>,
    baselines: Arc<InMemoryLocalReconciliationBaselines>,
) -> (LocalScanConflictSource, RuntimeLocalConflictResolution<'a>) {
    let states = Arc::new(DatabaseLocalReconciliationStateAdapter::new(
        app_state.db.clone(),
        baselines.clone(),
    ));
    (
        LocalScanConflictSource::new(coordinator.clone(), states),
        RuntimeLocalConflictResolution::new(app_state, coordinator, baselines),
    )
}

fn setup() -> (tempfile::TempDir, TestHomeGuard, AppState) {
    let temp = tempfile::tempdir().unwrap();
    #[cfg(target_os = "windows")]
    if let Some(wsl_test_root) = std::env::var_os("CC_SWITCH_WSL_TEST_DIR") {
        assert!(
            temp.path()
                .starts_with(std::path::Path::new(&wsl_test_root)),
            "native Windows contract temp {} must remain below WSL UNC root {}",
            temp.path().display(),
            std::path::Path::new(&wsl_test_root).display()
        );
    }
    let guard = TestHomeGuard::set(temp.path());
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database);
    state
        .db
        .save_mcp_server(&McpServer {
            id: "fixture".to_string(),
            name: "Fixture".to_string(),
            server: json!({ "command": "before" }),
            apps: McpApps {
                claude: true,
                codex: false,
                opencode: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        })
        .unwrap();
    McpService::sync_enabled_for_app(&state, ManagedClientId::Claude).unwrap();
    (temp, guard, state)
}

fn write_external_change() {
    let path = wsl_code_switch_lib::get_claude_mcp_path();
    std::fs::write(path, br#"{"mcpServers":{"fixture":{"command":"after"}}}"#).unwrap();
}

fn skill_dir(
    home: &std::path::Path,
    client: ManagedClientId,
    directory: &str,
) -> std::path::PathBuf {
    match client {
        ManagedClientId::Claude => home.join(".claude/skills").join(directory),
        ManagedClientId::Codex => home.join(".codex/skills").join(directory),
        ManagedClientId::Opencode => home.join(".config/opencode/skills").join(directory),
    }
}

fn write_skill(home: &std::path::Path, client: ManagedClientId, directory: &str, body: &str) {
    let root = skill_dir(home, client, directory);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {directory}\ndescription: Conflict fixture\n---\n{body}\n"),
    )
    .unwrap();
}

#[test]
#[serial]
fn accepting_external_mcp_updates_only_database_and_clears_the_pending_item() {
    let (_temp, _guard, state) = setup();
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(target());
    write_external_change();
    coordinator.rescan_target(target());
    let (source, resolver) = source_and_resolver(&state, coordinator.clone(), baselines.clone());
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].record_id.as_deref(), Some("fixture"));
    assert!(listed[0]
        .actions
        .contains(&ConflictResolutionAction::AcceptExternal));
    let live_before = std::fs::read(wsl_code_switch_lib::get_claude_mcp_path()).unwrap();
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        10,
        &ConflictResolutionRequest {
            item_id: listed[0].item_id.clone(),
            action: ConflictResolutionAction::AcceptExternal,
        },
    )
    .unwrap();

    assert_eq!(
        state.db.get_all_mcp_servers().unwrap()["fixture"].server["command"],
        "after"
    );
    assert_eq!(
        std::fs::read(wsl_code_switch_lib::get_claude_mcp_path()).unwrap(),
        live_before
    );
    let remaining = list_conflict_center_items(&[&source], &resolver).unwrap();
    assert!(remaining.is_empty(), "remaining items: {remaining:#?}");
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 1);
}

#[test]
#[serial]
fn accepting_one_external_mcp_keeps_other_pending_records_visible() {
    let (_temp, _guard, state) = setup();
    state
        .db
        .save_mcp_server(&McpServer {
            id: "second".to_string(),
            name: "Second".to_string(),
            server: json!({ "command": "before-second" }),
            apps: McpApps {
                claude: true,
                codex: false,
                opencode: false,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        })
        .unwrap();
    McpService::sync_enabled_for_app(&state, ManagedClientId::Claude).unwrap();
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(target());
    std::fs::write(
        wsl_code_switch_lib::get_claude_mcp_path(),
        br#"{"mcpServers":{"fixture":{"command":"after"},"second":{"command":"after-second"}}}"#,
    )
    .unwrap();
    coordinator.rescan_target(target());
    let (source, resolver) = source_and_resolver(&state, coordinator, baselines);
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    let fixture = listed
        .iter()
        .find(|item| item.record_id.as_deref() == Some("fixture"))
        .unwrap();
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        15,
        &ConflictResolutionRequest {
            item_id: fixture.item_id.clone(),
            action: ConflictResolutionAction::AcceptExternal,
        },
    )
    .unwrap();

    let remaining = list_conflict_center_items(&[&source], &resolver).unwrap();
    assert_eq!(remaining.len(), 1, "remaining items: {remaining:#?}");
    assert_eq!(remaining[0].record_id.as_deref(), Some("second"));
    assert_eq!(
        state.db.get_all_mcp_servers().unwrap()["second"].server["command"],
        "before-second"
    );
}

#[test]
#[serial]
fn keeping_missing_local_opencode_provider_removes_only_the_external_live_record() {
    let (_temp, _guard, state) = setup();
    let provider_target = target_for(LocalScanDomain::Provider, ManagedClientId::Opencode);
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(provider_target);
    let live = runtime_adapter(ManagedClientId::Opencode);
    live.write(&LiveProviderRecord {
        client_id: ManagedClientId::Opencode,
        provider_id: "external-only".to_string(),
        category: Some("custom".to_string()),
        settings: json!({
            "npm": "@ai-sdk/openai-compatible",
            "options": { "baseURL": "https://example.invalid/v1" }
        }),
    })
    .unwrap();
    coordinator.rescan_target(provider_target);
    let (source, resolver) = source_and_resolver(&state, coordinator, baselines);
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    let item = listed
        .iter()
        .find(|item| item.record_id.as_deref() == Some("external-only"))
        .unwrap();
    assert!(item.actions.contains(&ConflictResolutionAction::KeepLocal));
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        30,
        &ConflictResolutionRequest {
            item_id: item.item_id.clone(),
            action: ConflictResolutionAction::KeepLocal,
        },
    )
    .unwrap();

    assert!(!live.contains("external-only").unwrap());
    assert!(state.db.get_all_providers("opencode").unwrap().is_empty());
    assert!(list_conflict_center_items(&[&source], &resolver)
        .unwrap()
        .is_empty());
}

#[test]
#[serial]
fn keeping_local_skill_restores_the_confirmed_tree_from_another_enabled_client() {
    let (temp, _guard, state) = setup();
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "shared-skill",
        "confirmed body",
    );
    LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "shared-skill".to_string(),
            source_client: ManagedClientId::Claude,
            apps: ManagedClientApps {
                claude: true,
                codex: true,
                opencode: false,
            },
        }],
    )
    .unwrap();
    let skill_target = target_for(LocalScanDomain::Skill, ManagedClientId::Claude);
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(skill_target);
    write_skill(
        temp.path(),
        ManagedClientId::Claude,
        "shared-skill",
        "third-party edit",
    );
    coordinator.rescan_target(skill_target);
    let (source, resolver) = source_and_resolver(&state, coordinator, baselines);
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    let item = listed
        .iter()
        .find(|item| item.record_id.as_deref() == Some("shared-skill"))
        .unwrap();
    assert!(item.actions.contains(&ConflictResolutionAction::KeepLocal));
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        40,
        &ConflictResolutionRequest {
            item_id: item.item_id.clone(),
            action: ConflictResolutionAction::KeepLocal,
        },
    )
    .unwrap();

    assert_eq!(
        std::fs::read(
            skill_dir(temp.path(), ManagedClientId::Claude, "shared-skill").join("SKILL.md")
        )
        .unwrap(),
        std::fs::read(
            skill_dir(temp.path(), ManagedClientId::Codex, "shared-skill").join("SKILL.md")
        )
        .unwrap()
    );
    assert!(list_conflict_center_items(&[&source], &resolver)
        .unwrap()
        .is_empty());
}

#[test]
#[serial]
fn one_database_projection_failure_does_not_hide_another_targets_pending_item() {
    let (_temp, _guard, state) = setup();
    state
        .db
        .save_provider(
            "opencode",
            &Provider {
                id: "broken".to_string(),
                name: "Broken".to_string(),
                settings_config: json!("not-an-object"),
                website_url: None,
                category: Some("custom".to_string()),
                created_at: Some(1),
                sort_index: None,
                notes: None,
                meta: Some(ProviderMeta {
                    live_config_managed: Some(true),
                    ..ProviderMeta::default()
                }),
                icon: None,
                icon_color: None,
            },
        )
        .unwrap();
    let provider_target = target_for(LocalScanDomain::Provider, ManagedClientId::Opencode);
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(provider_target);
    coordinator.rescan_target(target());
    runtime_adapter(ManagedClientId::Opencode)
        .write(&LiveProviderRecord {
            client_id: ManagedClientId::Opencode,
            provider_id: "broken".to_string(),
            category: Some("custom".to_string()),
            settings: json!({
                "npm": "@ai-sdk/openai-compatible",
                "options": { "baseURL": "https://example.invalid/v1" }
            }),
        })
        .unwrap();
    write_external_change();
    coordinator.rescan_target(provider_target);
    coordinator.rescan_target(target());
    let states = Arc::new(DatabaseLocalReconciliationStateAdapter::new(
        state.db.clone(),
        baselines,
    ));
    let source = LocalScanConflictSource::new(coordinator, states);

    let listed = source.list_pending().unwrap();

    assert_eq!(listed.len(), 2, "pending items: {listed:#?}");
    assert!(listed.iter().any(|item| {
        item.domain == wsl_code_switch_lib::domain::PortableDomain::Provider
            && item.client_id == Some(ManagedClientId::Opencode)
            && item.record_id.is_none()
            && item.disposition
                == ConflictCenterDisposition::Conflict(LocalConflictKind::IntegrityMismatch)
    }));
    assert!(listed.iter().any(|item| {
        item.domain == wsl_code_switch_lib::domain::PortableDomain::Mcp
            && item.client_id == Some(ManagedClientId::Claude)
            && item.record_id.as_deref() == Some("fixture")
    }));
}

#[test]
#[serial]
fn keeping_local_mcp_rewrites_only_target_live_and_clears_the_pending_item() {
    let (_temp, _guard, state) = setup();
    let (coordinator, baselines) = runtime(&state);
    coordinator.rescan_target(target());
    write_external_change();
    coordinator.rescan_target(target());
    let (source, resolver) = source_and_resolver(&state, coordinator, baselines);
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        20,
        &ConflictResolutionRequest {
            item_id: listed[0].item_id.clone(),
            action: ConflictResolutionAction::KeepLocal,
        },
    )
    .unwrap();

    let parsed = FixedLocalScanParserAdapter::runtime()
        .parse_changed(target())
        .unwrap();
    assert_eq!(parsed.records[0].value["command"], "before");
    assert_eq!(
        state.db.get_all_mcp_servers().unwrap()["fixture"].server["command"],
        "before"
    );
    assert!(list_conflict_center_items(&[&source], &resolver)
        .unwrap()
        .is_empty());
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 1);
}

#[test]
fn production_conflict_ipc_is_registered_once_and_has_no_deleted_sync_dependency() {
    let commands = include_str!("../src/commands/conflict_center.rs").to_ascii_lowercase();
    let lib = include_str!("../src/lib.rs").to_ascii_lowercase();
    for command in [
        "commands::list_conflict_center_items_command",
        "commands::resolve_conflict_center_item_command",
    ] {
        assert_eq!(lib.matches(command).count(), 1, "missing {command}");
    }
    for forbidden in ["s3", "webdav_auto_sync", "proxyservice", "profile"] {
        assert!(
            !commands.contains(forbidden),
            "conflict IPC gained deleted dependency: {forbidden}"
        );
    }
}
