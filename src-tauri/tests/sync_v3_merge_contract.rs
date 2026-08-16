use serde_json::json;
use wsl_code_switch_lib::domain::{
    merge_sync_records, DomainErrorCode, PortableDomain, PortablePayload, PortableRecordId,
    Sha256Digest, SyncDeviceId, SyncMergeConflictKind, SyncMergeInput, SyncMergeSideAction,
    SyncRecord, SyncRecordBaseline, SyncSchemaVersion,
};

const NOW: i64 = 1_800_000_000_000;

fn device(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn live(key: &str, value: &str, owner: &str, counter: u64, updated_at_ms: i64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        device(owner),
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
        device(owner),
        counter,
        deleted_at_ms,
        introduced_generation,
    )
    .unwrap()
}

fn baseline(record: SyncRecord) -> SyncRecordBaseline {
    SyncRecordBaseline {
        schema_version: SyncSchemaVersion::V1,
        confirmed_generation: 10,
        record,
    }
}

fn input(
    baselines: Vec<SyncRecordBaseline>,
    local_records: Vec<SyncRecord>,
    remote_records: Vec<SyncRecord>,
) -> SyncMergeInput {
    SyncMergeInput {
        schema_version: SyncSchemaVersion::V1,
        baselines,
        local_records,
        remote_records,
    }
}

