use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::domain::{
    confirm_first_sync as confirm_first_sync_domain, plan_sync_cas_attempt,
    plan_sync_device_retirement, preview_first_sync as preview_first_sync_domain,
    resolve_sync_cas_write, FixedSyncDeviceIdentity, Sha256Digest, SyncCasAttemptInput,
    SyncCasAttemptKind, SyncCasDecision, SyncCasFailureKind, SyncCasWriteOutcome, SyncDevice,
    SyncDeviceId, SyncDeviceRetirementConsent, SyncDeviceRetirementInput, SyncDeviceStatus,
    SyncEncryptedEnvelope, SyncFirstSyncConfirmationInput, SyncFirstSyncConsent,
    SyncFirstSyncInput, SyncFirstSyncPreview, SyncFirstSyncRemoteGuard, SyncFirstSyncRemoteState,
    SyncLocalCommitPlan, SyncMergeBatch, SyncMergeSideAction, SyncObjectIdentity,
    SyncProtocolVersion, SyncRecord, SyncRecordBaseline, SyncRecordIndexEntry, SyncRemoteObject,
    SyncRemotePath, SyncSchemaVersion, SyncV3Manifest, SyncWriteCondition,
};
use crate::ports::{
    SyncCryptoError, SyncCryptoErrorCode, SyncCryptoPort, SyncCryptoSession, SyncLocalApplyPort,
    SyncTransportError, SyncTransportErrorCode, SyncTransportPort, TemporaryRollbackStore,
    MAX_SYNC_REMOTE_OBJECT_BYTES,
};

use super::{apply_committed_sync_batch, WebDavConflictSource};

const SYNC_ROOT: &str = "sync-v3";
const RECORDS_DIRECTORY: &str = "records";
const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncRunErrorCode {
    InvalidInput,
    RemoteMissing,
    InvalidRemote,
    AuthenticationFailed,
    Crypto,
    Transport,
    ConcurrentWrite,
    ConflictRouting,
    LocalApply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRunError {
    pub code: SyncRunErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
}

impl SyncRunError {
    pub(crate) fn new(code: SyncRunErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: BTreeMap::new(),
        }
    }

    fn with_context(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.context.insert(key.into(), value.into());
        self
    }
}

impl fmt::Display for SyncRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncRunError {}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRunRequest {
    pub schema_version: SyncSchemaVersion,
    pub device_id: SyncDeviceId,
    pub now_ms: i64,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
}

impl fmt::Debug for SyncRunRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncRunRequest")
            .field("schema_version", &self.schema_version)
            .field("device_id", &self.device_id)
            .field("now_ms", &self.now_ms)
            .field("baselines", &self.baselines.len())
            .field("local_records", &self.local_records.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRunResult {
    pub schema_version: SyncSchemaVersion,
    pub committed_generation: u64,
    pub attempts: u8,
    pub resolved_records: u64,
    pub conflicts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub committed_etag: Option<crate::domain::SyncEtag>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncPreviewRequest {
    pub schema_version: SyncSchemaVersion,
    pub candidate_device_id: SyncDeviceId,
    pub display_name: String,
    pub observed_at_ms: i64,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
}

impl fmt::Debug for SyncFirstSyncPreviewRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncFirstSyncPreviewRequest")
            .field("schema_version", &self.schema_version)
            .field("candidate_device_id", &self.candidate_device_id)
            .field("display_name", &self.display_name)
            .field("observed_at_ms", &self.observed_at_ms)
            .field("baselines", &self.baselines.len())
            .field("local_records", &self.local_records.len())
            .finish()
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncConfirmRequest {
    pub schema_version: SyncSchemaVersion,
    pub candidate_device_id: SyncDeviceId,
    pub display_name: String,
    pub observed_at_ms: i64,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
    pub expected_preview_token: Sha256Digest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_identity: Option<FixedSyncDeviceIdentity>,
    pub confirmed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDeviceRetireRequest {
    pub schema_version: SyncSchemaVersion,
    pub writer_device_id: SyncDeviceId,
    pub target_device_id: SyncDeviceId,
    pub confirmed_target_device_id: SyncDeviceId,
    pub retired_at_ms: i64,
}

impl fmt::Debug for SyncFirstSyncConfirmRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncFirstSyncConfirmRequest")
            .field("schema_version", &self.schema_version)
            .field("candidate_device_id", &self.candidate_device_id)
            .field("display_name", &self.display_name)
            .field("observed_at_ms", &self.observed_at_ms)
            .field("baselines", &self.baselines.len())
            .field("local_records", &self.local_records.len())
            .field("expected_preview_token", &self.expected_preview_token)
            .field("existing_identity", &self.existing_identity)
            .field("confirmed_at_ms", &self.confirmed_at_ms)
            .finish()
    }
}

pub fn sync_manifest_remote_path() -> Result<SyncRemotePath, crate::domain::DomainError> {
    SyncRemotePath::new([SYNC_ROOT, MANIFEST_FILE])
}

pub fn sync_record_remote_path(
    index: &SyncRecordIndexEntry,
) -> Result<SyncRemotePath, crate::domain::DomainError> {
    SyncRemotePath::new([
        SYNC_ROOT.to_string(),
        RECORDS_DIRECTORY.to_string(),
        format!("{}.json", index.record_sha256.as_str()),
    ])
}

fn sync_records_directory() -> Result<SyncRemotePath, crate::domain::DomainError> {
    SyncRemotePath::new([SYNC_ROOT, RECORDS_DIRECTORY])
}

pub struct SyncV3Orchestrator<'a> {
    transport: &'a dyn SyncTransportPort,
    crypto: &'a dyn SyncCryptoPort,
    local_applier: &'a dyn SyncLocalApplyPort,
    rollback_store: &'a dyn TemporaryRollbackStore,
    conflicts: &'a WebDavConflictSource,
}

