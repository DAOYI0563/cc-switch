//! Prompt version persistence owned by the three-client core schema.

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::database::{lock_conn, Database};
use crate::domain::{ManagedClientId, PromptVersion, MAX_PROMPT_VERSIONS_PER_NAME};
use crate::error::AppError;
use indexmap::IndexMap;
use rusqlite::{params, OptionalExtension, Row};

const PROMPT_SELECT: &str = "SELECT id, name, version, content, description, is_active, created_at_ms, updated_at_ms FROM core_prompt_versions";

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn row_to_prompt(row: &Row<'_>) -> rusqlite::Result<PromptVersion> {
    Ok(PromptVersion {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        content: row.get(3)?,
        description: row.get(4)?,
        enabled: row.get(5)?,
        created_at: Some(row.get(6)?),
        updated_at: Some(row.get(7)?),
    })
}

fn database_error(context: &str, error: impl std::fmt::Display) -> AppError {
    let detail = error.to_string();
    if detail.contains("prompt version limit reached") {
        AppError::InvalidInput(format!(
            "同一客户端和名称最多保留 {MAX_PROMPT_VERSIONS_PER_NAME} 个 Prompt 版本"
        ))
    } else {
        AppError::Database(format!("{context}: {detail}"))
    }
}

impl Database {
    pub fn get_prompt_versions(
        &self,
        client: ManagedClientId,
    ) -> Result<IndexMap<String, PromptVersion>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(&format!(
                "{PROMPT_SELECT} WHERE client_id = ?1 ORDER BY name COLLATE NOCASE, version, id"
            ))
            .map_err(|error| database_error("准备读取 Prompt 版本失败", error))?;
        let rows = statement
            .query_map(params![client.as_str()], row_to_prompt)
            .map_err(|error| database_error("读取 Prompt 版本失败", error))?;
        let mut versions = IndexMap::new();
        for row in rows {
            let version = row.map_err(|error| database_error("解析 Prompt 版本失败", error))?;
            version
                .validate_stored()
                .map_err(|error| AppError::InvalidInput(error.to_string()))?;
            versions.insert(version.id.clone(), version);
        }
        Ok(versions)
    }

    pub fn disable_all_prompt_versions(
        &self,
        client: ManagedClientId,
        updated_at_ms: i64,
    ) -> Result<(), AppError> {
        if updated_at_ms < 0 {
            return Err(AppError::InvalidInput(
                "Prompt update time must not be negative".to_string(),
            ));
        }
        let conn = lock_conn!(self.conn);
        conn.execute(
            "UPDATE core_prompt_versions
             SET is_active = 0, updated_at_ms = MAX(updated_at_ms, ?1)
             WHERE client_id = ?2 AND is_active = 1",
            params![updated_at_ms, client.as_str()],
        )
        .map(|_| ())
        .map_err(|error| database_error("停用 Prompt 版本失败", error))
    }

    pub fn prepare_prompt_version(
        &self,
        client: ManagedClientId,
        mut input: PromptVersion,
    ) -> Result<PromptVersion, AppError> {
        input
            .validate_input()
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        input.name = input.name.trim().to_string();
        input.description = input
            .description
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let conn = lock_conn!(self.conn);
        let owner: Option<String> = conn
            .query_row(
                "SELECT client_id FROM core_prompt_versions WHERE id = ?1",
                params![input.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| database_error("读取 Prompt 版本归属失败", error))?;
        if owner
            .as_deref()
            .is_some_and(|owner| owner != client.as_str())
        {
            return Err(AppError::InvalidInput(format!(
                "Prompt 版本 ID '{}' 已属于其他客户端",
                input.id
            )));
        }

        let existing = conn
            .query_row(
                &format!("{PROMPT_SELECT} WHERE client_id = ?1 AND id = ?2"),
                params![client.as_str(), input.id],
                row_to_prompt,
            )
            .optional()
            .map_err(|error| database_error("读取现有 Prompt 版本失败", error))?;
        let now = current_time_ms();
        if let Some(existing) = existing {
            input.version = existing.version;
            input.created_at = existing.created_at;
            input.updated_at = Some(now.max(existing.created_at.unwrap_or(0)));
        } else {
            let highest: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) FROM core_prompt_versions
                     WHERE client_id = ?1 AND name = ?2",
                    params![client.as_str(), input.name],
                    |row| row.get(0),
                )
                .map_err(|error| database_error("计算下一个 Prompt 版本失败", error))?;
            if highest >= MAX_PROMPT_VERSIONS_PER_NAME {
                return Err(AppError::InvalidInput(format!(
                    "同一客户端和名称最多保留 {MAX_PROMPT_VERSIONS_PER_NAME} 个 Prompt 版本"
                )));
            }
            input.version = highest + 1;
            input.created_at = Some(now);
            input.updated_at = Some(now);
        }
        input
            .validate_stored()
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        Ok(input)
    }

    pub fn save_prompt_version(
        &self,
        client: ManagedClientId,
        prompt: &PromptVersion,
    ) -> Result<(), AppError> {
        prompt
            .validate_stored()
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let conn = lock_conn!(self.conn);
        let transaction = conn
            .unchecked_transaction()
            .map_err(|error| database_error("开始保存 Prompt 版本事务失败", error))?;
        let owner: Option<String> = transaction
            .query_row(
                "SELECT client_id FROM core_prompt_versions WHERE id = ?1",
                params![prompt.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| database_error("读取 Prompt 版本归属失败", error))?;
        if owner
            .as_deref()
            .is_some_and(|owner| owner != client.as_str())
        {
            return Err(AppError::InvalidInput(format!(
                "Prompt 版本 ID '{}' 已属于其他客户端",
                prompt.id
            )));
        }

        if prompt.enabled {
            transaction
                .execute(
                    "UPDATE core_prompt_versions SET is_active = 0, updated_at_ms = ?1
                     WHERE client_id = ?2 AND is_active = 1 AND id <> ?3",
                    params![prompt.updated_at, client.as_str(), prompt.id],
                )
                .map_err(|error| database_error("停用旧 Prompt 版本失败", error))?;
        }

        if owner.is_some() {
            let affected = transaction
                .execute(
                    "UPDATE core_prompt_versions SET
                        name = ?1, version = ?2, content = ?3, description = ?4,
                        is_active = ?5, updated_at_ms = ?6
                     WHERE id = ?7 AND client_id = ?8",
                    params![
                        prompt.name,
                        prompt.version,
                        prompt.content,
                        prompt.description,
                        prompt.enabled,
                        prompt.updated_at,
                        prompt.id,
                        client.as_str(),
                    ],
                )
                .map_err(|error| database_error("更新 Prompt 版本失败", error))?;
            if affected != 1 {
                return Err(AppError::Database(
                    "更新 Prompt 版本未命中唯一记录".to_string(),
                ));
            }
        } else {
            transaction
                .execute(
                    "INSERT INTO core_prompt_versions (
                        id, client_id, name, version, content, description, is_active,
                        created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        prompt.id,
                        client.as_str(),
                        prompt.name,
                        prompt.version,
                        prompt.content,
                        prompt.description,
                        prompt.enabled,
                        prompt.created_at,
                        prompt.updated_at,
                    ],
                )
                .map_err(|error| database_error("新增 Prompt 版本失败", error))?;
        }
        transaction
            .commit()
            .map_err(|error| database_error("提交 Prompt 版本事务失败", error))
    }

    pub fn delete_prompt_version(
        &self,
        client: ManagedClientId,
        id: &str,
    ) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let affected = conn
            .execute(
                "DELETE FROM core_prompt_versions WHERE client_id = ?1 AND id = ?2",
                params![client.as_str(), id],
            )
            .map_err(|error| database_error("删除 Prompt 版本失败", error))?;
        Ok(affected == 1)
    }

    // Compatibility methods used only by legacy modules pending their scheduled removal.
    pub fn get_prompts(&self, app_type: &str) -> Result<IndexMap<String, PromptVersion>, AppError> {
        let client = ManagedClientId::from_str(app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        self.get_prompt_versions(client)
    }

    pub fn save_prompt(&self, app_type: &str, prompt: &PromptVersion) -> Result<(), AppError> {
        let client = ManagedClientId::from_str(app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let prepared = self.prepare_prompt_version(client, prompt.clone())?;
        self.save_prompt_version(client, &prepared)
    }

    pub fn delete_prompt(&self, app_type: &str, id: &str) -> Result<(), AppError> {
        let client = ManagedClientId::from_str(app_type)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        self.delete_prompt_version(client, id).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(id: &str, name: &str) -> PromptVersion {
        PromptVersion {
            id: id.to_string(),
            name: name.to_string(),
            version: 0,
            content: format!("content for {id}"),
            description: None,
            enabled: false,
            created_at: None,
            updated_at: None,
        }
    }

    #[test]
    fn production_prompt_crud_uses_core_table_only() {
        let db = Database::memory().expect("memory database");
        assert!(db
            .get_prompt_versions(ManagedClientId::Claude)
            .unwrap()
            .is_empty());
        let prepared = db
            .prepare_prompt_version(ManagedClientId::Claude, input("core", "Core"))
            .unwrap();
        db.save_prompt_version(ManagedClientId::Claude, &prepared)
            .unwrap();

        let conn = db.conn.lock().expect("database lock");
        let core_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM core_prompt_versions", [], |row| {
                row.get(0)
            })
            .unwrap();
        let legacy_table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'prompts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(core_count, 1);
        assert_eq!(legacy_table_count, 0);
    }

    #[test]
    fn activating_a_version_atomically_deactivates_every_other_name() {
        let db = Database::memory().expect("memory database");
        let mut first = db
            .prepare_prompt_version(ManagedClientId::Claude, input("first", "One"))
            .unwrap();
        first.enabled = true;
        db.save_prompt_version(ManagedClientId::Claude, &first)
            .unwrap();
        let mut second = db
            .prepare_prompt_version(ManagedClientId::Claude, input("second", "Two"))
            .unwrap();
        second.enabled = true;
        db.save_prompt_version(ManagedClientId::Claude, &second)
            .unwrap();

        let stored = db.get_prompt_versions(ManagedClientId::Claude).unwrap();
        assert!(!stored["first"].enabled);
        assert!(stored["second"].enabled);
    }
}
