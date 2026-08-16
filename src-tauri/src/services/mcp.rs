use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::adapters::mcp_live_files::{McpLiveFileAdapter, McpLiveFileSnapshot};
use crate::app_config::McpServer;
use crate::domain::{LocalScanDomain, LocalScanTarget, ManagedClientId};
use crate::error::AppError;
use crate::mcp;
use crate::store::AppState;

/// MCP 相关业务逻辑（v3.7.0 统一结构）
pub struct McpService;

fn mcp_mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn rollback_error(primary: AppError, failures: Vec<String>) -> AppError {
    if failures.is_empty() {
        primary
    } else {
        AppError::Message(format!(
            "{primary}; MCP live rollback also failed: {}",
            failures.join("; ")
        ))
    }
}

impl McpService {
    /// 获取所有 MCP 服务器（统一结构）
    pub fn get_all_servers(state: &AppState) -> Result<IndexMap<String, McpServer>, AppError> {
        state.db.get_all_mcp_servers()
    }

    /// 添加或更新 MCP 服务器
    pub fn upsert_server(state: &AppState, server: McpServer) -> Result<(), AppError> {
        let _guard = mcp_mutation_lock().lock()?;
        server
            .validate()
            .map_err(|error| AppError::McpValidation(error.to_string()))?;
        let previous = state.db.get_all_mcp_servers()?.get(&server.id).cloned();
        Self::apply_compensated_change(state, previous.as_ref(), Some(&server))
    }

