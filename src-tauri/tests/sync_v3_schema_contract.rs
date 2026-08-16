use std::collections::BTreeMap;

use serde_json::{json, Value};
use wsl_code_switch_lib::domain::{
    DomainErrorCode, PermanentTombstone, PortableDomain, PortablePayload, PortableRecordId,
    RecordRevision, Sha256Digest, SyncDevice, SyncDeviceId, SyncDeviceStatus, SyncProtocolVersion,
    SyncRecord, SyncRecordBaseline, SyncRecordIndexEntry, SyncRecordState, SyncSchemaVersion,
    SyncV3Manifest, TombstoneRetention,
};

const NOW: i64 = 1_800_000_000_000;

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn payload(domain: PortableDomain, pairs: &[(&str, Value)]) -> PortablePayload {
    PortablePayload::new(
        domain,
        Value::Object(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        ),
    )
    .unwrap()
}

fn live_record(domain: PortableDomain, key: &str, owner: &str) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(domain, key).unwrap(),
        device_id(owner),
        1,
        NOW,
        payload(domain, &[("name", json!(key))]),
    )
    .unwrap()
}

fn device(id: &str, acknowledged_generation: u64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(id),
        display_name: format!("Device {id}"),
        acknowledged_generation,
        registered_at_ms: NOW - 2_000,
        last_seen_at_ms: NOW,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

fn manifest(records: &[SyncRecord]) -> SyncV3Manifest {
    let mut entries = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by(|left, right| left.id.cmp(&right.id));

    SyncV3Manifest {
        protocol_version: SyncProtocolVersion::V3,
        schema_version: SyncSchemaVersion::V1,
        generation: 7,
        generated_at_ms: NOW,
        generated_by_device_id: device_id("device-a"),
        records: entries,
        devices: vec![device("device-a", 7), device("device-b", 6)],
    }
}

#[test]
fn manifest_record_device_and_tombstone_contracts_are_versioned_and_canonical() {
    let live = live_record(PortableDomain::Mcp, "mcp-a", "device-a");
    let deleted = SyncRecord::deleted(
        PortableRecordId::new(PortableDomain::Prompt, "prompt-a").unwrap(),
        device_id("device-b"),
        4,
        NOW - 1,
        6,
    )
    .unwrap();
    let manifest = manifest(&[deleted.clone(), live.clone()]);
    manifest.validate().unwrap();

    let encoded = manifest.to_canonical_json_bytes().unwrap();
    let decoded = SyncV3Manifest::from_json_slice(&encoded).unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(decoded.protocol_version, SyncProtocolVersion::V3);
    assert_eq!(decoded.schema_version, SyncSchemaVersion::V1);
    assert_eq!(decoded.records[0].id, live.id);
    assert_eq!(decoded.records[0].state, SyncRecordState::Live);
    assert_eq!(decoded.records[1].state, SyncRecordState::Deleted);

    assert_eq!(live.schema_version, SyncSchemaVersion::V1);
    assert_eq!(live.revision.schema_version, SyncSchemaVersion::V1);
    assert_eq!(
        String::from_utf8(live.to_canonical_json_bytes().unwrap()).unwrap(),
        r#"{"schemaVersion":1,"id":{"domain":"mcp","key":"mcp-a"},"revision":{"schemaVersion":1,"deviceId":"device-a","counter":1,"contentHash":"07306a57e5f9f046158376da1f159b509cb89a649979f976f310fa55b728f2f1","updatedAtMs":1800000000000},"payload":{"schemaVersion":1,"domain":"mcp","content":{"name":"mcp-a"}}}"#
    );
    assert_eq!(
        deleted.tombstone.as_ref().unwrap().schema_version,
        SyncSchemaVersion::V1
    );
    assert_eq!(
        deleted.tombstone.as_ref().unwrap().retention,
        TombstoneRetention::Permanent
    );
}

#[test]
fn unknown_future_versions_and_unknown_envelope_fields_fail_closed() {
    let record = live_record(PortableDomain::Provider, "provider-a", "device-a");
    let mut record_json = serde_json::to_value(&record).unwrap();
    record_json["schemaVersion"] = json!(2);
    assert!(serde_json::from_value::<SyncRecord>(record_json).is_err());

    let manifest = manifest(&[record]);
    let mut manifest_json = serde_json::to_value(&manifest).unwrap();
    manifest_json["protocolVersion"] = json!(4);
    assert!(serde_json::from_value::<SyncV3Manifest>(manifest_json).is_err());

    let mut unknown_field = serde_json::to_value(&manifest).unwrap();
    unknown_field["webdavPassword"] = json!("must-not-be-accepted");
    assert!(serde_json::from_value::<SyncV3Manifest>(unknown_field).is_err());
}

#[test]
fn canonical_record_bytes_do_not_depend_on_json_object_insertion_order() {
    let mut left_nested = serde_json::Map::new();
    left_nested.insert("zeta".to_string(), json!(2));
    left_nested.insert("alpha".to_string(), json!(1));
    let mut left_content = serde_json::Map::new();
    left_content.insert("portableConfig".to_string(), Value::Object(left_nested));
    left_content.insert("name".to_string(), json!("Custom"));

    let mut right_nested = serde_json::Map::new();
    right_nested.insert("alpha".to_string(), json!(1));
    right_nested.insert("zeta".to_string(), json!(2));
    let mut right_content = serde_json::Map::new();
    right_content.insert("name".to_string(), json!("Custom"));
    right_content.insert("portableConfig".to_string(), Value::Object(right_nested));

    let record_id = PortableRecordId::new(PortableDomain::Provider, "provider-a").unwrap();
    let left = SyncRecord::live(
        record_id.clone(),
        device_id("device-a"),
        1,
        NOW,
        PortablePayload::new(PortableDomain::Provider, Value::Object(left_content)).unwrap(),
    )
    .unwrap();
    let right = SyncRecord::live(
        record_id,
        device_id("device-a"),
        1,
        NOW,
        PortablePayload::new(PortableDomain::Provider, Value::Object(right_content)).unwrap(),
    )
    .unwrap();

    assert_eq!(left.revision.content_hash, right.revision.content_hash);
    assert_eq!(
        left.to_canonical_json_bytes().unwrap(),
        right.to_canonical_json_bytes().unwrap()
    );
}

#[test]
fn manifest_requires_strict_stable_order_and_rejects_duplicate_ids() {
    let first = live_record(PortableDomain::Mcp, "a", "device-a");
    let second = live_record(PortableDomain::Prompt, "b", "device-a");

    let mut unsorted = manifest(&[first.clone(), second.clone()]);
    unsorted.records.reverse();
    assert_eq!(
        unsorted.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let mut duplicate_record = manifest(std::slice::from_ref(&first));
    duplicate_record
        .records
        .push(duplicate_record.records[0].clone());
    assert_eq!(
        duplicate_record.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let mut duplicate_device = manifest(&[first]);
    duplicate_device.devices.push(device("device-a", 7));
    assert_eq!(
        duplicate_device.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn record_requires_exactly_one_matching_live_payload_or_permanent_tombstone() {
    let mut live = live_record(PortableDomain::Mcp, "mcp-a", "device-a");
    live.payload = None;
    assert_eq!(
        live.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let mut mismatched = live_record(PortableDomain::Mcp, "mcp-a", "device-a");
    mismatched.payload = Some(payload(
        PortableDomain::Prompt,
        &[("name", json!("prompt-a"))],
    ));
    assert_eq!(
        mismatched.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let mut deleted = SyncRecord::deleted(
        PortableRecordId::new(PortableDomain::Skill, "skill-a").unwrap(),
        device_id("device-a"),
        3,
        NOW,
        7,
    )
    .unwrap();
    deleted.payload = Some(payload(
        PortableDomain::Skill,
        &[("name", json!("skill-a"))],
    ));
    assert_eq!(
        deleted.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn revisions_devices_and_acknowledged_generations_are_strictly_validated() {
    let mut revision = RecordRevision {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id("device-a"),
        counter: 0,
        content_hash: Sha256Digest::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap(),
        updated_at_ms: NOW,
    };
    assert_eq!(
        revision.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
    revision.counter = 1;
    revision.updated_at_ms = -1;
    assert_eq!(
        revision.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
    assert!(
        Sha256Digest::new("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .is_err()
    );
    assert!(SyncDeviceId::new("bad device").is_err());

    let record = live_record(PortableDomain::Mcp, "mcp-a", "device-a");
    let mut invalid_ack = manifest(&[record]);
    invalid_ack.devices[1].acknowledged_generation = invalid_ack.generation + 1;
    assert_eq!(
        invalid_ack.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn device_retirement_and_tombstone_generation_support_safe_compaction_checks() {
    let mut retired = device("device-b", 6);
    retired.status = SyncDeviceStatus::Retired;
    retired.retired_at_ms = Some(NOW + 1);
    retired.validate().unwrap();

    retired.retired_at_ms = None;
    assert_eq!(
        retired.validate().unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let tombstone = PermanentTombstone {
        schema_version: SyncSchemaVersion::V1,
        deleted_at_ms: NOW,
        deleted_by_device_id: device_id("device-a"),
        introduced_generation: 7,
        retention: TombstoneRetention::Permanent,
    };
    tombstone.validate().unwrap();
    assert!(tombstone.can_compact_after([7_u64, 8_u64]));
    assert!(!tombstone.can_compact_after([7_u64, 6_u64]));
}

#[test]
fn sync_baseline_carries_validated_record_content_and_confirmed_generation() {
    let record = live_record(PortableDomain::CommonSnippet, "snippet-a", "device-a");
    let baseline = SyncRecordBaseline {
        schema_version: SyncSchemaVersion::V1,
        confirmed_generation: 7,
        record,
    };
    baseline.validate().unwrap();
    let encoded = serde_json::to_vec(&baseline).unwrap();
    assert_eq!(
        serde_json::from_slice::<SyncRecordBaseline>(&encoded).unwrap(),
        baseline
    );
}

#[test]
fn payload_schema_rejects_device_settings_credentials_and_raw_session_material() {
    for forbidden in [
        json!({"currentProvider": "provider-a"}),
        json!({"deviceId": "device-a"}),
        json!({"fixedWslPath": "/home/zhldm/.codex"}),
        json!({"webdavPassword": "password"}),
        json!({"briefModel": "model-a"}),
        json!({"runtimeState": {"running": true}}),
        json!({"rawSession": [{"role": "user", "content": "secret"}]}),
        json!({"searchIndex": ["term"]}),
        json!({"restoreCommand": "codex resume --last"}),
        json!({"config": {"apiKey": "sk-secret"}}),
    ] {
        assert!(PortablePayload::new(PortableDomain::Provider, forbidden).is_err());
    }

    let allowed = PortablePayload::new(
        PortableDomain::Provider,
        json!({
            "name": "Custom",
            "portableConfig": {"baseUrl": "https://api.example.com/v1"}
        }),
    )
    .unwrap();
    allowed.validate().unwrap();

    let mut object = BTreeMap::new();
    object.insert("schemaVersion".to_string(), json!(1));
    object.insert("domain".to_string(), json!("session"));
    object.insert("content".to_string(), json!({"name": "raw"}));
    assert!(serde_json::from_value::<PortablePayload>(json!(object)).is_err());
}