impl<'a> SyncV3Orchestrator<'a> {
    pub fn new(
        transport: &'a dyn SyncTransportPort,
        crypto: &'a dyn SyncCryptoPort,
        local_applier: &'a dyn SyncLocalApplyPort,
        rollback_store: &'a dyn TemporaryRollbackStore,
        conflicts: &'a WebDavConflictSource,
    ) -> Self {
        Self {
            transport,
            crypto,
            local_applier,
            rollback_store,
            conflicts,
        }
    }

    pub async fn synchronize(
        &self,
        passphrase: &[u8],
        request: SyncRunRequest,
    ) -> Result<SyncRunResult, SyncRunError> {
        validate_request(&request)?;

        let mut attempt = SyncCasAttemptKind::Initial;
        let mut previous_failed_guard = None;
        loop {
            let snapshot = self.read_remote_snapshot(passphrase).await?;
            let plan = plan_sync_cas_attempt(SyncCasAttemptInput {
                schema_version: SyncSchemaVersion::V1,
                attempt,
                previous_failed_guard,
                manifest: snapshot.manifest.clone(),
                etag: snapshot.etag.clone(),
                baselines: request.baselines.clone(),
                local_records: request.local_records.clone(),
                remote_records: snapshot.records.clone(),
            })
            .map_err(|_| invalid_input("sync merge input is invalid"))?;

            let candidate_records = candidate_remote_records(&snapshot.records, &plan.merge_batch);
            let candidate_manifest = build_candidate_manifest(
                &snapshot.manifest,
                &candidate_records,
                &request.device_id,
                request.now_ms,
            )?;
            self.write_new_record_objects(
                snapshot.session.as_ref(),
                &snapshot.manifest.records,
                &candidate_records,
            )
            .await?;

            let manifest_identity = SyncObjectIdentity::manifest(candidate_manifest.generation)
                .map_err(|_| invalid_remote("candidate manifest identity is invalid"))?;
            let manifest_plaintext = candidate_manifest
                .to_canonical_json_bytes()
                .map_err(|_| invalid_input("candidate manifest is invalid"))?;
            let manifest_envelope = snapshot
                .session
                .seal(&manifest_identity, &manifest_plaintext)
                .map_err(map_crypto_write)?;
            let manifest_bytes = manifest_envelope
                .to_json_bytes()
                .map_err(|_| invalid_remote("candidate manifest envelope is invalid"))?;
            let manifest_path = sync_manifest_remote_path()
                .map_err(|_| invalid_remote("manifest path is invalid"))?;
            let outcome = match self
                .transport
                .conditional_write(&manifest_path, &manifest_bytes, &plan.write_condition)
                .await
            {
                Ok(receipt) => SyncCasWriteOutcome::Committed {
                    etag: receipt.etag().cloned(),
                },
                Err(error) if error.code == SyncTransportErrorCode::PreconditionFailed => {
                    SyncCasWriteOutcome::PreconditionFailed
                }
                Err(error) => SyncCasWriteOutcome::Failed {
                    failure: map_cas_transport_failure(error.code),
                },
            };

            match resolve_sync_cas_write(plan, outcome) {
                SyncCasDecision::RefetchAndRemergeOnce { failed_guard } => {
                    attempt = SyncCasAttemptKind::RemergeOnce;
                    previous_failed_guard = Some(failed_guard);
                }
                SyncCasDecision::ApplyLocalAfterRemoteCommit {
                    committed_etag,
                    merge_batch,
                } => {
                    self.conflicts
                        .replace_from_merge(&merge_batch)
                        .map_err(|_| {
                            SyncRunError::new(
                                SyncRunErrorCode::ConflictRouting,
                                "remote commit succeeded but conflicts could not be published",
                            )
                        })?;
                    let local_commit = SyncLocalCommitPlan {
                        schema_version: SyncSchemaVersion::V1,
                        committed_generation: candidate_manifest.generation,
                        fixed_identity: None,
                        devices: candidate_manifest.devices.clone(),
                        merge_batch: merge_batch.clone(),
                    };
                    apply_committed_sync_batch(
                        self.local_applier,
                        self.rollback_store,
                        request.now_ms,
                        &local_commit,
                    )
                    .map_err(|_| {
                        SyncRunError::new(
                            SyncRunErrorCode::LocalApply,
                            "remote commit succeeded but local apply failed",
                        )
                    })?;
                    return Ok(SyncRunResult {
                        schema_version: SyncSchemaVersion::V1,
                        committed_generation: candidate_manifest.generation,
                        attempts: match attempt {
                            SyncCasAttemptKind::Initial => 1,
                            SyncCasAttemptKind::RemergeOnce => 2,
                        },
                        resolved_records: usize_to_u64(
                            merge_batch.resolved.len(),
                            "resolved record count overflow",
                        )?,
                        conflicts: usize_to_u64(
                            merge_batch.conflicts.len(),
                            "conflict count overflow",
                        )?,
                        committed_etag,
                    });
                }
                SyncCasDecision::Stop { reason, failure } => {
                    if failure.is_some() {
                        return Err(SyncRunError::new(
                            SyncRunErrorCode::Transport,
                            "remote manifest commit failed",
                        ));
                    }
                    return Err(SyncRunError::new(
                        SyncRunErrorCode::ConcurrentWrite,
                        "remote changed again after the single permitted remerge",
                    )
                    .with_context("reason", format!("{reason:?}")));
                }
            }
        }
    }

