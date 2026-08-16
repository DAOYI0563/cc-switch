use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{core_schema, lock_conn, Database};
use crate::domain::{LegacyRetainedSnapshot, RetainedMigrationReport, RetainedMigrationStatus};
use crate::error::AppError;
use crate::ports::{RetainedMigrationTarget, RetainedMigrationTargetError};

const MIGRATION_MARKER_KEY: &str = "retained_migration_v1";
const RESOURCE_MARKER_KEY: &str = "retained_migration_resources_v1";
const RESOURCE_MARKER_SCHEMA_VERSION: u32 = 1;
const MIGRATED_TABLES: &[&str] = &[
    "core_providers",
    "core_mcp_servers",
    "core_prompt_versions",
    "core_skills",
    "core_common_snippets",
];

impl Database {
    pub(crate) fn migrate_retained_snapshot(
        &self,
        snapshot: &LegacyRetainedSnapshot,
        completed_at_ms: i64,
    ) -> Result<RetainedMigrationReport, AppError> {
        if completed_at_ms < 0 {
            return Err(AppError::Database(
                "保留数据迁移完成时间不能为负数".to_string(),
            ));
        }
        validate_source_snapshot(snapshot)?;
        let expected_content = expected_content(snapshot)?;
        let expected_hash = sha256_json(&expected_content)?;
        let mut conn = lock_conn!(self.conn);

        if let Some(mut report) = load_report(&conn)? {
            if report.source_fingerprint != snapshot.source_fingerprint
                || report.source != snapshot.source
                || report.source_version != snapshot.source_version
                || report.retained != snapshot.counts()
                || report.content_sha256 != expected_hash
            {
                return Err(AppError::Database(
                    "目标数据库已有来自不同源的保留数据迁移记录".to_string(),
                ));
            }
            validate_committed_content(&conn, snapshot, &expected_hash)?;
            report.status = RetainedMigrationStatus::AlreadyApplied;
            return Ok(report);
        }

        require_empty_targets(&conn)?;
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(format!("开始保留数据迁移事务失败: {error}")))?;
        insert_snapshot(&tx, snapshot)?;
        validate_committed_content(&tx, snapshot, &expected_hash)?;