#[test]
fn baseless_additions_and_same_value_additions_merge_in_stable_order() {
    let local_only = live("a-local", "local", "device-a", 1, NOW);
    let remote_only = live("b-remote", "remote", "device-b", 1, NOW + 1);
    let agreed = live("c-agreed", "same", "device-a", 1, NOW + 2);
    let local_conflict = live("d-conflict", "left", "device-a", 1, NOW + 3);
    let remote_conflict = live("d-conflict", "right", "device-b", 1, NOW + 4);

    let batch = merge_sync_records(input(
        Vec::new(),
        vec![local_only.clone(), agreed.clone(), local_conflict],
        vec![remote_only.clone(), agreed.clone(), remote_conflict],
    ))
    .unwrap();

    assert_eq!(
        batch
            .resolved
            .iter()
            .map(|resolution| resolution.record.id.key.as_str())
            .collect::<Vec<_>>(),
        vec!["a-local", "b-remote", "c-agreed"]
    );
    assert_eq!(batch.resolved[0].record, local_only);
    assert_eq!(
        batch.resolved[0].local_action,
        SyncMergeSideAction::Unchanged
    );
    assert_eq!(
        batch.resolved[0].remote_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(batch.resolved[1].record, remote_only);
    assert_eq!(
        batch.resolved[1].local_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(
        batch.resolved[1].remote_action,
        SyncMergeSideAction::Unchanged
    );
    assert_eq!(
        batch.resolved[2].local_action,
        SyncMergeSideAction::Unchanged
    );
    assert_eq!(
        batch.resolved[2].remote_action,
        SyncMergeSideAction::Unchanged
    );
    assert_eq!(batch.conflicts.len(), 1);
    assert_eq!(batch.conflicts[0].id.key, "d-conflict");
    assert_eq!(
        batch.conflicts[0].kind,
        SyncMergeConflictKind::ConcurrentUpdate
    );
    assert!(batch.conflicts[0].baseline.is_none());
}

#[test]
fn single_side_modifications_and_permanent_deletions_propagate() {
    let a_base = live("a-local-mod", "old", "device-a", 1, NOW);
    let a_local = live("a-local-mod", "new", "device-a", 2, NOW + 1);
    let b_base = live("b-remote-mod", "old", "device-a", 1, NOW);
    let b_remote = live("b-remote-mod", "new", "device-b", 1, NOW + 2);
    let c_base = live("c-local-delete", "old", "device-a", 1, NOW);
    let c_deleted = deleted("c-local-delete", "device-a", 2, NOW + 3, 11);
    let d_base = live("d-remote-delete", "old", "device-a", 1, NOW);
    let d_deleted = deleted("d-remote-delete", "device-b", 1, NOW + 4, 11);

    let batch = merge_sync_records(input(
        vec![
            baseline(a_base.clone()),
            baseline(b_base.clone()),
            baseline(c_base.clone()),
            baseline(d_base.clone()),
        ],
        vec![a_local.clone(), b_base, c_deleted.clone(), d_base],
        vec![a_base, b_remote.clone(), c_base, d_deleted.clone()],
    ))
    .unwrap();

    assert!(batch.conflicts.is_empty());
    assert_eq!(batch.resolved.len(), 4);
    assert_eq!(batch.resolved[0].record, a_local);
    assert_eq!(
        batch.resolved[0].remote_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(batch.resolved[1].record, b_remote);
    assert_eq!(
        batch.resolved[1].local_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(batch.resolved[2].record, c_deleted);
    assert_eq!(
        batch.resolved[2].remote_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(batch.resolved[3].record, d_deleted);
    assert_eq!(
        batch.resolved[3].local_action,
        SyncMergeSideAction::ApplyMerged
    );
}

#[test]
fn same_value_concurrent_updates_and_double_deletes_converge_without_conflict() {
    let update_base = live("a-update", "old", "device-a", 1, NOW);
    let update_local = live("a-update", "same-new", "device-a", 2, NOW + 1);
    let update_remote = live("a-update", "same-new", "device-b", 1, NOW + 2);
    let delete_base = live("b-delete", "old", "device-a", 1, NOW);
    let delete_local = deleted("b-delete", "device-a", 2, NOW + 3, 11);
    let delete_remote = deleted("b-delete", "device-b", 1, NOW + 4, 12);

    let batch = merge_sync_records(input(
        vec![baseline(update_base), baseline(delete_base)],
        vec![update_local, delete_local],
        vec![update_remote, delete_remote],
    ))
    .unwrap();

    assert!(batch.conflicts.is_empty());
    assert_eq!(batch.resolved.len(), 2);
    for resolution in &batch.resolved {
        assert_ne!(resolution.local_action, resolution.remote_action);
        assert!(matches!(
            (resolution.local_action, resolution.remote_action),
            (
                SyncMergeSideAction::ApplyMerged,
                SyncMergeSideAction::Unchanged
            ) | (
                SyncMergeSideAction::Unchanged,
                SyncMergeSideAction::ApplyMerged
            )
        ));
    }
    assert_eq!(
        batch.resolved[1]
            .record
            .tombstone
            .as_ref()
            .unwrap()
            .introduced_generation,
        12
    );
}

#[test]
fn conflicts_preserve_both_sides_and_do_not_block_clean_records() {
    let concurrent_base = live("a-concurrent", "base", "device-a", 1, NOW);
    let update_delete_base = live("b-update-delete", "base", "device-a", 1, NOW);
    let missing_base = live("c-missing", "base", "device-a", 1, NOW);
    let clean_base = live("d-clean", "base", "device-a", 1, NOW);
    let clean_local = live("d-clean", "local", "device-a", 2, NOW + 7);

    let local_records = vec![
        live("a-concurrent", "left", "device-a", 2, NOW + 1),
        live("b-update-delete", "left", "device-a", 2, NOW + 3),
        clean_local.clone(),
    ];
    let remote_records = vec![
        live("a-concurrent", "right", "device-b", 1, NOW + 2),
        deleted("b-update-delete", "device-b", 1, NOW + 4, 11),
        missing_base.clone(),
        clean_base.clone(),
    ];
    let original_local = local_records.clone();
    let original_remote = remote_records.clone();

    let batch = merge_sync_records(input(
        vec![
            baseline(concurrent_base),
            baseline(update_delete_base),
            baseline(missing_base),
            baseline(clean_base),
        ],
        local_records,
        remote_records,
    ))
    .unwrap();

    assert_eq!(batch.resolved.len(), 1);
    assert_eq!(batch.resolved[0].record, clean_local);
    assert_eq!(
        batch.resolved[0].remote_action,
        SyncMergeSideAction::ApplyMerged
    );
    assert_eq!(
        batch
            .conflicts
            .iter()
            .map(|conflict| (conflict.id.key.as_str(), conflict.kind))
            .collect::<Vec<_>>(),
        vec![
            ("a-concurrent", SyncMergeConflictKind::ConcurrentUpdate),
            ("b-update-delete", SyncMergeConflictKind::UpdateDelete),
            ("c-missing", SyncMergeConflictKind::UntrackedRemoval),
        ]
    );
    assert_eq!(original_local.len(), 3);
    assert_eq!(original_remote.len(), 4);
    assert!(batch.conflicts.iter().all(|conflict| {
        conflict
            .local
            .as_ref()
            .is_none_or(|summary| summary.id == conflict.id)
            && conflict
                .remote
                .as_ref()
                .is_none_or(|summary| summary.id == conflict.id)
    }));

    let conflicts_json = serde_json::to_string(&batch.conflicts).unwrap();
    for forbidden_payload in ["left", "right", "serverConfig", "payload"] {
        assert!(!conflicts_json.contains(forbidden_payload));
    }
}

#[test]
fn duplicate_tampered_stale_and_reused_revisions_fail_closed() {
    let duplicate = live("a-duplicate", "value", "device-a", 1, NOW);
    let duplicate_error = merge_sync_records(input(
        Vec::new(),
        vec![duplicate.clone(), duplicate],
        Vec::new(),
    ))
    .unwrap_err();
    assert_eq!(duplicate_error.code, DomainErrorCode::InvalidRecord);

    let mut tampered = live("b-tampered", "value", "device-a", 1, NOW);
    tampered.revision.content_hash = Sha256Digest::new("a".repeat(64)).unwrap();
    assert_eq!(
        merge_sync_records(input(Vec::new(), vec![tampered], Vec::new()))
            .unwrap_err()
            .code,
        DomainErrorCode::InvalidRecord
    );

    let baseline_record = live("c-stale", "current", "device-a", 2, NOW + 2);
    let stale = live("c-stale", "old", "device-a", 1, NOW + 1);
    assert_eq!(
        merge_sync_records(input(
            vec![baseline(baseline_record.clone())],
            vec![stale],
            vec![baseline_record],
        ))
        .unwrap_err()
        .code,
        DomainErrorCode::InvalidRecord
    );

    let reused_left = live("d-reused", "left", "device-a", 3, NOW + 3);
    let reused_right = live("d-reused", "right", "device-a", 3, NOW + 4);
    assert_eq!(
        merge_sync_records(input(Vec::new(), vec![reused_left], vec![reused_right],))
            .unwrap_err()
            .code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn merge_core_is_pure_serializable_and_infrastructure_free() {
    let batch = merge_sync_records(input(
        Vec::new(),
        vec![live("record", "value", "device-a", 1, NOW)],
        Vec::new(),
    ))
    .unwrap();
    serde_json::to_vec(&batch).unwrap();

    let source = include_str!("../src/domain/sync_merge.rs").to_ascii_lowercase();
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
            "merge core gained forbidden infrastructure dependency: {forbidden}"
        );
    }
}