    pub async fn preview_first_sync(
        &self,
        passphrase: &[u8],
        request: SyncFirstSyncPreviewRequest,
    ) -> Result<SyncFirstSyncPreview, SyncRunError> {
        validate_first_sync_request(&request)?;
        let snapshot = self.read_optional_remote_snapshot(passphrase).await?;
        let input = first_sync_input(&request, snapshot.as_ref());
        preview_first_sync_domain(&input)
            .map_err(|_| invalid_input("first-sync preview input is invalid"))
    }

    pub async fn confirm_first_sync(
        &self,
        passphrase: &[u8],
        request: SyncFirstSyncConfirmRequest,
    ) -> Result<SyncRunResult, SyncRunError> {
        let preview_request = SyncFirstSyncPreviewRequest {
            schema_version: request.schema_version,
            candidate_device_id: request.candidate_device_id.clone(),
            display_name: request.display_name.clone(),
            observed_at_ms: request.observed_at_ms,
            baselines: request.baselines.clone(),
            local_records: request.local_records.clone(),
        };
        validate_first_sync_request(&preview_request)?;
        let snapshot = self.read_optional_remote_snapshot(passphrase).await?;
        let current = first_sync_input(&preview_request, snapshot.as_ref());
        let registration = confirm_first_sync_domain(SyncFirstSyncConfirmationInput {
            schema_version: SyncSchemaVersion::V1,
            expected_preview_token: request.expected_preview_token,
            current,
            existing_identity: request.existing_identity,
            confirmed_at_ms: request.confirmed_at_ms,
            consent: SyncFirstSyncConsent::RegisterDeviceAndApplyPreview,
        })
        .map_err(|_| invalid_input("first-sync preview is stale or invalid"))?;

        let remote_records = snapshot
            .as_ref()
            .map(|value| value.records.as_slice())
            .unwrap_or_default();
        let candidate_records = candidate_remote_records(remote_records, &registration.merge_batch);
        let generation = snapshot
            .as_ref()
            .map(|value| value.manifest.generation)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid_remote("remote manifest generation overflow"))?;
        let mut devices = snapshot
            .as_ref()
            .map(|value| value.manifest.devices.clone())
            .unwrap_or_default();
        let mut registered_device = registration.registered_device.clone();
        registered_device.acknowledged_generation = generation;
        registered_device.last_seen_at_ms = request.confirmed_at_ms;
        devices.push(registered_device);
        devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        let candidate_manifest = first_sync_manifest(
            generation,
            request.confirmed_at_ms,
            &registration.identity.device_id,
            candidate_records.as_slice(),
            devices,
        )?;

