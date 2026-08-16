use serde_json::json;
use wsl_code_switch_lib::domain::{
    FixedSyncDeviceIdentity, PortableDomain, PortablePayload, PortableRecordId, SyncDevice,
    SyncDeviceId, SyncDeviceStatus, SyncLocalCommitPlan, SyncMergeBatch, SyncMergeResolution,
    SyncMergeSideAction, SyncRecord, SyncSchemaVersion,
};
use wsl_code_switch_lib::Database;

const NOW: i64 = 1_800_000_000_000;

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn record() -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, "server-a").unwrap(),
        device_id("device-a"),
        1,
        NOW,
        PortablePayload::new(
            PortableDomain::Mcp,
            json!({"id":"server-a","name":"A","serverConfig":{"command":"safe"}}),
        )
        .unwrap(),
    )
    .unwrap()
}

fn plan(identity_name: &str) -> SyncLocalCommitPlan {
    let record = record();
    SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: 3,
        fixed_identity: Some(FixedSyncDeviceIdentity {
            schema_version: SyncSchemaVersion::V1,
            device_id: device_id("device-a"),
            display_name: identity_name.to_string(),
            fixed_at_ms: NOW,
        }),
        devices: vec![SyncDevice {
            schema_version: SyncSchemaVersion::V1,
            device_id: device_id("device-a"),
            display_name: identity_name.to_string(),
            acknowledged_generation: 3,
            registered_at_ms: NOW,
            last_seen_at_ms: NOW,
            status: SyncDeviceStatus::Active,
            retired_at_ms: None,
        }],
        merge_batch: SyncMergeBatch {
            schema_version: SyncSchemaVersion::V1,
            resolved: vec![SyncMergeResolution {
                schema_version: SyncSchemaVersion::V1,
                record,
                local_action: SyncMergeSideAction::ApplyMerged,
                remote_action: SyncMergeSideAction::Unchanged,
            }],
            conflicts: Vec::new(),
        },
    }
}

#[test]
fn committed_metadata_round_trips_identity_devices_and_generation_bound_baselines() {
    let database = Database::memory().unwrap();
    database.commit_sync_metadata(&plan("Workstation")).unwrap();

    let identity = database.load_sync_identity().unwrap().unwrap();
    assert_eq!(identity.display_name, "Workstation");
    let devices = database.load_sync_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].acknowledged_generation, 3);
    let baselines = database.load_sync_baselines().unwrap();
    assert_eq!(baselines.len(), 1);
    assert_eq!(baselines[0].confirmed_generation, 3);
    assert_eq!(baselines[0].record, record());
}

#[test]
fn fixed_identity_cannot_be_replaced_by_a_later_commit() {
    let database = Database::memory().unwrap();
    database.commit_sync_metadata(&plan("Workstation")).unwrap();

    let error = database
        .commit_sync_metadata(&plan("Replacement"))
        .unwrap_err();
    assert!(error.to_string().contains("identity"));
    assert_eq!(
        database.load_sync_identity().unwrap().unwrap().display_name,
        "Workstation"
    );
}
