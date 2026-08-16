use super::*;
use crate::domain::{
    LegacyCommonSnippetRecord, LegacyMcpRecord, LegacyPromptRecord, LegacyProviderRecord,
    LegacyRetainedSnapshot, LegacySkillRecord, LegacySourceKind,
};

fn snapshot() -> LegacyRetainedSnapshot {
    LegacyRetainedSnapshot {
        source: LegacySourceKind::Sqlite,
        source_version: 16,
        source_fingerprint: "a".repeat(64),
        providers: vec![LegacyProviderRecord {
            id: "provider-a".to_string(),
            client_id: "claude".to_string(),
            name: "Provider A".to_string(),
            settings_config_json: r#"{"env":{"ANTHROPIC_BASE_URL":"https://example.invalid","ANTHROPIC_AUTH_TOKEN":"top-secret"}}"#.to_string(),
            website_url: None,
            category: Some("custom".to_string()),
            created_at_ms: 10,
            sort_index: 0,
            notes: None,
            icon: None,
            icon_color: None,
            meta_json: r#"{"commonConfigEnabled":true,"githubAccountId":"secret-account"}"#.to_string(),
            is_current: true,
        }],
        mcp_servers: vec![LegacyMcpRecord {
            id: "mcp-a".to_string(),
            name: "MCP A".to_string(),
            server_config_json: r#"{"command":"mcp-a"}"#.to_string(),
            description: None,
            homepage: None,
            docs: None,
            tags_json: "[]".to_string(),
            enabled_claude: true,
            enabled_codex: false,
            enabled_opencode: false,
        }],
        prompts: vec![LegacyPromptRecord {
            id: "prompt-a".to_string(),
            client_id: "claude".to_string(),
            name: "CLAUDE.md".to_string(),
            content: "instructions".to_string(),
            description: None,
            enabled: true,
            created_at_ms: 20,
            updated_at_ms: 21,
        }],
        skills: vec![LegacySkillRecord {
            id: "skill-a".to_string(),
            name: "Skill A".to_string(),
            description: None,
            directory: "skill-a".to_string(),
            content_hash: None,
            enabled_claude: true,
            enabled_codex: true,
            enabled_opencode: true,
            created_at_ms: 30,
            updated_at_ms: 31,
        }],
        common_snippets: vec![LegacyCommonSnippetRecord {
            id: "snippet-a".to_string(),
            client_id: "claude".to_string(),
            content: "snippet".to_string(),
        }],
        legacy_settings_json: None,
    }
}

#[test]
fn retained_migration_is_atomic_redacted_and_idempotent() {
    let db = Database::memory().expect("create database");
    let source = snapshot();

    let first = db
        .migrate_retained_snapshot(&source, 100)
        .expect("migrate retained data");
    assert_eq!(first.status, RetainedMigrationStatus::Applied);
    assert_eq!(first.retained.total(), 5);
    assert_eq!(first.content_sha256.len(), 64);

    let conn = db.conn.lock().expect("lock database");
    let (portable, local): (String, String) = conn
        .query_row(
            "SELECT portable_config_json, local_config_json FROM core_providers",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(!portable.contains("top-secret"));
    assert!(!portable.contains("secret-account"));
    assert!(portable.contains("ANTHROPIC_BASE_URL"));
    assert!(local.contains("top-secret"));
    drop(conn);

    let second = db
        .migrate_retained_snapshot(&source, 200)
        .expect("repeat migration");
    assert_eq!(second.status, RetainedMigrationStatus::AlreadyApplied);
    assert_eq!(second.completed_at_ms, 100);
}

#[test]
fn invalid_late_record_rolls_back_every_target_table() {
    let db = Database::memory().expect("create database");
    let mut source = snapshot();
    source.common_snippets.push(LegacyCommonSnippetRecord {
        id: "bad-snippet".to_string(),
        client_id: "opencode".to_string(),
        content: "not supported by schema".to_string(),
    });

    assert!(db.migrate_retained_snapshot(&source, 100).is_err());

    let conn = db.conn.lock().expect("lock database");
    for table in MIGRATED_TABLES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back");
    }
    assert!(load_report(&conn).unwrap().is_none());
}

