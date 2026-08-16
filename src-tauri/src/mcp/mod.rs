//! MCP (Model Context Protocol) 服务器管理模块
//!
//! 本模块负责 MCP 服务器配置的验证、同步和导入导出。
//!
//! ## 模块结构
//!
//! - `validation` - 服务器配置验证
//! - `claude` - Claude MCP 同步和导入
//! - `codex` - Codex MCP 同步和导入（含 TOML 转换）
//! - `opencode` - OpenCode MCP 同步和导入（含 local/remote 格式转换）

mod claude;
mod codex;
mod opencode;
mod validation;

// 重新导出公共 API
pub use claude::{
    import_from_claude, remove_server_from_claude, sync_enabled_to_claude,
    sync_single_server_to_claude,
};
pub use codex::{
    import_from_codex, remove_server_from_codex, sync_enabled_to_codex, sync_single_server_to_codex,
};
pub use opencode::{
    import_from_opencode, remove_server_from_opencode, sync_single_server_to_opencode,
};

pub(crate) fn validate_server_for_client(
    server: &crate::domain::McpServer,
    client: crate::domain::ManagedClientId,
) -> Result<(), crate::error::AppError> {
    server
        .validate()
        .map_err(|error| crate::error::AppError::McpValidation(error.to_string()))?;

    match client {
        crate::domain::ManagedClientId::Claude => Ok(()),
        crate::domain::ManagedClientId::Codex => {
            codex::json_server_to_toml_table(&server.server).map(|_| ())
        }
        crate::domain::ManagedClientId::Opencode => {
            opencode::convert_to_opencode_format(&server.server).map(|_| ())
        }
    }
}
