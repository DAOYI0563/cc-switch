use serde_json::json;
use wsl_code_switch_lib::domain::{
    confirm_first_sync, plan_sync_device_retirement, preview_first_sync, DomainErrorCode,
    PortableDomain, PortablePayload, PortableRecordId, SyncDevice, SyncDeviceId,
    SyncDeviceRetirementConsent, SyncDeviceRetirementInput, SyncDeviceStatus, SyncEtag,
    SyncFirstSyncConfirmationInput, SyncFirstSyncConsent, SyncFirstSyncInput,
    SyncFirstSyncRemoteGuard, SyncFirstSyncRemoteState, SyncProtocolVersion, SyncRecord,
    SyncRecordBaseline, SyncRecordIndexEntry, SyncSchemaVersion, SyncV3Manifest,
};

const NOW: i64 = 1_800_000_000_000;

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn live(key: &str, value: &str, owner: &str, counter: u64, updated_at_ms: i64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        device_id(owner),
        counter,
        updated_at_ms,
        PortablePayload::new(
            PortableDomain::Mcp,
            json!({"id": key, "name": value, "serverConfig": {"command": "safe"}}),
        )
        .unwrap(),
    )
    .unwrap()
}

fn deleted(
    key: &str,
    owner: &str,
    counter: u64,
    deleted_at_ms: i64,
    introduced_generation: u64,
) -> SyncRecord {
    SyncRecord::deleted(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        device_id(owner),
        counter,
        deleted_at_ms,
        introduced_generation,
    )
    .unwrap()
}

fn baseline(record: SyncRecord, generation: u64) -> SyncRecordBaseline {
    SyncRecordBaseline {
        schema_version: SyncSchemaVersion::V1,
        confirmed_generation: generation,
        record,
    }
}

fn active_device(id: &str, generation: u64, last_seen_at_ms: i64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(id),
        display_name: format!("Device {id}"),
        acknowledged_generation: generation,
        registered_at_ms: NOW - 10_000,
        last_seen_at_ms,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

fn manifest(
    generation: u64,
    writer: &str,
    devices: Vec<SyncDevice>,
    records: &[SyncRecord],
) -> SyncV3Manifest {
    let mut entries = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    entries.sort_by(|left, right| left.id.cmp(&right.id));
    let mut devices = devices;
    devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
    SyncV3Manifest {
        protocol_version: SyncProtocolVersion::V3,
        schema_version: SyncSchemaVersion::V1,
        generation,
        generated_at_ms: NOW,
        generated_by_device_id: device_id(writer),
        records: entries,
        devices,
    }
}

fn existing_input(
    candidate: &str,
    display_name: &str,
    manifest: SyncV3Manifest,
    baselines: Vec<SyncRecordBaseline>,
    local_records: Vec<SyncRecord>,
    remote_records: Vec<SyncRecord>,
) -> SyncFirstSyncInput {
    SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: device_id(candidate),
        display_name: display_name.to_string(),
        observed_at_ms: NOW + 1_000,
        remote_state: SyncFirstSyncRemoteState::Existing {
            manifest,
            etag: SyncEtag::new("\"manifest-7\"").unwrap(),
        },
        baselines,
        local_records,
        remote_records,
    }
}

#[test]
fn first_preview_is_pure_and_bootstrap_does_not_fix_identity_or_write() {
    let local = vec![live("local", "Local", "device-new", 1, NOW)];
    let input = SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: device_id("device-new"),
        display_name: "New device".to_string(),
        observed_at_ms: NOW,
        remote_state: SyncFirstSyncRemoteState::Empty,
        baselines: Vec::new(),
        local_records: local.clone(),
        remote_records: Vec::new(),
    };
    let original = input.clone();

    let preview = preview_first_sync(&input).unwrap();

    assert_eq!(input, original);
    assert_eq!(preview.changes.additions, 1);
    assert_eq!(preview.changes.modifications, 0);
    assert_eq!(preview.changes.deletions, 0);
    assert_eq!(preview.changes.conflicts, 0);
    assert_eq!(preview.remote_generation, 0);
    assert!(preview.remote_etag.is_none());
    assert_eq!(preview.candidate_device_id, device_id("device-new"));
}

#[test]
fn preview_counts_additions_modifications_deletions_and_conflicts() {
    let add_remote = live("a-add", "Remote", "device-remote", 1, NOW - 5);
    let modify_base = live("b-modify", "Old", "device-remote", 1, NOW - 20);
    let modify_remote = live("b-modify", "New", "device-remote", 2, NOW - 4);
    let delete_base = live("c-delete", "Old", "device-remote", 1, NOW - 20);
    let delete_remote = deleted("c-delete", "device-remote", 2, NOW - 3, 7);
    let conflict_base = live("d-conflict", "Old", "device-remote", 1, NOW - 20);
    let conflict_local = live("d-conflict", "Local", "device-new", 1, NOW - 2);
    let conflict_remote = live("d-conflict", "Remote", "device-remote", 2, NOW - 1);
    let remote_records = vec![
        add_remote.clone(),
        modify_remote,
        delete_remote,
        conflict_remote,
    ];
    let remote_manifest = manifest(
        7,
        "device-remote",
        vec![active_device("device-remote", 7, NOW)],
        &remote_records,
    );
    let input = existing_input(
        "device-new",
        "New device",
        remote_manifest,
        vec![
            baseline(modify_base.clone(), 6),
            baseline(delete_base.clone(), 6),
            baseline(conflict_base, 6),
        ],
        vec![modify_base, delete_base, conflict_local],
        remote_records,
    );

    let preview = preview_first_sync(&input).unwrap();

    assert_eq!(preview.changes.additions, 1);
    assert_eq!(preview.changes.modifications, 1);
    assert_eq!(preview.changes.deletions, 1);
    assert_eq!(preview.changes.conflicts, 1);
    assert_eq!(preview.remote_generation, 7);
    assert_eq!(preview.remote_etag.unwrap().as_str(), "\"manifest-7\"");
}

