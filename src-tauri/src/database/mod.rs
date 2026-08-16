//! 数据库模块 - SQLite 数据持久化
//!
//! 此模块提供应用的核心数据存储功能，包括：
//! - 供应商配置管理
//! - MCP 服务器配置
//! - 提示词管理
//! - Skills 管理
//! - 通用设置存储
//!
//! ## 架构设计
//!
//! ```text
//! database/
//! ├── mod.rs        - Database 结构体 + 初始化
//! ├── schema.rs     - 表结构定义 + Schema 迁移
//! ├── backup.rs     - SQL 导入导出 + 快照备份
//! ├── migration.rs  - JSON → SQLite 数据迁移
//! └── dao/          - 数据访问对象
//!     ├── providers.rs
//!     ├── mcp.rs
//!     ├── prompts.rs
//!     ├── skills.rs
//!     └── settings.rs
//! ```

pub(crate) mod backup;
mod core_schema;
mod dao;
mod retained_migration;
mod schema;

// DAO 类型导出供外部使用
use crate::config::get_app_config_dir;
use crate::error::AppError;
pub use dao::daily_brief::{DailyBriefDeviceIdentity, DailyBriefRecord};
use rusqlite::Connection;
use std::sync::Mutex;

// DAO 方法通过 impl Database 提供，无需额外导出

/// 当前 Schema 版本号
/// 每次修改表结构时递增，并在 schema.rs 中添加相应的迁移逻辑
pub(crate) const SCHEMA_VERSION: i32 = 18;

/// 安全地获取 Mutex 锁，避免 unwrap panic
macro_rules! lock_conn {
    ($mutex:expr) => {
        $mutex
            .lock()
            .map_err(|e| AppError::Database(format!("Mutex lock failed: {}", e)))?
    };
}

// 导出宏供子模块使用
pub(crate) use lock_conn;

/// 数据库连接封装
///
/// 使用 Mutex 包装 Connection 以支持在多线程环境（如 Tauri State）中共享。
/// rusqlite::Connection 本身不是 Sync 的，因此需要这层包装。
pub struct Database {
    pub(crate) conn: Mutex<Connection>,
}

impl Database {
    /// 初始化数据库连接并创建表
    ///
    /// 数据库文件位于 `~/.wsl-code-switch/cc-switch.db`
    pub fn init() -> Result<Self, AppError> {
        let db_path = get_app_config_dir().join("cc-switch.db");
        let db_exists = db_path.exists();

        // 确保父目录存在
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }

        let conn = Connection::open(&db_path).map_err(|e| AppError::Database(e.to_string()))?;

        // 启用外键约束
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        if !db_exists {
            // For a brand-new database, configure incremental auto-vacuum
            // before creating any tables so no rebuild is needed later.
            conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
                .map_err(|e| AppError::Database(e.to_string()))?;
        }

        // Inspect and protect an existing database before create_tables() can
        // repair or otherwise mutate any legacy schema objects.
        let initial_version = Self::get_user_version(&conn)?;
        if initial_version > SCHEMA_VERSION {
            return Err(AppError::Database(format!(
                "数据库版本过新（{initial_version}），当前应用仅支持 {SCHEMA_VERSION}，请升级应用后再尝试。"
            )));
        }

        let db = Self {
            conn: Mutex::new(conn),
        };
        db.create_tables()?;
        db.apply_schema_migrations()?;
        if let Err(e) = db.ensure_incremental_auto_vacuum() {
            log::warn!("Failed to ensure incremental auto-vacuum: {e}");
        }
        Ok(db)
    }

    /// 读取磁盘上数据库的 `user_version`；仅当它比应用支持的 [`SCHEMA_VERSION`]
    /// 更新时返回 `Some(version)`。
    ///
    /// 用于初始化失败后判断是否为「数据库版本过新（应用过旧，需升级应用）」的可恢复
    /// 场景——此时不应反复弹出无效的重试对话框，而应引导用户在应用内升级。
    pub fn stored_user_version_exceeds_supported(
        db_path: &std::path::Path,
    ) -> Result<Option<i32>, AppError> {
        if !db_path.exists() {
            return Ok(None);
        }
        let conn = Connection::open(db_path).map_err(|e| AppError::Database(e.to_string()))?;
        let version = Self::get_user_version(&conn)?;
        Ok((version > SCHEMA_VERSION).then_some(version))
    }

    /// 创建内存数据库（用于测试）
    pub fn memory() -> Result<Self, AppError> {
        let conn = Connection::open_in_memory().map_err(|e| AppError::Database(e.to_string()))?;

        // 启用外键约束
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(e.to_string()))?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.create_tables()?;
        db.apply_schema_migrations()?;
        Ok(db)
    }

    pub(crate) fn get_auto_vacuum_mode(conn: &Connection) -> Result<i32, AppError> {
        conn.query_row("PRAGMA auto_vacuum;", [], |row| row.get(0))
            .map_err(|e| AppError::Database(format!("读取 auto_vacuum 失败: {e}")))
    }

    fn has_user_tables(conn: &Connection) -> Result<bool, AppError> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("读取表数量失败: {e}")))?;
        Ok(count > 0)
    }

    pub(crate) fn ensure_incremental_auto_vacuum_on_conn(
        conn: &Connection,
    ) -> Result<bool, AppError> {
        let mode = Self::get_auto_vacuum_mode(conn)?;
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = Self::has_user_tables(conn)?;
        conn.execute("PRAGMA auto_vacuum = INCREMENTAL;", [])
            .map_err(|e| AppError::Database(format!("设置 auto_vacuum 失败: {e}")))?;

        if !has_tables {
            return Ok(false);
        }

        conn.execute("VACUUM;", [])
            .map_err(|e| AppError::Database(format!("执行 VACUUM 失败: {e}")))?;
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .map_err(|e| AppError::Database(format!("恢复 foreign_keys 失败: {e}")))?;
        Ok(true)
    }

    pub(crate) fn ensure_incremental_auto_vacuum(&self) -> Result<bool, AppError> {
        let mode = {
            let conn = lock_conn!(self.conn);
            Self::get_auto_vacuum_mode(&conn)?
        };
        if mode == 2 {
            return Ok(false);
        }

        let has_tables = {
            let conn = lock_conn!(self.conn);
            Self::has_user_tables(&conn)?
        };
        if has_tables {
            log::info!("Detected auto_vacuum={mode}, rebuilding database in place");
        }

        let rebuilt = {
            let conn = lock_conn!(self.conn);
            Self::ensure_incremental_auto_vacuum_on_conn(&conn)?
        };

        if rebuilt {
            log::info!("Incremental auto-vacuum enabled after database rebuild");
        } else {
            log::info!("Incremental auto-vacuum configured for new database");
        }

        Ok(rebuilt)
    }

    /// 检查 MCP 服务器表是否为空
    pub fn is_mcp_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM core_mcp_servers", [], |row| {
                row.get(0)
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }

    /// 检查提示词表是否为空
    pub fn is_prompts_table_empty(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM core_prompt_versions", [], |row| {
                row.get(0)
            })
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(count == 0)
    }
}
