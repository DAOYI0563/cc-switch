use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::domain::ManagedClientId;
use crate::ports::{
    LiveProviderConfigError, LiveProviderConfigErrorCode, LiveProviderConfigOperation,
    LiveProviderConfigPort, LiveProviderRecord, LiveProviderSnapshot,
};

use super::claude::{client_mismatch, map_app_error};

#[derive(Debug, Clone)]
pub struct CodexLiveProviderConfigAdapter {
    auth_path: PathBuf,
    config_path: PathBuf,
}

impl CodexLiveProviderConfigAdapter {
    pub fn runtime() -> Self {
        Self {
            auth_path: crate::codex_config::get_codex_auth_path(),
            config_path: crate::codex_config::get_codex_config_path(),
        }
    }

    pub fn at_paths(
        auth_path: impl Into<PathBuf>,
        config_path: impl Into<PathBuf>,
        _removed_catalog_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            auth_path: auth_path.into(),
            config_path: config_path.into(),
        }
    }

    fn error(
        &self,
        operation: LiveProviderConfigOperation,
        error: crate::AppError,
    ) -> LiveProviderConfigError {
        map_app_error(self.client_id(), operation, error)
    }

    fn prepare_write(
        &self,
        provider: &LiveProviderRecord,
    ) -> Result<(Value, String), LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Write;
        if provider.client_id != self.client_id() {
            return Err(client_mismatch(
                self.client_id(),
                operation,
                provider.client_id,
            ));
        }
        let object = provider.settings.as_object().ok_or_else(|| {
            LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::InvalidInput,
                self.client_id(),
                operation,
                "Codex provider settings must be a JSON object",
            )
        })?;
        let auth = object.get("auth").cloned().unwrap_or_else(|| json!({}));
        if !auth.is_object() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::InvalidInput,
                self.client_id(),
                operation,
                "Codex auth must be a JSON object",
            ));
        }
        let config = match object.get("config") {
            Some(Value::String(config)) => config.clone(),
            Some(Value::Null) | None => String::new(),
            Some(_) => {
                return Err(LiveProviderConfigError::new(
                    LiveProviderConfigErrorCode::InvalidInput,
                    self.client_id(),
                    operation,
                    "Codex config must be TOML text",
                ));
            }
        };
        crate::codex_config::validate_config_toml(&config)
            .map_err(|error| self.error(operation, error))?;
        Ok((auth, config))
    }

    fn rollback(&self, auth: &FileSnapshot, config: &FileSnapshot) -> Result<(), String> {
        let mut failures = Vec::new();
        for (snapshot, path) in [
            (config, self.config_path.as_path()),
            (auth, self.auth_path.as_path()),
        ] {
            if let Err(error) = snapshot.restore(path) {
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

impl LiveProviderConfigPort for CodexLiveProviderConfigAdapter {
    fn client_id(&self) -> ManagedClientId {
        ManagedClientId::Codex
    }

    fn read(&self) -> Result<LiveProviderSnapshot, LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Read;
        if !self.auth_path.exists() && !self.config_path.exists() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::Missing,
                self.client_id(),
                operation,
                "Codex auth.json and config.toml are both missing",
            ));
        }
        let auth = if self.auth_path.exists() {
            crate::config::read_json_file(&self.auth_path)
                .map_err(|error| self.error(operation, error))?
        } else {
            json!({})
        };
        let config = match std::fs::read_to_string(&self.config_path) {
            Ok(config) => config,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(self.error(operation, crate::AppError::io(&self.config_path, error)));
            }
        };
        crate::codex_config::validate_config_toml(&config)
            .map_err(|error| self.error(operation, error))?;
        Ok(LiveProviderSnapshot {
            client_id: self.client_id(),
            settings: json!({ "auth": auth, "config": config }),
        })
    }

    fn write(&self, provider: &LiveProviderRecord) -> Result<(), LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Write;
        let (auth, config) = self.prepare_write(provider)?;
        let auth_before =
            FileSnapshot::capture(&self.auth_path).map_err(|error| self.error(operation, error))?;
        let config_before = FileSnapshot::capture(&self.config_path)
            .map_err(|error| self.error(operation, error))?;
        let result = crate::config::write_json_file(&self.auth_path, &auth)
            .and_then(|_| crate::config::write_text_file(&self.config_path, &config));
        if let Err(error) = result {
            let message = match self.rollback(&auth_before, &config_before) {
                Ok(()) => error.to_string(),
                Err(rollback) => format!("{error}; rollback failed: {rollback}"),
            };
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::Io,
                self.client_id(),
                operation,
                message,
            ));
        }
        Ok(())
    }

    fn contains(&self, _provider_id: &str) -> Result<bool, LiveProviderConfigError> {
        Err(LiveProviderConfigError::unsupported(
            self.client_id(),
            LiveProviderConfigOperation::Contains,
        ))
    }

    fn remove(&self, _provider_id: &str) -> Result<(), LiveProviderConfigError> {
        Err(LiveProviderConfigError::unsupported(
            self.client_id(),
            LiveProviderConfigOperation::Remove,
        ))
    }
}

struct FileSnapshot(Option<Vec<u8>>);

impl FileSnapshot {
    fn capture(path: &Path) -> Result<Self, crate::AppError> {
        match std::fs::read(path) {
            Ok(contents) => Ok(Self(Some(contents))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self(None)),
            Err(error) => Err(crate::AppError::io(path, error)),
        }
    }

    fn restore(&self, path: &Path) -> Result<(), crate::AppError> {
        match &self.0 {
            Some(contents) => crate::config::atomic_write(path, contents),
            None if path.exists() => crate::config::delete_file(path),
            None => Ok(()),
        }
    }
}
