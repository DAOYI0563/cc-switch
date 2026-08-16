use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    merge_sync_records, DomainError, DomainErrorCode, Sha256Digest, SyncDevice, SyncDeviceId,
    SyncDeviceStatus, SyncEtag, SyncMergeBatch, SyncMergeInput, SyncMergeSideAction, SyncRecord,
    SyncRecordBaseline, SyncRecordIndexEntry, SyncRecordState, SyncSchemaVersion, SyncV3Manifest,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncFirstSyncRemoteState {
    Empty,
    Existing {
        manifest: SyncV3Manifest,
        etag: SyncEtag,
    },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncInput {
    pub schema_version: SyncSchemaVersion,
    pub candidate_device_id: SyncDeviceId,
    pub display_name: String,
    pub observed_at_ms: i64,
    pub remote_state: SyncFirstSyncRemoteState,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
    pub remote_records: Vec<SyncRecord>,
}

impl fmt::Debug for SyncFirstSyncInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncFirstSyncInput")
            .field("schema_version", &self.schema_version)
            .field("candidate_device_id", &self.candidate_device_id)
            .field("display_name", &self.display_name)
            .field("observed_at_ms", &self.observed_at_ms)
            .field("remote_state", &self.remote_state)
            .field("baselines", &self.baselines.len())
            .field("local_records", &self.local_records.len())
            .field("remote_records", &self.remote_records.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncChangeCounts {
    pub additions: u64,
    pub modifications: u64,
    pub deletions: u64,
    pub conflicts: u64,
}

impl SyncFirstSyncChangeCounts {
    pub fn total(&self) -> u64 {
        self.additions
            .saturating_add(self.modifications)
            .saturating_add(self.deletions)
            .saturating_add(self.conflicts)
    }
}

/// Content-free preview safe for presentation before any local or remote write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncPreview {
    pub schema_version: SyncSchemaVersion,
    pub candidate_device_id: SyncDeviceId,
    pub display_name: String,
    pub observed_at_ms: i64,
    pub remote_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_etag: Option<SyncEtag>,
    pub remote_manifest_sha256: Sha256Digest,
    pub local_state_sha256: Sha256Digest,
    pub changes: SyncFirstSyncChangeCounts,
    pub preview_token: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncFirstSyncConsent {
    RegisterDeviceAndApplyPreview,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixedSyncDeviceIdentity {
    pub schema_version: SyncSchemaVersion,
    pub device_id: SyncDeviceId,
    pub display_name: String,
    pub fixed_at_ms: i64,
}

impl FixedSyncDeviceIdentity {
    pub fn validate(&self) -> Result<(), DomainError> {
        validate_device_name_and_time(&self.device_id, &self.display_name, self.fixed_at_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncConfirmationInput {
    pub schema_version: SyncSchemaVersion,
    pub expected_preview_token: Sha256Digest,
    pub current: SyncFirstSyncInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing_identity: Option<FixedSyncDeviceIdentity>,
    pub confirmed_at_ms: i64,
    pub consent: SyncFirstSyncConsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncFirstSyncRemoteGuard {
    CreateOnly,
    Match {
        generation: u64,
        manifest_sha256: Sha256Digest,
        etag: SyncEtag,
    },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncFirstSyncRegistrationPlan {
    pub schema_version: SyncSchemaVersion,
    pub identity: FixedSyncDeviceIdentity,
    pub registered_device: SyncDevice,
    pub remote_guard: SyncFirstSyncRemoteGuard,
    pub merge_batch: SyncMergeBatch,
}

impl fmt::Debug for SyncFirstSyncRegistrationPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncFirstSyncRegistrationPlan")
            .field("schema_version", &self.schema_version)
            .field("identity", &self.identity)
            .field("registered_device", &self.registered_device)
            .field("remote_guard", &self.remote_guard)
            .field("resolved_records", &self.merge_batch.resolved.len())
            .field("conflicts", &self.merge_batch.conflicts.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncDeviceRetirementConsent {
    AcceptDeviceReappearanceRisk { target_device_id: SyncDeviceId },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDeviceRetirementInput {
    pub schema_version: SyncSchemaVersion,
    pub manifest: SyncV3Manifest,
    pub writer_device_id: SyncDeviceId,
    pub target_device_id: SyncDeviceId,
    pub retired_at_ms: i64,
    pub consent: SyncDeviceRetirementConsent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDeviceRetirementPlan {
    pub schema_version: SyncSchemaVersion,
    pub expected_manifest_generation: u64,
    pub writer_device_id: SyncDeviceId,
    pub devices: Vec<SyncDevice>,
}

pub fn preview_first_sync(input: &SyncFirstSyncInput) -> Result<SyncFirstSyncPreview, DomainError> {
    let prepared = prepare_first_sync(input)?;
    Ok(prepared.preview)
}

pub fn confirm_first_sync(
    input: SyncFirstSyncConfirmationInput,
) -> Result<SyncFirstSyncRegistrationPlan, DomainError> {
    if let Some(identity) = &input.existing_identity {
        identity.validate()?;
        return Err(invalid_lifecycle(
            "a fixed sync device identity cannot be replaced by first-sync confirmation",
        ));
    }
    if input.confirmed_at_ms < input.current.observed_at_ms {
        return Err(invalid_lifecycle(
            "first-sync confirmation time cannot precede its preview",
        ));
    }

    let prepared = prepare_first_sync(&input.current)?;
    if prepared.preview.preview_token != input.expected_preview_token {
        return Err(invalid_lifecycle(
            "first-sync preview is stale and must be regenerated",
        ));
    }

    let identity = FixedSyncDeviceIdentity {
        schema_version: SyncSchemaVersion::V1,
        device_id: prepared.preview.candidate_device_id.clone(),
        display_name: prepared.preview.display_name.clone(),
        fixed_at_ms: input.confirmed_at_ms,
    };
    identity.validate()?;
    let registered_device = SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: identity.device_id.clone(),
        display_name: identity.display_name.clone(),
        acknowledged_generation: prepared.preview.remote_generation,
        registered_at_ms: input.confirmed_at_ms,
        last_seen_at_ms: input.confirmed_at_ms,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    };
    registered_device.validate()?;

    Ok(SyncFirstSyncRegistrationPlan {
        schema_version: SyncSchemaVersion::V1,
        identity,
        registered_device,
        remote_guard: prepared.remote_guard,
        merge_batch: prepared.merge_batch,
    })
}

pub fn plan_sync_device_retirement(
    input: SyncDeviceRetirementInput,
) -> Result<SyncDeviceRetirementPlan, DomainError> {
    input.manifest.validate()?;
    if input.retired_at_ms < input.manifest.generated_at_ms {
        return Err(invalid_lifecycle(
            "device retirement time cannot precede the observed manifest",
        ));
    }
    if input.writer_device_id == input.target_device_id {
        return Err(invalid_lifecycle(
            "the current manifest writer cannot retire itself",
        ));
    }
    let SyncDeviceRetirementConsent::AcceptDeviceReappearanceRisk { target_device_id } =
        &input.consent;
    if target_device_id != &input.target_device_id {
        return Err(invalid_lifecycle(
            "device retirement consent is not bound to the target device",
        ));
    }

    let writer = input
        .manifest
        .devices
        .iter()
        .find(|device| device.device_id == input.writer_device_id)
        .ok_or_else(|| invalid_lifecycle("device retirement writer is not registered"))?;
    if writer.status != SyncDeviceStatus::Active {
        return Err(invalid_lifecycle(
            "a retired device cannot plan another device retirement",
        ));
    }

    let mut devices = input.manifest.devices.clone();
    let target = devices
        .iter_mut()
        .find(|device| device.device_id == input.target_device_id)
        .ok_or_else(|| invalid_lifecycle("device retirement target is not registered"))?;
    if target.status != SyncDeviceStatus::Active {
        return Err(invalid_lifecycle(
            "device retirement target is already retired",
        ));
    }
    if input.retired_at_ms < target.last_seen_at_ms {
        return Err(invalid_lifecycle(
            "device retirement time cannot precede the target last-seen time",
        ));
    }
    target.status = SyncDeviceStatus::Retired;
    target.retired_at_ms = Some(input.retired_at_ms);
    target.validate()?;

    Ok(SyncDeviceRetirementPlan {
        schema_version: SyncSchemaVersion::V1,
        expected_manifest_generation: input.manifest.generation,
        writer_device_id: input.writer_device_id,
        devices,
    })
}

struct PreparedFirstSync {
    preview: SyncFirstSyncPreview,
    remote_guard: SyncFirstSyncRemoteGuard,
    merge_batch: SyncMergeBatch,
}

fn prepare_first_sync(input: &SyncFirstSyncInput) -> Result<PreparedFirstSync, DomainError> {
    if input.observed_at_ms < 0 {
        return Err(invalid_lifecycle(
            "first-sync observation time cannot be negative",
        ));
    }

    let (remote_generation, remote_etag, remote_manifest_sha256, remote_guard, devices) =
        match &input.remote_state {
            SyncFirstSyncRemoteState::Empty => {
                if !input.remote_records.is_empty() || !input.baselines.is_empty() {
                    return Err(invalid_lifecycle(
                        "an empty first-sync remote cannot contain records or baselines",
                    ));
                }
                (
                    0,
                    None,
                    Sha256Digest::of_bytes(b"sync-v3-empty-remote"),
                    SyncFirstSyncRemoteGuard::CreateOnly,
                    &[][..],
                )
            }
            SyncFirstSyncRemoteState::Existing { manifest, etag } => {
                manifest.validate()?;
                if input.observed_at_ms < manifest.generated_at_ms {
                    return Err(invalid_lifecycle(
                        "first-sync observation time cannot precede the remote manifest",
                    ));
                }
                validate_remote_records(manifest, &input.remote_records)?;
                let manifest_sha256 = Sha256Digest::of_bytes(&manifest.to_canonical_json_bytes()?);
                (
                    manifest.generation,
                    Some(etag.clone()),
                    manifest_sha256.clone(),
                    SyncFirstSyncRemoteGuard::Match {
                        generation: manifest.generation,
                        manifest_sha256,
                        etag: etag.clone(),
                    },
                    manifest.devices.as_slice(),
                )
            }
        };

    if devices
        .iter()
        .any(|device| device.device_id == input.candidate_device_id)
    {
        return Err(invalid_lifecycle(
            "first-sync candidate device ID is already registered",
        ));
    }
    validate_device_name_and_time(
        &input.candidate_device_id,
        &input.display_name,
        input.observed_at_ms,
    )?;
    validate_record_owners(input, devices)?;

    let merge_input = SyncMergeInput {
        schema_version: SyncSchemaVersion::V1,
        baselines: input.baselines.clone(),
        local_records: input.local_records.clone(),
        remote_records: input.remote_records.clone(),
    };
    let merge_batch = merge_sync_records(merge_input)?;
    let changes = count_changes(input, &merge_batch)?;
    let local_state_sha256 = local_state_digest(&input.baselines, &input.local_records)?;
    let preview_token = preview_digest(&PreviewBinding {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: &input.candidate_device_id,
        display_name: input.display_name.trim(),
        observed_at_ms: input.observed_at_ms,
        remote_generation,
        remote_etag: remote_etag.as_ref(),
        remote_manifest_sha256: &remote_manifest_sha256,
        local_state_sha256: &local_state_sha256,
        changes,
    })?;

    Ok(PreparedFirstSync {
        preview: SyncFirstSyncPreview {
            schema_version: SyncSchemaVersion::V1,
            candidate_device_id: input.candidate_device_id.clone(),
            display_name: input.display_name.trim().to_string(),
            observed_at_ms: input.observed_at_ms,
            remote_generation,
            remote_etag,
            remote_manifest_sha256,
            local_state_sha256,
            changes,
            preview_token,
        },
        remote_guard,
        merge_batch,
    })
}

fn validate_remote_records(
    manifest: &SyncV3Manifest,
    records: &[SyncRecord],
) -> Result<(), DomainError> {
    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    if indexes != manifest.records {
        return Err(invalid_lifecycle(
            "first-sync remote records do not match the observed manifest",
        ));
    }
    Ok(())
}

fn validate_record_owners(
    input: &SyncFirstSyncInput,
    devices: &[SyncDevice],
) -> Result<(), DomainError> {
    let owner_is_known = |owner: &SyncDeviceId| {
        owner == &input.candidate_device_id
            || devices.iter().any(|device| &device.device_id == owner)
    };
    for record in input
        .baselines
        .iter()
        .map(|baseline| &baseline.record)
        .chain(input.local_records.iter())
    {
        if !owner_is_known(&record.revision.device_id) {
            return Err(invalid_lifecycle(
                "first-sync local record revision owner is not registered or the candidate device",
            ));
        }
    }
    Ok(())
}

fn count_changes(
    input: &SyncFirstSyncInput,
    batch: &SyncMergeBatch,
) -> Result<SyncFirstSyncChangeCounts, DomainError> {
    let local = input
        .local_records
        .iter()
        .map(|record| (record.id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let remote = input
        .remote_records
        .iter()
        .map(|record| (record.id.clone(), record))
        .collect::<BTreeMap<_, _>>();
    let mut counts = SyncFirstSyncChangeCounts {
        conflicts: u64::try_from(batch.conflicts.len())
            .map_err(|_| invalid_lifecycle("first-sync conflict count overflow"))?,
        ..SyncFirstSyncChangeCounts::default()
    };

    for resolution in &batch.resolved {
        if resolution.local_action == SyncMergeSideAction::ApplyMerged {
            classify_change(
                local.get(&resolution.record.id).copied(),
                &resolution.record,
                &mut counts,
            )?;
        }
        if resolution.remote_action == SyncMergeSideAction::ApplyMerged {
            classify_change(
                remote.get(&resolution.record.id).copied(),
                &resolution.record,
                &mut counts,
            )?;
        }
    }
    Ok(counts)
}

fn classify_change(
    current: Option<&SyncRecord>,
    merged: &SyncRecord,
    counts: &mut SyncFirstSyncChangeCounts,
) -> Result<(), DomainError> {
    let field = match (current.map(SyncRecord::state), merged.state()) {
        (None, SyncRecordState::Live) | (Some(SyncRecordState::Deleted), SyncRecordState::Live) => {
            &mut counts.additions
        }
        (None, SyncRecordState::Deleted)
        | (Some(SyncRecordState::Live), SyncRecordState::Deleted) => &mut counts.deletions,
        (Some(_), _) => &mut counts.modifications,
    };
    *field = field
        .checked_add(1)
        .ok_or_else(|| invalid_lifecycle("first-sync change count overflow"))?;
    Ok(())
}

fn validate_device_name_and_time(
    device_id: &SyncDeviceId,
    display_name: &str,
    timestamp_ms: i64,
) -> Result<(), DomainError> {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id.clone(),
        display_name: display_name.to_string(),
        acknowledged_generation: 0,
        registered_at_ms: timestamp_ms,
        last_seen_at_ms: timestamp_ms,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
    .validate()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalStateBinding<'a> {
    schema_version: SyncSchemaVersion,
    baselines: &'a [SyncRecordBaseline],
    local_records: &'a [SyncRecord],
}

fn local_state_digest(
    baselines: &[SyncRecordBaseline],
    local_records: &[SyncRecord],
) -> Result<Sha256Digest, DomainError> {
    preview_digest(&LocalStateBinding {
        schema_version: SyncSchemaVersion::V1,
        baselines,
        local_records,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBinding<'a> {
    schema_version: SyncSchemaVersion,
    candidate_device_id: &'a SyncDeviceId,
    display_name: &'a str,
    observed_at_ms: i64,
    remote_generation: u64,
    remote_etag: Option<&'a SyncEtag>,
    remote_manifest_sha256: &'a Sha256Digest,
    local_state_sha256: &'a Sha256Digest,
    changes: SyncFirstSyncChangeCounts,
}

fn preview_digest<T: Serialize>(value: &T) -> Result<Sha256Digest, DomainError> {
    serde_json::to_vec(value)
        .map(|bytes| Sha256Digest::of_bytes(&bytes))
        .map_err(|_| invalid_lifecycle("failed to encode first-sync preview binding"))
}

fn invalid_lifecycle(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
