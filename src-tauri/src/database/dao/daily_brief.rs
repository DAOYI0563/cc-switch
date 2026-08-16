use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::database::{lock_conn, Database};
use crate::domain::DailyBriefSettings;
use crate::error::AppError;

const SETTINGS_KEY: &str = "daily_brief_config";
const DEVICE_KEY: &str = "daily_brief_device_identity";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBriefDeviceIdentity {
    pub device_id: String,
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyBriefRecord {
    pub date: String,
    pub device_id: String,
    pub status: String,
    pub source_fingerprint: Option<String>,
    pub content_hash: Option<String>,
    pub local_path: Option<String>,
    pub source_state: String,
    pub model_name: Option<String>,
    pub template_version: Option<String>,
    pub prompt_version: Option<String>,
    pub generated_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

impl Database {
    pub fn load_or_create_daily_brief_device(
        &self,
        now_ms: i64,
    ) -> Result<DailyBriefDeviceIdentity, AppError> {
        if let Some(identity) = self.load_sync_identity()? {
            return Ok(DailyBriefDeviceIdentity {
                device_id: identity.device_id.to_string(),
                device_name: identity.display_name,
            });
        }
        let conn = lock_conn!(self.conn);
        let existing: Option<String> = conn
            .query_row(
                "SELECT value_json FROM core_settings WHERE key = ?1 AND storage_scope = 'device'",
                [DEVICE_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Database(error.to_string()))?;
        if let Some(existing) = existing {
            return serde_json::from_str(&existing)
                .map_err(|_| AppError::Config("每日简报设备身份损坏".to_string()));
        }
        let identity = DailyBriefDeviceIdentity {
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: std::env::var("COMPUTERNAME")
                .ok()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| "WINDOWS".to_string()),
        };
        let json = serde_json::to_string(&identity)
            .map_err(|_| AppError::Config("每日简报设备身份序列化失败".to_string()))?;
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)",
            params![DEVICE_KEY, json, now_ms],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(identity)
    }

    pub fn load_daily_brief_settings(&self) -> Result<DailyBriefSettings, AppError> {
        let conn = lock_conn!(self.conn);
        let json: Option<String> = conn
            .query_row(
                "SELECT value_json FROM core_settings WHERE key = ?1 AND storage_scope = 'device'",
                [SETTINGS_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Database(error.to_string()))?;
        match json {
            Some(json) => serde_json::from_str(&json)
                .map_err(|_| AppError::Config("每日简报设置损坏".to_string())),
            None => Ok(DailyBriefSettings::default()),
        }
    }

    pub fn save_daily_brief_settings(
        &self,
        settings: &DailyBriefSettings,
        now_ms: i64,
    ) -> Result<(), AppError> {
        settings.validate().map_err(AppError::Config)?;
        let json = serde_json::to_string(settings)
            .map_err(|_| AppError::Config("每日简报设置序列化失败".to_string()))?;
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)
             ON CONFLICT(key) DO UPDATE SET
               value_json = excluded.value_json,
               storage_scope = 'device',
               updated_at_ms = excluded.updated_at_ms",
            params![SETTINGS_KEY, json, now_ms],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn list_daily_briefs(&self) -> Result<Vec<DailyBriefRecord>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT date, device_id, status, source_fingerprint, content_hash, local_path,
                        source_state, model_name, template_version, prompt_version,
                        generated_at_ms, updated_at_ms
                 FROM core_daily_briefs
                 ORDER BY date DESC, device_id",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(DailyBriefRecord {
                    date: row.get(0)?,
                    device_id: row.get(1)?,
                    status: row.get(2)?,
                    source_fingerprint: row.get(3)?,
                    content_hash: row.get(4)?,
                    local_path: row.get(5)?,
                    source_state: row.get(6)?,
                    model_name: row.get(7)?,
                    template_version: row.get(8)?,
                    prompt_version: row.get(9)?,
                    generated_at_ms: row.get(10)?,
                    updated_at_ms: row.get(11)?,
                })
            })
            .map_err(|error| AppError::Database(error.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn upsert_daily_brief(&self, record: &DailyBriefRecord) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO core_daily_briefs (
                date, device_id, status, source_fingerprint, content_hash, local_path,
                source_state, model_name, template_version, prompt_version,
                generated_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(date, device_id) DO UPDATE SET
                status = excluded.status,
                source_fingerprint = excluded.source_fingerprint,
                content_hash = excluded.content_hash,
                local_path = excluded.local_path,
                source_state = excluded.source_state,
                model_name = excluded.model_name,
                template_version = excluded.template_version,
                prompt_version = excluded.prompt_version,
                generated_at_ms = excluded.generated_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![
                record.date,
                record.device_id,
                record.status,
                record.source_fingerprint,
                record.content_hash,
                record.local_path,
                record.source_state,
                record.model_name,
                record.template_version,
                record.prompt_version,
                record.generated_at_ms,
                record.updated_at_ms,
            ],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn delete_daily_brief_record(&self, date: &str, device_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM core_daily_briefs WHERE date = ?1 AND device_id = ?2",
            params![date, device_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn save_brief_checkpoint(
        &self,
        date: &str,
        device_id: &str,
        protected_blob: &[u8],
        now_ms: i64,
        expires_at_ms: i64,
    ) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "INSERT INTO core_brief_checkpoints
                (date, device_id, protected_blob, created_at_ms, updated_at_ms, expires_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5)
             ON CONFLICT(date, device_id) DO UPDATE SET
                protected_blob = excluded.protected_blob,
                updated_at_ms = excluded.updated_at_ms,
                expires_at_ms = excluded.expires_at_ms",
            params![date, device_id, protected_blob, now_ms, expires_at_ms],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn load_brief_checkpoint(
        &self,
        date: &str,
        device_id: &str,
        now_ms: i64,
    ) -> Result<Option<Vec<u8>>, AppError> {
        let conn = lock_conn!(self.conn);
        let checkpoint = conn
            .query_row(
                "SELECT protected_blob FROM core_brief_checkpoints
                 WHERE date = ?1 AND device_id = ?2 AND expires_at_ms > ?3",
                params![date, device_id, now_ms],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(checkpoint)
    }

    pub fn delete_brief_checkpoints(&self, date: &str, device_id: &str) -> Result<(), AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM core_brief_checkpoints WHERE date = ?1 AND device_id = ?2",
            params![date, device_id],
        )
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub fn prune_expired_brief_checkpoints(&self, now_ms: i64) -> Result<usize, AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute(
            "DELETE FROM core_brief_checkpoints WHERE expires_at_ms <= ?1",
            [now_ms],
        )
        .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn completed_brief_dates(
        &self,
    ) -> Result<std::collections::BTreeSet<chrono::NaiveDate>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT date FROM core_daily_briefs WHERE status IN ('complete', 'no_sessions')",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut dates = std::collections::BTreeSet::new();
        for row in rows {
            let date = row.map_err(|error| AppError::Database(error.to_string()))?;
            if let Ok(date) = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                dates.insert(date);
            }
        }
        Ok(dates)
    }
}
