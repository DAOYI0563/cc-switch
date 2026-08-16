use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;
use sha2::Digest;

use super::*;
use crate::adapters::device_settings::FixedDeviceSettingsStore;
use crate::adapters::legacy_data::FixedLegacyDataSource;
use crate::database::Database;
use crate::domain::{RollbackPointMetadata, RollbackPointPurpose};
use crate::ports::{
    DeviceSettingsError, DeviceSettingsStore, RetainedMigrationTargetError, SecretStoreError,
    TemporaryRollbackError,
};

const V16_FIXTURE: &str = include_str!("../../../tests/fixtures/v16/cc-switch-v16.sql");
const SETTINGS_FIXTURE: &[u8] = include_bytes!("../../../tests/fixtures/v16/settings.json");

fn create_legacy_source(home: &Path) -> FixedLegacyDataSource {
    let root = home.join(".cc-switch");
    fs::create_dir(&root).unwrap();
    let connection = Connection::open(root.join("cc-switch.db")).unwrap();
    connection.execute_batch(V16_FIXTURE).unwrap();
    drop(connection);
    fs::write(root.join("settings.json"), SETTINGS_FIXTURE).unwrap();
    FixedLegacyDataSource::from_home(home)
}

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<DeviceSecretId, String>>,
    fail_write: Mutex<bool>,
}

impl MemorySecrets {
    fn fail_next_write(&self) {
        *self.fail_write.lock().unwrap() = true;
    }
}

impl SecretStore for MemorySecrets {
    fn read(&self, id: DeviceSecretId) -> Result<Option<String>, SecretStoreError> {
        Ok(self.values.lock().unwrap().get(&id).cloned())
    }

    fn write(&self, id: DeviceSecretId, secret: &str) -> Result<(), SecretStoreError> {
        if std::mem::take(&mut *self.fail_write.lock().unwrap()) {
            return Err(SecretStoreError::new(
                crate::ports::SecretStoreErrorCode::WriteFailed,
                "injected credential write failure",
            ));
        }
        self.values.lock().unwrap().insert(id, secret.to_string());
        Ok(())
    }

    fn delete(&self, id: DeviceSecretId) -> Result<(), SecretStoreError> {
        self.values.lock().unwrap().remove(&id);
        Ok(())
    }
}

#[derive(Default)]
struct MemoryRollbacks {
    points: Mutex<HashMap<String, (RollbackPointMetadata, Vec<u8>)>>,
    next_id: Mutex<u64>,
    fail_delete: Mutex<bool>,
    created_count: Mutex<u64>,
}

impl MemoryRollbacks {
    fn fail_next_delete(&self) {
        *self.fail_delete.lock().unwrap() = true;
    }

    fn created_count(&self) -> u64 {
        *self.created_count.lock().unwrap()
    }
}

impl TemporaryRollbackStore for MemoryRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        *self.created_count.lock().unwrap() += 1;
        let mut next_id = self.next_id.lock().unwrap();
        *next_id += 1;
        let metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: format!("fixture-rollback-{next_id}"),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: format!("{:x}", sha2::Sha256::digest(payload)),
        };
        self.points
            .lock()
            .unwrap()
            .insert(metadata.id.clone(), (metadata.clone(), payload.to_vec()));
        Ok(metadata)
    }

    fn restore(&self, id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        self.points
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, payload)| payload.clone())
            .ok_or_else(|| {
                TemporaryRollbackError::new(
                    crate::ports::TemporaryRollbackErrorCode::NotFound,
                    "fixture rollback not found",
                )
            })
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        if std::mem::take(&mut *self.fail_delete.lock().unwrap()) {
            return Err(TemporaryRollbackError::new(
                crate::ports::TemporaryRollbackErrorCode::Io,
                "injected rollback deletion failure",
            ));
        }
        self.points.lock().unwrap().remove(id);
        Ok(())
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let mut points = self.points.lock().unwrap();
        let (metadata, _) = points.get_mut(id).ok_or_else(|| {
            TemporaryRollbackError::new(
                crate::ports::TemporaryRollbackErrorCode::NotFound,
                "fixture rollback not found",
            )
        })?;
        metadata.state = RollbackPointState::Failed;
        metadata.failed_at_ms = Some(failed_at_ms);
        Ok(metadata.clone())
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self
            .points
            .lock()
            .unwrap()
            .values()
            .map(|(metadata, _)| metadata.clone())
            .collect())
    }
}

