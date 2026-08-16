use std::path::PathBuf;

use serde_json::Value;

use crate::domain::ManagedClientId;
use crate::ports::{
    LiveProviderConfigError, LiveProviderConfigErrorCode, LiveProviderConfigOperation,
    LiveProviderConfigPort, LiveProviderRecord, LiveProviderSnapshot,
};

#[derive(Debug, Clone)]
pub struct ClaudeLiveProviderConfigAdapter {
    settings_path: PathBuf,
}

impl ClaudeLiveProviderConfigAdapter {
    pub fn runtime() -> Self {
        Self::at_path(crate::config::get_claude_settings_path())
    }

    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            settings_path: path.into(),
        }
    }

    fn error(
        &self,
        operation: LiveProviderConfigOperation,
        error: crate::AppError,
    ) -> LiveProviderConfigError {
        map_app_error(ManagedClientId::Claude, operation, error)
    }
}

impl LiveProviderConfigPort for ClaudeLiveProviderConfigAdapter {
    fn client_id(&self) -> ManagedClientId {
        ManagedClientId::Claude
    }

    fn read(&self) -> Result<LiveProviderSnapshot, LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Read;
        if !self.settings_path.exists() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::Missing,
                self.client_id(),
                operation,
                format!("{} does not exist", self.settings_path.display()),
            ));
        }
        let settings: Value = crate::config::read_json_file(&self.settings_path)
            .map_err(|error| self.error(operation, error))?;
        if !settings.is_object() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::Parse,
                self.client_id(),
                operation,
                "Claude settings root must be a JSON object",
            ));
        }
        Ok(LiveProviderSnapshot {
            client_id: self.client_id(),
            settings,
        })
    }

    fn write(&self, provider: &LiveProviderRecord) -> Result<(), LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Write;
        if provider.client_id != self.client_id() {
            return Err(client_mismatch(
                self.client_id(),
                operation,
                provider.client_id,
            ));
        }
        if !provider.settings.is_object() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::InvalidInput,
                self.client_id(),
                operation,
                "Claude provider settings must be a JSON object",
            ));
        }

        let mut settings = provider.settings.clone();
        if let Some(object) = settings.as_object_mut() {
            for key in [
                "api_format",
                "apiFormat",
                "openrouter_compat_mode",
                "openrouterCompatMode",
            ] {
                object.remove(key);
            }
        }
        crate::config::write_json_file(&self.settings_path, &settings)
            .map_err(|error| self.error(operation, error))
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

pub(super) fn map_app_error(
    client_id: ManagedClientId,
    operation: LiveProviderConfigOperation,
    error: crate::AppError,
) -> LiveProviderConfigError {
    use crate::AppError;

    let code = match &error {
        AppError::Io { .. } | AppError::IoContext { .. } | AppError::Lock(_) => {
            LiveProviderConfigErrorCode::Io
        }
        AppError::InvalidInput(_) => LiveProviderConfigErrorCode::InvalidInput,
        _ => LiveProviderConfigErrorCode::Parse,
    };
    LiveProviderConfigError::new(code, client_id, operation, error.to_string())
}

pub(super) fn client_mismatch(
    expected: ManagedClientId,
    operation: LiveProviderConfigOperation,
    actual: ManagedClientId,
) -> LiveProviderConfigError {
    LiveProviderConfigError::new(
        LiveProviderConfigErrorCode::InvalidInput,
        expected,
        operation,
        format!("expected {expected} record, received {actual}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{LiveProviderConfigErrorCode, LiveProviderConfigPort};
    use serde_json::json;

    fn record(settings: Value) -> LiveProviderRecord {
        LiveProviderRecord {
            client_id: ManagedClientId::Claude,
            provider_id: "fixture".to_string(),
            category: None,
            settings,
        }
    }

    #[test]
    fn roundtrip_preserves_unknown_fields_and_strips_private_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let adapter = ClaudeLiveProviderConfigAdapter::at_path(&path);
        let settings = json!({
            "model": "fixture-model",
            "apiFormat": "private",
            "futureTopLevel": { "revision": 7 },
            "permissions": { "futureMode": "preserve-me" }
        });

        adapter.write(&record(settings)).unwrap();
        let snapshot = adapter.read().unwrap();

        assert_eq!(snapshot.settings["futureTopLevel"]["revision"], 7);
        assert_eq!(
            snapshot.settings["permissions"]["futureMode"],
            "preserve-me"
        );
        assert!(snapshot.settings.get("apiFormat").is_none());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_input_is_zero_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, b"{\"keep\":true}").unwrap();
        let before = std::fs::read(&path).unwrap();
        let adapter = ClaudeLiveProviderConfigAdapter::at_path(&path);

        let error = adapter.write(&record(json!([]))).unwrap_err();

        assert_eq!(error.code, LiveProviderConfigErrorCode::InvalidInput);
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn additive_operations_are_structurally_unsupported() {
        let adapter = ClaudeLiveProviderConfigAdapter::at_path("unused.json");
        assert_eq!(
            adapter.contains("fixture").unwrap_err().code,
            LiveProviderConfigErrorCode::UnsupportedOperation
        );
        assert_eq!(
            adapter.remove("fixture").unwrap_err().code,
            LiveProviderConfigErrorCode::UnsupportedOperation
        );
    }
}
