use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::domain::{
    LegacyMigrationStatus, RetainedMigrationReport, RollbackPointPurpose, RollbackPointState,
};
use crate::error::AppError;
use crate::ports::{
    DeviceSecretId, DeviceSettingsStore, LegacyDataSource, RetainedMigrationTarget, SecretStore,
    TemporaryRollbackStore,
};

const ROLLBACK_SCHEMA_VERSION: u32 = 1;

/// Sensitive rollback body. It deliberately has no `Debug` implementation.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MigrationRollbackPayload {
    schema_version: u32,
    source_fingerprint: String,
    previous_settings: Option<Vec<u8>>,
    previous_webdav_password: Option<String>,
    webdav_password_changed: bool,
}

struct PreparedDeviceChanges {
    settings: Option<Vec<u8>>,
    webdav_password: Option<String>,
}

pub fn migrate_retained_data<S, T, D, C, R>(
    source: &S,
    target: &T,
    device_settings: &D,
    secrets: &C,
    rollback_store: &R,
    now_ms: i64,
) -> Result<Option<RetainedMigrationReport>, AppError>
where
    S: LegacyDataSource,
    T: RetainedMigrationTarget,
    D: DeviceSettingsStore,
    C: SecretStore,
    R: TemporaryRollbackStore,
{
    if now_ms < 0 {
        return Err(AppError::Config("保留数据迁移时间不能为负数".to_string()));
    }
    recover_pending_migrations(target, device_settings, secrets, rollback_store, now_ms)?;
    if target
        .retained_resources_complete()
        .map_err(|error| migration_error("检查迁移完成状态", error))?
    {
        return Ok(None);
    }

    let preview = source
        .preview()
        .map_err(|error| migration_error("只读检查旧数据", error))?;
    if preview.status != LegacyMigrationStatus::Ready {
        return Ok(None);
    }
    let fingerprint = preview
        .directory_fingerprint
        .as_deref()
        .ok_or_else(|| AppError::Config("旧数据迁移预览缺少来源指纹".to_string()))?;
    let snapshot = source
        .load_retained(fingerprint)
        .map_err(|error| migration_error("只读加载保留数据", error))?;
    let previous_settings = device_settings
        .read()
        .map_err(|error| migration_error("读取目标设备设置", error))?;
    let previous_webdav_password = secrets
        .read(DeviceSecretId::WebdavPassword)
        .map_err(|error| migration_error("读取目标 WebDAV 凭据", error))?;
    let prepared = prepare_device_changes(
        previous_settings.as_deref(),
        snapshot.legacy_settings_json.as_deref(),
        previous_webdav_password.as_deref(),
    )?;
    let webdav_password_changed = prepared.webdav_password.is_some();
    let rollback_payload = MigrationRollbackPayload {
        schema_version: ROLLBACK_SCHEMA_VERSION,
        source_fingerprint: fingerprint.to_string(),
        previous_settings,
        previous_webdav_password,
        webdav_password_changed,
    };
    let rollback_bytes = serde_json::to_vec(&rollback_payload)
        .map_err(|error| AppError::Config(format!("序列化迁移回滚载荷失败: {error}")))?;
    let rollback = rollback_store
        .create(RollbackPointPurpose::DataMigration, now_ms, &rollback_bytes)
        .map_err(|error| migration_error("创建 DPAPI 临时回滚点", error))?;

    let operation = (|| {
        let report = target
            .apply_retained(&snapshot, now_ms)
            .map_err(|error| migration_error("写入保留数据库记录", error))?;
        if let Some(settings) = &prepared.settings {
            device_settings
                .replace(settings)
                .map_err(|error| migration_error("写入设备设置", error))?;
        }
        if let Some(password) = &prepared.webdav_password {
            secrets
                .write(DeviceSecretId::WebdavPassword, password)
                .map_err(|error| migration_error("迁移 WebDAV 凭据", error))?;
        }
        target
            .mark_retained_resources_complete(fingerprint, now_ms)
            .map_err(|error| migration_error("标记跨资源迁移完成", error))?;
        Ok(report)
    })();

    match operation {
        Ok(report) => {
            rollback_store
                .delete_after_success(&rollback.id)
                .map_err(|error| migration_error("删除迁移临时回滚点", error))?;
            Ok(Some(report))
        }
        Err(operation_error) => {
            let restoration =
                restore_resources(target, device_settings, secrets, &rollback_payload);
            let retained = rollback_store.retain_after_failure(&rollback.id, now_ms);
            match (restoration, retained) {
                (Ok(()), Ok(_)) => Err(operation_error),
                (restore_result, retain_result) => Err(AppError::Config(format!(
                    "保留数据迁移失败，且回滚状态不完整: {operation_error}; 恢复={}; 保留回滚点={}",
                    result_label(restore_result),
                    result_label(retain_result.map(|_| ()))
                ))),
            }
        }
    }
}

