use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::{atomic_write, delete_file, read_json_file, write_json_file, write_text_file};
use crate::error::AppError;

pub fn get_codex_config_dir() -> PathBuf {
    use crate::ports::WslPathResolver;

    crate::adapters::wsl_paths::FixedWslPathResolver::runtime()
        .client_config_root(crate::domain::ManagedClientId::Codex)
        .windows
}

pub fn get_codex_auth_path() -> PathBuf {
    get_codex_config_dir().join("auth.json")
}

pub fn get_codex_config_path() -> PathBuf {
    get_codex_config_dir().join("config.toml")
}

pub fn read_codex_config_text() -> Result<String, AppError> {
    let path = get_codex_config_path();
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(AppError::io(&path, error)),
    }
}

pub fn validate_config_toml(text: &str) -> Result<(), AppError> {
    if text.trim().is_empty() {
        return Ok(());
    }
    text.parse::<toml::Table>()
        .map(|_| ())
        .map_err(|error| AppError::toml(Path::new("config.toml"), error))
}

pub fn read_codex_live_settings() -> Result<Value, AppError> {
    let auth_path = get_codex_auth_path();
    let auth = if auth_path.exists() {
        read_json_file(&auth_path)?
    } else {
        json!({})
    };
    let config = read_codex_config_text()?;
    validate_config_toml(&config)?;
    Ok(json!({ "auth": auth, "config": config }))
}

pub fn write_codex_live_atomic(auth: &Value, config: Option<&str>) -> Result<(), AppError> {
    if !auth.is_object() {
        return Err(AppError::InvalidInput(
            "Codex auth 必须是 JSON 对象".to_string(),
        ));
    }
    let config = config.unwrap_or_default();
    validate_config_toml(config)?;
    let auth_path = get_codex_auth_path();
    let config_path = get_codex_config_path();
    let auth_before = read_optional(&auth_path)?;
    let config_before = read_optional(&config_path)?;

    if let Err(primary) =
        write_json_file(&auth_path, auth).and_then(|_| write_text_file(&config_path, config))
    {
        let mut failures = Vec::new();
        if let Err(error) = restore(&auth_path, auth_before.as_deref()) {
            failures.push(error.to_string());
        }
        if let Err(error) = restore(&config_path, config_before.as_deref()) {
            failures.push(error.to_string());
        }
        return if failures.is_empty() {
            Err(primary)
        } else {
            Err(AppError::Message(format!(
                "{primary}; Codex live 配置回滚失败: {}",
                failures.join("; ")
            )))
        };
    }
    Ok(())
}

pub fn extract_codex_base_url(config: &str) -> Option<String> {
    let document = config.parse::<toml::Value>().ok()?;
    if let Some(active) = document.get("model_provider").and_then(toml::Value::as_str) {
        if let Some(value) = document
            .get("model_providers")
            .and_then(|providers| providers.get(active))
            .and_then(|provider| provider.get("base_url"))
            .and_then(toml::Value::as_str)
        {
            return Some(value.trim_end_matches('/').to_string());
        }
    }
    document
        .get("base_url")
        .and_then(toml::Value::as_str)
        .map(|value| value.trim_end_matches('/').to_string())
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io(path, error)),
    }
}

fn restore(path: &Path, contents: Option<&[u8]>) -> Result<(), AppError> {
    match contents {
        Some(contents) => atomic_write(path, contents),
        None if path.exists() => delete_file(path),
        None => Ok(()),
    }
}