        let report = RetainedMigrationReport {
            schema_version: RetainedMigrationReport::SCHEMA_VERSION,
            status: RetainedMigrationStatus::Applied,
            source: snapshot.source,
            source_version: snapshot.source_version,
            source_fingerprint: snapshot.source_fingerprint.clone(),
            retained: snapshot.counts(),
            content_sha256: expected_hash,
            completed_at_ms,
        };
        let report_json = serde_json::to_string(&report)
            .map_err(|error| AppError::Database(format!("序列化迁移报告失败: {error}")))?;
        tx.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)",
            params![MIGRATION_MARKER_KEY, report_json, completed_at_ms],
        )
        .map_err(|error| AppError::Database(format!("写入迁移完成标记失败: {error}")))?;
        tx.commit()
            .map_err(|error| AppError::Database(format!("提交保留数据迁移失败: {error}")))?;
        Ok(report)
    }

    pub(crate) fn rollback_retained_snapshot(
        &self,
        source_fingerprint: &str,
    ) -> Result<(), AppError> {
        let mut conn = lock_conn!(self.conn);
        let Some(report) = load_report(&conn)? else {
            return Ok(());
        };
        if report.source_fingerprint != source_fingerprint {
            return Err(AppError::Database(
                "拒绝回滚来自不同源的保留数据迁移".to_string(),
            ));
        }
        let tx = conn
            .transaction()
            .map_err(|error| AppError::Database(format!("开始迁移回滚事务失败: {error}")))?;
        for table in MIGRATED_TABLES {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .map_err(|error| AppError::Database(format!("清理迁移表 {table} 失败: {error}")))?;
        }
        tx.execute(
            "DELETE FROM core_settings
             WHERE key IN (?1, ?2) OR key IN (
                 'current_provider_claude',
                 'current_provider_codex',
                 'current_provider_opencode'
             )",
            [MIGRATION_MARKER_KEY, RESOURCE_MARKER_KEY],
        )
        .map_err(|error| AppError::Database(format!("清理迁移设置失败: {error}")))?;
        tx.commit()
            .map_err(|error| AppError::Database(format!("提交迁移回滚失败: {error}")))
    }

    fn retained_resources_complete_for(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let report = load_report(&conn)?;
        let value: Option<String> = conn
            .query_row(
                "SELECT value_json FROM core_settings WHERE key = ?1",
                [RESOURCE_MARKER_KEY],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| AppError::Database(format!("读取跨资源迁移完成标记失败: {error}")))?;
        let Some(value) = value else {
            if let Some(report) = report.as_ref() {
                validate_report_and_content(&conn, report)?;
            }
            return Ok(false);
        };
        let marker = serde_json::from_str::<RetainedResourcesMarker>(&value)
            .map_err(|error| AppError::Database(format!("跨资源迁移完成标记损坏: {error}")))?;
        validate_resource_marker(&marker)?;
        let report = report.ok_or_else(|| {
            AppError::Database("跨资源迁移完成标记存在，但数据库迁移标记缺失".to_string())
        })?;
        validate_report_and_content(&conn, &report)?;
        if marker.source_fingerprint != report.source_fingerprint
            || marker.content_sha256 != report.content_sha256
            || marker.migration_completed_at_ms != report.completed_at_ms
        {
            return Err(AppError::Database(
                "跨资源迁移完成标记与数据库迁移标记不一致".to_string(),
            ));
        }
        Ok(true)
    }

    fn mark_retained_resources_complete_for(
        &self,
        source_fingerprint: &str,
        completed_at_ms: i64,
    ) -> Result<(), AppError> {
        if completed_at_ms < 0 {
            return Err(AppError::Database(
                "跨资源迁移完成时间不能为负数".to_string(),
            ));
        }
        let conn = lock_conn!(self.conn);
        let report = load_report(&conn)?.ok_or_else(|| {
            AppError::Database("数据库保留数据尚未迁移，不能标记跨资源完成".to_string())
        })?;
        validate_report_and_content(&conn, &report)?;
        if report.source_fingerprint != source_fingerprint {
            return Err(AppError::Database(
                "拒绝为不同来源标记跨资源迁移完成".to_string(),
            ));
        }
        let value = serde_json::to_string(&RetainedResourcesMarker {
            schema_version: RESOURCE_MARKER_SCHEMA_VERSION,
            source_fingerprint: source_fingerprint.to_string(),
            content_sha256: report.content_sha256,
            migration_completed_at_ms: report.completed_at_ms,
            resources_completed_at_ms: completed_at_ms,
        })
        .map_err(|error| AppError::Database(format!("序列化跨资源迁移完成标记失败: {error}")))?;
        conn.execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)
             ON CONFLICT(key) DO UPDATE SET
                value_json = excluded.value_json,
                storage_scope = excluded.storage_scope,
                updated_at_ms = excluded.updated_at_ms",
            params![RESOURCE_MARKER_KEY, value, completed_at_ms],
        )
        .map_err(|error| AppError::Database(format!("写入跨资源迁移完成标记失败: {error}")))?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetainedResourcesMarker {
    schema_version: u32,
    source_fingerprint: String,
    content_sha256: String,
    migration_completed_at_ms: i64,
    resources_completed_at_ms: i64,
}

fn validate_resource_marker(marker: &RetainedResourcesMarker) -> Result<(), AppError> {
    if marker.schema_version != RESOURCE_MARKER_SCHEMA_VERSION {
        return Err(AppError::Database("跨资源迁移完成标记版本无效".to_string()));
    }
    validate_fingerprint(&marker.source_fingerprint)?;
    validate_sha256(&marker.content_sha256, "跨资源迁移内容哈希")?;
    if marker.migration_completed_at_ms < 0 || marker.resources_completed_at_ms < 0 {
        return Err(AppError::Database("跨资源迁移完成标记时间无效".to_string()));
    }
    Ok(())
}

fn validate_report_and_content(
    conn: &Connection,
    report: &RetainedMigrationReport,
) -> Result<(), AppError> {
    if report.schema_version != RetainedMigrationReport::SCHEMA_VERSION
        || report.source_version == 0
        || report.completed_at_ms < 0
    {
        return Err(AppError::Database("数据库迁移完成标记内容无效".to_string()));
    }
    validate_fingerprint(&report.source_fingerprint)?;
    validate_sha256(&report.content_sha256, "数据库迁移内容哈希")?;
    let actual_hash = sha256_json(&committed_content(conn)?)?;
    if actual_hash != report.content_sha256 {
        return Err(AppError::Database(
            "数据库迁移完成标记与实际保留数据不一致".to_string(),
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), AppError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Database(format!("{label}无效")))
    }
}