#[test]
fn existing_target_data_or_different_source_is_never_overwritten() {
    let db = Database::memory().expect("create database");
    let source = snapshot();
    {
        let conn = db.conn.lock().expect("lock database");
        conn.execute(
            "INSERT INTO core_skills (id, name, directory, enabled_claude, enabled_codex,
             enabled_opencode, cloud_eligible, created_at_ms, updated_at_ms)
             VALUES ('existing', 'Existing', 'existing', 0, 0, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
    }
    assert!(db.migrate_retained_snapshot(&source, 100).is_err());
    let conn = db.conn.lock().expect("lock database");
    assert_eq!(
        conn.query_row(
            "SELECT name FROM core_skills WHERE id = 'existing'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "Existing"
    );
    drop(conn);

    let db = Database::memory().expect("create second database");
    db.migrate_retained_snapshot(&source, 100).unwrap();
    let mut other = source.clone();
    other.source_fingerprint = "b".repeat(64);
    assert!(db.migrate_retained_snapshot(&other, 200).is_err());
}

#[test]
fn explicit_rollback_requires_matching_source_and_clears_only_migration_owned_data() {
    let db = Database::memory().expect("create database");
    let source = snapshot();
    {
        let conn = db.conn.lock().expect("lock database");
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES ('unrelated', 'true', 'device', 0)",
            [],
        )
        .unwrap();
    }
    db.migrate_retained_snapshot(&source, 100).unwrap();
    assert!(db.rollback_retained_snapshot(&"b".repeat(64)).is_err());
    db.rollback_retained_snapshot(&source.source_fingerprint)
        .unwrap();

    let conn = db.conn.lock().expect("lock database");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM core_providers", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT value_json FROM core_settings WHERE key = 'unrelated'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "true"
    );
}

#[test]
fn cross_resource_marker_requires_a_matching_database_migration_report() {
    let db = Database::memory().expect("create database");
    let source = snapshot();
    assert!(!db.retained_resources_complete_for().unwrap());

    db.migrate_retained_snapshot(&source, 100).unwrap();
    assert!(!db.retained_resources_complete_for().unwrap());
    db.mark_retained_resources_complete_for(&source.source_fingerprint, 100)
        .unwrap();
    assert!(db.retained_resources_complete_for().unwrap());

    let conn = db.conn.lock().expect("lock database");
    let report = load_report(&conn).unwrap().unwrap();
    conn.execute(
        "UPDATE core_settings SET value_json = ?1 WHERE key = ?2",
        [
            &serde_json::to_string(&RetainedResourcesMarker {
                schema_version: RESOURCE_MARKER_SCHEMA_VERSION,
                source_fingerprint: "b".repeat(64),
                content_sha256: report.content_sha256,
                migration_completed_at_ms: report.completed_at_ms,
                resources_completed_at_ms: 100,
            })
            .unwrap(),
            RESOURCE_MARKER_KEY,
        ],
    )
    .unwrap();
    drop(conn);
    assert!(db.retained_resources_complete_for().is_err());
}

#[test]
fn damaged_or_orphaned_cross_resource_marker_fails_closed() {
    let db = Database::memory().expect("create database");
    {
        let conn = db.conn.lock().expect("lock database");
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, '{broken', 'device', 0)",
            [RESOURCE_MARKER_KEY],
        )
        .unwrap();
    }
    assert!(db.retained_resources_complete_for().is_err());

    let db = Database::memory().expect("create second database");
    {
        let conn = db.conn.lock().expect("lock database");
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', 0)",
            [
                RESOURCE_MARKER_KEY,
                &serde_json::to_string(&RetainedResourcesMarker {
                    schema_version: RESOURCE_MARKER_SCHEMA_VERSION,
                    source_fingerprint: "a".repeat(64),
                    content_sha256: "b".repeat(64),
                    migration_completed_at_ms: 0,
                    resources_completed_at_ms: 0,
                })
                .unwrap(),
            ],
        )
        .unwrap();
    }
    assert!(db.retained_resources_complete_for().is_err());
}

#[test]
fn damaged_database_migration_marker_fails_closed() {
    let db = Database::memory().expect("create database");
    let source = snapshot();
    db.migrate_retained_snapshot(&source, 100).unwrap();
    db.mark_retained_resources_complete_for(&source.source_fingerprint, 100)
        .unwrap();
    {
        let conn = db.conn.lock().expect("lock database");
        conn.execute(
            "UPDATE core_settings SET value_json = '{broken' WHERE key = ?1",
            [MIGRATION_MARKER_KEY],
        )
        .unwrap();
    }

    assert!(db.retained_resources_complete_for().is_err());
}
