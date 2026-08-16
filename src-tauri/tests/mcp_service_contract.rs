mod support;

use std::fs;

use serde_json::json;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};
use wsl_code_switch_lib::domain::ManagedClientId;
use wsl_code_switch_lib::{McpApps, McpServer, McpService};

fn server(id: &str, apps: McpApps) -> McpServer {
    McpServer {
        id: id.to_string(),
        name: "Fixture MCP".to_string(),
        server: json!({
            "type": "stdio",
            "command": "new-command",
            "args": ["--new"]
        }),
        apps,
        description: Some("fixture".to_string()),
        homepage: None,
        docs: None,
        tags: vec!["contract".to_string()],
    }
}

#[test]
fn multi_client_upsert_rolls_back_live_and_leaves_database_unchanged() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("Claude root");
    fs::create_dir_all(home.join(".codex")).expect("Codex root");

    let claude_path = home.join(".claude.json");
    let claude_before = br#"{"mcpServers":{"keep":{"command":"keep"}},"future":true}"#;
    fs::write(&claude_path, claude_before).expect("seed Claude");
    let codex_path = home.join(".codex/config.toml");
    let codex_before = b"model = \"gpt-5\"\ninvalid = [\n";
    fs::write(&codex_path, codex_before).expect("seed invalid Codex");

    let state = create_test_state().expect("state");
    let error = McpService::upsert_server(
        &state,
        server(
            "transactional",
            McpApps {
                claude: true,
                codex: true,
                opencode: false,
            },
        ),
    )
    .expect_err("Codex parse failure must abort the operation");

    assert!(error.to_string().contains("config.toml"));
    assert!(state.db.get_all_mcp_servers().unwrap().is_empty());
    assert_eq!(fs::read(claude_path).unwrap(), claude_before);
    assert_eq!(fs::read(codex_path).unwrap(), codex_before);
    assert_eq!(state.local_scan_writes.pending_count(), 0);
    assert_eq!(state.local_scan_writes.last_generation(), 0);
}

#[test]
fn failed_toggle_and_delete_preserve_the_authoritative_database_row() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".codex")).expect("Codex root");
    let codex_path = home.join(".codex/config.toml");
    let broken = b"invalid = [\n";
    fs::write(&codex_path, broken).expect("seed invalid Codex");

    let state = create_test_state().expect("state");
    let disabled = server(
        "fixture",
        McpApps {
            claude: false,
            codex: false,
            opencode: false,
        },
    );
    state.db.save_mcp_server(&disabled).expect("seed database");

    McpService::toggle_app(&state, "fixture", ManagedClientId::Codex, true)
        .expect_err("invalid live config must reject enable");
    let after_toggle = state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .shift_remove("fixture")
        .expect("row survives toggle");
    assert!(!after_toggle.apps.codex);
    assert_eq!(fs::read(&codex_path).unwrap(), broken);

    let mut enabled = after_toggle;
    enabled.apps.codex = true;
    state
        .db
        .save_mcp_server(&enabled)
        .expect("enable in fixture DB");
    McpService::delete_server(&state, "fixture")
        .expect_err("invalid live config must reject delete");
    assert!(state
        .db
        .get_all_mcp_servers()
        .unwrap()
        .contains_key("fixture"));
    assert_eq!(fs::read(codex_path).unwrap(), broken);
}

