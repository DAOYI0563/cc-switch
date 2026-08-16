//! Final core-only SQLite schema for the retained product domains.

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

pub(crate) const CORE_TABLES: &[&str] = &[
    "core_providers",
    "core_mcp_servers",
    "core_prompt_versions",
    "core_skills",
    "core_common_snippets",
    "core_conflicts",
    "core_sync_records",
    "core_sync_devices",
    "core_daily_briefs",
    "core_brief_checkpoints",
    "core_settings",
];

const REQUIRED_COLUMNS: &[(&str, &[&str])] = &[
    (
        "core_providers",
        &[
            "id",
            "client_id",
            "kind",
            "name",
            "portable_config_json",
            "local_config_json",
            "created_at_ms",
            "updated_at_ms",
        ],
    ),
    (
        "core_mcp_servers",
        &[
            "id",
            "name",
            "server_config_json",
            "enabled_claude",
            "enabled_codex",
            "enabled_opencode",
        ],
    ),
    (
        "core_prompt_versions",
        &["id", "client_id", "name", "version", "content", "is_active"],
    ),
    (
        "core_skills",
        &[
            "id",
            "name",
            "directory",
            "content_hash",
            "enabled_claude",
            "enabled_codex",
            "enabled_opencode",
            "cloud_eligible",
        ],
    ),
    (
        "core_common_snippets",
        &[
            "id",
            "client_id",
            "name",
            "content",
            "provider_id",
            "enabled",
        ],
    ),
    (
        "core_conflicts",
        &[
            "conflict_id",
            "domain",
            "record_key",
            "kind",
            "status",
            "local_json",
            "external_json",
        ],
    ),
    (
        "core_sync_records",
        &[
            "domain",
            "record_key",
            "record_version",
            "device_id",
            "content_hash",
            "baseline_json",
            "tombstone",
        ],
    ),
    (
        "core_sync_devices",
        &[
            "device_id",
            "device_name",
            "last_confirmed_generation",
            "retired_at_ms",
        ],
    ),
    (
        "core_daily_briefs",
        &[
            "date",
            "device_id",
            "status",
            "source_fingerprint",
            "content_hash",
            "local_path",
        ],
    ),
    (
        "core_brief_checkpoints",
        &["date", "device_id", "protected_blob", "expires_at_ms"],
    ),
    (
        "core_settings",
        &["key", "value_json", "storage_scope", "updated_at_ms"],
    ),
];

pub(crate) fn create_core_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(CORE_SCHEMA_SQL)
        .map_err(|error| AppError::Database(format!("创建核心 schema 失败: {error}")))?;
    validate_core_schema(conn)
}

pub(super) fn ensure_prompt_active_index(conn: &Connection) -> Result<(), AppError> {
    let metadata: Option<(i64, i64)> = conn
        .query_row(
            "SELECT \"unique\", partial
             FROM pragma_index_list('core_prompt_versions')
             WHERE name = 'idx_core_prompt_one_active'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| AppError::Database(format!("检查 Prompt 启用索引元数据失败: {error}")))?;
    let columns = if metadata.is_some() {
        let mut statement = conn
            .prepare(
                "SELECT name FROM pragma_index_info('idx_core_prompt_one_active') ORDER BY seqno",
            )
            .map_err(|error| AppError::Database(format!("检查 Prompt 启用索引列失败: {error}")))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| AppError::Database(format!("读取 Prompt 启用索引列失败: {error}")))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(format!("解析 Prompt 启用索引列失败: {error}")))?;
        columns
    } else {
        Vec::new()
    };
    if metadata == Some((1, 1)) && columns == ["client_id"] {
        return Ok(());
    }

    conn.execute(
        "UPDATE core_prompt_versions
         SET is_active = 0
         WHERE is_active = 1
           AND EXISTS (
               SELECT 1
               FROM core_prompt_versions AS winner
               WHERE winner.client_id = core_prompt_versions.client_id
                 AND winner.is_active = 1
                 AND (
                     winner.updated_at_ms > core_prompt_versions.updated_at_ms
                     OR (
                         winner.updated_at_ms = core_prompt_versions.updated_at_ms
                         AND winner.created_at_ms > core_prompt_versions.created_at_ms
                     )
                     OR (
                         winner.updated_at_ms = core_prompt_versions.updated_at_ms
                         AND winner.created_at_ms = core_prompt_versions.created_at_ms
                         AND winner.id > core_prompt_versions.id
                     )
                 )
           )",
        [],
    )
    .map_err(|error| AppError::Database(format!("归一化 Prompt 启用记录失败: {error}")))?;
    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_core_prompt_one_active;
         CREATE UNIQUE INDEX idx_core_prompt_one_active
             ON core_prompt_versions(client_id) WHERE is_active = 1;",
    )
    .map_err(|error| AppError::Database(format!("修复 Prompt 启用索引失败: {error}")))
}