impl RetainedMigrationTarget for Database {
    fn apply_retained(
        &self,
        snapshot: &LegacyRetainedSnapshot,
        completed_at_ms: i64,
    ) -> Result<RetainedMigrationReport, RetainedMigrationTargetError> {
        self.migrate_retained_snapshot(snapshot, completed_at_ms)
            .map_err(|error| RetainedMigrationTargetError::new(error.to_string()))
    }

    fn rollback_retained(
        &self,
        source_fingerprint: &str,
    ) -> Result<(), RetainedMigrationTargetError> {
        self.rollback_retained_snapshot(source_fingerprint)
            .map_err(|error| RetainedMigrationTargetError::new(error.to_string()))
    }

    fn retained_resources_complete(&self) -> Result<bool, RetainedMigrationTargetError> {
        self.retained_resources_complete_for()
            .map_err(|error| RetainedMigrationTargetError::new(error.to_string()))
    }

    fn mark_retained_resources_complete(
        &self,
        source_fingerprint: &str,
        completed_at_ms: i64,
    ) -> Result<(), RetainedMigrationTargetError> {
        self.mark_retained_resources_complete_for(source_fingerprint, completed_at_ms)
            .map_err(|error| RetainedMigrationTargetError::new(error.to_string()))
    }
}

fn validate_source_snapshot(snapshot: &LegacyRetainedSnapshot) -> Result<(), AppError> {
    if snapshot.source_version == 0 {
        return Err(AppError::Database("保留数据源版本或指纹无效".to_string()));
    }
    validate_fingerprint(&snapshot.source_fingerprint)
}

fn validate_fingerprint(fingerprint: &str) -> Result<(), AppError> {
    if fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::Database("保留数据源指纹无效".to_string()))
    }
}

fn load_report(conn: &Connection) -> Result<Option<RetainedMigrationReport>, AppError> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value_json FROM core_settings WHERE key = ?1",
            [MIGRATION_MARKER_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| AppError::Database(format!("读取迁移完成标记失败: {error}")))?;
    value
        .map(|value| {
            serde_json::from_str::<RetainedMigrationReport>(&value)
                .map_err(|error| AppError::Database(format!("迁移完成标记损坏: {error}")))
        })
        .transpose()
}

fn require_empty_targets(conn: &Connection) -> Result<(), AppError> {
    core_schema::validate_core_schema(conn)?;
    for table in MIGRATED_TABLES {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| AppError::Database(format!("检查迁移目标 {table} 失败: {error}")))?;
        if count != 0 {
            return Err(AppError::Database(format!(
                "迁移目标 {table} 已有数据，拒绝静默覆盖"
            )));
        }
    }
    let owned_settings: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM core_settings WHERE key IN (
                'current_provider_claude',
                'current_provider_codex',
                'current_provider_opencode'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| AppError::Database(format!("检查迁移设备设置失败: {error}")))?;
    if owned_settings != 0 {
        return Err(AppError::Database(
            "迁移目标已有设备级供应商选择，拒绝静默覆盖".to_string(),
        ));
    }
    Ok(())
}