struct FailingMarkTarget<'a> {
    database: &'a Database,
}

impl RetainedMigrationTarget for FailingMarkTarget<'_> {
    fn apply_retained(
        &self,
        snapshot: &crate::domain::LegacyRetainedSnapshot,
        completed_at_ms: i64,
    ) -> Result<RetainedMigrationReport, RetainedMigrationTargetError> {
        self.database.apply_retained(snapshot, completed_at_ms)
    }

    fn rollback_retained(
        &self,
        source_fingerprint: &str,
    ) -> Result<(), RetainedMigrationTargetError> {
        self.database.rollback_retained(source_fingerprint)
    }

    fn retained_resources_complete(&self) -> Result<bool, RetainedMigrationTargetError> {
        self.database.retained_resources_complete()
    }

    fn mark_retained_resources_complete(
        &self,
        _source_fingerprint: &str,
        _completed_at_ms: i64,
    ) -> Result<(), RetainedMigrationTargetError> {
        Err(RetainedMigrationTargetError::new(
            "injected completion marker failure",
        ))
    }
}

struct FailingSettings {
    contents: Mutex<Option<Vec<u8>>>,
    fail_replace: Mutex<bool>,
}

impl FailingSettings {
    fn new(contents: Option<Vec<u8>>) -> Self {
        Self {
            contents: Mutex::new(contents),
            fail_replace: Mutex::new(true),
        }
    }
}

impl DeviceSettingsStore for FailingSettings {
    fn read(&self) -> Result<Option<Vec<u8>>, DeviceSettingsError> {
        Ok(self.contents.lock().unwrap().clone())
    }

    fn replace(&self, contents: &[u8]) -> Result<(), DeviceSettingsError> {
        if std::mem::take(&mut *self.fail_replace.lock().unwrap()) {
            return Err(DeviceSettingsError::new(
                crate::ports::DeviceSettingsErrorCode::WriteFailed,
                "injected settings write failure",
            ));
        }
        *self.contents.lock().unwrap() = Some(contents.to_vec());
        Ok(())
    }

    fn delete(&self) -> Result<(), DeviceSettingsError> {
        *self.contents.lock().unwrap() = None;
        Ok(())
    }
}

fn core_count(database: &Database, table: &str) -> i64 {
    database
        .conn
        .lock()
        .unwrap()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn representative_v16_migration_preserves_source_and_migrates_only_allowed_state() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let source_db_before = fs::read(source.root().join("cc-switch.db")).unwrap();
    let source_settings_before = fs::read(source.root().join("settings.json")).unwrap();
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    let rollbacks = MemoryRollbacks::default();

    let report = migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .unwrap()
    .expect("migration applied");

    assert_eq!(report.retained.total(), 9);
    assert_eq!(core_count(&database, "core_providers"), 3);
    assert_eq!(core_count(&database, "core_mcp_servers"), 1);
    assert_eq!(core_count(&database, "core_prompt_versions"), 2);
    assert_eq!(core_count(&database, "core_skills"), 1);
    assert_eq!(core_count(&database, "core_common_snippets"), 2);
    let written = settings.read().unwrap().unwrap();
    let value: Value = serde_json::from_slice(&written).unwrap();
    assert_eq!(value["launchOnStartup"], true);
    assert_eq!(value["silentStartup"], true);
    assert!(value["webdavSync"].get("autoSync").is_none());
    assert!(value["webdavSync"].get("enabled").is_none());
    assert!(value["webdavSync"].get("password").is_none());
    for discarded in [
        "enableLocalProxy",
        "preserveCodexOfficialAuthOnSwitch",
        "showProfileSwitcher",
        "usageDashboardRefreshIntervalMs",
        "s3Sync",
    ] {
        assert!(value.get(discarded).is_none(), "discard {discarded}");
    }
    assert_eq!(
        secrets
            .read(DeviceSecretId::WebdavPassword)
            .unwrap()
            .as_deref(),
        Some("FIXTURE_WEBDAV_PASSWORD")
    );
    assert!(rollbacks.list().unwrap().is_empty());
    assert_eq!(
        fs::read(source.root().join("cc-switch.db")).unwrap(),
        source_db_before
    );
    assert_eq!(
        fs::read(source.root().join("settings.json")).unwrap(),
        source_settings_before
    );

    fs::write(
        source.root().join("settings.json"),
        b"{broken-after-success",
    )
    .expect("mutate preserved legacy source after migration");
    assert!(migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_100,
    )
    .unwrap()
    .is_none());
}