        let created_session = match snapshot.as_ref() {
            Some(_) => None,
            None => {
                let profile = self.crypto.create_profile().map_err(map_crypto_write)?;
                Some(
                    self.crypto
                        .unlock(passphrase, &profile)
                        .map_err(map_crypto_write)?,
                )
            }
        };
        let session: &dyn SyncCryptoSession = snapshot
            .as_ref()
            .map(|value| value.session.as_ref())
            .or(created_session.as_deref())
            .ok_or_else(|| {
                SyncRunError::new(SyncRunErrorCode::Crypto, "sync session is missing")
            })?;
        let previous_indexes = snapshot
            .as_ref()
            .map(|value| value.manifest.records.as_slice())
            .unwrap_or_default();
        self.write_new_record_objects(session, previous_indexes, &candidate_records)
            .await?;

        let write_condition = match registration.remote_guard {
            SyncFirstSyncRemoteGuard::CreateOnly => SyncWriteCondition::CreateOnly,
            SyncFirstSyncRemoteGuard::Match { etag, .. } => SyncWriteCondition::Match(etag),
        };
        let receipt = self
            .write_manifest(session, &candidate_manifest, &write_condition)
            .await?;

        self.conflicts
            .replace_from_merge(&registration.merge_batch)
            .map_err(|_| {
                SyncRunError::new(
                    SyncRunErrorCode::ConflictRouting,
                    "remote commit succeeded but conflicts could not be published",
                )
            })?;
        let local_commit = SyncLocalCommitPlan {
            schema_version: SyncSchemaVersion::V1,
            committed_generation: generation,
            fixed_identity: Some(registration.identity),
            devices: candidate_manifest.devices.clone(),
            merge_batch: registration.merge_batch.clone(),
        };
        apply_committed_sync_batch(
            self.local_applier,
            self.rollback_store,
            request.confirmed_at_ms,
            &local_commit,
        )
        .map_err(|_| {
            SyncRunError::new(
                SyncRunErrorCode::LocalApply,
                "remote commit succeeded but local apply failed",
            )
        })?;