pub(super) fn validate_core_schema(conn: &Connection) -> Result<(), AppError> {
    for table in CORE_TABLES {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|error| AppError::Database(format!("校验核心表 {table} 失败: {error}")))?;
        if count != 1 {
            return Err(AppError::Database(format!("核心表 {table} 不存在")));
        }
    }
    for (table, columns) in REQUIRED_COLUMNS {
        for column in *columns {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    params![table, column],
                    |row| row.get(0),
                )
                .map_err(|error| AppError::Database(format!("校验核心表 {table} 失败: {error}")))?;
            if count != 1 {
                return Err(AppError::Database(format!(
                    "核心表 {table} 不兼容：缺少列 {column}"
                )));
            }
        }
    }
    Ok(())
}

const CORE_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS core_providers (
    id TEXT NOT NULL,
    client_id TEXT NOT NULL CHECK (client_id IN ('claude', 'codex', 'opencode')),
    kind TEXT NOT NULL CHECK (kind IN ('official', 'custom')),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    portable_config_json TEXT NOT NULL DEFAULT '{}',
    local_config_json TEXT NOT NULL DEFAULT '{}',
    quota_config_json TEXT NOT NULL DEFAULT '{}',
    sort_index INTEGER NOT NULL DEFAULT 0,
    notes TEXT,
    icon TEXT,
    icon_color TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (id, client_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_core_providers_official_client
    ON core_providers(client_id) WHERE kind = 'official';
CREATE INDEX IF NOT EXISTS idx_core_providers_client_sort
    ON core_providers(client_id, sort_index, name);

CREATE TABLE IF NOT EXISTS core_mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    server_config_json TEXT NOT NULL,
    description TEXT,
    homepage TEXT,
    docs TEXT,
    tags_json TEXT NOT NULL DEFAULT '[]',
    enabled_claude INTEGER NOT NULL DEFAULT 0 CHECK (enabled_claude IN (0, 1)),
    enabled_codex INTEGER NOT NULL DEFAULT 0 CHECK (enabled_codex IN (0, 1)),
    enabled_opencode INTEGER NOT NULL DEFAULT 0 CHECK (enabled_opencode IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS core_prompt_versions (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL CHECK (client_id IN ('claude', 'codex', 'opencode')),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    version INTEGER NOT NULL CHECK (version > 0),
    content TEXT NOT NULL,
    description TEXT,
    is_active INTEGER NOT NULL DEFAULT 0 CHECK (is_active IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (client_id, name, version)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_core_prompt_one_active
    ON core_prompt_versions(client_id) WHERE is_active = 1;
CREATE TRIGGER IF NOT EXISTS trg_core_prompt_limit_insert
BEFORE INSERT ON core_prompt_versions
WHEN (SELECT COUNT(*) FROM core_prompt_versions
      WHERE client_id = NEW.client_id AND name = NEW.name) >= 20
BEGIN
    SELECT RAISE(ABORT, 'prompt version limit reached');
END;
CREATE TRIGGER IF NOT EXISTS trg_core_prompt_limit_move
BEFORE UPDATE OF client_id, name ON core_prompt_versions
WHEN (OLD.client_id <> NEW.client_id OR OLD.name <> NEW.name)
 AND (SELECT COUNT(*) FROM core_prompt_versions
      WHERE client_id = NEW.client_id AND name = NEW.name) >= 20
BEGIN
    SELECT RAISE(ABORT, 'prompt version limit reached');
END;

CREATE TABLE IF NOT EXISTS core_skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT,
    directory TEXT NOT NULL,
    content_hash TEXT,
    total_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (total_size_bytes >= 0),
    file_count INTEGER NOT NULL DEFAULT 0 CHECK (file_count >= 0),
    enabled_claude INTEGER NOT NULL DEFAULT 0 CHECK (enabled_claude IN (0, 1)),
    enabled_codex INTEGER NOT NULL DEFAULT 0 CHECK (enabled_codex IN (0, 1)),
    enabled_opencode INTEGER NOT NULL DEFAULT 0 CHECK (enabled_opencode IN (0, 1)),
    cloud_eligible INTEGER NOT NULL DEFAULT 1 CHECK (cloud_eligible IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_core_skills_directory ON core_skills(directory);

CREATE TABLE IF NOT EXISTS core_common_snippets (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL CHECK (client_id IN ('claude', 'codex')),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    content TEXT NOT NULL,
    provider_id TEXT,
    enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_core_snippets_client_provider
    ON core_common_snippets(client_id, provider_id, enabled);

CREATE TABLE IF NOT EXISTS core_conflicts (
    conflict_id TEXT PRIMARY KEY,
    domain TEXT NOT NULL CHECK (domain IN (
        'provider', 'mcp', 'prompt', 'skill', 'common_snippet',
        'daily_brief', 'portable_setting'
    )),
    record_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'concurrent_update', 'update_delete', 'ambiguous_local_match', 'integrity_mismatch'
    )),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN (
        'pending', 'resolved_local', 'resolved_external', 'kept_both', 'dismissed'
    )),
    local_json TEXT NOT NULL,
    external_json TEXT NOT NULL,
    local_summary_hash TEXT NOT NULL,
    external_summary_hash TEXT NOT NULL,
    detected_at_ms INTEGER NOT NULL,
    resolved_at_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_core_conflicts_status_detected
    ON core_conflicts(status, detected_at_ms DESC);

CREATE TABLE IF NOT EXISTS core_sync_records (
    domain TEXT NOT NULL CHECK (domain IN (
        'provider', 'mcp', 'prompt', 'skill', 'common_snippet',
        'daily_brief', 'portable_setting'
    )),
    record_key TEXT NOT NULL,
    record_version INTEGER NOT NULL CHECK (record_version >= 0),
    device_id TEXT NOT NULL,
    content_hash TEXT,
    baseline_json TEXT,
    tombstone INTEGER NOT NULL DEFAULT 0 CHECK (tombstone IN (0, 1)),
    deleted_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    last_sync_generation INTEGER NOT NULL DEFAULT 0 CHECK (last_sync_generation >= 0),
    PRIMARY KEY (domain, record_key),
    CHECK (
        (tombstone = 0 AND content_hash IS NOT NULL AND deleted_at_ms IS NULL)
        OR (tombstone = 1 AND deleted_at_ms IS NOT NULL)
    )
);
CREATE INDEX IF NOT EXISTS idx_core_sync_records_generation
    ON core_sync_records(last_sync_generation, domain);

CREATE TABLE IF NOT EXISTS core_sync_devices (
    device_id TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    last_confirmed_generation INTEGER NOT NULL DEFAULT 0 CHECK (last_confirmed_generation >= 0),
    registered_at_ms INTEGER NOT NULL,
    last_seen_at_ms INTEGER NOT NULL,
    retired_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS core_daily_briefs (
    date TEXT NOT NULL,
    device_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'disabled', 'pending', 'waiting_for_stability', 'running', 'pending_resume',
        'complete', 'failed', 'no_sessions', 'integrity_invalid'
    )),
    source_fingerprint TEXT,
    content_hash TEXT,
    local_path TEXT,
    source_state TEXT NOT NULL DEFAULT 'present' CHECK (source_state IN ('present', 'changed', 'missing')),
    model_name TEXT,
    template_version TEXT,
    prompt_version TEXT,
    generated_at_ms INTEGER,
    updated_at_ms INTEGER NOT NULL,
    PRIMARY KEY (date, device_id),
    CHECK (status <> 'complete' OR content_hash IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_core_daily_briefs_status_date
    ON core_daily_briefs(status, date DESC);

CREATE TABLE IF NOT EXISTS core_brief_checkpoints (
    date TEXT NOT NULL,
    device_id TEXT NOT NULL,
    protected_blob BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    PRIMARY KEY (date, device_id),
    CHECK (expires_at_ms > created_at_ms)
);
CREATE INDEX IF NOT EXISTS idx_core_brief_checkpoints_expiry
    ON core_brief_checkpoints(expires_at_ms);

CREATE TABLE IF NOT EXISTS core_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL,
    storage_scope TEXT NOT NULL CHECK (storage_scope IN ('device', 'portable')),
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_core_settings_scope ON core_settings(storage_scope, key);
"#;