#[test]
fn target_device_settings_are_preserved_sanitized_and_win_over_legacy_values() {
    let current = serde_json::to_vec(&serde_json::json!({
        "launchOnStartup": false,
        "silentStartup": false,
        "theme": "dark",
        "currentProviderClaude": "target-claude",
        "currentProviderCodex": "target-codex",
        "currentProviderOpenCode": "target-opencode",
        "skillSyncMethod": "copy",
        "skillStorageLocation": "unified",
        "futureDeviceSetting": {"enabled": true},
        "enableLocalProxy": true,
        "proxyConfirmed": true,
        "usageConfirmed": true,
        "usageDashboardRefreshIntervalMs": 30000,
        "enableFailoverToggle": true,
        "failoverConfirmed": true,
        "showProfileSwitcher": true,
        "visibleApps": {"claude": true, "gemini": true},
        "claudeConfigDir": "/tmp/claude",
        "codexConfigDir": "/tmp/codex",
        "opencodeConfigDir": "/tmp/opencode",
        "currentProviderGemini": "gemini-provider",
        "unifyCodexSessionHistory": true,
        "localMigrations": {"legacy": true},
        "s3Sync": {"secretAccessKey": "s3-secret"},
        "webdavBackup": {"password": "backup-secret"},
        "backupIntervalHours": 24,
        "preferredTerminal": "powershell",
        "dailyBrief": {"apiKey": "brief-secret", "model": "summary-model"},
        "webdavSync": {
            "enabled": true,
            "autoSync": true,
            "baseUrl": "https://target.example.invalid/dav",
            "username": "target-user",
            "password": "target-password",
            "remoteRoot": "target-root",
            "profile": "obsolete-profile",
            "status": {"lastSyncAt": 42},
            "futureTransportOption": "keep"
        }
    }))
    .unwrap();
    let legacy = r#"{
        "launchOnStartup": true,
        "silentStartup": true,
        "skipClaudeOnboarding": true,
        "webdavSync": {
            "enabled": false,
            "baseUrl": "https://legacy.example.invalid/dav",
            "username": "legacy-user",
            "password": "legacy-password",
            "remoteRoot": "legacy-root"
        }
    }"#;

    let prepared = prepare_device_changes(Some(&current), Some(legacy), None).unwrap();
    let value: Value = serde_json::from_slice(&prepared.settings.unwrap()).unwrap();

    assert_eq!(value["launchOnStartup"], false);
    assert_eq!(value["silentStartup"], false);
    assert_eq!(value["skipClaudeOnboarding"], true);
    assert_eq!(value["theme"], "dark");
    assert_eq!(value["currentProviderClaude"], "target-claude");
    assert_eq!(value["currentProviderCodex"], "target-codex");
    assert_eq!(value["currentProviderOpenCode"], "target-opencode");
    assert!(value.get("skillSyncMethod").is_none());
    assert!(value.get("skillStorageLocation").is_none());
    assert_eq!(value["futureDeviceSetting"]["enabled"], true);
    assert_eq!(
        value["webdavSync"]["baseUrl"],
        "https://target.example.invalid/dav"
    );
    assert_eq!(value["webdavSync"]["username"], "target-user");
    assert_eq!(value["webdavSync"]["remoteRoot"], "target-root");
    assert_eq!(value["webdavSync"]["profile"], "obsolete-profile");
    assert!(value["webdavSync"].get("status").is_none());
    assert!(value["webdavSync"].get("futureTransportOption").is_none());
    assert!(value["webdavSync"].get("autoSync").is_none());
    assert!(value["webdavSync"].get("enabled").is_none());
    assert_eq!(value["dailyBrief"]["model"], "summary-model");
    assert_eq!(prepared.webdav_password.as_deref(), Some("target-password"));

    for discarded in [
        "enableLocalProxy",
        "proxyConfirmed",
        "usageConfirmed",
        "usageDashboardRefreshIntervalMs",
        "enableFailoverToggle",
        "failoverConfirmed",
        "showProfileSwitcher",
        "visibleApps",
        "claudeConfigDir",
        "codexConfigDir",
        "opencodeConfigDir",
        "currentProviderGemini",
        "unifyCodexSessionHistory",
        "localMigrations",
        "s3Sync",
        "webdavBackup",
        "backupIntervalHours",
        "preferredTerminal",
    ] {
        assert!(value.get(discarded).is_none(), "discard {discarded}");
    }
    assert!(value["webdavSync"].get("password").is_none());
    assert!(value["dailyBrief"].get("apiKey").is_none());
}