fn insert_snapshot(
    tx: &Transaction<'_>,
    snapshot: &LegacyRetainedSnapshot,
) -> Result<(), AppError> {
    for provider in &snapshot.providers {
        let settings: Value = parse_json("供应商配置", &provider.settings_config_json)?;
        let metadata: Value = parse_json("供应商元数据", &provider.meta_json)?;
        let local = json!({"settingsConfig": settings, "meta": metadata});
        let portable = redact_sensitive_json(&local);
        let quota = metadata
            .get("usage_script")
            .or_else(|| metadata.get("usageScript"))
            .map(redact_sensitive_json)
            .unwrap_or_else(|| json!({}));
        let kind = if provider.category.as_deref() == Some("official") {
            "official"
        } else {
            "custom"
        };
        tx.execute(
            "INSERT INTO core_providers (
                id, client_id, kind, name, portable_config_json, local_config_json,
                quota_config_json, sort_index, notes, icon, icon_color,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                provider.id,
                provider.client_id,
                kind,
                provider.name,
                json_string(&portable)?,
                json_string(&local)?,
                json_string(&quota)?,
                provider.sort_index,
                provider.notes,
                provider.icon,
                provider.icon_color,
                provider.created_at_ms.max(0),
                provider.created_at_ms.max(0),
            ],
        )
        .map_err(|error| AppError::Database(format!("迁移供应商记录失败: {error}")))?;
        if provider.is_current {
            let key = format!("current_provider_{}", provider.client_id);
            tx.execute(
                "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
                 VALUES (?1, ?2, 'device', ?3)",
                params![
                    key,
                    json_string(&Value::String(provider.id.clone()))?,
                    provider.created_at_ms.max(0)
                ],
            )
            .map_err(|error| AppError::Database(format!("迁移当前供应商失败: {error}")))?;
        }
    }

    for server in &snapshot.mcp_servers {
        parse_json("MCP 配置", &server.server_config_json)?;
        parse_json("MCP 标签", &server.tags_json)?;
        tx.execute(
            "INSERT INTO core_mcp_servers (
                id, name, server_config_json, description, homepage, docs, tags_json,
                enabled_claude, enabled_codex, enabled_opencode, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0, 0)",
            params![
                server.id,
                server.name,
                server.server_config_json,
                server.description,
                server.homepage,
                server.docs,
                server.tags_json,
                server.enabled_claude,
                server.enabled_codex,
                server.enabled_opencode,
            ],
        )
        .map_err(|error| AppError::Database(format!("迁移 MCP 记录失败: {error}")))?;
    }

    for prompt in &snapshot.prompts {
        tx.execute(
            "INSERT INTO core_prompt_versions (
                id, client_id, name, version, content, description, is_active,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8)",
            params![
                prompt.id,
                prompt.client_id,
                prompt.name,
                prompt.content,
                prompt.description,
                prompt.enabled,
                prompt.created_at_ms.max(0),
                prompt.updated_at_ms.max(prompt.created_at_ms).max(0),
            ],
        )
        .map_err(|error| AppError::Database(format!("迁移 Prompt 记录失败: {error}")))?;
    }

    for skill in &snapshot.skills {
        tx.execute(
            "INSERT INTO core_skills (
                id, name, description, directory, content_hash, total_size_bytes, file_count,
                enabled_claude, enabled_codex, enabled_opencode, cloud_eligible,
                created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0, ?6, ?7, ?8, 0, ?9, ?10)",
            params![
                skill.id,
                skill.name,
                skill.description,
                skill.directory,
                skill.content_hash,
                skill.enabled_claude,
                skill.enabled_codex,
                skill.enabled_opencode,
                skill.created_at_ms.max(0),
                skill.updated_at_ms.max(skill.created_at_ms).max(0),
            ],
        )
        .map_err(|error| AppError::Database(format!("迁移 Skill 记录失败: {error}")))?;
    }

    for snippet in &snapshot.common_snippets {
        tx.execute(
            "INSERT INTO core_common_snippets (
                id, client_id, name, content, provider_id, enabled, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, NULL, 1, 0, 0)",
            params![
                snippet.id,
                snippet.client_id,
                format!("{} common configuration", snippet.client_id),
                snippet.content,
            ],
        )
        .map_err(|error| AppError::Database(format!("迁移通用配置片段失败: {error}")))?;
    }
    Ok(())
}

fn validate_committed_content(
    conn: &Connection,
    snapshot: &LegacyRetainedSnapshot,
    expected_hash: &str,
) -> Result<(), AppError> {
    let counts = snapshot.counts();
    let expected = [
        (
            "core_providers",
            counts.claude_providers + counts.codex_providers + counts.opencode_providers,
        ),
        ("core_mcp_servers", counts.mcp_servers),
        (
            "core_prompt_versions",
            counts.claude_prompts + counts.codex_prompts + counts.opencode_prompts,
        ),
        ("core_skills", counts.skills),
        ("core_common_snippets", counts.common_snippets),
    ];
    for (table, expected_count) in expected {
        let actual: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| AppError::Database(format!("校验迁移表 {table} 失败: {error}")))?;
        if u64::try_from(actual).ok() != Some(expected_count) {
            return Err(AppError::Database(format!("迁移表 {table} 记录数量不匹配")));
        }
    }
    let actual_hash = sha256_json(&committed_content(conn)?)?;
    if actual_hash != expected_hash {
        return Err(AppError::Database(
            "保留数据迁移内容完整性校验失败".to_string(),
        ));
    }
    let quick_check: String = conn
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(|error| AppError::Database(format!("迁移后数据库完整性检查失败: {error}")))?;
    if quick_check != "ok" {
        return Err(AppError::Database(format!(
            "迁移后数据库完整性检查失败: {quick_check}"
        )));
    }
    Ok(())
}