        Ok(SyncRunResult {
            schema_version: SyncSchemaVersion::V1,
            committed_generation: generation,
            attempts: 1,
            resolved_records: usize_to_u64(
                registration.merge_batch.resolved.len(),
                "resolved record count overflow",
            )?,
            conflicts: usize_to_u64(
                registration.merge_batch.conflicts.len(),
                "conflict count overflow",
            )?,
            committed_etag: receipt.etag().cloned(),
        })
    }

    pub async fn list_devices(&self, passphrase: &[u8]) -> Result<Vec<SyncDevice>, SyncRunError> {
        self.read_remote_snapshot(passphrase)
            .await
            .map(|snapshot| snapshot.manifest.devices)
    }

    pub async fn retire_device(
        &self,
        passphrase: &[u8],
        request: SyncDeviceRetireRequest,
    ) -> Result<SyncRunResult, SyncRunError> {
        if request.retired_at_ms < 0 {
            return Err(invalid_input("device retirement time must not be negative"));
        }
        let snapshot = self.read_remote_snapshot(passphrase).await?;
        let retirement = plan_sync_device_retirement(SyncDeviceRetirementInput {
            schema_version: SyncSchemaVersion::V1,
            manifest: snapshot.manifest.clone(),
            writer_device_id: request.writer_device_id.clone(),
            target_device_id: request.target_device_id,
            retired_at_ms: request.retired_at_ms,
            consent: SyncDeviceRetirementConsent::AcceptDeviceReappearanceRisk {
                target_device_id: request.confirmed_target_device_id,
            },
        })
        .map_err(|_| invalid_input("device retirement request is invalid"))?;
        let generation = snapshot
            .manifest
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid_remote("remote manifest generation overflow"))?;
        let mut devices = retirement.devices;
        let writer = devices
            .iter_mut()
            .find(|device| device.device_id == retirement.writer_device_id)
            .ok_or_else(|| invalid_input("device retirement writer is not registered"))?;
        writer.acknowledged_generation = generation;
        writer.last_seen_at_ms = request.retired_at_ms;
        devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        let candidate_manifest = first_sync_manifest(
            generation,
            request.retired_at_ms,
            &retirement.writer_device_id,
            &snapshot.records,
            devices,
        )?;
        let receipt = self
            .write_manifest(
                snapshot.session.as_ref(),
                &candidate_manifest,
                &SyncWriteCondition::Match(snapshot.etag),
            )
            .await?;
        let local_commit = SyncLocalCommitPlan {
            schema_version: SyncSchemaVersion::V1,
            committed_generation: generation,
            fixed_identity: None,
            devices: candidate_manifest.devices.clone(),
            merge_batch: SyncMergeBatch {
                schema_version: SyncSchemaVersion::V1,
                resolved: Vec::new(),
                conflicts: Vec::new(),
            },
        };
        apply_committed_sync_batch(
            self.local_applier,
            self.rollback_store,
            request.retired_at_ms,
            &local_commit,
        )
        .map_err(|_| {
            SyncRunError::new(
                SyncRunErrorCode::LocalApply,
                "remote commit succeeded but local device registry update failed",
            )
        })?;
        Ok(SyncRunResult {
            schema_version: SyncSchemaVersion::V1,
            committed_generation: generation,
            attempts: 1,
            resolved_records: 0,
            conflicts: 0,
            committed_etag: receipt.etag().cloned(),
        })
    }

    async fn read_remote_snapshot(
        &self,
        passphrase: &[u8],
    ) -> Result<RemoteSnapshot, SyncRunError> {
        self.read_optional_remote_snapshot(passphrase)
            .await?
            .ok_or_else(|| {
                SyncRunError::new(SyncRunErrorCode::RemoteMissing, "sync-v3 remote is empty")
            })
    }

    async fn read_optional_remote_snapshot(
        &self,
        passphrase: &[u8],
    ) -> Result<Option<RemoteSnapshot>, SyncRunError> {
        let manifest_path =
            sync_manifest_remote_path().map_err(|_| invalid_remote("manifest path is invalid"))?;
        let Some(manifest_object) = self
            .transport
            .read(&manifest_path, MAX_SYNC_REMOTE_OBJECT_BYTES)
            .await
            .map_err(map_transport)?
        else {
            return Ok(None);
        };
        let etag = manifest_object.etag().cloned().ok_or_else(|| {
            invalid_remote("remote manifest does not provide the ETag required for CAS")
        })?;
        let manifest_envelope = SyncEncryptedEnvelope::from_json_bytes(manifest_object.bytes())
            .map_err(|_| invalid_remote("remote manifest envelope is invalid"))?;
        let manifest_identity =
            SyncObjectIdentity::manifest(manifest_envelope.identity().object_version())
                .map_err(|_| invalid_remote("remote manifest identity is invalid"))?;
        let session = self
            .crypto
            .unlock(passphrase, manifest_envelope.kdf())
            .map_err(map_crypto_read)?;
        let manifest_plaintext = session
            .open(&manifest_identity, &manifest_envelope)
            .map_err(map_crypto_read)?;
        let manifest = SyncV3Manifest::from_json_slice(manifest_plaintext.as_bytes())
            .map_err(|_| invalid_remote("remote manifest plaintext is invalid"))?;
        if manifest.generation != manifest_identity.object_version() {
            return Err(invalid_remote(
                "remote manifest generation does not match its encrypted identity",
            ));
        }

        let mut records = Vec::with_capacity(manifest.records.len());
        for index in &manifest.records {
            let path = sync_record_remote_path(index)
                .map_err(|_| invalid_remote("remote record path is invalid"))?;
            let object = self
                .transport
                .read(&path, MAX_SYNC_REMOTE_OBJECT_BYTES)
                .await
                .map_err(map_transport)?
                .ok_or_else(|| invalid_remote("remote manifest references a missing record"))?;
            records.push(open_record(session.as_ref(), index, &object)?);
        }

        Ok(Some(RemoteSnapshot {
            manifest,
            etag,
            records,
            session,
        }))
    }

    async fn write_new_record_objects(
        &self,
        session: &dyn SyncCryptoSession,
        previous_indexes: &[SyncRecordIndexEntry],
        records: &[SyncRecord],
    ) -> Result<(), SyncRunError> {
        let previous = previous_indexes
            .iter()
            .map(|entry| (&entry.id, entry))
            .collect::<BTreeMap<_, _>>();
        let mut pending = Vec::new();
        for record in records {
            let index = SyncRecordIndexEntry::from_record(record)
                .map_err(|_| invalid_input("candidate remote record is invalid"))?;
            if previous.get(&record.id).copied() == Some(&index) {
                continue;
            }
            pending.push((record, index));
        }
        if pending.is_empty() {
            return Ok(());
        }

        let directory = sync_records_directory()
            .map_err(|_| invalid_remote("record directory path is invalid"))?;
        self.transport
            .ensure_directories(&directory)
            .await
            .map_err(map_transport)?;

        for (record, index) in pending {
            let identity = SyncObjectIdentity::record(record.id.clone(), record.revision.counter)
                .map_err(|_| invalid_input("candidate record identity is invalid"))?;
            let plaintext = record
                .to_canonical_json_bytes()
                .map_err(|_| invalid_input("candidate remote record is invalid"))?;
            let envelope = session
                .seal(&identity, &plaintext)
                .map_err(map_crypto_write)?;
            let bytes = envelope
                .to_json_bytes()
                .map_err(|_| invalid_remote("candidate record envelope is invalid"))?;
            let path = sync_record_remote_path(&index)
                .map_err(|_| invalid_remote("remote record path is invalid"))?;
            match self
                .transport
                .conditional_write(&path, &bytes, &SyncWriteCondition::CreateOnly)
                .await
            {
                Ok(_) => {}
                Err(error) if error.code == SyncTransportErrorCode::PreconditionFailed => {
                    let existing = self
                        .transport
                        .read(&path, MAX_SYNC_REMOTE_OBJECT_BYTES)
                        .await
                        .map_err(map_transport)?
                        .ok_or_else(|| {
                            invalid_remote("immutable remote record disappeared after collision")
                        })?;
                    let existing_record = open_record(session, &index, &existing)?;
                    if existing_record != *record {
                        return Err(invalid_remote(
                            "immutable remote record path contains different content",
                        ));
                    }
                }
                Err(error) => return Err(map_transport(error)),
            }
        }
        Ok(())
    }

    async fn write_manifest(
        &self,
        session: &dyn SyncCryptoSession,
        manifest: &SyncV3Manifest,
        condition: &SyncWriteCondition,
    ) -> Result<crate::domain::SyncWriteReceipt, SyncRunError> {
        let identity = SyncObjectIdentity::manifest(manifest.generation)
            .map_err(|_| invalid_remote("candidate manifest identity is invalid"))?;
        let plaintext = manifest
            .to_canonical_json_bytes()
            .map_err(|_| invalid_input("candidate manifest is invalid"))?;
        let envelope = session
            .seal(&identity, &plaintext)
            .map_err(map_crypto_write)?;
        let bytes = envelope
            .to_json_bytes()
            .map_err(|_| invalid_remote("candidate manifest envelope is invalid"))?;
        let path =
            sync_manifest_remote_path().map_err(|_| invalid_remote("manifest path is invalid"))?;
        self.transport
            .conditional_write(&path, &bytes, condition)
            .await
            .map_err(|error| {
                if error.code == SyncTransportErrorCode::PreconditionFailed {
                    SyncRunError::new(
                        SyncRunErrorCode::ConcurrentWrite,
                        "first-sync preview is stale and must be regenerated",
                    )
                } else {
                    map_transport(error)
                }
            })
    }
}

