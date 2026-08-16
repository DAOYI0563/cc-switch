use indexmap::IndexMap;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::provider::{Provider, ProviderMeta};

impl Database {
    pub fn get_all_providers(
        &self,
        app_type: &str,
    ) -> Result<IndexMap<String, Provider>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT id, name, kind, local_config_json, sort_index, notes,
                        icon, icon_color, created_at_ms
                 FROM core_providers
                 WHERE client_id = ?1
                 ORDER BY sort_index, lower(name), id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([app_type], |row| {
                let local: String = row.get(3)?;
                let local: Value = serde_json::from_str(&local).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                let kind: String = row.get(2)?;
                Ok(Provider {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    settings_config: local
                        .get("settingsConfig")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    website_url: local
                        .get("websiteUrl")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    category: local
                        .get("category")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .or(Some(kind)),
                    created_at: Some(row.get(8)?),
                    sort_index: usize::try_from(row.get::<_, i64>(4)?).ok(),
                    notes: row.get(5)?,
                    meta: local
                        .get("meta")
                        .cloned()
                        .and_then(|value| serde_json::from_value::<ProviderMeta>(value).ok()),
                    icon: row.get(6)?,
                    icon_color: row.get(7)?,
                })
            })
            .map_err(database_error)?;
        let mut providers = IndexMap::new();
        for provider in rows {
            let provider = provider.map_err(database_error)?;
            providers.insert(provider.id.clone(), provider);
        }
        Ok(providers)
    }

    pub fn get_provider_by_id(
        &self,
        id: &str,
        app_type: &str,
    ) -> Result<Option<Provider>, AppError> {
        Ok(self.get_all_providers(app_type)?.shift_remove(id))
    }

    pub fn get_current_provider(&self, app_type: &str) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        let key = current_provider_key(app_type);
        let value = conn
            .query_row(
                "SELECT value_json FROM core_settings WHERE key = ?1",
                [key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?;
        value
            .map(|value| serde_json::from_str::<String>(&value).map_err(json_database_error))
            .transpose()
    }

    pub fn save_provider(&self, app_type: &str, provider: &Provider) -> Result<(), AppError> {
        let now = chrono::Utc::now().timestamp_millis();
        let created_at = provider.created_at.unwrap_or(now).max(0);
        let sort_index = i64::try_from(provider.sort_index.unwrap_or(0))
            .map_err(|_| AppError::InvalidInput("供应商排序值超出范围".to_string()))?;
        let kind = if provider.category.as_deref() == Some("official") {
            "official"
        } else {
            "custom"
        };
        let local = json!({
            "settingsConfig": provider.settings_config,
            "meta": provider.meta.clone().unwrap_or_default(),
            "websiteUrl": provider.website_url,
            "category": provider.category,
        });
        let portable = redact_sensitive_json(&local);
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO core_providers (
                id, client_id, kind, name, portable_config_json, local_config_json,
                quota_config_json, sort_index, notes, icon, icon_color,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '{}', ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id, client_id) DO UPDATE SET
                kind = excluded.kind,
                name = excluded.name,
                portable_config_json = excluded.portable_config_json,
                local_config_json = excluded.local_config_json,
                sort_index = excluded.sort_index,
                notes = excluded.notes,
                icon = excluded.icon,
                icon_color = excluded.icon_color,
                updated_at_ms = excluded.updated_at_ms",
            params![
                provider.id,
                app_type,
                kind,
                provider.name,
                serde_json::to_string(&portable).map_err(json_database_error)?,
                serde_json::to_string(&local).map_err(json_database_error)?,
                sort_index,
                provider.notes,
                provider.icon,
                provider.icon_color,
                created_at,
                now.max(created_at),
            ],
        )
        .map_err(database_error)?;
        Ok(())
    }

    pub fn delete_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM core_providers WHERE client_id = ?1 AND id = ?2",
                params![app_type, id],
            )
            .map_err(database_error)?;
        let key = current_provider_key(app_type);
        let encoded = serde_json::to_string(id).map_err(json_database_error)?;
        transaction
            .execute(
                "DELETE FROM core_settings WHERE key = ?1 AND value_json = ?2",
                params![key, encoded],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    }

    pub fn set_current_provider(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM core_providers WHERE client_id = ?1 AND id = ?2
                 )",
                params![app_type, id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if !exists {
            return Err(AppError::InvalidInput(format!("供应商 {id} 不存在")));
        }
        let key = current_provider_key(app_type);
        let value = serde_json::to_string(id).map_err(json_database_error)?;
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                storage_scope = 'device',
                updated_at_ms = excluded.updated_at_ms",
            params![key, value, chrono::Utc::now().timestamp_millis()],
        )
        .map_err(database_error)?;
        Ok(())
    }

    pub fn set_current_provider_optional(
        &self,
        app_type: &str,
        id: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(id) = id {
            return self.set_current_provider(app_type, id);
        }
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM core_settings WHERE key = ?1",
            [current_provider_key(app_type)],
        )
        .map_err(database_error)?;
        Ok(())
    }

    pub fn update_provider_sort_index(
        &self,
        app_type: &str,
        id: &str,
        sort_index: usize,
    ) -> Result<(), AppError> {
        let sort_index = i64::try_from(sort_index)
            .map_err(|_| AppError::InvalidInput("供应商排序值超出范围".to_string()))?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE core_providers
             SET sort_index = ?1, updated_at_ms = ?2
             WHERE client_id = ?3 AND id = ?4",
            params![
                sort_index,
                chrono::Utc::now().timestamp_millis(),
                app_type,
                id
            ],
        )
        .map_err(database_error)?;
        Ok(())
    }

    pub fn init_default_official_providers(&self) -> Result<usize, AppError> {
        let seeds = [
            Provider {
                id: "claude-official".to_string(),
                name: "Claude 官方".to_string(),
                settings_config: json!({ "env": {} }),
                website_url: Some("https://www.anthropic.com/claude-code".to_string()),
                category: Some("official".to_string()),
                created_at: Some(0),
                sort_index: Some(0),
                notes: None,
                meta: Some(ProviderMeta {
                    common_config_enabled: Some(true),
                    ..ProviderMeta::default()
                }),
                icon: Some("anthropic".to_string()),
                icon_color: Some("#D4915D".to_string()),
            },
            Provider {
                id: "codex-official".to_string(),
                name: "Codex 官方".to_string(),
                settings_config: json!({ "auth": {}, "config": "" }),
                website_url: Some("https://openai.com/codex".to_string()),
                category: Some("official".to_string()),
                created_at: Some(0),
                sort_index: Some(0),
                notes: None,
                meta: Some(ProviderMeta {
                    common_config_enabled: Some(true),
                    ..ProviderMeta::default()
                }),
                icon: Some("openai".to_string()),
                icon_color: Some("#10A37F".to_string()),
            },
        ];
        let mut inserted = 0;
        for (client, provider) in ["claude", "codex"].into_iter().zip(seeds) {
            if self.get_provider_by_id(&provider.id, client)?.is_none() {
                self.save_provider(client, &provider)?;
                inserted += 1;
            }
        }
        Ok(inserted)
    }
}

fn current_provider_key(app_type: &str) -> String {
    format!("current_provider_{app_type}")
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

fn json_database_error(error: serde_json::Error) -> AppError {
    AppError::Database(error.to_string())
}
