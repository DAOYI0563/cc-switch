use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Map, Value};

use crate::config::write_json_file_with_contents;
use crate::error::AppError;

fn config_lock() -> Result<std::sync::MutexGuard<'static, ()>, AppError> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|error| AppError::Config(format!("OpenCode 配置锁已损坏: {error}")))
}

pub fn get_opencode_dir() -> PathBuf {
    use crate::ports::WslPathResolver;

    crate::adapters::wsl_paths::FixedWslPathResolver::runtime()
        .client_config_root(crate::domain::ManagedClientId::Opencode)
        .windows
}

pub fn get_opencode_config_path() -> PathBuf {
    get_opencode_dir().join("opencode.json")
}

pub(crate) fn read_opencode_config_from_path(path: &Path) -> Result<Value, AppError> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "$schema": "https://opencode.ai/config.json" }));
        }
        Err(error) => return Err(AppError::io(path, error)),
    };
    let value: Value = json5::from_str(&content).map_err(|error| {
        AppError::Config(format!(
            "解析 OpenCode 配置失败 {}: {error}",
            path.display()
        ))
    })?;
    if !value.is_object() {
        return Err(AppError::Config(format!(
            "OpenCode 配置文件根节点必须是 JSON 对象: {}",
            path.display()
        )));
    }
    Ok(value)
}

pub(crate) fn get_providers_from_path(path: &Path) -> Result<Map<String, Value>, AppError> {
    Ok(read_opencode_config_from_path(path)?
        .get("provider")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn get_providers() -> Result<Map<String, Value>, AppError> {
    get_providers_from_path(&get_opencode_config_path())
}

pub(crate) fn set_provider_at_path(path: &Path, id: &str, provider: Value) -> Result<(), AppError> {
    let _guard = config_lock()?;
    let mut config = read_opencode_config_from_path(path)?;
    ensure_object_field(&mut config, "provider");
    config["provider"]
        .as_object_mut()
        .expect("provider field normalized")
        .insert(id.to_string(), provider);
    write_json_file_with_contents(path, &config).map(|_| ())
}

pub fn set_provider(id: &str, provider: Value) -> Result<(), AppError> {
    set_provider_at_path(&get_opencode_config_path(), id, provider)
}

pub(crate) fn remove_provider_at_path(path: &Path, id: &str) -> Result<(), AppError> {
    let _guard = config_lock()?;
    let mut config = read_opencode_config_from_path(path)?;
    if let Some(providers) = config.get_mut("provider").and_then(Value::as_object_mut) {
        providers.remove(id);
    }
    write_json_file_with_contents(path, &config).map(|_| ())
}

pub fn remove_provider(id: &str) -> Result<(), AppError> {
    remove_provider_at_path(&get_opencode_config_path(), id)
}

pub fn get_mcp_servers() -> Result<Map<String, Value>, AppError> {
    Ok(read_managed_mcp_config()?
        .get("mcp")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

pub fn set_mcp_server_preserving_unknown(
    id: &str,
    server: Value,
    managed_fields: &[&str],
) -> Result<(), AppError> {
    let _guard = config_lock()?;
    let mut config = read_managed_mcp_config()?;
    ensure_object_field(&mut config, "mcp");
    let incoming = server
        .as_object()
        .ok_or_else(|| AppError::McpValidation("OpenCode MCP 服务器必须为对象".to_string()))?;
    let mcp = config["mcp"].as_object_mut().expect("mcp field normalized");
    let mut merged = mcp
        .get(id)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for field in managed_fields {
        merged.remove(*field);
    }
    merged.extend(incoming.clone());
    mcp.insert(id.to_string(), Value::Object(merged));
    write_managed_mcp_config(&config)
}

pub fn remove_mcp_server(id: &str) -> Result<(), AppError> {
    let _guard = config_lock()?;
    let mut config = read_managed_mcp_config()?;
    if let Some(mcp) = config.get_mut("mcp").and_then(Value::as_object_mut) {
        mcp.remove(id);
    }
    write_managed_mcp_config(&config)
}

fn ensure_object_field(config: &mut Value, field: &str) {
    if !config.get(field).is_some_and(Value::is_object) {
        config[field] = json!({});
    }
}

fn read_managed_mcp_config() -> Result<Value, AppError> {
    let Some(contents) = crate::adapters::mcp_live_files::McpLiveFileAdapter::runtime()
        .read_optional(crate::domain::ManagedClientId::Opencode)?
    else {
        return Ok(json!({ "$schema": "https://opencode.ai/config.json" }));
    };
    let text = std::str::from_utf8(&contents)
        .map_err(|error| AppError::Config(format!("OpenCode 配置不是 UTF-8: {error}")))?;
    let value: Value = json5::from_str(text)
        .map_err(|error| AppError::Config(format!("解析 OpenCode 配置失败: {error}")))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err(AppError::Config(
            "OpenCode 配置文件根节点必须是 JSON 对象".to_string(),
        ))
    }
}

fn write_managed_mcp_config(config: &Value) -> Result<(), AppError> {
    let contents =
        serde_json::to_vec_pretty(config).map_err(|source| AppError::JsonSerialize { source })?;
    crate::adapters::mcp_live_files::McpLiveFileAdapter::runtime()
        .write(crate::domain::ManagedClientId::Opencode, &contents)
}
