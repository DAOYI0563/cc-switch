use std::sync::Arc;

use serde_json::json;
use serial_test::serial;
use wsl_code_switch_lib::adapters::local_reconciliation_state::DatabaseLocalReconciliationStateAdapter;
use wsl_code_switch_lib::domain::{
    LocalReconciliationRecord, LocalScanDomain, LocalScanTarget, LocalSkill, ManagedClientApps,
    ManagedClientId, PromptVersion,
};
use wsl_code_switch_lib::ports::{LocalReconciliationBaselinePort, LocalReconciliationStatePort};
use wsl_code_switch_lib::{
    AppState, Database, InMemoryLocalReconciliationBaselines, McpApps, McpServer, Provider,
    ProviderMeta,
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

fn target(domain: LocalScanDomain, client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget { domain, client_id }
}

fn provider(id: &str, settings: serde_json::Value, live_managed: bool) -> Provider {
    Provider {
        id: id.to_string(),
        name: id.to_string(),
        settings_config: settings,
        website_url: None,
        category: Some("custom".to_string()),
        created_at: Some(1),
        sort_index: None,
        notes: None,
        meta: Some(ProviderMeta {
            live_config_managed: Some(live_managed),
            ..ProviderMeta::default()
        }),
        icon: None,
        icon_color: None,
    }
}

#[test]
#[serial]
fn database_projection_covers_all_twelve_targets_and_keeps_baselines_independent() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    let database = Arc::new(Database::init().unwrap());
    let state = AppState::new(database.clone());

    state
        .db
        .save_provider(
            "claude",
            &provider("claude-current", json!({ "model": "c" }), true),
        )
        .unwrap();
    state
        .db
        .set_current_provider("claude", "claude-current")
        .unwrap();
    state
        .db
        .save_provider(
            "codex",
            &provider(
                "codex-current",
                json!({ "auth": {}, "config": "model = 'c'" }),
                true,
            ),
        )
        .unwrap();
    state
        .db
        .set_current_provider("codex", "codex-current")
        .unwrap();
    state
        .db
        .save_provider(
            "opencode",
            &provider(
                "vendor",
                json!({ "npm": "@ai-sdk/openai-compatible", "options": {} }),
                true,
            ),
        )
        .unwrap();

    state
        .db
        .save_mcp_server(&McpServer {
            id: "fixture-mcp".to_string(),
            name: "Fixture MCP".to_string(),
            server: json!({ "command": "echo" }),
            apps: McpApps {
                claude: true,
                codex: true,
                opencode: true,
            },
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        })
        .unwrap();

    for client in ManagedClientId::ALL {
        let prompt = state
            .db
            .prepare_prompt_version(
                client,
                PromptVersion {
                    id: format!("prompt-{}", client.as_str()),
                    name: "Fixture".to_string(),
                    version: 0,
                    content: format!("{} prompt", client.as_str()),
                    description: None,
                    enabled: true,
                    created_at: None,
                    updated_at: None,
                },
            )
            .unwrap();
        state.db.save_prompt_version(client, &prompt).unwrap();
    }

    state
        .db
        .save_core_skills(&[LocalSkill {
            id: "fixture-skill".to_string(),
            name: "Fixture Skill".to_string(),
            description: None,
            directory: "fixture-skill".to_string(),
            content_hash: Some("a".repeat(64)),
            total_size_bytes: 10,
            file_count: 1,
            apps: ManagedClientApps {
                claude: true,
                codex: true,
                opencode: true,
            },
            cloud_eligible: true,
            created_at_ms: 1,
            updated_at_ms: 1,
        }])
        .unwrap();

    let baselines = Arc::new(InMemoryLocalReconciliationBaselines::default());
    baselines
        .confirm_record(
            target(LocalScanDomain::Mcp, ManagedClientId::Claude),
            "fixture-mcp",
            Some(&"b".repeat(64)),
        )
        .unwrap();
    let adapter = DatabaseLocalReconciliationStateAdapter::new(database, baselines);

    for domain in LocalScanDomain::ALL {
        for client_id in ManagedClientId::ALL {
            let state = adapter
                .read_reconciliation_state(target(domain, client_id))
                .unwrap();
            assert_eq!(state.target, target(domain, client_id));
            assert!(!state.local.records.is_empty(), "{domain:?}/{client_id:?}");
            if domain == LocalScanDomain::Mcp && client_id == ManagedClientId::Claude {
                assert_eq!(
                    state.baseline.unwrap().records,
                    vec![LocalReconciliationRecord::new("fixture-mcp", "b".repeat(64)).unwrap()]
                );
            } else {
                assert!(state.baseline.is_none());
            }
        }
    }
}

#[test]
fn production_projection_is_read_only_and_has_no_live_or_network_dependency() {
    let source = include_str!("../src/adapters/local_reconciliation_state.rs").to_ascii_lowercase();
    for forbidden in [
        "atomic_write",
        "remove_file",
        "reqwest",
        "webdav",
        "temporaryrollbackstore",
    ] {
        assert!(
            !source.contains(forbidden),
            "database projection gained forbidden dependency: {forbidden}"
        );
    }
}
