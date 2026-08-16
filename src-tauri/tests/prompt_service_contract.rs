mod support;

use std::fs;

use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};
use wsl_code_switch_lib::domain::ManagedClientId;
use wsl_code_switch_lib::{Prompt, PromptService};

fn prompt(id: &str, name: &str, content: &str, enabled: bool) -> Prompt {
    Prompt {
        id: id.to_string(),
        name: name.to_string(),
        version: 0,
        content: content.to_string(),
        description: None,
        enabled,
        created_at: None,
        updated_at: None,
    }
}

fn prompt_path(client: ManagedClientId) -> std::path::PathBuf {
    let home = ensure_test_home();
    match client {
        ManagedClientId::Claude => home.join(".claude/CLAUDE.md"),
        ManagedClientId::Codex => home.join(".codex/AGENTS.md"),
        ManagedClientId::Opencode => home.join(".config/opencode/AGENTS.md"),
    }
}

#[test]
fn three_clients_keep_independent_versions_and_live_files() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let state = create_test_state().expect("state");

    let claude_v1 = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "claude-v1",
        prompt("claude-v1", "工作约定", "claude one", false),
    )
    .expect("create Claude v1");
    let claude_v2 = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "claude-v2",
        prompt("claude-v2", "工作约定", "claude two", false),
    )
    .expect("create Claude v2");
    let codex_v1 = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Codex,
        "codex-v1",
        prompt("codex-v1", "工作约定", "codex one", false),
    )
    .expect("create Codex v1");
    let opencode_v1 = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Opencode,
        "opencode-v1",
        prompt("opencode-v1", "工作约定", "opencode one", false),
    )
    .expect("create OpenCode v1");

    assert_eq!((claude_v1.version, claude_v2.version), (1, 2));
    assert_eq!(codex_v1.version, 1);
    assert_eq!(opencode_v1.version, 1);

    PromptService::enable_prompt(&state, ManagedClientId::Claude, "claude-v2")
        .expect("enable Claude v2");
    PromptService::enable_prompt(&state, ManagedClientId::Codex, "codex-v1")
        .expect("enable Codex v1");
    PromptService::enable_prompt(&state, ManagedClientId::Opencode, "opencode-v1")
        .expect("enable OpenCode v1");

    assert_eq!(state.local_scan_writes.pending_count(), 3);
    assert_eq!(state.local_scan_writes.last_generation(), 3);

    assert_eq!(
        fs::read_to_string(prompt_path(ManagedClientId::Claude)).unwrap(),
        "claude two"
    );
    assert_eq!(
        fs::read_to_string(prompt_path(ManagedClientId::Codex)).unwrap(),
        "codex one"
    );
    assert_eq!(
        fs::read_to_string(prompt_path(ManagedClientId::Opencode)).unwrap(),
        "opencode one"
    );

    let claude = PromptService::get_prompts(&state, ManagedClientId::Claude).unwrap();
    let codex = PromptService::get_prompts(&state, ManagedClientId::Codex).unwrap();
    let opencode = PromptService::get_prompts(&state, ManagedClientId::Opencode).unwrap();
    assert_eq!(claude.len(), 2);
    assert_eq!(codex.len(), 1);
    assert_eq!(opencode.len(), 1);
    assert!(claude["claude-v2"].enabled);
    assert!(codex["codex-v1"].enabled);
    assert!(opencode["opencode-v1"].enabled);
}

#[test]
fn version_limit_is_per_client_and_name() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let state = create_test_state().expect("state");

    for index in 1..=20 {
        let id = format!("claude-{index}");
        let stored = PromptService::upsert_prompt(
            &state,
            ManagedClientId::Claude,
            &id,
            prompt(&id, "同名版本", &format!("content {index}"), false),
        )
        .expect("version within limit");
        assert_eq!(stored.version, index);
    }

    let error = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "claude-21",
        prompt("claude-21", "同名版本", "too many", false),
    )
    .expect_err("the twenty-first version must be rejected");
    assert!(error.to_string().contains("20"));

    let other_name = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "claude-other-name",
        prompt("claude-other-name", "另一个名称", "allowed", false),
    )
    .expect("a separate name has its own limit");
    let other_client = PromptService::upsert_prompt(
        &state,
        ManagedClientId::Codex,
        "codex-same-name",
        prompt("codex-same-name", "同名版本", "allowed", false),
    )
    .expect("a separate client has its own limit");
    assert_eq!(other_name.version, 1);
    assert_eq!(other_client.version, 1);
}

#[test]
fn external_live_change_blocks_switch_without_persisting_or_overwriting() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let state = create_test_state().expect("state");

    PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "first",
        prompt("first", "默认", "managed first", true),
    )
    .expect("seed active prompt");
    PromptService::upsert_prompt(
        &state,
        ManagedClientId::Claude,
        "second",
        prompt("second", "候选", "managed second", false),
    )
    .expect("seed inactive prompt");

    let path = prompt_path(ManagedClientId::Claude);
    fs::write(&path, b"external bytes\r\n").expect("external edit");
    let before = PromptService::get_prompts(&state, ManagedClientId::Claude).unwrap();
    let generation_before = state.local_scan_writes.last_generation();

    let error = PromptService::enable_prompt(&state, ManagedClientId::Claude, "second")
        .expect_err("unmanaged live change must stop the switch");
    assert!(error.to_string().contains("导入"));
    assert_eq!(fs::read(&path).unwrap(), b"external bytes\r\n");
    assert_eq!(
        PromptService::get_prompts(&state, ManagedClientId::Claude).unwrap(),
        before
    );
    assert_eq!(state.local_scan_writes.last_generation(), generation_before);
}

#[test]
fn manual_import_creates_versions_without_writing_live_and_manual_sync_is_explicit() {
    let _guard = test_mutex().lock().expect("test lock");
    reset_test_fs();
    let state = create_test_state().expect("state");
    let path = prompt_path(ManagedClientId::Opencode);
    fs::create_dir_all(path.parent().unwrap()).expect("OpenCode root");
    fs::write(&path, b"external one\r\n").expect("seed live file");

    let first_id = PromptService::import_from_file(&state, ManagedClientId::Opencode)
        .expect("import first live version");
    fs::write(&path, b"external two\n").expect("second external edit");
    let second_id = PromptService::import_from_file(&state, ManagedClientId::Opencode)
        .expect("import second live version");

    let versions = PromptService::get_prompts(&state, ManagedClientId::Opencode).unwrap();
    assert_eq!(versions[&first_id].version, 1);
    assert_eq!(versions[&first_id].content, "external one\r\n");
    assert_eq!(versions[&second_id].version, 2);
    assert_eq!(versions[&second_id].content, "external two\n");
    assert!(!versions[&first_id].enabled);
    assert!(!versions[&second_id].enabled);
    assert_eq!(fs::read(&path).unwrap(), b"external two\n");

    PromptService::enable_prompt(&state, ManagedClientId::Opencode, &second_id)
        .expect("adopt the current live version");
    fs::write(&path, b"third-party replacement").expect("third-party change");
    PromptService::sync_to_live(&state, ManagedClientId::Opencode)
        .expect("explicit sync replaces live with active content");
    assert_eq!(fs::read(&path).unwrap(), b"external two\n");
}
