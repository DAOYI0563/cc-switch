use wsl_code_switch_lib::domain::{
    classify_local_reconciliation, LocalConflictKind, LocalDifferenceKind,
    LocalReconciliationExternal, LocalReconciliationInput, LocalReconciliationRecord,
    LocalReconciliationSnapshot, LocalScanDomain, LocalScanFailure, LocalScanFailureKind,
    LocalScanTarget, ManagedClientId,
};
use wsl_code_switch_lib::ports::{LocalScanParsedRecord, LocalScanParsedSnapshot};
use wsl_code_switch_lib::{reconciliation_snapshot_from_parsed, LocalScanParsedChange};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn target() -> LocalScanTarget {
    LocalScanTarget {
        domain: LocalScanDomain::Mcp,
        client_id: ManagedClientId::Claude,
    }
}

fn snapshot(records: &[(&str, &str)]) -> LocalReconciliationSnapshot {
    LocalReconciliationSnapshot::new(
        target(),
        records
            .iter()
            .map(|(record_id, digest)| LocalReconciliationRecord::new(*record_id, *digest).unwrap())
            .collect(),
    )
    .unwrap()
}

fn parsed_input(
    baseline: Option<LocalReconciliationSnapshot>,
    local: LocalReconciliationSnapshot,
    external: LocalReconciliationSnapshot,
    scope_changed: bool,
) -> LocalReconciliationInput {
    LocalReconciliationInput {
        target: target(),
        baseline,
        local,
        external: LocalReconciliationExternal::Parsed {
            snapshot: external,
            scope_changed,
        },
    }
}

#[test]
fn external_add_modify_and_delete_are_confirmable_differences_in_stable_order() {
    let baseline = snapshot(&[("b-modified", A), ("c-deleted", B), ("z-unchanged", C)]);
    let local = baseline.clone();
    let external = snapshot(&[("a-added", D), ("b-modified", E), ("z-unchanged", C)]);

    let batch =
        classify_local_reconciliation(parsed_input(Some(baseline), local, external, true)).unwrap();

    assert!(batch.conflicts.is_empty());
    assert_eq!(
        batch
            .differences
            .iter()
            .map(|difference| (difference.record_id.as_str(), difference.kind))
            .collect::<Vec<_>>(),
        vec![
            ("a-added", LocalDifferenceKind::Added),
            ("b-modified", LocalDifferenceKind::Modified),
            ("c-deleted", LocalDifferenceKind::Deleted),
        ]
    );
    assert_eq!(batch.differences[0].baseline_digest, None);
    assert_eq!(batch.differences[0].external_digest.as_deref(), Some(D));
    assert_eq!(batch.differences[1].baseline_digest.as_deref(), Some(A));
    assert_eq!(batch.differences[1].external_digest.as_deref(), Some(E));
    assert_eq!(batch.differences[2].baseline_digest.as_deref(), Some(B));
    assert_eq!(batch.differences[2].external_digest, None);
}

#[test]
fn ambiguous_concurrent_and_baseless_deletions_conflict_without_blocking_differences() {
    let baseline = snapshot(&[("a-both", A), ("b-update-delete", A)]);
    let local = snapshot(&[
        ("a-both", B),
        ("c-no-baseline-delete", C),
        ("d-ambiguous", D),
    ]);
    let external = snapshot(&[
        ("a-both", C),
        ("b-update-delete", B),
        ("d-ambiguous", E),
        ("e-added", A),
    ]);

    let batch =
        classify_local_reconciliation(parsed_input(Some(baseline), local, external, true)).unwrap();

    assert_eq!(batch.differences.len(), 1);
    assert_eq!(batch.differences[0].record_id, "e-added");
    assert_eq!(batch.differences[0].kind, LocalDifferenceKind::Added);
    assert_eq!(
        batch
            .conflicts
            .iter()
            .map(|conflict| (conflict.record_id.as_deref(), conflict.kind))
            .collect::<Vec<_>>(),
        vec![
            (Some("a-both"), LocalConflictKind::ConcurrentUpdate),
            (Some("b-update-delete"), LocalConflictKind::UpdateDelete),
            (
                Some("c-no-baseline-delete"),
                LocalConflictKind::DeleteWithoutBaseline,
            ),
            (Some("d-ambiguous"), LocalConflictKind::AmbiguousLocalMatch),
        ]
    );
}

