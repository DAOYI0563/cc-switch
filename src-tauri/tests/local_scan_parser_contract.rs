use std::collections::HashMap;
use std::fs;

use serial_test::serial;
use wsl_code_switch_lib::adapters::local_scan_parser::FixedLocalScanParserAdapter;
use wsl_code_switch_lib::domain::{
    LocalScanDomain, LocalScanFailureKind, LocalScanTarget, ManagedClientId,
};
use wsl_code_switch_lib::ports::LocalScanParserPort;

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

fn target(domain: LocalScanDomain, client_id: ManagedClientId) -> LocalScanTarget {
    LocalScanTarget { domain, client_id }
}

#[test]
#[serial]
fn fixed_parser_normalizes_all_twelve_targets_without_mutation_or_debug_leaks() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    let fixtures = [
        (
            ".claude/settings.json",
            br#"{"env":{"ANTHROPIC_AUTH_TOKEN":"CLAUDE_PARSE_SECRET"}}"#.as_slice(),
        ),
        (
            ".claude.json",
            br#"{"mcpServers":{"claude-mcp":{"command":"echo","env":{"TOKEN":"MCP_PARSE_SECRET"}}}}"#.as_slice(),
        ),
        (".claude/CLAUDE.md", b"Claude prompt".as_slice()),
        (
            ".claude/skills/example/SKILL.md",
            b"---\nname: Example\ndescription: Claude skill\n---\nBody".as_slice(),
        ),
        (
            ".codex/auth.json",
            br#"{"OPENAI_API_KEY":"CODEX_PARSE_SECRET"}"#.as_slice(),
        ),
        (
            ".codex/config.toml",
            b"model = \"fixture\"\n[mcp_servers.codex-mcp]\ncommand = \"echo\"\n".as_slice(),
        ),
        (
            ".codex/cc-switch-model-catalog.json",
            br#"{"models":[]}"#.as_slice(),
        ),
        (".codex/AGENTS.md", b"Codex prompt".as_slice()),
        (
            ".codex/skills/example/SKILL.md",
            b"---\nname: Example\ndescription: Codex skill\n---\nBody".as_slice(),
        ),
        (
            ".config/opencode/opencode.json",
            br#"{
  "provider":{"vendor":{"npm":"@ai-sdk/openai-compatible","options":{"apiKey":"OPENCODE_PARSE_SECRET"}}},
  "mcp":{"opencode-mcp":{"type":"local","command":["echo"]}}
}"#
            .as_slice(),
        ),
        (
            ".config/opencode/AGENTS.md",
            b"OpenCode prompt".as_slice(),
        ),
        (
            ".config/opencode/skills/example/SKILL.md",
            b"---\nname: Example\ndescription: OpenCode skill\n---\nBody".as_slice(),
        ),
    ];
    for (relative, contents) in fixtures {
        let path = temp.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    let before: HashMap<_, _> = fixtures
        .iter()
        .map(|(relative, _)| {
            (
                (*relative).to_string(),
                fs::read(temp.path().join(relative)).unwrap(),
            )
        })
        .collect();

    let parser = FixedLocalScanParserAdapter::runtime();
    let mut parsed = Vec::new();
    for domain in LocalScanDomain::ALL {
        for client_id in ManagedClientId::ALL {
            let snapshot = parser.parse_changed(target(domain, client_id)).unwrap();
            assert_eq!(snapshot.target, target(domain, client_id));
            assert!(!snapshot.records.is_empty(), "{domain:?}/{client_id:?}");
            parsed.push(snapshot);
        }
    }
    assert_eq!(parsed.len(), 12);
    let debug = format!("{parsed:?}");
    for secret in [
        "CLAUDE_PARSE_SECRET",
        "MCP_PARSE_SECRET",
        "CODEX_PARSE_SECRET",
        "OPENCODE_PARSE_SECRET",
    ] {
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("[REDACTED]"));
    for (relative, contents) in before {
        assert_eq!(fs::read(temp.path().join(relative)).unwrap(), contents);
    }
}

#[test]
#[serial]
fn invalid_full_content_fails_with_only_stable_classification() {
    let temp = tempfile::tempdir().unwrap();
    let _home = TestHomeGuard::set(temp.path());
    let path = temp.path().join(".claude/CLAUDE.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

    let error = FixedLocalScanParserAdapter::runtime()
        .parse_changed(target(LocalScanDomain::Prompt, ManagedClientId::Claude))
        .unwrap_err();
    assert_eq!(error.kind, LocalScanFailureKind::ParseFailed);
    assert_eq!(error.record_id.as_deref(), Some("prompt-live"));
    let encoded = serde_json::to_string(&error).unwrap();
    assert!(!encoded.contains("UTF-8"));
    assert!(!encoded.contains(&path.to_string_lossy().to_string()));
    assert_eq!(fs::read(path).unwrap(), [0xff, 0xfe, 0xfd]);
}

#[test]
fn production_full_parser_has_no_write_or_network_dependency() {
    let source = include_str!("../src/adapters/local_scan_parser.rs").to_ascii_lowercase();
    for forbidden in [
        "atomic_write",
        "remove_file",
        "webdav",
        "reqwest",
        "rusqlite",
        "appstate",
    ] {
        assert!(
            !source.contains(forbidden),
            "full parser gained forbidden dependency: {forbidden}"
        );
    }
}
