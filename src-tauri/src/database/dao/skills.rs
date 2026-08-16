//! Local Skill metadata persistence.

use crate::database::{lock_conn, Database};
use crate::domain::{LocalSkill, ManagedClientApps};
use crate::error::AppError;
use crate::ports::{LocalSkillRepository, LocalSkillRepositoryError};
use rusqlite::params;

impl Database {
    pub fn list_core_skills(&self) -> Result<Vec<LocalSkill>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT id, name, description, directory, content_hash,
                        total_size_bytes, file_count, enabled_claude, enabled_codex,
                        enabled_opencode, cloud_eligible, created_at_ms, updated_at_ms
                 FROM core_skills ORDER BY lower(name), id",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = statement
            .query_map([], local_skill_from_row)
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut skills = Vec::new();
        for row in rows {
            let skill = row.map_err(|error| AppError::Database(error.to_string()))?;
            skill
                .validate()
                .map_err(|error| AppError::Database(format!("core_skills 记录无效: {error}")))?;
            skills.push(skill);
        }
        Ok(skills)
    }

    pub fn get_core_skill(&self, id: &str) -> Result<Option<LocalSkill>, AppError> {
        let conn = lock_conn!(self.conn);
        let result = conn.query_row(
            "SELECT id, name, description, directory, content_hash,
                    total_size_bytes, file_count, enabled_claude, enabled_codex,
                    enabled_opencode, cloud_eligible, created_at_ms, updated_at_ms
             FROM core_skills WHERE id = ?1",
            [id],
            local_skill_from_row,
        );
        match result {
            Ok(skill) => {
                skill.validate().map_err(|error| {
                    AppError::Database(format!("core_skills 记录无效: {error}"))
                })?;
                Ok(Some(skill))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(AppError::Database(error.to_string())),
        }
    }

    pub fn save_core_skills(&self, skills: &[LocalSkill]) -> Result<(), AppError> {
        for skill in skills {
            skill
                .validate()
                .map_err(|error| AppError::InvalidInput(error.to_string()))?;
            checked_sqlite_u64(skill.total_size_bytes, "total_size_bytes")?;
            checked_sqlite_u64(skill.file_count, "file_count")?;
        }
        let mut conn = lock_conn!(self.conn);
        let transaction = conn
            .transaction()
            .map_err(|error| AppError::Database(error.to_string()))?;
        for skill in skills {
            transaction
                .execute(
                    "INSERT INTO core_skills (
                        id, name, description, directory, content_hash,
                        total_size_bytes, file_count, enabled_claude, enabled_codex,
                        enabled_opencode, cloud_eligible, created_at_ms, updated_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(id) DO UPDATE SET
                        name = excluded.name,
                        description = excluded.description,
                        directory = excluded.directory,
                        content_hash = excluded.content_hash,
                        total_size_bytes = excluded.total_size_bytes,
                        file_count = excluded.file_count,
                        enabled_claude = excluded.enabled_claude,
                        enabled_codex = excluded.enabled_codex,
                        enabled_opencode = excluded.enabled_opencode,
                        cloud_eligible = excluded.cloud_eligible,
                        updated_at_ms = excluded.updated_at_ms",
                    params![
                        skill.id,
                        skill.name,
                        skill.description,
                        skill.directory,
                        skill.content_hash,
                        checked_sqlite_u64(skill.total_size_bytes, "total_size_bytes")?,
                        checked_sqlite_u64(skill.file_count, "file_count")?,
                        skill.apps.claude,
                        skill.apps.codex,
                        skill.apps.opencode,
                        skill.cloud_eligible,
                        skill.created_at_ms,
                        skill.updated_at_ms,
                    ],
                )
                .map_err(|error| AppError::Database(error.to_string()))?;
        }
        transaction
            .commit()
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub fn delete_core_skill(&self, id: &str) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        conn.execute("DELETE FROM core_skills WHERE id = ?1", [id])
            .map(|affected| affected > 0)
            .map_err(|error| AppError::Database(error.to_string()))
    }
}

impl LocalSkillRepository for Database {
    fn list_local_skills(&self) -> Result<Vec<LocalSkill>, LocalSkillRepositoryError> {
        self.list_core_skills().map_err(repository_error)
    }

    fn get_local_skill(&self, id: &str) -> Result<Option<LocalSkill>, LocalSkillRepositoryError> {
        self.get_core_skill(id).map_err(repository_error)
    }

    fn save_local_skills(&self, skills: &[LocalSkill]) -> Result<(), LocalSkillRepositoryError> {
        self.save_core_skills(skills).map_err(repository_error)
    }

    fn delete_local_skill(&self, id: &str) -> Result<bool, LocalSkillRepositoryError> {
        self.delete_core_skill(id).map_err(repository_error)
    }
}

fn local_skill_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalSkill> {
    let total_size_bytes: i64 = row.get(5)?;
    let file_count: i64 = row.get(6)?;
    if total_size_bytes < 0 || file_count < 0 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Integer,
            "negative Skill size or file count".into(),
        ));
    }
    Ok(LocalSkill {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        directory: row.get(3)?,
        content_hash: row.get(4)?,
        total_size_bytes: total_size_bytes as u64,
        file_count: file_count as u64,
        apps: ManagedClientApps {
            claude: row.get(7)?,
            codex: row.get(8)?,
            opencode: row.get(9)?,
        },
        cloud_eligible: row.get(10)?,
        created_at_ms: row.get(11)?,
        updated_at_ms: row.get(12)?,
    })
}

fn checked_sqlite_u64(value: u64, field: &str) -> Result<i64, AppError> {
    i64::try_from(value)
        .map_err(|_| AppError::InvalidInput(format!("{field} exceeds SQLite INTEGER range")))
}

fn repository_error(error: AppError) -> LocalSkillRepositoryError {
    LocalSkillRepositoryError::new(error.to_string())
}
