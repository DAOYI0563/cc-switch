use serde::{Deserialize, Serialize};

use super::{
    DomainError, DomainErrorCode, FixedSyncDeviceIdentity, SyncDevice, SyncDeviceId,
    SyncDeviceStatus, SyncMergeBatch, SyncRecord, SyncRecordBaseline, SyncSchemaVersion,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncLocalSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<FixedSyncDeviceIdentity>,
    pub baselines: Vec<SyncRecordBaseline>,
    pub local_records: Vec<SyncRecord>,
}

impl SyncLocalSnapshot {
    pub fn validate_for(&self, device_id: &SyncDeviceId) -> Result<(), DomainError> {
        if let Some(identity) = &self.identity {
            identity.validate()?;
            if &identity.device_id != device_id {
                return Err(invalid_local_commit(
                    "local snapshot device does not match the fixed sync identity",
                ));
            }
        }
        validate_sorted_baselines(&self.baselines)?;
        validate_sorted_records(&self.local_records)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncLocalCommitPlan {
    pub schema_version: SyncSchemaVersion,
    pub committed_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_identity: Option<FixedSyncDeviceIdentity>,
    pub devices: Vec<SyncDevice>,
    pub merge_batch: SyncMergeBatch,
}

impl SyncLocalCommitPlan {
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.committed_generation == 0 {
            return Err(invalid_local_commit(
                "sync committed generation must be greater than zero",
            ));
        }
        let mut previous_device = None;
        for device in &self.devices {
            device.validate()?;
            if device.acknowledged_generation > self.committed_generation {
                return Err(invalid_local_commit(
                    "sync device cannot acknowledge a future committed generation",
                ));
            }
            if previous_device.is_some_and(|id| id >= &device.device_id) {
                return Err(invalid_local_commit(
                    "sync local device registry must be unique and sorted",
                ));
            }
            previous_device = Some(&device.device_id);
        }
        if let Some(identity) = &self.fixed_identity {
            identity.validate()?;
            let registered = self
                .devices
                .iter()
                .find(|device| device.device_id == identity.device_id)
                .ok_or_else(|| {
                    invalid_local_commit("fixed sync identity must be registered remotely")
                })?;
            if registered.status != SyncDeviceStatus::Active {
                return Err(invalid_local_commit(
                    "fixed sync identity cannot reference a retired device",
                ));
            }
        }
        for resolution in &self.merge_batch.resolved {
            resolution.record.validate()?;
        }
        Ok(())
    }

    pub fn requires_local_write(&self) -> bool {
        self.fixed_identity.is_some()
            || !self.devices.is_empty()
            || !self.merge_batch.resolved.is_empty()
    }
}

fn invalid_local_commit(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorCode::InvalidRecord, message)
}

fn validate_sorted_baselines(baselines: &[SyncRecordBaseline]) -> Result<(), DomainError> {
    let mut previous = None;
    for baseline in baselines {
        baseline.validate()?;
        if previous.is_some_and(|id| id >= &baseline.record.id) {
            return Err(invalid_local_commit(
                "local sync baselines must be unique and sorted",
            ));
        }
        previous = Some(&baseline.record.id);
    }
    Ok(())
}

fn validate_sorted_records(records: &[SyncRecord]) -> Result<(), DomainError> {
    let mut previous = None;
    for record in records {
        record.validate()?;
        if previous.is_some_and(|id| id >= &record.id) {
            return Err(invalid_local_commit(
                "local sync records must be unique and sorted",
            ));
        }
        previous = Some(&record.id);
    }
    Ok(())
}
