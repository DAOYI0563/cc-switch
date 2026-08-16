#![allow(non_snake_case)]

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;
use zeroize::Zeroizing;

use crate::adapters::{FixedSyncCryptoEngine, ReqwestSyncWebDavTransport, RuntimeSyncLocalAdapter};
use crate::domain::{
    Sha256Digest, SyncDevice, SyncDeviceId, SyncFirstSyncPreview, SyncSchemaVersion,
};
use crate::services::{
    ConflictCenterRuntimeState, SyncDeviceRetireRequest, SyncFirstSyncConfirmRequest,
    SyncFirstSyncPreviewRequest, SyncRunError, SyncRunErrorCode, SyncRunResult, SyncV3Orchestrator,
};
use crate::settings::{self, WebDavSyncSettings};
use crate::store::AppState;

use crate::adapters::temporary_rollback::FixedTemporaryRollbackStore;

fn resolve_password_for_request(
    mut incoming: WebDavSyncSettings,
    existing: Option<WebDavSyncSettings>,
    preserve_empty_password: bool,
) -> WebDavSyncSettings {
    if let Some(existing_settings) = existing {
        if preserve_empty_password && incoming.password.is_empty() {
            incoming.password = existing_settings.password;
        }
    }
    incoming
}

#[tauri::command]
pub async fn webdav_test_connection(
    settings: WebDavSyncSettings,
    #[allow(non_snake_case)] preserveEmptyPassword: Option<bool>,
) -> Result<Value, String> {
    let preserve_empty = preserveEmptyPassword.unwrap_or(true);
    let mut resolved = resolve_password_for_request(
        settings,
        settings::get_webdav_sync_settings(),
        preserve_empty,
    );
    resolved.normalize();
    resolved.validate().map_err(|error| error.to_string())?;

    let transport =
        ReqwestSyncWebDavTransport::new(&resolved.base_url, &resolved.username, &resolved.password)
            .map_err(|error| error.to_string())?;
    transport
        .test_connection()
        .await
        .map_err(|error| error.to_string())?;

    Ok(json!({
        "success": true,
        "message": "WebDAV connection ok"
    }))
}