fn recover_pending_migrations<T, D, C, R>(
    target: &T,
    device_settings: &D,
    secrets: &C,
    rollback_store: &R,
    now_ms: i64,
) -> Result<(), AppError>
where
    T: RetainedMigrationTarget,
    D: DeviceSettingsStore,
    C: SecretStore,
    R: TemporaryRollbackStore,
{
    let points = rollback_store
        .list()
        .map_err(|error| migration_error("列出迁移临时回滚点", error))?;
    for point in points.into_iter().filter(|point| {
        point.purpose == RollbackPointPurpose::DataMigration
            && point.state == RollbackPointState::Pending
    }) {
        let bytes = rollback_store
            .restore(&point.id)
            .map_err(|error| migration_error("读取迁移临时回滚点", error))?;
        let payload = decode_rollback_payload(&bytes)?;
        if target
            .retained_resources_complete()
            .map_err(|error| migration_error("检查中断迁移完成状态", error))?
        {
            rollback_store
                .delete_after_success(&point.id)
                .map_err(|error| migration_error("清理已完成迁移回滚点", error))?;
            continue;
        }
        restore_resources(target, device_settings, secrets, &payload)?;
        rollback_store
            .retain_after_failure(&point.id, now_ms.max(point.created_at_ms))
            .map_err(|error| migration_error("保留中断迁移回滚点", error))?;
    }
    Ok(())
}

fn restore_resources<T, D, C>(
    target: &T,
    device_settings: &D,
    secrets: &C,
    payload: &MigrationRollbackPayload,
) -> Result<(), AppError>
where
    T: RetainedMigrationTarget,
    D: DeviceSettingsStore,
    C: SecretStore,
{
    let mut failures = Vec::new();
    if let Err(error) = target.rollback_retained(&payload.source_fingerprint) {
        failures.push(format!("数据库={error}"));
    }
    let settings_result = match &payload.previous_settings {
        Some(contents) => device_settings.replace(contents),
        None => device_settings.delete(),
    };
    if let Err(error) = settings_result {
        failures.push(format!("设置={error}"));
    }
    if payload.webdav_password_changed {
        let secret_result = match &payload.previous_webdav_password {
            Some(password) => secrets.write(DeviceSecretId::WebdavPassword, password),
            None => secrets.delete(DeviceSecretId::WebdavPassword),
        };
        if let Err(error) = secret_result {
            failures.push(format!("凭据={error}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::Config(format!(
            "迁移资源恢复失败: {}",
            failures.join("; ")
        )))
    }
}

fn decode_rollback_payload(bytes: &[u8]) -> Result<MigrationRollbackPayload, AppError> {
    let payload: MigrationRollbackPayload = serde_json::from_slice(bytes)
        .map_err(|error| AppError::Config(format!("迁移回滚载荷损坏: {error}")))?;
    if payload.schema_version != ROLLBACK_SCHEMA_VERSION
        || payload.source_fingerprint.len() != 64
        || !payload
            .source_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(AppError::Config("迁移回滚载荷版本或指纹无效".to_string()));
    }
    Ok(payload)
}

