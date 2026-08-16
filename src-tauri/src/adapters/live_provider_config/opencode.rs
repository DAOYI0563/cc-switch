use std::path::PathBuf;

use serde_json::Value;

use crate::domain::ManagedClientId;
use crate::ports::{
    LiveProviderConfigError, LiveProviderConfigErrorCode, LiveProviderConfigOperation,
    LiveProviderConfigPort, LiveProviderRecord, LiveProviderSnapshot,
};

use super::claude::{client_mismatch, map_app_error};

#[derive(Debug, Clone)]
pub struct OpenCodeLiveProviderConfigAdapter {
    config_path: PathBuf,
}

impl OpenCodeLiveProviderConfigAdapter {
    pub fn runtime() -> Self {
        Self::at_path(crate::opencode_config::get_opencode_config_path())
    }

    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: path.into(),
        }
    }

    fn error(
        &self,
        operation: LiveProviderConfigOperation,
        error: crate::AppError,
    ) -> LiveProviderConfigError {
        map_app_error(self.client_id(), operation, error)
    }

    fn provider_fragment(
        &self,
        provider: &LiveProviderRecord,
    ) -> Result<Value, LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Write;
        if provider.client_id != self.client_id() {
            return Err(client_mismatch(
                self.client_id(),
                operation,
                provider.client_id,
            ));
        }
        let Some(object) = provider.settings.as_object() else {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::InvalidInput,
                self.client_id(),
                operation,
                "OpenCode provider settings must be a JSON object",
            ));
        };
        let fragment = if object.contains_key("$schema") || object.contains_key("provider") {
            object
                .get("provider")
                .and_then(|providers| providers.get(&provider.provider_id))
                .cloned()
                .ok_or_else(|| {
                    LiveProviderConfigError::new(
                        LiveProviderConfigErrorCode::InvalidInput,
                        self.client_id(),
                        operation,
                        format!(
                            "full OpenCode config does not contain provider '{}'",
                            provider.provider_id
                        ),
                    )
                })?
        } else {
            provider.settings.clone()
        };

        if !fragment.as_object().is_some_and(|fragment| {
            fragment.contains_key("npm") || fragment.contains_key("options")
        }) {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::InvalidInput,
                self.client_id(),
                operation,
                format!(
                    "invalid OpenCode provider '{}': expected npm or options",
                    provider.provider_id
                ),
            ));
        }
        Ok(fragment)
    }
}

impl LiveProviderConfigPort for OpenCodeLiveProviderConfigAdapter {
    fn client_id(&self) -> ManagedClientId {
        ManagedClientId::Opencode
    }

    fn read(&self) -> Result<LiveProviderSnapshot, LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Read;
        if !self.config_path.exists() {
            return Err(LiveProviderConfigError::new(
                LiveProviderConfigErrorCode::Missing,
                self.client_id(),
                operation,
                format!("{} does not exist", self.config_path.display()),
            ));
        }
        let settings = crate::opencode_config::read_opencode_config_from_path(&self.config_path)
            .map_err(|error| self.error(operation, error))?;
        Ok(LiveProviderSnapshot {
            client_id: self.client_id(),
            settings,
        })
    }

    fn write(&self, provider: &LiveProviderRecord) -> Result<(), LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Write;
        let fragment = self.provider_fragment(provider)?;
        crate::opencode_config::set_provider_at_path(
            &self.config_path,
            &provider.provider_id,
            fragment,
        )
        .map_err(|error| self.error(operation, error))
    }

    fn contains(&self, provider_id: &str) -> Result<bool, LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Contains;
        crate::opencode_config::get_providers_from_path(&self.config_path)
            .map(|providers| providers.contains_key(provider_id))
            .map_err(|error| self.error(operation, error))
    }

    fn remove(&self, provider_id: &str) -> Result<(), LiveProviderConfigError> {
        let operation = LiveProviderConfigOperation::Remove;
        if !self.config_path.exists() {
            return Ok(());
        }
        crate::opencode_config::remove_provider_at_path(&self.config_path, provider_id)
            .map_err(|error| self.error(operation, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{LiveProviderConfigErrorCode, LiveProviderConfigPort};
    use serde_json::json;

    fn record(settings: Value) -> LiveProviderRecord {
        LiveProviderRecord {
            client_id: ManagedClientId::Opencode,
            provider_id: "fixture_vendor".to_string(),
            category: None,
            settings,
        }
    }

    #[test]
    fn additive_roundtrip_preserves_unknown_top_level_and_provider_fields() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.json");
        std::fs::write(
            &path,
            r#"{
              // JSONC input and unknown client-owned values.
              theme: 'fixture-theme',
              futureTopLevel: { preserve: true },
              provider: {
                fixture_vendor: {
                  npm: '@ai-sdk/openai-compatible',
                  name: 'Before',
                  options: { baseURL: 'https://example.invalid/v1', futureOption: 'keep' },
                  models: {},
                  futureProviderField: { revision: 3 },
                },
              },
            }"#,
        )
        .unwrap();
        let adapter = OpenCodeLiveProviderConfigAdapter::at_path(&path);
        let mut fragment = adapter.read().unwrap().settings["provider"]["fixture_vendor"].clone();
        fragment["name"] = json!("After");

        adapter.write(&record(fragment)).unwrap();
        assert!(adapter.contains("fixture_vendor").unwrap());
        let snapshot = adapter.read().unwrap();
        assert_eq!(snapshot.settings["theme"], "fixture-theme");
        assert_eq!(snapshot.settings["futureTopLevel"]["preserve"], true);
        assert_eq!(
            snapshot.settings["provider"]["fixture_vendor"]["futureProviderField"]["revision"],
            3
        );
        assert_eq!(
            snapshot.settings["provider"]["fixture_vendor"]["options"]["futureOption"],
            "keep"
        );

        adapter.remove("fixture_vendor").unwrap();
        assert!(!adapter.contains("fixture_vendor").unwrap());
        assert_eq!(adapter.read().unwrap().settings["theme"], "fixture-theme");
    }

    #[test]
    fn invalid_fragment_is_zero_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.json");
        std::fs::write(&path, b"{\"theme\":\"keep\"}").unwrap();
        let before = std::fs::read(&path).unwrap();
        let adapter = OpenCodeLiveProviderConfigAdapter::at_path(&path);

        let error = adapter
            .write(&record(json!({"name": "missing npm"})))
            .unwrap_err();

        assert_eq!(error.code, LiveProviderConfigErrorCode::InvalidInput);
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn remove_missing_config_is_zero_write() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing/opencode.json");
        let adapter = OpenCodeLiveProviderConfigAdapter::at_path(&path);

        adapter.remove("fixture").unwrap();

        assert!(!temp.path().join("missing").exists());
    }
}
