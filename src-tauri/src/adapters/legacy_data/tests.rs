use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::Connection;

use super::*;
use crate::domain::LegacySourceKind;
use crate::ports::LegacyDataErrorCode;

const V16_FIXTURE: &str = include_str!("../../../tests/fixtures/v16/cc-switch-v16.sql");

#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeEntry {
    is_dir: bool,
    bytes: Vec<u8>,
    modified: SystemTime,
}

fn create_v16_database(home: &Path) -> PathBuf {
    let root = home.join(LEGACY_DIRECTORY);
    fs::create_dir_all(&root).expect("create legacy directory");
    let path = root.join(DATABASE_FILE);
    let connection = Connection::open(&path).expect("create fixture database");
    connection
        .execute_batch(V16_FIXTURE)
        .expect("load v16 fixture");
    drop(connection);
    path
}

fn tree_snapshot(root: &Path) -> BTreeMap<PathBuf, TreeEntry> {
    fn visit(base: &Path, current: &Path, output: &mut BTreeMap<PathBuf, TreeEntry>) {
        let mut entries = fs::read_dir(current)
            .expect("read snapshot directory")
            .collect::<Result<Vec<_>, _>>()
            .expect("read snapshot entry");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("snapshot metadata");
            let relative = path.strip_prefix(base).expect("relative snapshot path");
            let is_dir = metadata.is_dir();
            let bytes = if metadata.is_file() {
                fs::read(&path).expect("snapshot file bytes")
            } else {
                Vec::new()
            };
            output.insert(
                relative.to_path_buf(),
                TreeEntry {
                    is_dir,
                    bytes,
                    modified: metadata.modified().expect("snapshot modified time"),
                },
            );
            if is_dir {
                visit(base, &path, output);
            }
        }
    }

    let mut output = BTreeMap::new();
    if root.is_dir() {
        visit(root, root, &mut output);
    }
    output
}

#[test]
fn missing_and_empty_legacy_directories_are_distinct() {
    let home = tempfile::tempdir().expect("create fixture home");
    let source = FixedLegacyDataSource::from_home(home.path());
    assert_eq!(
        source.preview().expect("preview missing source").status,
        LegacyMigrationStatus::NotFound
    );
    fs::create_dir(source.root()).expect("create empty legacy source");
    let preview = source.preview().expect("preview empty source");
    assert_eq!(preview.status, LegacyMigrationStatus::Empty);
    assert!(preview.source.is_none());
    assert_eq!(preview.files, Vec::new());
    assert_eq!(
        preview.directory_fingerprint.as_deref().map(str::len),
        Some(64)
    );
}

#[test]
fn v16_database_preview_is_content_free_and_byte_for_byte_read_only() {
    let home = tempfile::tempdir().expect("create fixture home");
    create_v16_database(home.path());
    let source = FixedLegacyDataSource::from_home(home.path());
    let before = tree_snapshot(source.root());

    let preview = source.preview().expect("preview v16 database");

    assert_eq!(preview.status, LegacyMigrationStatus::Ready);
    assert_eq!(preview.source, Some(LegacySourceKind::Sqlite));
    assert_eq!(preview.source_version, Some(16));
    assert_eq!(preview.retained.claude_providers, 1);
    assert_eq!(preview.retained.codex_providers, 1);
    assert_eq!(preview.retained.opencode_providers, 1);
    assert_eq!(preview.retained.mcp_servers, 1);
    assert_eq!(preview.retained.claude_prompts, 1);
    assert_eq!(preview.retained.codex_prompts, 1);
    assert_eq!(preview.retained.opencode_prompts, 0);
    assert_eq!(preview.retained.skills, 1);
    assert_eq!(preview.ignored.non_target_client_records, 2);
    assert_eq!(preview.ignored.profiles, 1);
    assert_eq!(preview.ignored.proxy_and_routing, 1);
    assert_eq!(preview.ignored.usage_and_pricing, 1);
    assert_eq!(preview.files.len(), 1);
    assert_eq!(preview.files[0].name, DATABASE_FILE);
    assert_eq!(preview.files[0].sha256.len(), 64);
    let serialized = serde_json::to_string(&preview).expect("serialize preview");
    for forbidden in [
        "FIXTURE_CLAUDE_TOKEN",
        "FIXTURE_CODEX_TOKEN",
        "FIXTURE_OPENCODE_TOKEN",
        "claude.example.invalid",
    ] {
        assert!(!serialized.contains(forbidden));
    }
    assert_eq!(tree_snapshot(source.root()), before);
    assert!(!source.root().join("cc-switch.db-wal").exists());
    assert!(!source.root().join("cc-switch.db-shm").exists());
    assert!(!source.root().join("cc-switch.db-journal").exists());
}

