use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::domain::{
    LegacyCommonSnippetRecord, LegacyIgnoredCounts, LegacyMcpRecord, LegacyMigrationPreview,
    LegacyMigrationStatus, LegacyPromptRecord, LegacyProviderRecord, LegacyRetainedCounts,
    LegacyRetainedSnapshot, LegacySkillRecord, LegacySourceKind,
};
use crate::ports::{LegacyDataError, LegacyDataErrorCode};

use super::LEGACY_MAX_DATABASE_VERSION;

pub(super) fn preview_database(path: &Path) -> Result<LegacyMigrationPreview, LegacyDataError> {
    let connection = open_read_only(path)?;

    let quick_check: String = connection
        .query_row("PRAGMA quick_check(1);", [], |row| row.get(0))
        .map_err(|error| database_error(path, "validate legacy database", error))?;
    if quick_check != "ok" {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            "legacy database integrity check failed",
        )
        .with_context("result", quick_check));
    }

    let version = supported_version(path, &connection)?;

    let retained = LegacyRetainedCounts {
        claude_providers: count_where(
            &connection,
            "providers",
            "SELECT COUNT(*) FROM providers WHERE app_type = 'claude'",
        )?,
        codex_providers: count_where(
            &connection,
            "providers",
            "SELECT COUNT(*) FROM providers WHERE app_type = 'codex'",
        )?,
        opencode_providers: count_where(
            &connection,
            "providers",
            "SELECT COUNT(*) FROM providers WHERE app_type = 'opencode'",
        )?,
        mcp_servers: count_table(&connection, "mcp_servers")?,
        claude_prompts: count_where(
            &connection,
            "prompts",
            "SELECT COUNT(*) FROM prompts WHERE app_type = 'claude'",
        )?,
        codex_prompts: count_where(
            &connection,
            "prompts",
            "SELECT COUNT(*) FROM prompts WHERE app_type = 'codex'",
        )?,
        opencode_prompts: count_where(
            &connection,
            "prompts",
            "SELECT COUNT(*) FROM prompts WHERE app_type = 'opencode'",
        )?,
        skills: count_table(&connection, "skills")?,
        common_snippets: count_where(
            &connection,
            "settings",
            "SELECT COUNT(*) FROM settings
             WHERE key IN ('common_config_claude', 'common_config_codex')
               AND value IS NOT NULL AND trim(value) <> ''",
        )?,
    };

    let mut ignored = LegacyIgnoredCounts {
        non_target_client_records: count_where(
            &connection,
            "providers",
            "SELECT COUNT(*) FROM providers
             WHERE app_type NOT IN ('claude', 'codex', 'opencode')",
        )? + count_where(
            &connection,
            "prompts",
            "SELECT COUNT(*) FROM prompts
             WHERE app_type NOT IN ('claude', 'codex', 'opencode')",
        )?,
        profiles: count_table(&connection, "profiles")?,
        proxy_and_routing: count_tables(
            &connection,
            &[
                "provider_endpoints",
                "provider_health",
                "proxy_config",
                "proxy_live_backup",
                "proxy_request_logs",
            ],
        )?,
        usage_and_pricing: count_tables(
            &connection,
            &[
                "model_pricing",
                "session_log_sync",
                "stream_check_logs",
                "usage_daily_rollups",
            ],
        )?,
        failover: 0,
        online_skill_repositories: count_table(&connection, "skill_repos")?,
    };
    if column_exists(&connection, "providers", "in_failover_queue")? {
        ignored.failover = count_where(
            &connection,
            "providers",
            "SELECT COUNT(*) FROM providers WHERE in_failover_queue <> 0",
        )?;
    }

    Ok(LegacyMigrationPreview {
        status: LegacyMigrationStatus::Ready,
        source: Some(LegacySourceKind::Sqlite),
        source_version: Some(version as u32),
        retained,
        ignored,
        files: Vec::new(),
        directory_fingerprint: None,
    })
}

fn open_read_only(path: &Path) -> Result<Connection, LegacyDataError> {
    let file_url = url::Url::from_file_path(path).map_err(|_| {
        LegacyDataError::new(
            LegacyDataErrorCode::InspectionFailed,
            "legacy database path cannot be represented as a file URL",
        )
        .with_context("path", path.display().to_string())
    })?;
    let mut url = file_url;
    url.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");

    let connection = Connection::open_with_flags(
        url.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| database_error(path, "open legacy database", error))?;
    connection
        .execute_batch("PRAGMA query_only = ON;")
        .map_err(|error| database_error(path, "enable query-only mode", error))?;
    let query_only: i32 = connection
        .query_row("PRAGMA query_only;", [], |row| row.get(0))
        .map_err(|error| database_error(path, "verify query-only mode", error))?;
    if query_only != 1 {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            "legacy database did not enter query-only mode",
        ));
    }

    Ok(connection)
}