fn prepare_device_changes(
    current_bytes: Option<&[u8]>,
    legacy_json: Option<&str>,
    stored_password: Option<&str>,
) -> Result<PreparedDeviceChanges, AppError> {
    if current_bytes.is_none() && legacy_json.is_none() {
        return Ok(PreparedDeviceChanges {
            settings: None,
            webdav_password: None,
        });
    }
    let legacy: Value = match legacy_json {
        Some(json) => serde_json::from_str(json)
            .map_err(|error| AppError::Config(format!("解析旧设备设置失败: {error}")))?,
        None => Value::Object(Map::new()),
    };
    let legacy = legacy
        .as_object()
        .ok_or_else(|| AppError::Config("旧设备设置根节点必须是对象".to_string()))?;
    let current: Value = match current_bytes {
        Some(bytes) => serde_json::from_slice(bytes)
            .map_err(|error| AppError::Config(format!("解析目标设备设置失败: {error}")))?,
        None => Value::Object(Map::new()),
    };
    let current = current
        .as_object()
        .ok_or_else(|| AppError::Config("目标设备设置根节点必须是对象".to_string()))?;

    let mut output = sanitize_device_settings(current);
    output.insert("showInTray".to_string(), Value::Bool(true));
    output.insert("useAppWindowControls".to_string(), Value::Bool(false));
    output.insert("language".to_string(), Value::String("zh".to_string()));
    for (camel, snake) in [
        (
            "enableClaudePluginIntegration",
            "enable_claude_plugin_integration",
        ),
        ("skipClaudeOnboarding", "skip_claude_onboarding"),
        ("launchOnStartup", "launch_on_startup"),
        ("silentStartup", "silent_startup"),
        ("firstRunNoticeConfirmed", "first_run_notice_confirmed"),
        ("commonConfigConfirmed", "common_config_confirmed"),
    ] {
        remove_aliases(&mut output, camel, snake);
        if let Some(value) =
            bool_value(current, camel, snake).or_else(|| bool_value(legacy, camel, snake))
        {
            output.insert(camel.to_string(), Value::Bool(value));
        }
    }

    let current_webdav = object_value(current, "webdavSync", "webdav_sync");
    let legacy_webdav = object_value(legacy, "webdavSync", "webdav_sync");
    let mut webdav_password = None;
    remove_aliases(&mut output, "webdavSync", "webdav_sync");
    if current_webdav.is_some() || legacy_webdav.is_some() {
        let mut sanitized = Map::new();
        for (camel, snake) in [
            ("baseUrl", "base_url"),
            ("username", "username"),
            ("remoteRoot", "remote_root"),
            ("profile", "profile"),
        ] {
            if let Some(value) = current_webdav
                .and_then(|webdav| string_value(webdav, camel, snake))
                .or_else(|| legacy_webdav.and_then(|webdav| string_value(webdav, camel, snake)))
            {
                sanitized.insert(camel.to_string(), Value::String(value.to_string()));
            }
        }
        output.insert("webdavSync".to_string(), Value::Object(sanitized));

        if stored_password.is_none() {
            webdav_password = current_webdav
                .and_then(|webdav| string_value(webdav, "password", "password"))
                .or_else(|| {
                    legacy_webdav.and_then(|webdav| string_value(webdav, "password", "password"))
                })
                .filter(|password| !password.is_empty())
                .map(str::to_string);
        }
    }

    let settings = serde_json::to_vec_pretty(&Value::Object(output))
        .map_err(|error| AppError::Config(format!("序列化目标设备设置失败: {error}")))?;
    Ok(PreparedDeviceChanges {
        settings: Some(settings),
        webdav_password,
    })
}