#[test]
fn json_only_preview_counts_target_and_ignored_domains_without_rewriting() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    fs::write(
        root.join(CONFIG_FILE),
        r#"{
          "version": 2,
          "claude": {"providers": {"c": {"secret": "claude-secret"}}, "current": "c"},
          "codex": {"providers": {"x": {}}, "current": "x"},
          "opencode": {"providers": {"o": {}}, "current": "o"},
          "gemini": {"providers": {"g": {}}, "current": "g"},
          "mcp": {
            "servers": {"shared": {}},
            "claude": {"servers": {"legacy": {}}},
            "gemini": {"servers": {"ignored-mcp": {}}}
          },
          "prompts": {
            "claude": {"prompts": {"p1": {}}},
            "opencode": {"prompts": {"p2": {}}},
            "gemini": {"prompts": {"ignored-prompt": {}}}
          },
          "skills": {"skills": {"local-skill": {}}, "repos": [{"name": "online"}]},
          "common_config_snippets": {
            "claude": "private snippet", "codex": "codex snippet", "gemini": "ignored"
          },
          "profiles": {"profile-a": {}},
          "proxy": {"enabled": true},
          "usage": [{"request": 1}],
          "failoverQueue": ["c"]
        }"#,
    )
    .expect("write legacy config");
    let source = FixedLegacyDataSource::from_home(home.path());
    let before = tree_snapshot(source.root());

    let preview = source.preview().expect("preview legacy JSON");

    assert_eq!(preview.source, Some(LegacySourceKind::Json));
    assert_eq!(preview.source_version, Some(2));
    assert_eq!(preview.retained.claude_providers, 1);
    assert_eq!(preview.retained.codex_providers, 1);
    assert_eq!(preview.retained.opencode_providers, 1);
    assert_eq!(preview.retained.mcp_servers, 2);
    assert_eq!(preview.retained.claude_prompts, 1);
    assert_eq!(preview.retained.opencode_prompts, 1);
    assert_eq!(preview.retained.skills, 1);
    assert_eq!(preview.retained.common_snippets, 2);
    assert_eq!(preview.ignored.non_target_client_records, 4);
    assert_eq!(preview.ignored.profiles, 1);
    assert_eq!(preview.ignored.proxy_and_routing, 1);
    assert_eq!(preview.ignored.usage_and_pricing, 1);
    assert_eq!(preview.ignored.failover, 1);
    assert_eq!(preview.ignored.online_skill_repositories, 1);
    let serialized = serde_json::to_string(&preview).expect("serialize preview");
    assert!(!serialized.contains("claude-secret"));
    assert!(!serialized.contains("private snippet"));
    assert_eq!(tree_snapshot(source.root()), before);
}

#[test]
fn database_takes_precedence_over_stale_json_without_double_counting() {
    let home = tempfile::tempdir().expect("create fixture home");
    create_v16_database(home.path());
    fs::write(
        home.path().join(LEGACY_DIRECTORY).join(CONFIG_FILE),
        r#"{"version":2,"claude":{"providers":{"stale":{}},"current":"stale"}}"#,
    )
    .expect("write stale JSON");

    let preview = FixedLegacyDataSource::from_home(home.path())
        .preview()
        .expect("preview preferred database");
    assert_eq!(preview.source, Some(LegacySourceKind::Sqlite));
    assert_eq!(preview.retained.claude_providers, 1);
    assert_eq!(preview.files.len(), 2);
}