#[test]
fn upsert_preserves_root_and_per_server_unknown_fields_in_all_clients() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("Claude root");
    fs::create_dir_all(home.join(".codex")).expect("Codex root");
    fs::create_dir_all(home.join(".config/opencode")).expect("OpenCode root");

    fs::write(
        home.join(".claude.json"),
        serde_json::to_vec_pretty(&json!({
            "rootFuture": { "keep": true },
            "mcpServers": {
                "fixture": {
                    "type": "stdio",
                    "command": "old",
                    "clientFuture": { "nested": [true, 2, "three"] }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        home.join(".codex/config.toml"),
        r#"# keep-comment
model = "gpt-5"

[mcp_servers.fixture]
type = "stdio"
command = "old"
client_future = { nested = { flag = true }, values = [1, 2, 3] }
"#,
    )
    .unwrap();
    fs::write(
        home.join(".config/opencode/opencode.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "theme": "keep",
            "mcp": {
                "fixture": {
                    "type": "local",
                    "command": ["old"],
                    "enabled": true,
                    "clientFuture": { "nested": [true, 2, "three"] }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let state = create_test_state().expect("state");
    McpService::upsert_server(
        &state,
        server(
            "fixture",
            McpApps {
                claude: true,
                codex: true,
                opencode: true,
            },
        ),
    )
    .expect("three-client upsert");

    assert_eq!(state.local_scan_writes.pending_count(), 5);
    assert_eq!(state.local_scan_writes.last_generation(), 5);

    let claude: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(claude["rootFuture"]["keep"], true);
    assert_eq!(claude["mcpServers"]["fixture"]["command"], "new-command");
    assert_eq!(
        claude["mcpServers"]["fixture"]["clientFuture"]["nested"][2],
        "three"
    );

    let codex_text = fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let codex: toml::Value = toml::from_str(&codex_text).unwrap();
    assert!(codex_text.contains("# keep-comment"));
    assert_eq!(codex["model"].as_str(), Some("gpt-5"));
    assert_eq!(
        codex["mcp_servers"]["fixture"]["client_future"]["nested"]["flag"].as_bool(),
        Some(true)
    );
    assert_eq!(
        codex["mcp_servers"]["fixture"]["command"].as_str(),
        Some("new-command")
    );

    let opencode: serde_json::Value =
        serde_json::from_slice(&fs::read(home.join(".config/opencode/opencode.json")).unwrap())
            .unwrap();
    assert_eq!(opencode["theme"], "keep");
    assert_eq!(opencode["mcp"]["fixture"]["command"][0], "new-command");
    assert_eq!(
        opencode["mcp"]["fixture"]["clientFuture"]["nested"][2],
        "three"
    );
}

#[test]
fn invalid_domain_record_writes_neither_database_nor_live_files() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".claude")).expect("Claude root");
    let claude_path = home.join(".claude.json");
    fs::write(&claude_path, b"{\"root\":true}").unwrap();

    let state = create_test_state().expect("state");
    let mut invalid = server(
        "fixture",
        McpApps {
            claude: true,
            codex: false,
            opencode: false,
        },
    );
    invalid.server = json!({ "type": "stdio" });

    McpService::upsert_server(&state, invalid).expect_err("invalid record");
    assert!(state.db.get_all_mcp_servers().unwrap().is_empty());
    assert_eq!(fs::read(claude_path).unwrap(), b"{\"root\":true}");
}

#[test]
fn codex_and_opencode_import_keep_nested_client_extensions() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let home = ensure_test_home();
    fs::create_dir_all(home.join(".codex")).expect("Codex root");
    fs::create_dir_all(home.join(".config/opencode")).expect("OpenCode root");
    fs::write(
        home.join(".codex/config.toml"),
        r#"[mcp_servers.codex_nested]
type = "stdio"
command = "echo"
future = { nested = { flag = true }, values = [1, 2, 3] }
"#,
    )
    .unwrap();
    fs::write(
        home.join(".config/opencode/opencode.json"),
        serde_json::to_vec_pretty(&json!({
            "mcp": {
                "opencode-nested": {
                    "type": "local",
                    "command": ["echo"],
                    "enabled": true,
                    "future": { "nested": { "flag": true }, "values": [1, 2, 3] }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let state = create_test_state().expect("state");
    assert_eq!(McpService::import_from_codex(&state).unwrap(), 1);
    assert_eq!(McpService::import_from_opencode(&state).unwrap(), 1);
    let records = state.db.get_all_mcp_servers().unwrap();

    assert_eq!(
        records["codex_nested"].server["future"]["nested"]["flag"],
        true
    );
    assert_eq!(records["codex_nested"].server["future"]["values"][2], 3);
    assert_eq!(
        records["opencode-nested"].server["future"]["nested"]["flag"],
        true
    );
    assert_eq!(records["opencode-nested"].server["future"]["values"][2], 3);
}
