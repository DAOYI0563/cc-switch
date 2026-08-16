use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use super::{core_schema, lock_conn, Database, SCHEMA_VERSION};
use crate::error::AppError;

impl Database {
    pub(crate) fn create_tables(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::create_tables_on_conn(&conn)
    }

    pub(crate) fn create_tables_on_conn(conn: &Connection) -> Result<(), AppError> {
        core_schema::create_core_schema(conn)
    }

    pub(crate) fn apply_schema_migrations(&self) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        Self::apply_schema_migrations_on_conn(&conn)
    }

    pub(crate) fn apply_schema_migrations_on_conn(conn: &Connection) -> Result<(), AppError> {
        let version = Self::get_user_version(conn)?;
        if version > SCHEMA_VERSION {
            return Err(AppError::Database(format!(
                "数据库版本过新（{version}），当前应用仅支持 {SCHEMA_VERSION}"
            )));
        }

        core_schema::create_core_schema(conn)?;
        let transaction = conn.unchecked_transaction().map_err(database_error)?;
        if version > 0 && version <= 17 {
            migrate_legacy_providers(&transaction)?;
            migrate_legacy_common_snippets(&transaction)?;
        }
        if version > 0 && version <= 16 {
            migrate_legacy_mcp(&transaction)?;
            migrate_legacy_prompts(&transaction)?;
            migrate_legacy_skills(&transaction)?;
        }
        drop_non_core_tables(&transaction)?;
        transaction
            .execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;

        core_schema::ensure_prompt_active_index(conn)?;
        core_schema::validate_core_schema(conn)
    }

    pub(crate) fn get_user_version(conn: &Connection) -> Result<i32, AppError> {
        conn.query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(database_error)
    }
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(database_error)
}

fn migrate_legacy_providers(conn: &Connection) -> Result<(), AppError> {
    if !table_exists(conn, "providers")? {
        return Ok(());
    }
    let mut statement = conn
        .prepare(
            "SELECT id, app_type, name, settings_config, website_url, category,
                    COALESCE(created_at, 0), COALESCE(sort_index, 0), notes,
                    icon, icon_color, COALESCE(meta, '{}'), COALESCE(is_current, 0)
             FROM providers
             WHERE app_type IN ('claude', 'codex', 'opencode')",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, bool>(12)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (
            id,
            client,
            name,
            settings,
            website,
            category,
            created_at,
            sort_index,
            notes,
            icon,
            icon_color,
            metadata,
            is_current,
        ) = row.map_err(database_error)?;
        let settings: Value = serde_json::from_str(&settings).map_err(json_error)?;
        let metadata: Value = serde_json::from_str(&metadata).map_err(json_error)?;
        let local = json!({
            "settingsConfig": settings,
            "meta": metadata,
            "websiteUrl": website,
            "category": category,
        });
        let portable = redact_sensitive_json(&local);
        let kind = if category.as_deref() == Some("official") {
            "official"
        } else {
            "custom"
        };
        conn.execute(
            "INSERT INTO core_providers (
                id, client_id, kind, name, portable_config_json, local_config_json,
                quota_config_json, sort_index, notes, icon, icon_color,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT(id, client_id) DO UPDATE SET
                kind = excluded.kind, name = excluded.name,
                portable_config_json = excluded.portable_config_json,
                local_config_json = excluded.local_config_json,
                sort_index = excluded.sort_index, notes = excluded.notes,
                icon = excluded.icon, icon_color = excluded.icon_color,
                updated_at_ms = excluded.updated_at_ms",
            params![
                id,
                client,
                kind,
                name,
                serde_json::to_string(&portable).map_err(json_error)?,
                serde_json::to_string(&local).map_err(json_error)?,
                sort_index.max(0),
                notes,
                icon,
                icon_color,
                created_at.max(0),
            ],
        )
        .map_err(database_error)?;
        if is_current {
            conn.execute(
                "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
                 VALUES (?1, ?2, 'device', ?3)
                 ON CONFLICT(key) DO UPDATE SET
                    value_json = excluded.value_json,
                    storage_scope = 'device',
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    format!("current_provider_{client}"),
                    serde_json::to_string(&id).map_err(json_error)?,
                    created_at.max(0),
                ],
            )
            .map_err(database_error)?;
        }
    }
    Ok(())
}

fn migrate_legacy_common_snippets(conn: &Connection) -> Result<(), AppError> {
    if !table_exists(conn, "settings")? {
        return Ok(());
    }
    for client in ["claude", "codex"] {
        let key = format!("common_config_{client}");
        let value = conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [&key], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map_err(database_error)?
            .flatten()
            .filter(|value| !value.trim().is_empty());
        if let Some(value) = value {
            conn.execute(
                "DELETE FROM core_common_snippets WHERE client_id = ?1",
                [client],
            )
            .map_err(database_error)?;
            conn.execute(
                "INSERT INTO core_common_snippets (
                    id, client_id, name, content, provider_id, enabled,
                    created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, NULL, 1, 0, 0)",
                params![
                    format!("common-{client}"),
                    client,
                    format!("{client} common configuration"),
                    value,
                ],
            )
            .map_err(database_error)?;
        }
    }
    Ok(())
}

