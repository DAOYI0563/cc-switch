use serde_json::json;
use wsl_code_switch_lib::domain::{
    plan_sync_cas_attempt, resolve_sync_cas_write, DomainErrorCode, PortableDomain,
    PortablePayload, PortableRecordId, SyncCasAttemptInput, SyncCasAttemptKind, SyncCasDecision,
    SyncCasFailureKind, SyncCasStopReason, SyncCasWriteOutcome, SyncDevice, SyncDeviceId,
    SyncDeviceStatus, SyncEtag, SyncMergeSideAction, SyncProtocolVersion, SyncRecord,
    SyncRecordIndexEntry, SyncSchemaVersion, SyncV3Manifest, SyncWriteCondition,
};

const NOW: i64 = 1_800_000_000_000;

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn device(value: &str, generation: u64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(value),
        display_name: value.to_string(),
        acknowledged_generation: generation,
        registered_at_ms: NOW,
        last_seen_at_ms: NOW,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

fn live(key: &str, value: &str, owner: &str, counter: u64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        device_id(owner),
        counter,
        NOW,
        PortablePayload::new(
            PortableDomain::Mcp,
            json!({"id": key, "name": value, "serverConfig": {"command": "safe"}}),
        )
        .unwrap(),
    )
    .unwrap()
}

fn manifest(generation: u64, records: &[SyncRecord]) -> SyncV3Manifest {
    let mut indexes = records
        .iter()
        .map(SyncRecordIndexEntry::from_record)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    indexes.sort_by(|left, right| left.id.cmp(&right.id));
    SyncV3Manifest {
        protocol_version: SyncProtocolVersion::V3,
        schema_version: SyncSchemaVersion::V1,
        generation,
        generated_at_ms: NOW,
        generated_by_device_id: device_id("device-a"),
        records: indexes,
        devices: vec![device("device-a", generation)],
    }
}

fn input(
    attempt: SyncCasAttemptKind,
    previous_failed_guard: Option<wsl_code_switch_lib::domain::SyncCasRemoteGuard>,
    generation: u64,
    etag: &str,
    local_records: Vec<SyncRecord>,
    remote_records: Vec<SyncRecord>,
) -> SyncCasAttemptInput {
    SyncCasAttemptInput {
        schema_version: SyncSchemaVersion::V1,
        attempt,
        previous_failed_guard,
        manifest: manifest(generation, &remote_records),
        etag: SyncEtag::new(etag).unwrap(),
        baselines: Vec::new(),
        local_records,
        remote_records,
    }
}

#[test]
fn first_precondition_failure_requests_exactly_one_refetch_and_remerge() {
    let local = live("server", "local", "device-a", 1);
    let plan = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![local],
        Vec::new(),
    ))
    .unwrap();

    assert!(matches!(
        plan.write_condition,
        SyncWriteCondition::Match(ref etag) if etag.as_str() == "\"etag-10\""
    ));
    assert_eq!(plan.merge_batch.resolved.len(), 1);
    assert_eq!(
        plan.merge_batch.resolved[0].remote_action,
        SyncMergeSideAction::ApplyMerged
    );

    let decision = resolve_sync_cas_write(plan, SyncCasWriteOutcome::PreconditionFailed);
    let SyncCasDecision::RefetchAndRemergeOnce { failed_guard } = decision else {
        panic!("the first CAS conflict must request one fresh observation");
    };
    assert_eq!(failed_guard.generation, 10);
    assert_eq!(failed_guard.etag.as_str(), "\"etag-10\"");
}

#[test]
fn second_precondition_failure_stops_without_releasing_local_apply_actions() {
    let local = live("server", "local", "device-a", 1);
    let first = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![local.clone()],
        Vec::new(),
    ))
    .unwrap();
    let SyncCasDecision::RefetchAndRemergeOnce { failed_guard } =
        resolve_sync_cas_write(first, SyncCasWriteOutcome::PreconditionFailed)
    else {
        panic!("first conflict must permit one remerge");
    };

    let retry = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::RemergeOnce,
        Some(failed_guard),
        11,
        "\"etag-11\"",
        vec![local],
        Vec::new(),
    ))
    .unwrap();
    let decision = resolve_sync_cas_write(retry, SyncCasWriteOutcome::PreconditionFailed);
    assert_eq!(
        decision,
        SyncCasDecision::Stop {
            reason: SyncCasStopReason::ConcurrentWriteAfterRemerge,
            failure: None,
        }
    );
}

#[test]
fn retry_rejects_cached_or_partially_changed_remote_observations() {
    let local = live("server", "local", "device-a", 1);
    let first = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![local.clone()],
        Vec::new(),
    ))
    .unwrap();
    let previous = first.remote_guard;

    for (generation, etag) in [(10, "\"etag-10\""), (11, "\"etag-10\"")] {
        let error = plan_sync_cas_attempt(input(
            SyncCasAttemptKind::RemergeOnce,
            Some(previous.clone()),
            generation,
            etag,
            vec![local.clone()],
            Vec::new(),
        ))
        .unwrap_err();
        assert_eq!(error.code, DomainErrorCode::InvalidRecord);
    }
}

#[test]
fn local_actions_are_released_only_after_remote_commit() {
    let local = live("server", "local", "device-a", 1);
    let plan = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![local],
        Vec::new(),
    ))
    .unwrap();
    let decision = resolve_sync_cas_write(
        plan,
        SyncCasWriteOutcome::Committed {
            etag: Some(SyncEtag::new("\"etag-11\"").unwrap()),
        },
    );
    let SyncCasDecision::ApplyLocalAfterRemoteCommit {
        committed_etag,
        merge_batch,
    } = decision
    else {
        panic!("successful remote commit must release the local apply plan");
    };
    assert_eq!(committed_etag.unwrap().as_str(), "\"etag-11\"");
    assert_eq!(merge_batch.resolved.len(), 1);
}

#[test]
fn non_cas_transport_failures_stop_immediately() {
    let plan = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![live("server", "local", "device-a", 1)],
        Vec::new(),
    ))
    .unwrap();
    assert_eq!(
        resolve_sync_cas_write(
            plan,
            SyncCasWriteOutcome::Failed {
                failure: SyncCasFailureKind::Timeout,
            },
        ),
        SyncCasDecision::Stop {
            reason: SyncCasStopReason::TransportFailure,
            failure: Some(SyncCasFailureKind::Timeout),
        }
    );
}

#[test]
fn mismatched_remote_snapshot_fails_before_merge_or_write_planning() {
    let remote = live("remote", "value", "device-a", 1);
    let mut attempt = input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        Vec::new(),
        vec![remote],
    );
    attempt.remote_records.clear();
    assert_eq!(
        plan_sync_cas_attempt(attempt).unwrap_err().code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn cas_core_is_serializable_and_infrastructure_free() {
    let plan = plan_sync_cas_attempt(input(
        SyncCasAttemptKind::Initial,
        None,
        10,
        "\"etag-10\"",
        vec![live("server", "local", "device-a", 1)],
        Vec::new(),
    ))
    .unwrap();
    serde_json::to_vec(&plan).unwrap();

    let source = include_str!("../src/domain/sync_cas.rs").to_ascii_lowercase();
    for forbidden in [
        "tauri",
        "reqwest",
        "rusqlite",
        "std::fs",
        "tokio",
        "windows::",
    ] {
        assert!(
            !source.contains(forbidden),
            "CAS core gained infrastructure dependency: {forbidden}"
        );
    }
}