struct RemoteSnapshot {
    manifest: SyncV3Manifest,
    etag: crate::domain::SyncEtag,
    records: Vec<SyncRecord>,
    session: Box<dyn SyncCryptoSession>,
}

fn open_record(
    session: &dyn SyncCryptoSession,
    index: &SyncRecordIndexEntry,
    object: &SyncRemoteObject,
) -> Result<SyncRecord, SyncRunError> {
    let envelope = SyncEncryptedEnvelope::from_json_bytes(object.bytes())
        .map_err(|_| invalid_remote("remote record envelope is invalid"))?;
    let identity = SyncObjectIdentity::record(index.id.clone(), index.revision.counter)
        .map_err(|_| invalid_remote("remote record identity is invalid"))?;
    let plaintext = session
        .open(&identity, &envelope)
        .map_err(map_crypto_read)?;
    let record = SyncRecord::from_json_slice(plaintext.as_bytes())
        .map_err(|_| invalid_remote("remote record plaintext is invalid"))?;
    let actual = SyncRecordIndexEntry::from_record(&record)
        .map_err(|_| invalid_remote("remote record is invalid"))?;
    if &actual != index {
        return Err(invalid_remote(
            "remote record does not match the manifest index",
        ));
    }
    Ok(record)
}

fn validate_request(request: &SyncRunRequest) -> Result<(), SyncRunError> {
    if request.now_ms < 0 {
        return Err(invalid_input("sync time must not be negative"));
    }
    let mut previous_baseline = None;
    for baseline in &request.baselines {
        baseline
            .validate()
            .map_err(|_| invalid_input("local sync baseline is invalid"))?;
        if previous_baseline.is_some_and(|id| id >= &baseline.record.id) {
            return Err(invalid_input(
                "local sync baselines must be unique and sorted",
            ));
        }
        previous_baseline = Some(&baseline.record.id);
    }
    let mut previous_record = None;
    for record in &request.local_records {
        record
            .validate()
            .map_err(|_| invalid_input("local portable record is invalid"))?;
        if previous_record.is_some_and(|id| id >= &record.id) {
            return Err(invalid_input(
                "local portable records must be unique and sorted",
            ));
        }
        previous_record = Some(&record.id);
    }
    Ok(())
}

