use rusqlite::{params, OptionalExtension};

use crate::database::{lock_conn, Database};
use crate::error::AppError;

impl Database {
    fn config_snippet_cleared_key(app_type: &str) -> String {
        format!("common_config_{app_type}_cleared")
    }

    pub fn get_config_snippet(&self, app_type: &str) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row(
            "SELECT content FROM core_common_snippets
             WHERE client_id = ?1 AND enabled = 1
             ORDER BY updated_at_ms DESC, id LIMIT 1",
            [app_type],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
    }

    pub fn is_config_snippet_cleared(&self, app_type: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let encoded = conn
            .query_row(
                "SELECT value_json FROM core_settings WHERE key = ?1",
                [Self::config_snippet_cleared_key(app_type)],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?;
        encoded
            .map(|value| serde_json::from_str::<bool>(&value).map_err(json_database_error))
            .transpose()
            .map(|value| value.unwrap_or(false))
    }

    pub fn set_config_snippet_state(
        &self,
        app_type: &str,
        snippet: Option<&str>,
        cleared: bool,
    ) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM core_common_snippets WHERE client_id = ?1",
                [app_type],
            )
            .map_err(database_error)?;
        if let Some(content) = snippet {
            transaction
                .execute(
                    "INSERT INTO core_common_snippets (
                        id, client_id, name, content, provider_id, enabled,
                        created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, NULL, 1, ?5, ?5)",
                    params![
                        format!("common-{app_type}"),
                        app_type,
                        format!("{app_type} common configuration"),
                        content,
                        now,
                    ],
                )
                .map_err(database_error)?;
        }

        let cleared_key = Self::config_snippet_cleared_key(app_type);
        if cleared {
            transaction
                .execute(
                    "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
                     VALUES (?1, 'true', 'device', ?2)
                     ON CONFLICT(key) DO UPDATE SET
                        value_json = 'true', storage_scope = 'device',
                        updated_at_ms = excluded.updated_at_ms",
                    params![cleared_key, now],
                )
                .map_err(database_error)?;
        } else {
            transaction
                .execute("DELETE FROM core_settings WHERE key = ?1", [cleared_key])
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)
    }
}

fn database_error(error: rusqlite::Error) -> AppError {
    AppError::Database(error.to_string())
}

fn json_database_error(error: serde_json::Error) -> AppError {
    AppError::Database(error.to_string())
}