    /// 删除 MCP 服务器
    pub fn delete_server(state: &AppState, id: &str) -> Result<bool, AppError> {
        let _guard = mcp_mutation_lock().lock()?;
        let server = state.db.get_all_mcp_servers()?.shift_remove(id);

        if let Some(server) = server {
            Self::apply_compensated_change(state, Some(&server), None)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 切换指定应用的启用状态
    pub fn toggle_app(
        state: &AppState,
        server_id: &str,
        client: ManagedClientId,
        enabled: bool,
    ) -> Result<(), AppError> {
        let _guard = mcp_mutation_lock().lock()?;
        let Some(previous) = state.db.get_all_mcp_servers()?.get(server_id).cloned() else {
            return Ok(());
        };
        let mut updated = previous.clone();
        updated.apps.set_enabled_for(client, enabled);
        Self::apply_compensated_change(state, Some(&previous), Some(&updated))
    }

    fn apply_compensated_change(
        state: &AppState,
        previous: Option<&McpServer>,
        target: Option<&McpServer>,
    ) -> Result<(), AppError> {
        let id = target
            .map(|server| server.id.as_str())
            .or_else(|| previous.map(|server| server.id.as_str()))
            .ok_or_else(|| AppError::McpValidation("MCP mutation has no record".to_string()))?;
        let affected: Vec<_> = ManagedClientId::ALL
            .into_iter()
            .filter(|client| {
                previous.is_some_and(|server| server.apps.is_enabled_for(*client))
                    || target.is_some_and(|server| server.apps.is_enabled_for(*client))
            })
            .collect();

        if let Some(server) = target {
            for client in server.apps.enabled_clients() {
                mcp::validate_server_for_client(server, client)?;
            }
        }

        let files = McpLiveFileAdapter::runtime();
        let snapshots = affected
            .iter()
            .map(|client| files.capture(*client))
            .collect::<Result<Vec<_>, _>>()?;

        let live_result = affected.iter().try_for_each(|client| {
            if target.is_some_and(|server| server.apps.is_enabled_for(*client)) {
                Self::sync_server_to_app_no_config(target.expect("target checked"), *client)
            } else {
                Self::remove_server_from_app(state, id, *client)
            }
        });
        if let Err(primary) = live_result {
            return Err(Self::restore_live_snapshots(&files, &snapshots, primary));
        }

        let database_result = match target {
            Some(server) => state.db.save_mcp_server(server),
            None => state.db.delete_mcp_server(id),
        };
        if let Err(primary) = database_result {
            return Err(Self::restore_live_snapshots(&files, &snapshots, primary));
        }

        crate::services::record_runtime_local_writes(
            &state.local_scan_writes,
            affected.into_iter().map(|client_id| LocalScanTarget {
                domain: LocalScanDomain::Mcp,
                client_id,
            }),
        );

        Ok(())
    }

    fn restore_live_snapshots(
        files: &McpLiveFileAdapter,
        snapshots: &[McpLiveFileSnapshot],
        primary: AppError,
    ) -> AppError {
        let mut failures = Vec::new();
        for snapshot in snapshots.iter().rev() {
            if let Err(error) = files.restore(snapshot) {
                failures.push(error.to_string());
            }
        }
        rollback_error(primary, failures)
    }

    /// 将 MCP 服务器同步到指定应用
    fn sync_server_to_app(
        _state: &AppState,
        server: &McpServer,
        client: ManagedClientId,
    ) -> Result<(), AppError> {
        Self::sync_server_to_app_no_config(server, client)
    }

    fn sync_server_to_app_no_config(
        server: &McpServer,
        client: ManagedClientId,
    ) -> Result<(), AppError> {
        match client {
            ManagedClientId::Claude => {
                mcp::sync_single_server_to_claude(&Default::default(), &server.id, &server.server)?;
            }
            ManagedClientId::Codex => {
                // Codex uses TOML format, must use the correct function
                mcp::sync_single_server_to_codex(&Default::default(), &server.id, &server.server)?;
            }
            ManagedClientId::Opencode => {
                mcp::sync_single_server_to_opencode(
                    &Default::default(),
                    &server.id,
                    &server.server,
                )?;
            }
        }
        Ok(())
    }

    fn remove_server_from_app(
        _state: &AppState,
        id: &str,
        client: ManagedClientId,
    ) -> Result<(), AppError> {
        match client {
            ManagedClientId::Claude => mcp::remove_server_from_claude(id)?,
            ManagedClientId::Codex => mcp::remove_server_from_codex(id)?,
            ManagedClientId::Opencode => {
                mcp::remove_server_from_opencode(id)?;
            }
        }
        Ok(())
    }

    /// 手动同步所有启用的 MCP 服务器到对应的应用。
    ///
    /// Best-effort：单个应用投影失败（如 ~/.claude.json 坏 JSON）不阻断
    /// 其余应用——各应用的 live 文件互相独立，一处损坏没有理由让其他
    /// 应用的 MCP 状态陈旧。全部跑完后若有失败，聚合成一个错误上报，
    /// 保留调用方的可见性。
    pub fn sync_all_enabled(state: &AppState) -> Result<(), AppError> {
        let _guard = mcp_mutation_lock().lock()?;
        let servers = Self::get_all_servers(state)?;

        let mut failures: Vec<String> = Vec::new();
        for client in ManagedClientId::ALL {
            match Self::project_servers_to_app(state, &servers, client) {
                Ok(()) if !servers.is_empty() => {
                    crate::services::record_runtime_local_writes(
                        &state.local_scan_writes,
                        [LocalScanTarget {
                            domain: LocalScanDomain::Mcp,
                            client_id: client,
                        }],
                    );
                }
                Ok(()) => {}
                Err(err) => {
                    log::warn!("同步 MCP 到 {client:?} 失败: {err}");
                    failures.push(format!("{}: {err}", client.as_str()));
                }
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "部分应用 MCP 同步失败: {}",
                failures.join("; ")
            )))
        }
    }

    /// 只把启用状态投影到单个应用。某个应用的 live 被整体重写后用它做
    /// 定向重投影，避免把无关应用的失败面（如 ~/.claude.json 坏 JSON）
    /// 牵连进目标应用的关键路径。
    pub fn sync_enabled_for_app(state: &AppState, client: ManagedClientId) -> Result<(), AppError> {
        let _guard = mcp_mutation_lock().lock()?;
        let servers = Self::get_all_servers(state)?;
        Self::project_servers_to_app(state, &servers, client)?;
        if !servers.is_empty() {
            crate::services::record_runtime_local_writes(
                &state.local_scan_writes,
                [LocalScanTarget {
                    domain: LocalScanDomain::Mcp,
                    client_id: client,
                }],
            );
        }
        Ok(())
    }

