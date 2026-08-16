use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{DomainError, DomainErrorCode, ManagedClientApps};

/// Canonical MCP record stored by WSL Code Switch.
///
/// Client-specific syntax belongs to live adapters. The canonical connection
/// object remains intentionally open so extensions survive round trips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub server: Value,
    pub apps: ManagedClientApps,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl McpServer {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_label("MCP server id", &self.id, 256)?;
        validate_label("MCP server name", &self.name, 512)?;
        validate_server_spec(&self.server)
    }
}

/// Validate the client-neutral connection fields without filesystem or Tauri
/// dependencies. Unknown fields are accepted and preserved by design.
pub fn validate_server_spec(spec: &Value) -> Result<(), DomainError> {
    let object = spec.as_object().ok_or_else(|| {
        DomainError::new(
            DomainErrorCode::InvalidRecord,
            "MCP 服务器连接定义必须为 JSON 对象",
        )
    })?;
    let server_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("stdio");

    match server_type {
        "stdio" => require_non_empty_string(
            object.get("command"),
            "stdio 类型的 MCP 服务器缺少 command 字段",
        ),
        "http" => {
            require_non_empty_string(object.get("url"), "http 类型的 MCP 服务器缺少 url 字段")
        }
        "sse" => require_non_empty_string(object.get("url"), "sse 类型的 MCP 服务器缺少 url 字段"),
        _ => Err(DomainError::new(
            DomainErrorCode::InvalidRecord,
            "MCP 服务器 type 必须是 'stdio'、'http' 或 'sse'（或省略表示 stdio）",
        )),
    }
}

fn validate_label(label: &str, value: &str, max_len: usize) -> Result<(), DomainError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > max_len || trimmed.chars().any(char::is_control) {
        return Err(
            DomainError::new(DomainErrorCode::InvalidRecord, format!("invalid {label}"))
                .with_context("field", label),
        );
    }
    Ok(())
}

fn require_non_empty_string(value: Option<&Value>, message: &str) -> Result<(), DomainError> {
    if value
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        Ok(())
    } else {
        Err(DomainError::new(DomainErrorCode::InvalidRecord, message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(server: Value) -> McpServer {
        McpServer {
            id: "fixture".to_string(),
            name: "Fixture".to_string(),
            server,
            apps: ManagedClientApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn accepts_extensions_without_narrowing_the_client_neutral_record() {
        record(json!({
            "type": "stdio",
            "command": "uvx",
            "future": { "nested": [true, 2, "three"] }
        }))
        .validate()
        .expect("extended record");
    }

    #[test]
    fn rejects_invalid_identity_and_connection_before_any_adapter_write() {
        let mut invalid_id = record(json!({ "command": "echo" }));
        invalid_id.id = " \n".to_string();
        assert!(invalid_id.validate().is_err());

        assert!(record(json!({ "type": "stdio" })).validate().is_err());
        assert!(
            record(json!({ "type": "remote", "url": "https://example.com" }))
                .validate()
                .is_err()
        );
    }
}