fn expected_content(snapshot: &LegacyRetainedSnapshot) -> Result<Value, AppError> {
    let memory = Connection::open_in_memory()
        .map_err(|error| AppError::Database(format!("创建迁移验证数据库失败: {error}")))?;
    memory
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| AppError::Database(format!("初始化迁移验证数据库失败: {error}")))?;
    core_schema::create_core_schema(&memory)?;
    let tx = memory
        .unchecked_transaction()
        .map_err(|error| AppError::Database(format!("开始迁移验证事务失败: {error}")))?;
    insert_snapshot(&tx, snapshot)?;
    tx.commit()
        .map_err(|error| AppError::Database(format!("提交迁移验证事务失败: {error}")))?;
    committed_content(&memory)
}

fn committed_content(conn: &Connection) -> Result<Value, AppError> {
    Ok(json!({
        "providers": query_json_rows(conn, "SELECT id, client_id, kind, name, portable_config_json, local_config_json, quota_config_json, sort_index, notes, icon, icon_color, created_at_ms, updated_at_ms FROM core_providers ORDER BY client_id, id", 13)?,
        "mcp": query_json_rows(conn, "SELECT id, name, server_config_json, description, homepage, docs, tags_json, enabled_claude, enabled_codex, enabled_opencode, created_at_ms, updated_at_ms FROM core_mcp_servers ORDER BY id", 12)?,
        "prompts": query_json_rows(conn, "SELECT id, client_id, name, version, content, description, is_active, created_at_ms, updated_at_ms FROM core_prompt_versions ORDER BY client_id, id, version", 9)?,
        "skills": query_json_rows(conn, "SELECT id, name, description, directory, content_hash, total_size_bytes, file_count, enabled_claude, enabled_codex, enabled_opencode, cloud_eligible, created_at_ms, updated_at_ms FROM core_skills ORDER BY id", 13)?,
        "snippets": query_json_rows(conn, "SELECT id, client_id, name, content, provider_id, enabled, created_at_ms, updated_at_ms FROM core_common_snippets ORDER BY client_id, id", 8)?,
        "deviceSettings": query_json_rows(conn, "SELECT key, value_json, storage_scope, updated_at_ms FROM core_settings WHERE key IN ('current_provider_claude', 'current_provider_codex', 'current_provider_opencode') ORDER BY key", 4)?,
    }))
}

fn query_json_rows(
    conn: &Connection,
    sql: &str,
    column_count: usize,
) -> Result<Vec<Value>, AppError> {
    let mut statement = conn
        .prepare(sql)
        .map_err(|error| AppError::Database(format!("准备迁移校验查询失败: {error}")))?;
    let mut rows = statement
        .query([])
        .map_err(|error| AppError::Database(format!("执行迁移校验查询失败: {error}")))?;
    let mut output = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|error| AppError::Database(format!("读取迁移校验行失败: {error}")))?
    {
        let mut values = Vec::with_capacity(column_count);
        for index in 0..column_count {
            let value = row
                .get_ref(index)
                .map_err(|error| AppError::Database(format!("读取迁移校验字段失败: {error}")))?;
            values.push(match value {
                rusqlite::types::ValueRef::Null => Value::Null,
                rusqlite::types::ValueRef::Integer(value) => json!(value),
                rusqlite::types::ValueRef::Real(value) => json!(value),
                rusqlite::types::ValueRef::Text(value) => {
                    Value::String(String::from_utf8_lossy(value).into_owned())
                }
                rusqlite::types::ValueRef::Blob(value) => Value::String(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    value,
                )),
            });
        }
        output.push(Value::Array(values));
    }
    Ok(output)
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
    let normalized: String = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    [
        "apikey",
        "authtoken",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "githubaccountid",
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

fn parse_json(label: &str, value: &str) -> Result<Value, AppError> {
    serde_json::from_str(value)
        .map_err(|error| AppError::Database(format!("解析{label}失败: {error}")))
}

fn json_string(value: &Value) -> Result<String, AppError> {
    serde_json::to_string(value)
        .map_err(|error| AppError::Database(format!("序列化迁移内容失败: {error}")))
}

fn sha256_json(value: &Value) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| AppError::Database(format!("序列化完整性数据失败: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests;
