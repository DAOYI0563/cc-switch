use rusqlite::{params, OptionalExtension, Transaction};

use crate::database::{lock_conn, Database};
use crate::domain::{
    FixedSyncDeviceIdentity, PortableDomain, SyncDevice, SyncDeviceId, SyncDeviceStatus,
    SyncLocalCommitPlan, SyncRecordBaseline, SyncRecordState, SyncSchemaVersion,
};
use crate::error::AppError;

const SYNC_IDENTITY_KEY: &str = "sync_v3_device_identity";

impl Database {
    pub fn load_sync_identity(&self) -> Result<Option<FixedSyncDeviceIdentity>, AppError> {
        let conn = lock_conn!(self.conn);
        let encoded = conn
            .query_row(
                "SELECT value_json FROM core_settings
                 WHERE key = ?1 AND storage_scope = 'device'",
                [SYNC_IDENTITY_KEY],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?;
        encoded
            .map(|value| {
                let identity: FixedSyncDeviceIdentity =
                    serde_json::from_str(&value).map_err(invalid_json)?;
                identity
                    .validate()
                    .map_err(|error| invalid_data(error.to_string()))?;
                Ok(identity)
            })
            .transpose()
    }

    pub fn load_sync_devices(&self) -> Result<Vec<SyncDevice>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT device_id, device_name, last_confirmed_generation,
                        registered_at_ms, last_seen_at_ms, retired_at_ms
                 FROM core_sync_devices ORDER BY device_id",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                let retired_at_ms: Option<i64> = row.get(5)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    retired_at_ms,
                ))
            })
            .map_err(database_error)?;
        let mut devices = Vec::new();
        for row in rows {
            let (id, name, generation, registered_at_ms, last_seen_at_ms, retired_at_ms) =
                row.map_err(database_error)?;
            let device = SyncDevice {
                schema_version: SyncSchemaVersion::V1,
                device_id: SyncDeviceId::new(id)
                    .map_err(|error| invalid_data(error.to_string()))?,
                display_name: name,
                acknowledged_generation: sqlite_u64(generation, "device generation")?,
                registered_at_ms,
                last_seen_at_ms,
                status: if retired_at_ms.is_some() {
                    SyncDeviceStatus::Retired
                } else {
                    SyncDeviceStatus::Active
                },
                retired_at_ms,
            };
            device
                .validate()
                .map_err(|error| invalid_data(error.to_string()))?;
            devices.push(device);
        }
        Ok(devices)
    }

    pub fn load_sync_baselines(&self) -> Result<Vec<SyncRecordBaseline>, AppError> {
        let conn = lock_conn!(self.conn);
        let mut statement = conn
            .prepare(
                "SELECT domain, record_key, record_version, device_id, content_hash,
                        baseline_json, tombstone, deleted_at_ms, updated_at_ms,
                        last_sync_generation
                 FROM core_sync_records
                 WHERE baseline_json IS NOT NULL
                 ORDER BY domain, record_key",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .map_err(database_error)?;
        let mut baselines = Vec::new();
        for row in rows {
            let (
                domain,
                record_key,
                version,
                device_id,
                content_hash,
                baseline_json,
                tombstone,
                deleted_at_ms,
                updated_at_ms,
                generation,
            ) = row.map_err(database_error)?;
            let baseline: SyncRecordBaseline =
                serde_json::from_str(&baseline_json).map_err(invalid_json)?;
            baseline
                .validate()
                .map_err(|error| invalid_data(error.to_string()))?;
            validate_baseline_columns(
                &baseline,
                &domain,
                &record_key,
                version,
                &device_id,
                content_hash.as_deref(),
                tombstone,
                deleted_at_ms,
                updated_at_ms,
                generation,
            )?;
            baselines.push(baseline);
        }
        Ok(baselines)
    }

    pub fn commit_sync_metadata(&self, plan: &SyncLocalCommitPlan) -> Result<(), AppError> {
        plan.validate()
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let identity_json = plan
            .fixed_identity
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(invalid_json)?;
        let baselines = plan
            .merge_batch
            .resolved
            .iter()
            .map(|resolution| {
                let baseline = SyncRecordBaseline {
                    schema_version: SyncSchemaVersion::V1,
                    confirmed_generation: plan.committed_generation,
                    record: resolution.record.clone(),
                };
                baseline
                    .validate()
                    .map_err(|error| AppError::InvalidInput(error.to_string()))?;
                let encoded = serde_json::to_string(&baseline).map_err(invalid_json)?;
                Ok((baseline, encoded))
            })
            .collect::<Result<Vec<_>, AppError>>()?;

        let mut conn = lock_conn!(self.conn);
        let transaction = conn.transaction().map_err(database_error)?;
        commit_identity(
            &transaction,
            plan.fixed_identity.as_ref(),
            identity_json.as_deref(),
        )?;
        transaction
            .execute("DELETE FROM core_sync_devices", [])
            .map_err(database_error)?;
        for device in &plan.devices {
            transaction
                .execute(
                    "INSERT INTO core_sync_devices (
                        device_id, device_name, last_confirmed_generation,
                        registered_at_ms, last_seen_at_ms, retired_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        device.device_id.as_str(),
                        device.display_name,
                        checked_i64(device.acknowledged_generation, "device generation")?,
                        device.registered_at_ms,
                        device.last_seen_at_ms,
                        device.retired_at_ms,
                    ],
                )
                .map_err(database_error)?;
        }
        for (baseline, encoded) in baselines {
            let record = &baseline.record;
            let deleted_at_ms = record
                .tombstone
                .as_ref()
                .map(|tombstone| tombstone.deleted_at_ms);
            transaction
                .execute(
                    "INSERT INTO core_sync_records (
                        domain, record_key, record_version, device_id, content_hash,
                        baseline_json, tombstone, deleted_at_ms, updated_at_ms,
                        last_sync_generation
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                     ON CONFLICT(domain, record_key) DO UPDATE SET
                        record_version = excluded.record_version,
                        device_id = excluded.device_id,
                        content_hash = excluded.content_hash,
                        baseline_json = excluded.baseline_json,
                        tombstone = excluded.tombstone,
                        deleted_at_ms = excluded.deleted_at_ms,
                        updated_at_ms = excluded.updated_at_ms,
                        last_sync_generation = excluded.last_sync_generation",
                    params![
                        record.id.domain.as_str(),
                        record.id.key,
                        checked_i64(record.revision.counter, "record version")?,
                        record.revision.device_id.as_str(),
                        record.revision.content_hash.as_str(),
                        encoded,
                        record.state() == SyncRecordState::Deleted,
                        deleted_at_ms,
                        record.revision.updated_at_ms,
                        checked_i64(baseline.confirmed_generation, "sync generation")?,
                    ],
                )
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)
    }
}