fn supported_version(path: &Path, connection: &Connection) -> Result<i32, LegacyDataError> {
    let version: i32 = connection
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(|error| database_error(path, "read legacy database version", error))?;
    if !(0..=LEGACY_MAX_DATABASE_VERSION).contains(&version) {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::UnsupportedVersion,
            format!(
                "legacy database version {version} is newer than supported v{LEGACY_MAX_DATABASE_VERSION}"
            ),
        )
        .with_context("version", version.to_string())
        .with_context("supportedVersion", LEGACY_MAX_DATABASE_VERSION.to_string()));
    }

    Ok(version)
}

pub(super) fn load_retained_database(
    path: &Path,
    source_fingerprint: &str,
) -> Result<LegacyRetainedSnapshot, LegacyDataError> {
    let connection = open_read_only(path)?;
    let version = supported_version(path, &connection)?;
    if version != LEGACY_MAX_DATABASE_VERSION {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::UnsupportedVersion,
            format!("retained-data migration requires a v{LEGACY_MAX_DATABASE_VERSION} database"),
        )
        .with_context("version", version.to_string()));
    }

    let providers = collect_rows(
        &connection,
        "SELECT id, app_type, name, settings_config, website_url, category,
                COALESCE(created_at, 0), COALESCE(sort_index, 0), notes, icon, icon_color,
                COALESCE(meta, '{}'), is_current
         FROM providers
         WHERE app_type IN ('claude', 'codex', 'opencode')
         ORDER BY app_type, id",
        |row| {
            Ok(LegacyProviderRecord {
                id: row.get(0)?,
                client_id: row.get(1)?,
                name: row.get(2)?,
                settings_config_json: row.get(3)?,
                website_url: row.get(4)?,
                category: row.get(5)?,
                created_at_ms: row.get(6)?,
                sort_index: row.get(7)?,
                notes: row.get(8)?,
                icon: row.get(9)?,
                icon_color: row.get(10)?,
                meta_json: row.get(11)?,
                is_current: row.get(12)?,
            })
        },
    )?;
    for provider in &providers {
        require_non_empty("provider id", &provider.id)?;
        require_non_empty("provider name", &provider.name)?;
        require_json("provider settings", &provider.settings_config_json)?;
        require_json("provider metadata", &provider.meta_json)?;
    }

    let mcp_servers = collect_rows(
        &connection,
        "SELECT id, name, server_config, description, homepage, docs, tags,
                enabled_claude, enabled_codex, enabled_opencode
         FROM mcp_servers ORDER BY id",
        |row| {
            Ok(LegacyMcpRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                server_config_json: row.get(2)?,
                description: row.get(3)?,
                homepage: row.get(4)?,
                docs: row.get(5)?,
                tags_json: row.get(6)?,
                enabled_claude: row.get(7)?,
                enabled_codex: row.get(8)?,
                enabled_opencode: row.get(9)?,
            })
        },
    )?;
    for server in &mcp_servers {
        require_non_empty("MCP id", &server.id)?;
        require_non_empty("MCP name", &server.name)?;
        require_json("MCP config", &server.server_config_json)?;
        require_json("MCP tags", &server.tags_json)?;
    }

    let prompts = collect_rows(
        &connection,
        "SELECT id, app_type, name, content, description, enabled,
                COALESCE(created_at, 0), COALESCE(updated_at, 0)
         FROM prompts
         WHERE app_type IN ('claude', 'codex', 'opencode')
         ORDER BY app_type, id",
        |row| {
            Ok(LegacyPromptRecord {
                id: row.get(0)?,
                client_id: row.get(1)?,
                name: row.get(2)?,
                content: row.get(3)?,
                description: row.get(4)?,
                enabled: row.get(5)?,
                created_at_ms: row.get(6)?,
                updated_at_ms: row.get(7)?,
            })
        },
    )?;
    for prompt in &prompts {
        require_non_empty("prompt id", &prompt.id)?;
        require_non_empty("prompt name", &prompt.name)?;
    }

    let skills = collect_rows(
        &connection,
        "SELECT id, name, description, directory, content_hash,
                enabled_claude, enabled_codex, enabled_opencode,
                COALESCE(installed_at, 0), COALESCE(updated_at, 0)
         FROM skills ORDER BY id",
        |row| {
            Ok(LegacySkillRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                directory: row.get(3)?,
                content_hash: row.get(4)?,
                enabled_claude: row.get(5)?,
                enabled_codex: row.get(6)?,
                enabled_opencode: row.get(7)?,
                created_at_ms: row.get(8)?,
                updated_at_ms: row.get(9)?,
            })
        },
    )?;
    for skill in &skills {
        require_non_empty("skill id", &skill.id)?;
        require_non_empty("skill name", &skill.name)?;
        require_non_empty("skill directory", &skill.directory)?;
    }

    let common_snippets = collect_rows(
        &connection,
        "SELECT key, substr(key, length('common_config_') + 1), value
         FROM settings
         WHERE key IN ('common_config_claude', 'common_config_codex')
           AND value IS NOT NULL AND trim(value) <> ''
         ORDER BY key",
        |row| {
            let key: String = row.get(0)?;
            Ok(LegacyCommonSnippetRecord {
                id: format!("legacy-{key}"),
                client_id: row.get(1)?,
                content: row.get(2)?,
            })
        },
    )?;

    Ok(LegacyRetainedSnapshot {
        source: LegacySourceKind::Sqlite,
        source_version: version as u32,
        source_fingerprint: source_fingerprint.to_string(),
        providers,
        mcp_servers,
        prompts,
        skills,
        common_snippets,
        legacy_settings_json: None,
    })
}