fn sanitize_device_settings(current: &Map<String, Value>) -> Map<String, Value> {
    current
        .iter()
        .filter(|(key, _)| !is_forbidden_device_setting(key) && !is_sensitive_key(key))
        .map(|(key, value)| (key.clone(), sanitize_nested_value(value)))
        .collect()
}

fn sanitize_nested_object(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .filter(|(key, _)| !is_sensitive_key(key))
        .map(|(key, value)| (key.clone(), sanitize_nested_value(value)))
        .collect()
}

fn sanitize_nested_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(sanitize_nested_object(object)),
        Value::Array(values) => {
            Value::Array(values.iter().map(sanitize_nested_value).collect::<Vec<_>>())
        }
        _ => value.clone(),
    }
}

fn is_forbidden_device_setting(key: &str) -> bool {
    matches!(
        normalized_key(key).as_str(),
        "enablelocalproxy"
            | "proxyconfirmed"
            | "usageconfirmed"
            | "usagedashboardrefreshintervalms"
            | "enablefailovertoggle"
            | "showprofileswitcher"
            | "preservecodexofficialauthonswitch"
            | "unifycodexsessionhistory"
            | "unifycodexmigrateexisting"
            | "failoverconfirmed"
            | "visibleapps"
            | "claudeconfigdir"
            | "codexconfigdir"
            | "geminiconfigdir"
            | "grokconfigdir"
            | "opencodeconfigdir"
            | "openclawconfigdir"
            | "hermesconfigdir"
            | "currentproviderclaudedesktop"
            | "currentprovidergemini"
            | "currentprovidergrokbuild"
            | "currentprovideropenclaw"
            | "currentproviderhermes"
            | "localmigrations"
            | "s3sync"
            | "webdavbackup"
            | "backupintervalhours"
            | "backupretaincount"
            | "preferredterminal"
            | "skillsyncmethod"
            | "skillstoragelocation"
    )
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalized_key(key);
    [
        "apikey",
        "authtoken",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "password",
        "privatekey",
        "secret",
        "accesstoken",
        "refreshtoken",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
        || normalized == "auth"
}

fn normalized_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn remove_aliases(object: &mut Map<String, Value>, camel: &str, snake: &str) {
    object.remove(camel);
    object.remove(snake);
}

fn object_value<'a>(
    object: &'a Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a Map<String, Value>> {
    object
        .get(camel)
        .or_else(|| object.get(snake))
        .and_then(Value::as_object)
}

fn bool_value(object: &Map<String, Value>, camel: &str, snake: &str) -> Option<bool> {
    object
        .get(camel)
        .or_else(|| object.get(snake))
        .and_then(Value::as_bool)
}

fn string_value<'a>(object: &'a Map<String, Value>, camel: &str, snake: &str) -> Option<&'a str> {
    object
        .get(camel)
        .or_else(|| object.get(snake))
        .and_then(Value::as_str)
}

fn migration_error(stage: &str, error: impl std::fmt::Display) -> AppError {
    AppError::Config(format!("{stage}失败: {error}"))
}

fn result_label<T, E: std::fmt::Display>(result: Result<T, E>) -> String {
    match result {
        Ok(_) => "成功".to_string(),
        Err(error) => error.to_string(),
    }
}

#[cfg(target_os = "windows")]
pub fn migrate_retained_data_runtime(
    database: &crate::database::Database,
    now_ms: i64,
) -> Result<Option<RetainedMigrationReport>, AppError> {
    migrate_retained_data(
        &crate::adapters::legacy_data::FixedLegacyDataSource::runtime(),
        database,
        &crate::adapters::device_settings::FixedDeviceSettingsStore::runtime(),
        &crate::adapters::secret_store::WindowsCredentialStore::runtime(),
        &crate::adapters::temporary_rollback::FixedTemporaryRollbackStore::runtime(),
        now_ms,
    )
}

#[cfg(test)]
mod tests;