#[test]
fn confirmation_rejects_stale_remote_or_local_preview() {
    let remote = vec![live("remote", "Remote", "device-remote", 1, NOW)];
    let remote_manifest = manifest(
        7,
        "device-remote",
        vec![active_device("device-remote", 7, NOW)],
        &remote,
    );
    let input = existing_input(
        "device-new",
        "New device",
        remote_manifest,
        Vec::new(),
        Vec::new(),
        remote,
    );
    let preview = preview_first_sync(&input).unwrap();

    let mut stale_remote = input.clone();
    let SyncFirstSyncRemoteState::Existing { etag, .. } = &mut stale_remote.remote_state else {
        unreachable!();
    };
    *etag = SyncEtag::new("\"manifest-8\"").unwrap();
    assert_eq!(
        confirm_first_sync(SyncFirstSyncConfirmationInput {
            schema_version: SyncSchemaVersion::V1,
            expected_preview_token: preview.preview_token.clone(),
            current: stale_remote,
            existing_identity: None,
            confirmed_at_ms: NOW + 2_000,
            consent: SyncFirstSyncConsent::RegisterDeviceAndApplyPreview,
        })
        .unwrap_err()
        .code,
        DomainErrorCode::InvalidRecord
    );

    let mut stale_local = input;
    stale_local
        .local_records
        .push(live("local", "Changed", "device-new", 1, NOW + 1));
    assert_eq!(
        confirm_first_sync(SyncFirstSyncConfirmationInput {
            schema_version: SyncSchemaVersion::V1,
            expected_preview_token: preview.preview_token,
            current: stale_local,
            existing_identity: None,
            confirmed_at_ms: NOW + 2_000,
            consent: SyncFirstSyncConsent::RegisterDeviceAndApplyPreview,
        })
        .unwrap_err()
        .code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn confirmation_returns_fixed_identity_registration_and_exact_remote_guard() {
    let input = SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: device_id("device-new"),
        display_name: "New device".to_string(),
        observed_at_ms: NOW,
        remote_state: SyncFirstSyncRemoteState::Empty,
        baselines: Vec::new(),
        local_records: vec![live("local", "Local", "device-new", 1, NOW)],
        remote_records: Vec::new(),
    };
    let preview = preview_first_sync(&input).unwrap();

    let plan = confirm_first_sync(SyncFirstSyncConfirmationInput {
        schema_version: SyncSchemaVersion::V1,
        expected_preview_token: preview.preview_token,
        current: input,
        existing_identity: None,
        confirmed_at_ms: NOW + 1,
        consent: SyncFirstSyncConsent::RegisterDeviceAndApplyPreview,
    })
    .unwrap();

    assert_eq!(plan.identity.device_id, device_id("device-new"));
    assert_eq!(plan.identity.fixed_at_ms, NOW + 1);
    assert_eq!(plan.registered_device.device_id, plan.identity.device_id);
    assert_eq!(plan.registered_device.acknowledged_generation, 0);
    assert_eq!(plan.registered_device.status, SyncDeviceStatus::Active);
    assert_eq!(plan.remote_guard, SyncFirstSyncRemoteGuard::CreateOnly);
    assert_eq!(plan.merge_batch.resolved.len(), 1);
    assert!(plan.merge_batch.conflicts.is_empty());
}

#[test]
fn duplicate_identity_invalid_name_and_clock_rollback_fail_closed() {
    let remote = Vec::new();
    let remote_manifest = manifest(
        7,
        "device-existing",
        vec![active_device("device-existing", 7, NOW)],
        &remote,
    );
    let duplicate = existing_input(
        "device-existing",
        "Duplicate",
        remote_manifest.clone(),
        Vec::new(),
        Vec::new(),
        remote.clone(),
    );
    assert_eq!(
        preview_first_sync(&duplicate).unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let invalid_name = existing_input(
        "device-new",
        "  ",
        remote_manifest.clone(),
        Vec::new(),
        Vec::new(),
        remote.clone(),
    );
    assert_eq!(
        preview_first_sync(&invalid_name).unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );

    let mut rolled_back = existing_input(
        "device-new",
        "New device",
        remote_manifest,
        Vec::new(),
        Vec::new(),
        remote,
    );
    rolled_back.observed_at_ms = NOW - 1;
    assert_eq!(
        preview_first_sync(&rolled_back).unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn fixed_identity_cannot_be_replaced_during_first_sync_confirmation() {
    let input = SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: device_id("device-new"),
        display_name: "New device".to_string(),
        observed_at_ms: NOW,
        remote_state: SyncFirstSyncRemoteState::Empty,
        baselines: Vec::new(),
        local_records: Vec::new(),
        remote_records: Vec::new(),
    };
    let preview = preview_first_sync(&input).unwrap();
    let existing = wsl_code_switch_lib::domain::FixedSyncDeviceIdentity {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id("already-fixed"),
        display_name: "Already fixed".to_string(),
        fixed_at_ms: NOW - 1,
    };

    assert_eq!(
        confirm_first_sync(SyncFirstSyncConfirmationInput {
            schema_version: SyncSchemaVersion::V1,
            expected_preview_token: preview.preview_token,
            current: input,
            existing_identity: Some(existing),
            confirmed_at_ms: NOW + 1,
            consent: SyncFirstSyncConsent::RegisterDeviceAndApplyPreview,
        })
        .unwrap_err()
        .code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn retirement_requires_target_bound_risk_consent_and_rejects_writer_self_retirement() {
    let devices = vec![
        active_device("device-a", 7, NOW),
        active_device("device-b", 6, NOW - 1),
    ];
    let remote_manifest = manifest(7, "device-a", devices, &[]);

    let mismatched = plan_sync_device_retirement(SyncDeviceRetirementInput {
        schema_version: SyncSchemaVersion::V1,
        manifest: remote_manifest.clone(),
        writer_device_id: device_id("device-a"),
        target_device_id: device_id("device-b"),
        retired_at_ms: NOW + 1,
        consent: SyncDeviceRetirementConsent::AcceptDeviceReappearanceRisk {
            target_device_id: device_id("device-c"),
        },
    })
    .unwrap_err();
    assert_eq!(mismatched.code, DomainErrorCode::InvalidRecord);

    let self_retirement = plan_sync_device_retirement(SyncDeviceRetirementInput {
        schema_version: SyncSchemaVersion::V1,
        manifest: remote_manifest,
        writer_device_id: device_id("device-a"),
        target_device_id: device_id("device-a"),
        retired_at_ms: NOW + 1,
        consent: SyncDeviceRetirementConsent::AcceptDeviceReappearanceRisk {
            target_device_id: device_id("device-a"),
        },
    })
    .unwrap_err();
    assert_eq!(self_retirement.code, DomainErrorCode::InvalidRecord);
}

#[test]
fn retirement_plan_marks_only_the_confirmed_target_retired() {
    let devices = vec![
        active_device("device-a", 7, NOW),
        active_device("device-b", 6, NOW - 1),
    ];
    let remote_manifest = manifest(7, "device-a", devices, &[]);

    let plan = plan_sync_device_retirement(SyncDeviceRetirementInput {
        schema_version: SyncSchemaVersion::V1,
        manifest: remote_manifest,
        writer_device_id: device_id("device-a"),
        target_device_id: device_id("device-b"),
        retired_at_ms: NOW + 1,
        consent: SyncDeviceRetirementConsent::AcceptDeviceReappearanceRisk {
            target_device_id: device_id("device-b"),
        },
    })
    .unwrap();

    assert_eq!(plan.expected_manifest_generation, 7);
    assert_eq!(plan.writer_device_id, device_id("device-a"));
    assert_eq!(plan.devices.len(), 2);
    assert_eq!(plan.devices[0].status, SyncDeviceStatus::Active);
    assert_eq!(plan.devices[1].status, SyncDeviceStatus::Retired);
    assert_eq!(plan.devices[1].retired_at_ms, Some(NOW + 1));
    assert_eq!(plan.devices[1].acknowledged_generation, 6);
}

#[test]
fn lifecycle_core_is_serializable_content_free_and_infrastructure_independent() {
    let input = SyncFirstSyncInput {
        schema_version: SyncSchemaVersion::V1,
        candidate_device_id: device_id("device-new"),
        display_name: "New device".to_string(),
        observed_at_ms: NOW,
        remote_state: SyncFirstSyncRemoteState::Empty,
        baselines: Vec::new(),
        local_records: vec![live("private-record", "secret-value", "device-new", 1, NOW)],
        remote_records: Vec::new(),
    };
    let preview = preview_first_sync(&input).unwrap();
    let encoded = serde_json::to_string(&preview).unwrap();
    assert!(!encoded.contains("private-record"));
    assert!(!encoded.contains("secret-value"));
    assert!(!encoded.contains("serverConfig"));

    let source = include_str!("../src/domain/sync_device.rs").to_ascii_lowercase();
    for forbidden in [
        "rusqlite",
        "tauri",
        "reqwest",
        "std::fs",
        "webdav",
        "atomic_write",
        "remove_file",
    ] {
        assert!(
            !source.contains(forbidden),
            "device lifecycle core gained forbidden infrastructure dependency: {forbidden}"
        );
    }
}