#[test]
fn parse_failures_and_scope_only_changes_are_fail_closed_conflicts() {
    let failed = classify_local_reconciliation(LocalReconciliationInput {
        target: target(),
        baseline: Some(snapshot(&[("record", A)])),
        local: snapshot(&[("record", A)]),
        external: LocalReconciliationExternal::Failed {
            failure: LocalScanFailure {
                kind: LocalScanFailureKind::ParseFailed,
                record_id: Some("record".to_string()),
            },
        },
    })
    .unwrap();
    assert!(failed.differences.is_empty());
    assert_eq!(failed.conflicts.len(), 1);
    assert_eq!(failed.conflicts[0].kind, LocalConflictKind::ParseFailed);
    assert_eq!(
        failed.conflicts[0].failure_kind,
        Some(LocalScanFailureKind::ParseFailed)
    );

    let same = snapshot(&[("record", A)]);
    let scope_only =
        classify_local_reconciliation(parsed_input(Some(same.clone()), same.clone(), same, true))
            .unwrap();
    assert!(scope_only.differences.is_empty());
    assert_eq!(scope_only.conflicts.len(), 1);
    assert_eq!(scope_only.conflicts[0].record_id, None);
    assert_eq!(
        scope_only.conflicts[0].kind,
        LocalConflictKind::IntegrityMismatch
    );
}

#[test]
fn target_and_digest_validation_fail_before_any_classification() {
    assert!(LocalReconciliationRecord::new("record", "not-a-digest").is_err());

    let wrong_target = LocalScanTarget {
        domain: LocalScanDomain::Prompt,
        client_id: ManagedClientId::Claude,
    };
    let result = classify_local_reconciliation(LocalReconciliationInput {
        target: target(),
        baseline: None,
        local: snapshot(&[]),
        external: LocalReconciliationExternal::Parsed {
            snapshot: LocalReconciliationSnapshot::new(wrong_target, Vec::new()).unwrap(),
            scope_changed: true,
        },
    });
    assert!(result.is_err());
}

#[test]
fn classification_contract_is_serializable_content_free_and_infrastructure_free() {
    let batch = classify_local_reconciliation(parsed_input(
        None,
        snapshot(&[]),
        snapshot(&[("safe-id", A)]),
        true,
    ))
    .unwrap();
    let encoded = serde_json::to_string(&batch).unwrap();
    assert!(encoded.contains("safe-id"));
    assert!(encoded.contains(A));
    for forbidden in ["apiKey", "contents", "settingsConfig", "command"] {
        assert!(!encoded.contains(forbidden));
    }

    let source = include_str!("../src/domain/local_reconciliation.rs").to_ascii_lowercase();
    for forbidden in [
        "rusqlite",
        "tauri",
        "reqwest",
        "std::fs",
        "atomic_write",
        "remove_file",
    ] {
        assert!(
            !source.contains(forbidden),
            "classifier gained forbidden infrastructure dependency: {forbidden}"
        );
    }
}

#[test]
fn parsed_values_are_canonicalized_and_redacted_before_classification() {
    let first_value =
        serde_json::from_str(r#"{"z":1,"secret":"DO_NOT_LEAK","a":{"y":2,"x":3}}"#).unwrap();
    let reordered_value =
        serde_json::from_str(r#"{"a":{"x":3,"y":2},"secret":"DO_NOT_LEAK","z":1}"#).unwrap();
    let first = LocalScanParsedSnapshot::new(
        target(),
        vec![LocalScanParsedRecord::new("record", first_value).unwrap()],
    )
    .unwrap();
    let reordered = LocalScanParsedSnapshot::new(
        target(),
        vec![LocalScanParsedRecord::new("record", reordered_value).unwrap()],
    )
    .unwrap();

    let first_digest = reconciliation_snapshot_from_parsed(&first).unwrap();
    let reordered_digest = reconciliation_snapshot_from_parsed(&reordered).unwrap();
    assert_eq!(first_digest, reordered_digest);
    assert!(!serde_json::to_string(&first_digest)
        .unwrap()
        .contains("DO_NOT_LEAK"));

    let change = LocalScanParsedChange {
        event: wsl_code_switch_lib::domain::LocalScanEvent::Changed {
            target: target(),
            previous_scope_digest: A.to_string(),
            current_scope_digest: B.to_string(),
            records: Vec::new(),
        },
        snapshot: first,
    };
    let batch = change.classify_against(None, snapshot(&[])).unwrap();
    assert_eq!(batch.differences.len(), 1);
    assert_eq!(batch.differences[0].record_id, "record");
    assert_eq!(batch.differences[0].kind, LocalDifferenceKind::Added);
}