    fn project_servers_to_app(
        state: &AppState,
        servers: &IndexMap<String, McpServer>,
        client: ManagedClientId,
    ) -> Result<(), AppError> {
        for server in servers.values() {
            if server.apps.is_enabled_for(client) {
                Self::sync_server_to_app(state, server, client)?;
            } else {
                Self::remove_server_from_app(state, &server.id, client)?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // 兼容层：支持旧的 v3.6.x 命令（已废弃，将在 v4.0 移除）
    // ========================================================================

    /// [已废弃] 获取指定应用的 MCP 服务器（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use get_all_servers instead")]
    pub fn get_servers(
        state: &AppState,
        client: ManagedClientId,
    ) -> Result<HashMap<String, serde_json::Value>, AppError> {
        let all_servers = Self::get_all_servers(state)?;
        let mut result = HashMap::new();

        for (id, server) in all_servers {
            if server.apps.is_enabled_for(client) {
                result.insert(id, server.server);
            }
        }

        Ok(result)
    }

    /// [已废弃] 设置 MCP 服务器在指定应用的启用状态（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use toggle_app instead")]
    pub fn set_enabled(
        state: &AppState,
        client: ManagedClientId,
        id: &str,
        enabled: bool,
    ) -> Result<bool, AppError> {
        Self::toggle_app(state, id, client, enabled)?;
        Ok(true)
    }

    /// [已废弃] 同步启用的 MCP 到指定应用（兼容旧 API）
    #[deprecated(since = "3.7.0", note = "Use sync_all_enabled instead")]
    pub fn sync_enabled(state: &AppState, client: ManagedClientId) -> Result<(), AppError> {
        let servers = Self::get_all_servers(state)?;

        for server in servers.values() {
            if server.apps.is_enabled_for(client) {
                Self::sync_server_to_app(state, server, client)?;
            }
        }

        Ok(())
    }

    /// 从 Claude 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_claude(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp.rs）
        let count = crate::mcp::import_from_claude(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Claude，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.claude = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 Codex 导入 MCP（v3.7.0 已更新为统一结构）
    pub fn import_from_codex(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp.rs）
        let count = crate::mcp::import_from_codex(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 Codex，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.codex = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从 OpenCode 导入 MCP（v3.9.2+ 新增）
    pub fn import_from_opencode(state: &AppState) -> Result<usize, AppError> {
        // 创建临时 MultiAppConfig 用于导入
        let mut temp_config = crate::app_config::MultiAppConfig::default();

        // 调用原有的导入逻辑（从 mcp/opencode.rs）
        let count = crate::mcp::import_from_opencode(&mut temp_config)?;

        let mut new_count = 0;

        // 如果有导入的服务器，保存到数据库
        if count > 0 {
            if let Some(servers) = &temp_config.mcp.servers {
                let mut existing = state.db.get_all_mcp_servers()?;
                for server in servers.values() {
                    // 已存在：仅启用 OpenCode，不覆盖其他字段（与导入模块语义保持一致）
                    let to_save = if let Some(existing_server) = existing.get(&server.id) {
                        let mut merged = existing_server.clone();
                        merged.apps.opencode = true;
                        merged
                    } else {
                        // 真正的新服务器
                        new_count += 1;
                        server.clone()
                    };

                    state.db.save_mcp_server(&to_save)?;
                    existing.insert(to_save.id.clone(), to_save.clone());

                    // 导入是读取已有配置，不应反向写回任何应用的 live 配置。
                    // 显式编辑、启用/禁用或手动同步时再执行写回。
                }
            }
        }

        Ok(new_count)
    }

    /// 从所有支持 MCP 的应用导入服务器，返回新导入的数量。
    ///
    /// Best-effort：单个应用导入失败（如坏 config.toml）不阻断其余应用；
    /// 全部跑完后若有失败，聚合成一个错误上报——历史实现逐应用
    /// `unwrap_or(0)` 吞错，坏文件只会表现为"导入成功 0 个"，用户
    /// 无从得知哪个应用出了问题。
    pub fn import_from_all_apps(state: &AppState) -> Result<usize, AppError> {
        let mut total = 0;
        let mut failures: Vec<String> = Vec::new();

        let results: [(&str, Result<usize, AppError>); 3] = [
            ("claude", Self::import_from_claude(state)),
            ("codex", Self::import_from_codex(state)),
            ("opencode", Self::import_from_opencode(state)),
        ];
        for (app, result) in results {
            match result {
                Ok(count) => total += count,
                Err(err) => {
                    log::warn!("从 {app} 导入 MCP 失败: {err}");
                    failures.push(format!("{app}: {err}"));
                }
            }
        }

        if failures.is_empty() {
            Ok(total)
        } else {
            Err(AppError::Message(format!(
                "已导入 {total} 个，部分应用导入失败: {}",
                failures.join("; ")
            )))
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::app_config::McpApps;
    use crate::database::Database;
    use serde_json::{json, Value};
    use serial_test::serial;
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    fn remove_path(path: &Path) {
        if path.is_dir() {
            fs::remove_dir_all(path).expect("remove isolated MCP fixture directory");
        } else if path.exists() {
            fs::remove_file(path).expect("remove isolated MCP fixture file");
        }
    }

    fn cleanup(home: &Path) {
        for relative in [
            ".claude",
            ".claude.json",
            ".codex",
            ".config/opencode",
            ".wsl-code-switch",
        ] {
            remove_path(&home.join(relative));
        }
    }

    fn fixture_server(id: &str) -> McpServer {
        McpServer {
            id: id.to_string(),
            name: "Native MCP fixture".to_string(),
            server: json!({
                "type": "stdio",
                "command": "new-command",
                "args": ["--new"],
                "portableFuture": { "keep": true }
            }),
            apps: McpApps {
                claude: true,
                codex: true,
                opencode: true,
            },
            description: Some("native Windows to WSL UNC".to_string()),
            homepage: None,
            docs: None,
            tags: vec!["native-contract".to_string()],
        }
    }

    #[test]
    #[serial]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR and CC_SWITCH_TEST_HOME on isolated WSL2 UNC paths"]
    fn mcp_crud_sync_unknown_fields_and_rollback_on_wsl_unc() {
        let root = PathBuf::from(
            env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = PathBuf::from(
            env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set"),
        );
        let portable_root = root.to_string_lossy().replace('\\', "/");
        assert!(
            portable_root.starts_with("//wsl.localhost/") || portable_root.starts_with("//wsl$/"),
            "test root must be a WSL UNC path: {}",
            root.display()
        );
        assert!(
            home.starts_with(&root),
            "test home {} must be contained by isolated root {}",
            home.display(),
            root.display()
        );

        cleanup(&home);
        fs::create_dir_all(home.join(".claude")).expect("create Claude root");
        fs::create_dir_all(home.join(".codex")).expect("create Codex root");
        fs::create_dir_all(home.join(".config/opencode")).expect("create OpenCode root");

        fs::write(
            home.join(".claude.json"),
            br#"{
  "rootFuture": { "keep": true },
  "mcpServers": {
    "fixture": {
      "command": "old-command",
      "clientFuture": { "keep": true }
    }
  }
}"#,
        )
        .expect("seed Claude MCP config");
        fs::write(
            home.join(".codex/config.toml"),
            r#"root_future = { keep = true }

[mcp_servers.fixture]
command = "old-command"
client_future = { keep = true }
"#,
        )
        .expect("seed Codex MCP config");
        fs::write(
            home.join(".config/opencode/opencode.json"),
            br#"{
  "rootFuture": { "keep": true },
  "mcp": {
    "fixture": {
      "type": "local",
      "command": ["old-command"],
      "clientFuture": { "keep": true }
    }
  }
}"#,
        )
        .expect("seed OpenCode MCP config");

        let state = AppState::new(Arc::new(Database::memory().expect("in-memory database")));
        let mut server = fixture_server("fixture");
        McpService::upsert_server(&state, server.clone()).expect("create MCP server");

        let claude: Value = serde_json::from_slice(
            &fs::read(home.join(".claude.json")).expect("read Claude MCP config"),
        )
        .expect("parse Claude MCP config");
        assert_eq!(claude["rootFuture"]["keep"], true);
        assert_eq!(
            claude["mcpServers"]["fixture"]["clientFuture"]["keep"],
            true
        );
        assert_eq!(claude["mcpServers"]["fixture"]["command"], "new-command");

        let codex: toml::Value = fs::read_to_string(home.join(".codex/config.toml"))
            .expect("read Codex MCP config")
            .parse()
            .expect("parse Codex MCP config");
        assert_eq!(codex["root_future"]["keep"].as_bool(), Some(true));
        assert_eq!(
            codex["mcp_servers"]["fixture"]["client_future"]["keep"].as_bool(),
            Some(true)
        );
        assert_eq!(
            codex["mcp_servers"]["fixture"]["command"].as_str(),
            Some("new-command")
        );

        let opencode: Value = json5::from_str(
            &fs::read_to_string(home.join(".config/opencode/opencode.json"))
                .expect("read OpenCode MCP config"),
        )
        .expect("parse OpenCode MCP config");
        assert_eq!(opencode["rootFuture"]["keep"], true);
        assert_eq!(opencode["mcp"]["fixture"]["clientFuture"]["keep"], true);
        assert_eq!(opencode["mcp"]["fixture"]["command"][0], "new-command");

        server.server["command"] = json!("updated-command");
        McpService::upsert_server(&state, server.clone()).expect("update MCP server");
        assert_eq!(
            state
                .db
                .get_all_mcp_servers()
                .expect("read MCP database")
                .get("fixture")
                .expect("updated MCP server")
                .server,
            server.server
        );

        McpService::toggle_app(&state, "fixture", ManagedClientId::Codex, false)
            .expect("disable MCP server for Codex");
        McpService::sync_all_enabled(&state).expect("manually synchronize MCP live files");
        let codex_after_disable: toml::Value = fs::read_to_string(home.join(".codex/config.toml"))
            .expect("read disabled Codex MCP config")
            .parse()
            .expect("parse disabled Codex MCP config");
        assert!(codex_after_disable
            .get("mcp_servers")
            .and_then(|servers| servers.get("fixture"))
            .is_none());

        McpService::toggle_app(&state, "fixture", ManagedClientId::Codex, true)
            .expect("re-enable MCP server for Codex");
        assert!(McpService::delete_server(&state, "fixture").expect("delete MCP server"));
        assert!(state
            .db
            .get_all_mcp_servers()
            .expect("read empty MCP database")
            .is_empty());

        let claude_before = br#"{"mcpServers":{"keep":{"command":"keep"}},"future":true}"#;
        let codex_before = b"invalid = [\n";
        fs::write(home.join(".claude.json"), claude_before).expect("seed rollback Claude config");
        fs::write(home.join(".codex/config.toml"), codex_before)
            .expect("seed rollback Codex config");
        let mut failing = fixture_server("rollback-fixture");
        failing.apps.opencode = false;
        McpService::upsert_server(&state, failing)
            .expect_err("invalid second client must roll back the first client");
        assert_eq!(
            fs::read(home.join(".claude.json")).expect("read rolled-back Claude config"),
            claude_before
        );
        assert_eq!(
            fs::read(home.join(".codex/config.toml")).expect("read unchanged Codex config"),
            codex_before
        );
        assert!(state
            .db
            .get_all_mcp_servers()
            .expect("read rollback database")
            .is_empty());

        cleanup(&home);
    }
}