#[test]
fn existing_credential_wins_and_legacy_only_fills_missing_webdav_fields() {
    let current = br#"{
        "webdavSync": {
            "enabled": true,
            "baseUrl": "https://target.example.invalid/dav"
        }
    }"#;
    let legacy = r#"{
        "webdavSync": {
            "enabled": false,
            "baseUrl": "https://legacy.example.invalid/dav",
            "username": "legacy-user",
            "password": "legacy-password",
            "remoteRoot": "legacy-root"
        }
    }"#;

    let prepared =
        prepare_device_changes(Some(current), Some(legacy), Some("credential-password")).unwrap();
    let value: Value = serde_json::from_slice(&prepared.settings.unwrap()).unwrap();

    assert!(value["webdavSync"].get("enabled").is_none());
    assert_eq!(
        value["webdavSync"]["baseUrl"],
        "https://target.example.invalid/dav"
    );
    assert_eq!(value["webdavSync"]["username"], "legacy-user");
    assert_eq!(value["webdavSync"]["remoteRoot"], "legacy-root");
    assert!(value["webdavSync"].get("autoSync").is_none());
    assert_eq!(prepared.webdav_password, None);
}

#[test]
fn late_failure_restores_database_settings_and_credentials_and_retains_recovery_point() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    settings.replace(b"{\"launchOnStartup\":false}\n").unwrap();
    let previous_settings = settings.read().unwrap();
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    let rollbacks = MemoryRollbacks::default();
    let target = FailingMarkTarget {
        database: &database,
    };

    let error = migrate_retained_data(
        &source,
        &target,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .unwrap_err();

    assert!(error.to_string().contains("标记跨资源迁移完成"));
    assert_eq!(core_count(&database, "core_providers"), 0);
    assert_eq!(settings.read().unwrap(), previous_settings);
    assert_eq!(secrets.read(DeviceSecretId::WebdavPassword).unwrap(), None);
    let points = rollbacks.list().unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].state, RollbackPointState::Failed);
}

#[test]
fn credential_failure_also_rolls_back_database_and_settings() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    secrets.fail_next_write();
    let rollbacks = MemoryRollbacks::default();

    assert!(migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .is_err());
    assert_eq!(core_count(&database, "core_providers"), 0);
    assert_eq!(settings.read().unwrap(), None);
    assert_eq!(secrets.read(DeviceSecretId::WebdavPassword).unwrap(), None);
}

#[test]
fn settings_failure_rolls_back_database_before_credentials_are_touched() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let previous = b"{\"launchOnStartup\":false}\n".to_vec();
    let settings = FailingSettings::new(Some(previous.clone()));
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    let rollbacks = MemoryRollbacks::default();

    assert!(migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .is_err());
    assert_eq!(core_count(&database, "core_providers"), 0);
    assert_eq!(settings.read().unwrap(), Some(previous));
    assert_eq!(secrets.read(DeviceSecretId::WebdavPassword).unwrap(), None);
}