fn collect_rows<T, F>(
    connection: &Connection,
    sql: &str,
    mut map: F,
) -> Result<Vec<T>, LegacyDataError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let mut statement = connection.prepare(sql).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            format!("failed to prepare retained record query: {error}"),
        )
    })?;
    let records = statement
        .query_map([], |row| map(row))
        .map_err(|error| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidDatabase,
                format!("failed to query retained records: {error}"),
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidRecord,
                format!("failed to decode retained record: {error}"),
            )
        })?;
    Ok(records)
}

fn require_non_empty(label: &str, value: &str) -> Result<(), LegacyDataError> {
    if value.trim().is_empty() {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::InvalidRecord,
            format!("legacy {label} must not be empty"),
        ));
    }
    Ok(())
}

fn require_json(label: &str, value: &str) -> Result<(), LegacyDataError> {
    serde_json::from_str::<serde_json::Value>(value).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidRecord,
            format!("legacy {label} is invalid JSON: {error}"),
        )
    })?;
    Ok(())
}

fn count_tables(connection: &Connection, tables: &[&str]) -> Result<u64, LegacyDataError> {
    tables.iter().try_fold(0_u64, |total, table| {
        count_table(connection, table).map(|count| total + count)
    })
}

fn count_table(connection: &Connection, table: &str) -> Result<u64, LegacyDataError> {
    if !table_exists(connection, table)? {
        return Ok(0);
    }
    let sql = format!("SELECT COUNT(*) FROM {table}");
    query_count(connection, &sql)
}

fn count_where(connection: &Connection, table: &str, sql: &str) -> Result<u64, LegacyDataError> {
    if !table_exists(connection, table)? {
        return Ok(0);
    }
    query_count(connection, sql)
}

fn query_count(connection: &Connection, sql: &str) -> Result<u64, LegacyDataError> {
    let value: i64 = connection
        .query_row(sql, [], |row| row.get(0))
        .map_err(|error| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidDatabase,
                format!("failed to count legacy records: {error}"),
            )
        })?;
    u64::try_from(value).map_err(|_| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            "legacy record count cannot be negative",
        )
    })
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool, LegacyDataError> {
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )
        .map_err(|error| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidDatabase,
                format!("failed to inspect legacy schema: {error}"),
            )
        })?;
    Ok(count == 1)
}

fn column_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, LegacyDataError> {
    if !table_exists(connection, table)? {
        return Ok(false);
    }
    let sql = format!("PRAGMA table_info({table})");
    let mut statement = connection.prepare(&sql).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            format!("failed to inspect legacy columns: {error}"),
        )
    })?;
    let mut rows = statement.query([]).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            format!("failed to inspect legacy columns: {error}"),
        )
    })?;
    while let Some(row) = rows.next().map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidDatabase,
            format!("failed to inspect legacy columns: {error}"),
        )
    })? {
        let name: String = row.get(1).map_err(|error| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidDatabase,
                format!("failed to read legacy column: {error}"),
            )
        })?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn database_error(path: &Path, action: &str, error: rusqlite::Error) -> LegacyDataError {
    LegacyDataError::new(
        LegacyDataErrorCode::InvalidDatabase,
        format!("failed to {action}: {error}"),
    )
    .with_context("path", path.display().to_string())
}
