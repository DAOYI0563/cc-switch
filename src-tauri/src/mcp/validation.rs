//! MCP 服务器配置验证模块

use serde_json::Value;

use crate::error::AppError;

/// 基础校验：允许 stdio/http/sse；或省略 type（视为 stdio）。对应必填字段存在
pub fn validate_server_spec(spec: &Value) -> Result<(), AppError> {
    crate::domain::validate_server_spec(spec)
        .map_err(|error| AppError::McpValidation(error.to_string()))
}

/// Replace canonical connection fields while retaining client-only fields
/// already present in a live entry.
pub fn merge_preserving_unknown(
    existing: Option<&Value>,
    incoming: &Value,
    managed_fields: &[&str],
) -> Result<Value, AppError> {
    let incoming = incoming
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器连接定义必须为 JSON 对象".into()))?;
    let mut merged = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for field in managed_fields {
        merged.remove(*field);
    }
    for (key, value) in incoming {
        merged.insert(key.clone(), value.clone());
    }

    Ok(Value::Object(merged))
}

/// 从 MCP 条目中提取服务器规范
pub fn extract_server_spec(entry: &Value) -> Result<Value, AppError> {
    let obj = entry
        .as_object()
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目必须为 JSON 对象".into()))?;
    let server = obj
        .get("server")
        .ok_or_else(|| AppError::McpValidation("MCP 服务器条目缺少 server 字段".into()))?;

    if !server.is_object() {
        return Err(AppError::McpValidation(
            "MCP 服务器 server 字段必须为 JSON 对象".into(),
        ));
    }

    Ok(server.clone())
}