#[test]
fn retained_load_rejects_changed_fingerprint_without_writing_the_source() {
    let home = tempfile::tempdir().expect("create fixture home");
    create_v16_database(home.path());
    let source = FixedLegacyDataSource::from_home(home.path());
    source.preview().expect("preview source");
    let before = tree_snapshot(source.root());

    let error = match source.load_retained(&"f".repeat(64)) {
        Ok(_) => panic!("stale fingerprint must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code, LegacyDataErrorCode::SourceChanged);
    assert_eq!(tree_snapshot(source.root()), before);
}

#[test]
fn retained_load_rejects_bad_record_and_creates_no_sqlite_sidecars() {
    let home = tempfile::tempdir().expect("create fixture home");
    let database = create_v16_database(home.path());
    let connection = Connection::open(&database).expect("open fixture database");
    connection
        .execute(
            "UPDATE providers SET settings_config = '{broken' WHERE id = 'fixture-claude'",
            [],
        )
        .expect("corrupt retained row");
    drop(connection);
    let source = FixedLegacyDataSource::from_home(home.path());
    let preview = source.preview().expect("preview counts without bodies");
    let fingerprint = preview.directory_fingerprint.unwrap();
    let before = tree_snapshot(source.root());

    let error = match source.load_retained(&fingerprint) {
        Ok(_) => panic!("corrupt retained row must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.code, LegacyDataErrorCode::InvalidRecord);
    assert_eq!(tree_snapshot(source.root()), before);
    for suffix in ["wal", "shm", "journal"] {
        assert!(!source
            .root()
            .join(format!("cc-switch.db-{suffix}"))
            .exists());
    }
}

#[test]
fn invalid_external_skills_json_fails_closed_during_preview_and_load() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    fs::write(root.join(CONFIG_FILE), br#"{"version":2}"#).expect("write config");
    fs::write(root.join(SKILLS_FILE), b"{broken").expect("write broken skills");
    let source = FixedLegacyDataSource::from_home(home.path());
    let before = tree_snapshot(source.root());

    let error = source.preview().expect_err("reject broken skills JSON");

    assert_eq!(error.code, LegacyDataErrorCode::InvalidJson);
    assert_eq!(tree_snapshot(source.root()), before);
}

#[test]
fn corrupt_database_fails_closed_without_creating_sidecars() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    fs::write(root.join(DATABASE_FILE), b"not a sqlite database").expect("write corrupt database");
    let source = FixedLegacyDataSource::from_home(home.path());
    let before = tree_snapshot(source.root());

    let error = source.preview().expect_err("reject corrupt database");

    assert_eq!(error.code, LegacyDataErrorCode::InvalidDatabase);
    assert_eq!(tree_snapshot(source.root()), before);
}

#[test]
fn future_database_version_fails_closed() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    let connection = Connection::open(root.join(DATABASE_FILE)).expect("create future DB");
    connection
        .execute_batch("CREATE TABLE marker (id INTEGER); PRAGMA user_version = 17;")
        .expect("seed future DB");
    drop(connection);

    let error = FixedLegacyDataSource::from_home(home.path())
        .preview()
        .expect_err("reject future database");
    assert_eq!(error.code, LegacyDataErrorCode::UnsupportedVersion);
    assert_eq!(error.context.get("version").map(String::as_str), Some("17"));
}

#[test]
fn nonempty_wal_fails_before_opening_database() {
    let home = tempfile::tempdir().expect("create fixture home");
    create_v16_database(home.path());
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::write(
        root.join("cc-switch.db-wal"),
        b"pending fixture transaction",
    )
    .expect("write nonempty WAL");
    let source = FixedLegacyDataSource::from_home(home.path());
    let before = tree_snapshot(source.root());

    let error = source.preview().expect_err("reject nonempty WAL");

    assert_eq!(error.code, LegacyDataErrorCode::PendingDatabaseChanges);
    assert_eq!(
        error.context.get("file").map(String::as_str),
        Some("cc-switch.db-wal")
    );
    assert_eq!(tree_snapshot(source.root()), before);
}

#[test]
fn invalid_or_future_json_fails_closed() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    fs::write(root.join(CONFIG_FILE), b"{broken").expect("write invalid JSON");
    let source = FixedLegacyDataSource::from_home(home.path());
    assert_eq!(
        source.preview().expect_err("reject invalid JSON").code,
        LegacyDataErrorCode::InvalidJson
    );
    fs::write(root.join(CONFIG_FILE), br#"{"version":3}"#).expect("write future JSON");
    assert_eq!(
        source.preview().expect_err("reject future JSON").code,
        LegacyDataErrorCode::UnsupportedVersion
    );
}

#[test]
fn v1_json_shape_takes_precedence_over_a_misleading_version_field() {
    let home = tempfile::tempdir().expect("create fixture home");
    let root = home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create legacy directory");
    fs::write(
        root.join(CONFIG_FILE),
        br#"{"providers":{"legacy":{}},"current":"legacy","version":2}"#,
    )
    .expect("write v1-shaped JSON");

    let preview = FixedLegacyDataSource::from_home(home.path())
        .preview()
        .expect("preview v1-shaped JSON");

    assert_eq!(preview.source_version, Some(1));
    assert_eq!(preview.retained.claude_providers, 1);
}

#[cfg(unix)]
#[test]
fn symlinked_legacy_root_and_source_file_are_rejected() {
    use std::os::unix::fs::symlink;

    let home = tempfile::tempdir().expect("create fixture home");
    let outside = tempfile::tempdir().expect("create outside directory");
    symlink(outside.path(), home.path().join(LEGACY_DIRECTORY))
        .expect("create legacy root symlink");
    let root_error = FixedLegacyDataSource::from_home(home.path())
        .preview()
        .expect_err("reject linked root");
    assert_eq!(root_error.code, LegacyDataErrorCode::LinkNotAllowed);

    let second_home = tempfile::tempdir().expect("create second fixture home");
    let root = second_home.path().join(LEGACY_DIRECTORY);
    fs::create_dir(&root).expect("create real legacy root");
    let outside_file = outside.path().join(CONFIG_FILE);
    fs::write(&outside_file, b"{}").expect("write outside file");
    symlink(&outside_file, root.join(CONFIG_FILE)).expect("create source symlink");
    let file_error = FixedLegacyDataSource::from_home(second_home.path())
        .preview()
        .expect_err("reject linked source file");
    assert_eq!(file_error.code, LegacyDataErrorCode::LinkNotAllowed);
}

#[cfg(windows)]
#[test]
fn windows_junction_legacy_root_is_rejected() {
    let home = tempfile::tempdir().expect("create fixture home");
    let outside = tempfile::tempdir().expect("create junction target");
    let link = home.path().join(LEGACY_DIRECTORY);
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .expect("invoke mklink");
    assert!(
        output.status.success(),
        "create test junction: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let error = FixedLegacyDataSource::from_home(home.path())
        .preview()
        .expect_err("reject Windows junction");

    assert_eq!(error.code, LegacyDataErrorCode::LinkNotAllowed);
}