#[tauri::command]
pub async fn webdav_sync_save_settings(
    settings: WebDavSyncSettings,
    #[allow(non_snake_case)] passwordTouched: Option<bool>,
) -> Result<Value, String> {
    let password_touched = passwordTouched.unwrap_or(false);
    let existing = settings::get_webdav_sync_settings();
    let mut sync_settings = resolve_password_for_request(settings, existing, !password_touched);

    sync_settings.normalize();
    sync_settings
        .validate()
        .map_err(|error| error.to_string())?;
    settings::set_webdav_sync_settings(Some(sync_settings), password_touched)
        .map_err(|error| error.to_string())?;
    Ok(json!({ "success": true }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebDavFirstSyncPreviewCommandRequest {
    pub passphrase: String,
    pub display_name: String,
    pub candidate_device_id: Option<SyncDeviceId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebDavFirstSyncConfirmCommandRequest {
    pub passphrase: String,
    pub display_name: String,
    pub candidate_device_id: SyncDeviceId,
    pub observed_at_ms: i64,
    pub expected_preview_token: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebDavSyncNowCommandRequest {
    pub passphrase: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebDavDeviceRetireCommandRequest {
    pub passphrase: String,
    pub target_device_id: SyncDeviceId,
    pub confirmed_target_device_id: SyncDeviceId,
}

#[tauri::command]
pub async fn webdav_sync_preview_first(
    request: WebDavFirstSyncPreviewCommandRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<SyncFirstSyncPreview, SyncRunError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let candidate_device_id = request.candidate_device_id.unwrap_or_else(|| {
        SyncDeviceId::new(format!("device-{}", uuid::Uuid::new_v4().simple()))
            .expect("UUID-based sync device ID is valid")
    });
    let local = RuntimeSyncLocalAdapter::new(app_state.inner());
    let snapshot = local
        .snapshot(&candidate_device_id, now_ms)
        .map_err(map_local_snapshot_error)?;
    if snapshot.identity.is_some() {
        return Err(SyncRunError::new(
            SyncRunErrorCode::InvalidInput,
            "this device is already registered; use sync now",
        ));
    }
    let passphrase = Zeroizing::new(request.passphrase);
    let transport = configured_transport()?;
    let crypto = FixedSyncCryptoEngine::runtime();
    let rollbacks = FixedTemporaryRollbackStore::runtime();
    let conflicts = conflict_state.webdav();
    let orchestrator =
        SyncV3Orchestrator::new(&transport, &crypto, &local, &rollbacks, conflicts.as_ref());
    orchestrator
        .preview_first_sync(
            passphrase.as_bytes(),
            SyncFirstSyncPreviewRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id,
                display_name: request.display_name,
                observed_at_ms: now_ms,
                baselines: snapshot.baselines,
                local_records: snapshot.local_records,
            },
        )
        .await
}

#[tauri::command]
pub async fn webdav_sync_confirm_first(
    request: WebDavFirstSyncConfirmCommandRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<SyncRunResult, SyncRunError> {
    let confirmed_at_ms = chrono::Utc::now()
        .timestamp_millis()
        .max(request.observed_at_ms);
    let local = RuntimeSyncLocalAdapter::new(app_state.inner());
    let snapshot = local
        .snapshot(&request.candidate_device_id, request.observed_at_ms)
        .map_err(map_local_snapshot_error)?;
    let passphrase = Zeroizing::new(request.passphrase);
    let transport = configured_transport()?;
    let crypto = FixedSyncCryptoEngine::runtime();
    let rollbacks = FixedTemporaryRollbackStore::runtime();
    let conflicts = conflict_state.webdav();
    let orchestrator =
        SyncV3Orchestrator::new(&transport, &crypto, &local, &rollbacks, conflicts.as_ref());
    orchestrator
        .confirm_first_sync(
            passphrase.as_bytes(),
            SyncFirstSyncConfirmRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: request.candidate_device_id,
                display_name: request.display_name,
                observed_at_ms: request.observed_at_ms,
                baselines: snapshot.baselines,
                local_records: snapshot.local_records,
                expected_preview_token: request.expected_preview_token,
                existing_identity: snapshot.identity,
                confirmed_at_ms,
            },
        )
        .await
}

#[tauri::command]
pub async fn webdav_sync_now(
    request: WebDavSyncNowCommandRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<SyncRunResult, SyncRunError> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let identity = require_local_identity(app_state.inner())?;
    let local = RuntimeSyncLocalAdapter::new(app_state.inner());
    let snapshot = local
        .snapshot(&identity.device_id, now_ms)
        .map_err(map_local_snapshot_error)?;
    let passphrase = Zeroizing::new(request.passphrase);
    let transport = configured_transport()?;
    let crypto = FixedSyncCryptoEngine::runtime();
    let rollbacks = FixedTemporaryRollbackStore::runtime();
    let conflicts = conflict_state.webdav();
    let orchestrator =
        SyncV3Orchestrator::new(&transport, &crypto, &local, &rollbacks, conflicts.as_ref());
    orchestrator
        .synchronize(
            passphrase.as_bytes(),
            crate::services::SyncRunRequest {
                schema_version: SyncSchemaVersion::V1,
                device_id: identity.device_id,
                now_ms,
                baselines: snapshot.baselines,
                local_records: snapshot.local_records,
            },
        )
        .await
}

#[tauri::command]
pub async fn webdav_sync_list_devices(
    request: WebDavSyncNowCommandRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<Vec<SyncDevice>, SyncRunError> {
    require_local_identity(app_state.inner())?;
    let passphrase = Zeroizing::new(request.passphrase);
    let transport = configured_transport()?;
    let crypto = FixedSyncCryptoEngine::runtime();
    let local = RuntimeSyncLocalAdapter::new(app_state.inner());
    let rollbacks = FixedTemporaryRollbackStore::runtime();
    let conflicts = conflict_state.webdav();
    SyncV3Orchestrator::new(&transport, &crypto, &local, &rollbacks, conflicts.as_ref())
        .list_devices(passphrase.as_bytes())
        .await
}

#[tauri::command]
pub async fn webdav_sync_retire_device(
    request: WebDavDeviceRetireCommandRequest,
    app_state: State<'_, AppState>,
    conflict_state: State<'_, ConflictCenterRuntimeState>,
) -> Result<SyncRunResult, SyncRunError> {
    let identity = require_local_identity(app_state.inner())?;
    let passphrase = Zeroizing::new(request.passphrase);
    let transport = configured_transport()?;
    let crypto = FixedSyncCryptoEngine::runtime();
    let local = RuntimeSyncLocalAdapter::new(app_state.inner());
    let rollbacks = FixedTemporaryRollbackStore::runtime();
    let conflicts = conflict_state.webdav();
    SyncV3Orchestrator::new(&transport, &crypto, &local, &rollbacks, conflicts.as_ref())
        .retire_device(
            passphrase.as_bytes(),
            SyncDeviceRetireRequest {
                schema_version: SyncSchemaVersion::V1,
                writer_device_id: identity.device_id,
                target_device_id: request.target_device_id,
                confirmed_target_device_id: request.confirmed_target_device_id,
                retired_at_ms: chrono::Utc::now().timestamp_millis(),
            },
        )
        .await
}

fn configured_transport() -> Result<ReqwestSyncWebDavTransport, SyncRunError> {
    let mut configured = settings::get_webdav_sync_settings().ok_or_else(|| {
        SyncRunError::new(
            SyncRunErrorCode::InvalidInput,
            "WebDAV settings must be saved before synchronization",
        )
    })?;
    configured.normalize();
    configured.validate().map_err(|_| {
        SyncRunError::new(
            SyncRunErrorCode::InvalidInput,
            "saved WebDAV settings are invalid",
        )
    })?;
    let base_url = sync_profile_base_url(&configured)?;
    ReqwestSyncWebDavTransport::new(&base_url, &configured.username, &configured.password).map_err(
        |_| {
            SyncRunError::new(
                SyncRunErrorCode::InvalidInput,
                "saved WebDAV transport settings are invalid",
            )
        },
    )
}

fn sync_profile_base_url(settings: &WebDavSyncSettings) -> Result<String, SyncRunError> {
    validate_remote_component(&settings.remote_root)?;
    validate_remote_component(&settings.profile)?;
    let mut url = url::Url::parse(&settings.base_url)
        .map_err(|_| SyncRunError::new(SyncRunErrorCode::InvalidInput, "WebDAV URL is invalid"))?;
    let mut segments = url.path_segments_mut().map_err(|_| {
        SyncRunError::new(
            SyncRunErrorCode::InvalidInput,
            "WebDAV URL cannot contain sync paths",
        )
    })?;
    segments.pop_if_empty();
    segments.push(&settings.remote_root);
    segments.push(&settings.profile);
    drop(segments);
    Ok(url.to_string())
}

fn validate_remote_component(value: &str) -> Result<(), SyncRunError> {
    let valid = !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(SyncRunError::new(
            SyncRunErrorCode::InvalidInput,
            "WebDAV sync root or profile is invalid",
        ))
    }
}

fn require_local_identity(
    state: &AppState,
) -> Result<crate::domain::FixedSyncDeviceIdentity, SyncRunError> {
    state
        .db
        .load_sync_identity()
        .map_err(|_| {
            SyncRunError::new(
                SyncRunErrorCode::LocalApply,
                "fixed sync device identity could not be read",
            )
        })?
        .ok_or_else(|| {
            SyncRunError::new(
                SyncRunErrorCode::InvalidInput,
                "first sync must be previewed and confirmed on this device",
            )
        })
}

fn map_local_snapshot_error(_error: crate::ports::ConflictCenterError) -> SyncRunError {
    SyncRunError::new(
        SyncRunErrorCode::LocalApply,
        "local portable sync snapshot could not be prepared",
    )
}

#[cfg(test)]
mod tests {
    use super::resolve_password_for_request;
    use crate::settings::WebDavSyncSettings;

    #[test]
    fn resolve_password_for_request_preserves_existing_when_requested() {
        let incoming = WebDavSyncSettings {
            base_url: "https://dav.example.com".to_string(),
            username: "alice".to_string(),
            password: String::new(),
            ..WebDavSyncSettings::default()
        };
        let existing = Some(WebDavSyncSettings {
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        });
        let resolved = resolve_password_for_request(incoming, existing, true);
        assert_eq!(resolved.password, "secret");
    }

    #[test]
    fn resolve_password_for_request_allows_explicit_empty_password() {
        let incoming = WebDavSyncSettings {
            base_url: "https://dav.example.com".to_string(),
            username: "alice".to_string(),
            password: String::new(),
            ..WebDavSyncSettings::default()
        };
        let existing = Some(WebDavSyncSettings {
            password: "secret".to_string(),
            ..WebDavSyncSettings::default()
        });
        let resolved = resolve_password_for_request(incoming, existing, false);
        assert!(resolved.password.is_empty());
    }
}
