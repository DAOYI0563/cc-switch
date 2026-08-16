use serde_json::json;
use wsl_code_switch_lib::domain::{
    compare_local_scan_summaries, DomainErrorCode, LocalScanDomain, LocalScanEntrySummary,
    LocalScanEvent, LocalScanFailureKind, LocalScanRecordChange, LocalScanSummary, LocalScanTarget,
    ManagedClientId,
};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn target(domain: LocalScanDomain) -> LocalScanTarget {
    LocalScanTarget {
        domain,
        client_id: ManagedClientId::Claude,
    }
}

fn entry(id: &str, digest: &str, size_bytes: u64) -> LocalScanEntrySummary {
    LocalScanEntrySummary::new(id, digest, size_bytes, Some(1_700_000_000_000)).unwrap()
}

#[test]
fn summary_contract_is_shared_stable_and_content_free() {
    assert_eq!(
        LocalScanDomain::ALL.map(LocalScanDomain::as_str),
        ["provider", "mcp", "prompt", "skill"]
    );

    for domain in LocalScanDomain::ALL {
        let summary = LocalScanSummary::new(
            target(domain),
            HASH_A,
            vec![entry("z-last", HASH_C, 3), entry("a-first", HASH_B, 2)],
        )
        .unwrap();

        assert_eq!(summary.entries[0].record_id, "a-first");
        assert_eq!(summary.entries[1].record_id, "z-last");

        let value = serde_json::to_value(&summary).unwrap();
        assert_eq!(
            value,
            json!({
                "schemaVersion": 1,
                "target": {"domain": domain.as_str(), "clientId": "claude"},
                "scopeDigest": HASH_A,
                "entries": [
                    {
                        "recordId": "a-first",
                        "contentDigest": HASH_B,
                        "sizeBytes": 2,
                        "modifiedAtMs": 1_700_000_000_000_i64
                    },
                    {
                        "recordId": "z-last",
                        "contentDigest": HASH_C,
                        "sizeBytes": 3,
                        "modifiedAtMs": 1_700_000_000_000_i64
                    }
                ]
            })
        );
        let encoded = serde_json::to_string(&summary).unwrap();
        for forbidden in ["content\"", "settings", "apiKey", "path", "message"] {
            assert!(!encoded.contains(forbidden), "leaked field: {forbidden}");
        }
        assert_eq!(
            serde_json::from_str::<LocalScanSummary>(&encoded).unwrap(),
            summary
        );
    }
}

#[test]
fn comparison_emits_added_modified_and_deleted_records_in_identity_order() {
    let previous = LocalScanSummary::new(
        target(LocalScanDomain::Mcp),
        HASH_A,
        vec![
            entry("a-unchanged", HASH_A, 1),
            entry("b-modified", HASH_B, 2),
            entry("d-deleted", HASH_D, 4),
        ],
    )
    .unwrap();
    let current = LocalScanSummary::new(
        target(LocalScanDomain::Mcp),
        HASH_C,
        vec![
            entry("c-added", HASH_C, 3),
            entry("a-unchanged", HASH_A, 1),
            entry("b-modified", HASH_D, 5),
        ],
    )
    .unwrap();

    let event = compare_local_scan_summaries(&previous, &current).unwrap();
    let LocalScanEvent::Changed { records, .. } = event else {
        panic!("expected changed event");
    };
    assert_eq!(records.len(), 3);
    assert!(matches!(
        &records[0],
        LocalScanRecordChange::Modified { previous, current }
            if previous.record_id == "b-modified"
                && previous.content_digest == HASH_B
                && current.content_digest == HASH_D
    ));
    assert!(matches!(
        &records[1],
        LocalScanRecordChange::Added { current } if current.record_id == "c-added"
    ));
    assert!(matches!(
        &records[2],
        LocalScanRecordChange::Deleted { previous } if previous.record_id == "d-deleted"
    ));
}

#[test]
fn unchanged_and_scope_only_changes_are_explicit() {
    let previous = LocalScanSummary::new(
        target(LocalScanDomain::Provider),
        HASH_A,
        vec![entry("provider-a", HASH_B, 2)],
    )
    .unwrap();
    let same_content_with_new_metadata = LocalScanSummary::new(
        target(LocalScanDomain::Provider),
        HASH_A,
        vec![LocalScanEntrySummary::new("provider-a", HASH_B, 2, Some(9)).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        compare_local_scan_summaries(&previous, &same_content_with_new_metadata).unwrap(),
        LocalScanEvent::Unchanged { .. }
    ));

    let root_only_change = LocalScanSummary::new(
        target(LocalScanDomain::Provider),
        HASH_C,
        vec![entry("provider-a", HASH_B, 2)],
    )
    .unwrap();
    assert!(matches!(
        compare_local_scan_summaries(&previous, &root_only_change).unwrap(),
        LocalScanEvent::Changed { records, .. } if records.is_empty()
    ));
}

#[test]
fn invalid_or_ambiguous_summaries_fail_before_comparison() {
    let duplicate = LocalScanSummary::new(
        target(LocalScanDomain::Skill),
        HASH_A,
        vec![entry("same", HASH_A, 1), entry("same", HASH_B, 2)],
    )
    .unwrap_err();
    assert_eq!(duplicate.code, DomainErrorCode::InvalidRecord);

    let invalid_digest = LocalScanEntrySummary::new("record", "not-a-digest", 0, None).unwrap_err();
    assert_eq!(invalid_digest.code, DomainErrorCode::InvalidHash);

    let previous = LocalScanSummary::new(
        target(LocalScanDomain::Prompt),
        HASH_A,
        vec![entry("prompt", HASH_A, 1)],
    )
    .unwrap();
    let current = LocalScanSummary::new(
        LocalScanTarget {
            domain: LocalScanDomain::Prompt,
            client_id: ManagedClientId::Codex,
        },
        HASH_B,
        vec![entry("prompt", HASH_B, 1)],
    )
    .unwrap();
    assert_eq!(
        compare_local_scan_summaries(&previous, &current)
            .unwrap_err()
            .code,
        DomainErrorCode::InvalidRecord
    );
}

#[test]
fn read_failures_expose_only_stable_classification() {
    let event = LocalScanEvent::failed(
        target(LocalScanDomain::Prompt),
        LocalScanFailureKind::PermissionDenied,
        Some("prompt-live".to_string()),
    )
    .unwrap();
    let value = serde_json::to_value(event).unwrap();
    assert_eq!(
        value,
        json!({
            "status": "failed",
            "target": {"domain": "prompt", "clientId": "claude"},
            "failure": {"kind": "permission_denied", "recordId": "prompt-live"}
        })
    );
    let encoded = value.to_string();
    for forbidden in ["message", "path", "content", "credential"] {
        assert!(!encoded.contains(forbidden), "leaked field: {forbidden}");
    }
}