fn commit_identity(
    transaction: &Transaction<'_>,
    identity: Option<&FixedSyncDeviceIdentity>,
    encoded: Option<&str>,
) -> Result<(), AppError> {
    let Some(identity) = identity else {
        return Ok(());
    };
    let existing = transaction
        .query_row(
            "SELECT value_json FROM core_settings WHERE key = ?1",
            [SYNC_IDENTITY_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(existing) = existing {
        let existing: FixedSyncDeviceIdentity =
            serde_json::from_str(&existing).map_err(invalid_json)?;
        if existing != *identity {
            return Err(AppError::InvalidInput(
                "fixed sync device identity cannot be replaced".to_string(),
            ));
        }
        return Ok(());
    }
    transaction
        .execute(
            "INSERT INTO core_settings (key, value_json, storage_scope, updated_at_ms)
             VALUES (?1, ?2, 'device', ?3)",
            params![
                SYNC_IDENTITY_KEY,
                encoded.ok_or_else(|| invalid_data("sync identity encoding is missing"))?,
                identity.fixed_at_ms,
            ],
        )
        .map(|_| ())
        .map_err(database_error)
}

#[allow(clippy::too_many_arguments)]
fn validate_baseline_columns(
    baseline: &SyncRecordBaseline,
    domain: &str,
    record_key: &str,
    version: i64,
    device_id: &str,
    content_hash: Option<&str>,
    tombstone: bool,
    deleted_at_ms: Option<i64>,
    updated_at_ms: i64,
    generation: i64,
) -> Result<(), AppError> {
    let record = &baseline.record;
    let expected_domain = parse_domain(domain)?;
    let columns_match = record.id.domain == expected_domain
        && record.id.key == record_key
        && record.revision.counter == sqlite_u64(version, "record version")?
        && record.revision.device_id.as_str() == device_id
        && content_hash == Some(record.revision.content_hash.as_str())
        && (record.state() == SyncRecordState::Deleted) == tombstone
        && record.tombstone.as_ref().map(|value| value.deleted_at_ms) == deleted_at_ms
        && record.revision.updated_at_ms == updated_at_ms
        && baseline.confirmed_generation == sqlite_u64(generation, "sync generation")?;
    if columns_match {
        Ok(())
    } else {
        Err(invalid_data(
            "sync baseline JSON does not match its indexed columns",
        ))
    }
}

fn parse_domain(value: &str) -> Result<PortableDomain, AppError> {
    match value {
        "provider" => Ok(PortableDomain::Provider),
        "mcp" => Ok(PortableDomain::Mcp),
        "prompt" => Ok(PortableDomain::Prompt),
        "skill" => Ok(PortableDomain::Skill),
        "common_snippet" => Ok(PortableDomain::CommonSnippet),
        "daily_brief" => Ok(PortableDomain::DailyBrief),
        "portable_setting" => Ok(PortableDomain::PortableSetting),
        _ => Err(invalid_data("sync record contains an unknown domain")),
    }
}

fn checked_i64(value: u64, label: &str) -> Result<i64, AppError> {
    i64::try_from(value).map_err(|_| invalid_data(format!("{label} exceeds SQLite range")))
}

fn sqlite_u64(value: i64, label: &str) -> Result<u64, AppError> {
    u64::try_from(value).map_err(|_| invalid_data(format!("{label} is negative")))
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

fn invalid_json(error: impl std::fmt::Display) -> AppError {
    invalid_data(format!("invalid sync metadata JSON: {error}"))
}

fn invalid_data(message: impl Into<String>) -> AppError {
    AppError::Database(message.into())
}
