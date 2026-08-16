use serde_json::json;
use wsl_code_switch_lib::domain::{
    plan_tombstone_compaction, DomainErrorCode, PortableDomain, PortablePayload, PortableRecordId,
    SyncDevice, SyncDeviceId, SyncDeviceStatus, SyncProtocolVersion, SyncRecord,
    SyncRecordIndexEntry, SyncSchemaVersion, SyncTombstoneCompactionConsent,
    SyncTombstoneCompactionInput, SyncV3Manifest,
};

const NOW: i64 = 1_750_000_000_000;

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn record_id(value: &str) -> PortableRecordId {
    PortableRecordId::new(PortableDomain::Provider, value).unwrap()
}

fn live(value: &str) -> SyncRecord {
    SyncRecord::live(
        record_id(value),
        device_id("device-a"),
        1,
        NOW - 20,
        PortablePayload::new(
            PortableDomain::Provider,
            json!({"name": value, "portableConfig": {"baseUrl": "https://example.com"}}),
        )
        .unwrap(),
    )
    .unwrap()
}

fn tombstone(value: &str, introduced_generation: u64) -> SyncRecord {
    SyncRecord::deleted(
        record_id(value),
        device_id("device-a"),
        2,
        NOW - 10,
        introduced_generation,
    )
    .unwrap()
}

fn device(id: &str, acknowledged_generation: u64, status: SyncDeviceStatus) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(id),
        display_name: id.to_string(),
        acknowledged_generation,
        registered_at_ms: NOW - 100,
        last_seen_at_ms: NOW - 2,
        status,
        retired_at_ms: (status == SyncDeviceStatus::Retired).then_some(NOW - 1),
    }
}

fn manifest(devices: Vec<SyncDevice>, records: &[SyncRecord]) -> SyncV3Manifest {
    let mut devices = devices;
    devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    SyncV3Manifest {
        protocol_version: SyncProtocolVersion::V3,
        schema_version: SyncSchemaVersion::V1,
        generation: 9,
        generated_at_ms: NOW,
        generated_by_device_id: device_id("device-a"),
        records: indexes,
        devices,
    }
}

fn input(
    devices: Vec<SyncDevice>,
    records: Vec<SyncRecord>,
    selected: Vec<PortableRecordId>,
) -> SyncTombstoneCompactionInput {
    SyncTombstoneCompactionInput {
        schema_version: SyncSchemaVersion::V1,
        manifest: manifest(devices, &records),
        records,
        consent: SyncTombstoneCompactionConsent::CompactSelectedTombstones {
            record_ids: selected,
        },
    }
}

#[test]
fn any_active_device_behind_a_tombstone_blocks_the_entire_plan() {
    let deleted = tombstone("deleted-a", 7);
    let error = plan_tombstone_compaction(input(
        vec![
            device("device-a", 9, SyncDeviceStatus::Active),
            device("device-b", 6, SyncDeviceStatus::Active),
            device("device-c", 1, SyncDeviceStatus::Retired),
        ],
        vec![deleted.clone()],
        vec![deleted.id],
    ))
    .unwrap_err();

    assert_eq!(error.code, DomainErrorCode::InvalidRecord);
}

#[test]
fn retired_devices_do_not_block_compaction_after_active_devices_catch_up() {
    let retained = live("live-a");
    let deleted = tombstone("deleted-a", 7);
    let plan = plan_tombstone_compaction(input(
        vec![
            device("device-a", 9, SyncDeviceStatus::Active),
            device("device-b", 8, SyncDeviceStatus::Active),
            device("device-c", 1, SyncDeviceStatus::Retired),
        ],
        vec![retained.clone(), deleted.clone()],
        vec![deleted.id.clone()],
    ))
    .unwrap();

    assert_eq!(plan.expected_manifest_generation, 9);
    assert_eq!(plan.next_manifest_generation, 10);
    assert_eq!(plan.writer_device_id, device_id("device-a"));
    assert_eq!(plan.compacted_record_ids, vec![deleted.id]);
    assert_eq!(plan.remaining_records.len(), 1);
    assert_eq!(plan.remaining_records[0].id, retained.id);
    assert_eq!(plan.active_devices_checked, 2);
    assert_eq!(plan.retired_devices_excluded, 1);
}

#[test]
fn compaction_requires_an_explicit_sorted_unique_tombstone_selection() {
    let live_record = live("live-a");
    let deleted_a = tombstone("deleted-a", 7);
    let deleted_b = tombstone("deleted-b", 7);
    let devices = vec![device("device-a", 9, SyncDeviceStatus::Active)];
    let records = vec![live_record.clone(), deleted_a.clone(), deleted_b.clone()];

    for selected in [
        Vec::new(),
        vec![live_record.id],
        vec![deleted_b.id.clone(), deleted_a.id.clone()],
        vec![deleted_a.id.clone(), deleted_a.id],
    ] {
        let error = plan_tombstone_compaction(input(devices.clone(), records.clone(), selected))
            .unwrap_err();
        assert_eq!(error.code, DomainErrorCode::InvalidRecord);
    }
}

#[test]
fn manifest_record_mismatch_and_future_tombstones_fail_closed() {
    let deleted = tombstone("deleted-a", 7);
    let devices = vec![device("device-a", 9, SyncDeviceStatus::Active)];
    let mut mismatched = input(
        devices.clone(),
        vec![deleted.clone()],
        vec![deleted.id.clone()],
    );
    mismatched.records.push(live("unindexed"));
    assert_eq!(
        plan_tombstone_compaction(mismatched).unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let future = tombstone("future", 10);
    assert_eq!(
        plan_tombstone_compaction(input(devices, vec![future.clone()], vec![future.id],))
            .unwrap_err()
            .code,
        DomainErrorCode::InvalidRecord
    );
}