fn validate_first_sync_request(request: &SyncFirstSyncPreviewRequest) -> Result<(), SyncRunError> {
    validate_request(&SyncRunRequest {
        schema_version: request.schema_version,
        device_id: request.candidate_device_id.clone(),
        now_ms: request.observed_at_ms,
        baselines: request.baselines.clone(),
        local_records: request.local_records.clone(),
    })?;
    if request.display_name.trim().is_empty() {
        return Err(invalid_input("first-sync device name must not be empty"));
    }
    Ok(())
}

fn first_sync_input(
    request: &SyncFirstSyncPreviewRequest,
    snapshot: Option<&RemoteSnapshot>,
) -> SyncFirstSyncInput {
    SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: request.candidate_device_id.clone(),
        display_name: request.display_name.clone(),
        observed_at_ms: request.observed_at_ms,
        remote_state: snapshot.map_or(SyncFirstSyncRemoteState::Empty, |value| {
            SyncFirstSyncRemoteState::Existing {
                manifest: value.manifest.clone(),
                etag: value.etag.clone(),
            }
        }),
        baselines: request.baselines.clone(),
        local_records: request.local_records.clone(),
        remote_records: snapshot
            .map(|value| value.records.clone())
            .unwrap_or_default(),
    }
}

fn first_sync_manifest(
    generation: u64,
    generated_at_ms: i64,
    writer_id: &SyncDeviceId,
    records: &[SyncRecord],
    devices: Vec<crate::domain::SyncDevice>,
) -> Result<SyncV3Manifest, SyncRunError> {
    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_input("candidate remote records are invalid"))?;
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    let manifest = SyncV3Manifest {
        protocol_version: SyncProtocolVersion::V3,
        schema_version: SyncSchemaVersion::V1,
        generation,
        generated_at_ms,
        generated_by_device_id: writer_id.clone(),
        records: indexes,
        devices,
    };
    manifest
        .validate()
        .map_err(|_| invalid_input("candidate first-sync manifest is invalid"))?;
    Ok(manifest)
}