fn migrate_legacy_mcp(conn: &Connection) -> Result<(), AppError> {
    if !table_exists(conn, "mcp_servers")? {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT OR REPLACE INTO core_mcp_servers (
            id, name, server_config_json, description, homepage, docs, tags_json,
            enabled_claude, enabled_codex, enabled_opencode, created_at_ms, updated_at_ms
         )
         SELECT id, name, server_config, description, homepage, docs, COALESCE(tags, '[]'),
                enabled_claude, enabled_codex, enabled_opencode, 0, 0
         FROM mcp_servers;",
    )
    .map_err(database_error)
}

fn migrate_legacy_prompts(conn: &Connection) -> Result<(), AppError> {
    if !table_exists(conn, "prompts")? {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT OR IGNORE INTO core_prompt_versions (
            id, client_id, name, version, content, description, is_active,
            created_at_ms, updated_at_ms
         )
         SELECT id, app_type, name, 1, content, description, enabled,
                MAX(COALESCE(created_at, 0), 0),
                MAX(COALESCE(updated_at, created_at, 0), 0)
         FROM prompts WHERE app_type IN ('claude', 'codex', 'opencode');",
    )
    .map_err(database_error)
}

fn migrate_legacy_skills(conn: &Connection) -> Result<(), AppError> {
    if !table_exists(conn, "skills")? {
        return Ok(());
    }
    conn.execute_batch(
        "INSERT OR REPLACE INTO core_skills (
            id, name, description, directory, content_hash, total_size_bytes, file_count,
            enabled_claude, enabled_codex, enabled_opencode, cloud_eligible,
            created_at_ms, updated_at_ms
         )
         SELECT id, name, description, directory, content_hash, 0, 0,
                enabled_claude, enabled_codex, enabled_opencode, 1,
                MAX(COALESCE(installed_at, 0), 0), MAX(COALESCE(updated_at, 0), 0)
         FROM skills;",
    )
    .map_err(database_error)
}

fn drop_non_core_tables(conn: &Connection) -> Result<(), AppError> {
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .map_err(database_error)?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for table in tables {
        if !core_schema::CORE_TABLES.contains(&table.as_str()) {
            let quoted = format!("\"{}\"", table.replace('"', "\"\""));
            conn.execute_batch(&format!("DROP TABLE {quoted};"))
                .map_err(database_error)?;
        }
    }
    Ok(())
}

fn redact_sensitive_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !is_sensitive_key(key))
                .map(|(key, value)| (key.clone(), redact_sensitive_json(value)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(redact_sensitive_json).collect()),
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "apikey",
        "authtoken",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "password",
        "privatekey",
        "secret",
        "accesstoken",
        "refreshtoken",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
        || normalized == "auth"
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

fn json_error(error: serde_json::Error) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_tables(conn: &Connection) -> Vec<String> {
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .expect("prepare table list");
        statement
            .query_map([], |row| row.get(0))
            .expect("query table list")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table list")
    }

    #[test]
    fn fresh_database_contains_only_final_core_tables() {
        let db = Database::memory().expect("create memory database");
        let conn = db.conn.lock().expect("lock database");
        let mut expected = core_schema::CORE_TABLES
            .iter()
            .map(|table| (*table).to_string())
            .collect::<Vec<_>>();
        expected.sort();

        assert_eq!(user_tables(&conn), expected);
        assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn v16_upgrade_keeps_three_clients_redacts_portable_data_and_drops_legacy_tables() {
        let conn = Connection::open_in_memory().expect("open legacy database");
        conn.execute_batch(include_str!("../../tests/fixtures/v16/cc-switch-v16.sql"))
            .expect("load v16 fixture");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("enable foreign keys");

        Database::create_tables_on_conn(&conn).expect("create core schema");
        Database::apply_schema_migrations_on_conn(&conn).expect("upgrade v16 database");

        assert_eq!(Database::get_user_version(&conn).unwrap(), SCHEMA_VERSION);
        assert_eq!(user_tables(&conn).len(), core_schema::CORE_TABLES.len());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM core_providers", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM core_providers WHERE client_id NOT IN ('claude', 'codex', 'opencode')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        let (portable, local): (String, String) = conn
            .query_row(
                "SELECT portable_config_json, local_config_json
                 FROM core_providers WHERE id = 'fixture-codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(!portable.contains("FIXTURE_CODEX_TOKEN"));
        assert!(local.contains("FIXTURE_CODEX_TOKEN"));
        for removed in [
            "providers",
            "profiles",
            "proxy_request_logs",
            "usage_daily_rollups",
        ] {
            assert!(
                !table_exists(&conn, removed).unwrap(),
                "legacy table {removed} remains"
            );
        }
    }
}