#[test]
fn pending_crash_rollback_is_restored_before_a_clean_retry() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let preview = source.preview().unwrap();
    let fingerprint = preview.directory_fingerprint.unwrap();
    let snapshot = source.load_retained(&fingerprint).unwrap();
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    settings.replace(b"{\"launchOnStartup\":false}\n").unwrap();
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    secrets
        .write(DeviceSecretId::WebdavPassword, "previous-password")
        .unwrap();
    let rollbacks = MemoryRollbacks::default();

    let payload = MigrationRollbackPayload {
        schema_version: ROLLBACK_SCHEMA_VERSION,
        source_fingerprint: fingerprint.clone(),
        previous_settings: settings.read().unwrap(),
        previous_webdav_password: Some("previous-password".to_string()),
        webdav_password_changed: true,
    };
    rollbacks
        .create(
            RollbackPointPurpose::DataMigration,
            1_700_000_000_000,
            &serde_json::to_vec(&payload).unwrap(),
        )
        .unwrap();
    database
        .apply_retained(&snapshot, 1_700_000_000_000)
        .unwrap();
    settings.replace(b"{\"partial\":true}\n").unwrap();
    secrets
        .write(DeviceSecretId::WebdavPassword, "partial-password")
        .unwrap();

    let report = migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_100,
    )
    .unwrap()
    .expect("clean retry applied");

    assert_eq!(report.retained.total(), 9);
    assert_eq!(core_count(&database, "core_providers"), 3);
    let written: Value = serde_json::from_slice(&settings.read().unwrap().unwrap()).unwrap();
    assert!(written.get("partial").is_none());
    assert!(written["webdavSync"].get("autoSync").is_none());
    assert_eq!(
        secrets
            .read(DeviceSecretId::WebdavPassword)
            .unwrap()
            .as_deref(),
        Some("previous-password"),
        "an existing target credential wins over the legacy password"
    );
    let points = rollbacks.list().unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].state, RollbackPointState::Failed);
}

#[test]
fn committed_migration_with_cleanup_failure_retries_cleanup_without_reapplying() {
    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    let database = Database::memory().unwrap();
    let secrets = MemorySecrets::default();
    let rollbacks = MemoryRollbacks::default();
    rollbacks.fail_next_delete();

    let first = migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .unwrap_err();
    assert!(first.to_string().contains("删除迁移临时回滚点"));
    assert_eq!(core_count(&database, "core_providers"), 3);
    assert_eq!(rollbacks.created_count(), 1);
    let points = rollbacks.list().unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].state, RollbackPointState::Pending);

    let second = migrate_retained_data(
        &source,
        &database,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_100,
    )
    .unwrap();

    assert!(second.is_none());
    assert_eq!(core_count(&database, "core_providers"), 3);
    assert_eq!(rollbacks.created_count(), 1, "migration must not reapply");
    assert!(rollbacks.list().unwrap().is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn native_windows_dpapi_credential_and_file_rollback_is_exact() {
    use crate::adapters::local_protection::WindowsDpapiProtector;
    use crate::adapters::secret_store::WindowsCredentialStore;
    use crate::adapters::temporary_rollback::FixedTemporaryRollbackStore;

    struct CredentialCleanup(WindowsCredentialStore);
    impl Drop for CredentialCleanup {
        fn drop(&mut self) {
            let _ = self.0.delete(DeviceSecretId::WebdavPassword);
        }
    }

    let source_home = tempfile::tempdir().unwrap();
    let source = create_legacy_source(source_home.path());
    let target_root = tempfile::tempdir().unwrap();
    let settings = FixedDeviceSettingsStore::new(target_root.path().join("app"));
    let original_settings = b"{\"launchOnStartup\":false}\r\n".to_vec();
    settings.replace(&original_settings).unwrap();
    let database = Database::memory().unwrap();
    let secrets = WindowsCredentialStore::isolated("p2-05-native-rollback");
    let _cleanup = CredentialCleanup(secrets.clone());
    secrets.delete(DeviceSecretId::WebdavPassword).unwrap();
    let rollbacks = FixedTemporaryRollbackStore::new(
        target_root.path().join("rollbacks"),
        WindowsDpapiProtector,
    );
    let target = FailingMarkTarget {
        database: &database,
    };

    assert!(migrate_retained_data(
        &source,
        &target,
        &settings,
        &secrets,
        &rollbacks,
        1_700_000_000_000,
    )
    .is_err());

    assert_eq!(core_count(&database, "core_providers"), 0);
    assert_eq!(settings.read().unwrap(), Some(original_settings));
    assert_eq!(secrets.read(DeviceSecretId::WebdavPassword).unwrap(), None);
    let points = rollbacks.list().unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].state, RollbackPointState::Failed);
    assert!(!rollbacks.restore(&points[0].id).unwrap().is_empty());
    rollbacks.delete_after_success(&points[0].id).unwrap();
    assert!(rollbacks.list().unwrap().is_empty());
}

#[allow(dead_code)]
fn assert_ports_are_object_safe(
    _settings: &dyn DeviceSettingsStore,
    _error: Option<DeviceSettingsError>,
) {
}