fn candidate_remote_records(remote: &[SyncRecord], batch: &SyncMergeBatch) -> Vec<SyncRecord> {
    let mut candidate = remote
        .iter()
        .cloned()
        .map(|record| (record.id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    for resolution in &batch.resolved {
        if resolution.remote_action == SyncMergeSideAction::ApplyMerged {
            candidate.insert(resolution.record.id.clone(), resolution.record.clone());
        }
    }
    candidate.into_values().collect()
}

fn build_candidate_manifest(
    previous: &SyncV3Manifest,
    records: &[SyncRecord],
    writer_id: &SyncDeviceId,
    now_ms: i64,
) -> Result<SyncV3Manifest, SyncRunError> {
    if now_ms < previous.generated_at_ms {
        return Err(invalid_input(
            "sync time cannot precede the observed remote manifest",
        ));
    }
    let generation = previous
        .generation
        .checked_add(1)
        .ok_or_else(|| invalid_remote("remote manifest generation overflow"))?;
    let mut devices = previous.devices.clone();
    let writer = devices
        .iter_mut()
        .find(|device| &device.device_id == writer_id)
        .ok_or_else(|| invalid_input("current sync device is not registered"))?;
    if writer.status != SyncDeviceStatus::Active {
        return Err(invalid_input("a retired device cannot synchronize"));
    }
    if now_ms < writer.last_seen_at_ms {
        return Err(invalid_input(
            "sync time cannot precede the current device last-seen time",
        ));
    }
    writer.acknowledged_generation = generation;
    writer.last_seen_at_ms = now_ms;

    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_input("candidate remote records are invalid"))?;
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    let manifest = SyncV3Manifest {
        protocol_version: previous.protocol_version,
        schema_version: SyncSchemaVersion::V1,
        generation,
        generated_at_ms: now_ms,
        generated_by_device_id: writer_id.clone(),
        records: indexes,
        devices,
    };
    manifest
        .validate()
        .map_err(|_| invalid_input("candidate remote manifest is invalid"))?;
    Ok(manifest)
}

fn map_crypto_read(error: SyncCryptoError) -> SyncRunError {
    match error.code {
        SyncCryptoErrorCode::AuthenticationFailed => SyncRunError::new(
            SyncRunErrorCode::AuthenticationFailed,
            "sync passphrase is incorrect or remote ciphertext was modified",
        ),
        SyncCryptoErrorCode::IdentityMismatch | SyncCryptoErrorCode::ProfileMismatch => {
            invalid_remote("remote encryption metadata is inconsistent")
        }
        _ => SyncRunError::new(
            SyncRunErrorCode::Crypto,
            "remote sync object could not be decrypted",
        ),
    }
}

fn map_crypto_write(_error: SyncCryptoError) -> SyncRunError {
    SyncRunError::new(SyncRunErrorCode::Crypto, "sync object encryption failed")
}

fn map_transport(error: SyncTransportError) -> SyncRunError {
    SyncRunError::new(
        SyncRunErrorCode::Transport,
        "sync transport operation failed",
    )
    .with_context("transportCode", format!("{:?}", error.code))
}

fn map_cas_transport_failure(code: SyncTransportErrorCode) -> SyncCasFailureKind {
    match code {
        SyncTransportErrorCode::AuthenticationFailed => SyncCasFailureKind::Authentication,
        SyncTransportErrorCode::LimitExceeded => SyncCasFailureKind::LimitExceeded,
        SyncTransportErrorCode::Timeout => SyncCasFailureKind::Timeout,
        SyncTransportErrorCode::ConnectionFailed => SyncCasFailureKind::Connection,
        SyncTransportErrorCode::HttpStatus => SyncCasFailureKind::RemoteRejected,
        SyncTransportErrorCode::InvalidResponse | SyncTransportErrorCode::InvalidConfiguration => {
            SyncCasFailureKind::InvalidResponse
        }
        SyncTransportErrorCode::InvalidInput
        | SyncTransportErrorCode::PreconditionFailed
        | SyncTransportErrorCode::TransportFailed => SyncCasFailureKind::Transport,
    }
}

fn invalid_input(message: impl Into<String>) -> SyncRunError {
    SyncRunError::new(SyncRunErrorCode::InvalidInput, message)
}

fn invalid_remote(message: impl Into<String>) -> SyncRunError {
    SyncRunError::new(SyncRunErrorCode::InvalidRemote, message)
}

fn usize_to_u64(value: usize, message: &str) -> Result<u64, SyncRunError> {
    u64::try_from(value).map_err(|_| invalid_remote(message))
}
