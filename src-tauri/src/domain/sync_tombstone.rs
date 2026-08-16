use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    DomainError, DomainErrorCode, PortableRecordId, SyncDeviceId, SyncDeviceStatus, SyncRecord,
    SyncRecordIndexEntry, SyncSchemaVersion, SyncV3Manifest,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SyncTombstoneCompactionConsent {
    CompactSelectedTombstones { record_ids: Vec<PortableRecordId> },
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncTombstoneCompactionInput {
    pub schema_version: SyncSchemaVersion,
    pub manifest: SyncV3Manifest,
    pub records: Vec<SyncRecord>,
    pub consent: SyncTombstoneCompactionConsent,
}

impl fmt::Debug for SyncTombstoneCompactionInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncTombstoneCompactionInput")
            .field("schema_version", &self.schema_version)
            .field("manifest_generation", &self.manifest.generation)
            .field("records", &self.records.len())
            .field("consent", &self.consent)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncTombstoneCompactionPlan {
    pub schema_version: SyncSchemaVersion,
    pub expected_manifest_generation: u64,
    pub next_manifest_generation: u64,
    pub writer_device_id: SyncDeviceId,
    pub compacted_record_ids: Vec<PortableRecordId>,
    pub remaining_records: Vec<SyncRecordIndexEntry>,
    pub active_devices_checked: u64,
    pub retired_devices_excluded: u64,
}

/// Produces an all-or-nothing plan. Applying remote deletes and the next
/// manifest is deliberately left to the sync application layer.
pub fn plan_tombstone_compaction(
    input: SyncTombstoneCompactionInput,
) -> Result<SyncTombstoneCompactionPlan, DomainError> {
    input.manifest.validate()?;
    if input.schema_version != input.manifest.schema_version {
        return Err(invalid_compaction(
            "compaction input and manifest schema versions differ",
        ));
    }

    let records = validate_complete_snapshot(&input.manifest, &input.records)?;
    let SyncTombstoneCompactionConsent::CompactSelectedTombstones { record_ids } = input.consent;
    validate_selection(&record_ids)?;

    for record_id in &record_ids {
        let record = records
            .get(record_id)
            .ok_or_else(|| invalid_compaction("selected compaction record is missing"))?;
        let tombstone = record
            .tombstone
            .as_ref()
            .ok_or_else(|| invalid_compaction("only tombstones can be compacted"))?;

        if input
            .manifest
            .devices
            .iter()
            .filter(|device| device.status == SyncDeviceStatus::Active)
            .any(|device| device.acknowledged_generation < tombstone.introduced_generation)
        {
            return Err(invalid_compaction(
                "an active device has not acknowledged the tombstone generation",
            ));
        }
    }

    let selected = record_ids.iter().cloned().collect::<BTreeSet<_>>();
    let remaining_records = input
        .manifest
        .records
        .iter()
        .filter(|entry| !selected.contains(&entry.id))
        .cloned()
        .collect();
    let next_manifest_generation = input
        .manifest
        .generation
        .checked_add(1)
        .ok_or_else(|| invalid_compaction("manifest generation overflow"))?;
    let active_devices_checked = u64::try_from(
        input
            .manifest
            .devices
            .iter()
            .filter(|device| device.status == SyncDeviceStatus::Active)
            .count(),
    )
    .map_err(|_| invalid_compaction("active device count overflow"))?;
    let retired_devices_excluded = u64::try_from(
        input
            .manifest
            .devices
            .iter()
            .filter(|device| device.status == SyncDeviceStatus::Retired)
            .count(),
    )
    .map_err(|_| invalid_compaction("retired device count overflow"))?;

    Ok(SyncTombstoneCompactionPlan {
        schema_version: SyncSchemaVersion::V1,
        expected_manifest_generation: input.manifest.generation,
        next_manifest_generation,
        writer_device_id: input.manifest.generated_by_device_id,
        compacted_record_ids: record_ids,
        remaining_records,
        active_devices_checked,
        retired_devices_excluded,
    })
}

fn validate_complete_snapshot<'a>(
    manifest: &SyncV3Manifest,
    records: &'a [SyncRecord],
) -> Result<BTreeMap<PortableRecordId, &'a SyncRecord>, DomainError> {
    let mut records_by_id = BTreeMap::new();
    let mut indexes = Vec::with_capacity(records.len());
    for record in records {
        record.validate()?;
        if record
            .tombstone
            .as_ref()
            .is_some_and(|tombstone| tombstone.introduced_generation > manifest.generation)
        {
            return Err(invalid_compaction(
                "tombstone cannot originate from a future manifest generation",
            ));
        }
        indexes.push(SyncRecordIndexEntry::from_record(record)?);
        if records_by_id.insert(record.id.clone(), record).is_some() {
            return Err(invalid_compaction(
                "compaction snapshot contains duplicate record IDs",
            ));
        }
    }
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    if indexes != manifest.records {
        return Err(invalid_compaction(
            "compaction snapshot does not match the manifest",
        ));
    }
    Ok(records_by_id)
}

fn validate_selection(record_ids: &[PortableRecordId]) -> Result<(), DomainError> {
    if record_ids.is_empty() {
        return Err(invalid_compaction(
            "tombstone compaction requires an explicit selection",
        ));
    }
    if record_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_compaction(
            "selected tombstones must be strictly sorted without duplicates",
        ));
    }
    Ok(())
}

fn invalid_compaction(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}
