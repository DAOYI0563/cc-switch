use std::collections::HashMap;
use std::sync::Mutex;

use wsl_code_switch_lib::domain::{
    ConflictCenterDisposition, ConflictCenterItem, ConflictCenterSource, ConflictResolutionAction,
    ConflictResolutionRequest, LocalConflictKind, LocalDifferenceKind, ManagedClientId,
    PortableDomain, RollbackPointMetadata, RollbackPointPurpose, RollbackPointState,
};
use wsl_code_switch_lib::ports::{
    ConflictCenterError, ConflictCenterErrorCode, ConflictCenterResolutionPort,
    ConflictCenterSourcePort, TemporaryRollbackError, TemporaryRollbackErrorCode,
    TemporaryRollbackStore,
};
use wsl_code_switch_lib::{list_conflict_center_items, resolve_conflict_center_item};

const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn item(
    id: &str,
    source: ConflictCenterSource,
    domain: PortableDomain,
    client_id: Option<ManagedClientId>,
    record_id: &str,
    disposition: ConflictCenterDisposition,
) -> ConflictCenterItem {
    ConflictCenterItem {
        schema_version: ConflictCenterItem::SCHEMA_VERSION,
        item_id: id.to_string(),
        source,
        domain,
        client_id,
        record_id: Some(record_id.to_string()),
        display_name: record_id.to_string(),
        modified_at_ms: Some(100),
        disposition,
        baseline_digest: Some(A.to_string()),
        local_digest: Some(A.to_string()),
        external_digest: Some(B.to_string()),
        failure_kind: None,
        actions: Vec::new(),
    }
}

struct FixedSource(Vec<ConflictCenterItem>);

impl ConflictCenterSourcePort for FixedSource {
    fn list_pending(&self) -> Result<Vec<ConflictCenterItem>, ConflictCenterError> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct RecordingResolver {
    applied: Mutex<Vec<(String, ConflictResolutionAction)>>,
    fail_item: Mutex<Option<String>>,
}

impl ConflictCenterResolutionPort for RecordingResolver {
    fn supported_actions(
        &self,
        item: &ConflictCenterItem,
    ) -> Result<Vec<ConflictResolutionAction>, ConflictCenterError> {
        Ok(match item.disposition {
            ConflictCenterDisposition::Difference(_) => vec![
                ConflictResolutionAction::KeepLocal,
                ConflictResolutionAction::AcceptExternal,
                ConflictResolutionAction::KeepLocal,
            ],
            ConflictCenterDisposition::Conflict(LocalConflictKind::IntegrityMismatch) => {
                vec![ConflictResolutionAction::Retry]
            }
            ConflictCenterDisposition::Conflict(_) => vec![
                ConflictResolutionAction::KeepBoth,
                ConflictResolutionAction::AcceptExternal,
            ],
        })
    }

    fn capture_rollback(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<Vec<u8>, ConflictCenterError> {
        Ok(format!("SECRET:{}:{:?}", item.item_id, request.action).into_bytes())
    }

    fn apply_and_validate(
        &self,
        item: &ConflictCenterItem,
        request: &ConflictResolutionRequest,
    ) -> Result<(), ConflictCenterError> {
        if self.fail_item.lock().unwrap().as_deref() == Some(&item.item_id) {
            return Err(ConflictCenterError::new(
                ConflictCenterErrorCode::Apply,
                "fixture apply failed",
            ));
        }
        self.applied
            .lock()
            .unwrap()
            .push((item.item_id.clone(), request.action));
        Ok(())
    }
}

#[derive(Default)]
struct MemoryRollbacks {
    points: Mutex<HashMap<String, (RollbackPointMetadata, Vec<u8>)>>,
    created_payloads: Mutex<Vec<Vec<u8>>>,
    deleted: Mutex<Vec<String>>,
    retained: Mutex<Vec<String>>,
}

impl TemporaryRollbackStore for MemoryRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let id = format!("point-{}", self.created_payloads.lock().unwrap().len() + 1);
        let metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: id.clone(),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: A.to_string(),
        };
        self.created_payloads.lock().unwrap().push(payload.to_vec());
        self.points
            .lock()
            .unwrap()
            .insert(id, (metadata.clone(), payload.to_vec()));
        Ok(metadata)
    }

    fn restore(&self, id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        self.points
            .lock()
            .unwrap()
            .get(id)
            .map(|(_, payload)| payload.clone())
            .ok_or_else(not_found)
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        self.points
            .lock()
            .unwrap()
            .remove(id)
            .ok_or_else(not_found)?;
        self.deleted.lock().unwrap().push(id.to_string());
        Ok(())
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let mut points = self.points.lock().unwrap();
        let (metadata, _) = points.get_mut(id).ok_or_else(not_found)?;
        metadata.state = RollbackPointState::Failed;
        metadata.failed_at_ms = Some(failed_at_ms);
        let result = metadata.clone();
        drop(points);
        self.retained.lock().unwrap().push(id.to_string());
        Ok(result)
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self
            .points
            .lock()
            .unwrap()
            .values()
            .map(|(metadata, _)| metadata.clone())
            .collect())
    }
}

fn not_found() -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::NotFound,
        "missing fixture point",
    )
}

#[test]
fn local_and_webdav_feeds_share_one_stable_content_free_list() {
    let local = FixedSource(vec![item(
        "local_item",
        ConflictCenterSource::LocalScan,
        PortableDomain::Mcp,
        Some(ManagedClientId::Claude),
        "z-local",
        ConflictCenterDisposition::Conflict(LocalConflictKind::ConcurrentUpdate),
    )]);
    let webdav = FixedSource(vec![item(
        "webdav_item",
        ConflictCenterSource::Webdav,
        PortableDomain::Provider,
        None,
        "a-cloud",
        ConflictCenterDisposition::Difference(LocalDifferenceKind::Modified),
    )]);
    let resolver = RecordingResolver::default();

    let listed = list_conflict_center_items(&[&webdav, &local], &resolver).unwrap();

    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].source, ConflictCenterSource::LocalScan);
    assert_eq!(listed[1].source, ConflictCenterSource::Webdav);
    assert_eq!(
        listed[0].actions,
        vec![
            ConflictResolutionAction::AcceptExternal,
            ConflictResolutionAction::KeepBoth,
        ]
    );
    let encoded = serde_json::to_string(&listed).unwrap();
    for forbidden in ["SECRET", "local_json", "external_json", "apiKey"] {
        assert!(!encoded.contains(forbidden));
    }
}

#[test]
fn one_conflict_does_not_block_a_confirmable_record() {
    let source = FixedSource(vec![
        item(
            "conflict_item",
            ConflictCenterSource::LocalScan,
            PortableDomain::Mcp,
            Some(ManagedClientId::Codex),
            "conflict",
            ConflictCenterDisposition::Conflict(LocalConflictKind::ConcurrentUpdate),
        ),
        item(
            "difference_item",
            ConflictCenterSource::LocalScan,
            PortableDomain::Mcp,
            Some(ManagedClientId::Codex),
            "difference",
            ConflictCenterDisposition::Difference(LocalDifferenceKind::Added),
        ),
    ]);
    let resolver = RecordingResolver::default();
    let rollbacks = MemoryRollbacks::default();

    resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        200,
        &ConflictResolutionRequest {
            item_id: "difference_item".to_string(),
            action: ConflictResolutionAction::AcceptExternal,
        },
    )
    .unwrap();

    assert_eq!(resolver.applied.lock().unwrap().len(), 1);
    assert_eq!(rollbacks.deleted.lock().unwrap().len(), 1);
    assert!(rollbacks.retained.lock().unwrap().is_empty());
    assert!(rollbacks.created_payloads.lock().unwrap()[0].starts_with(b"SECRET:"));
}

#[test]
fn failed_resolution_retains_rollback_and_does_not_consume_other_items() {
    let source = FixedSource(vec![
        item(
            "failed_item",
            ConflictCenterSource::LocalScan,
            PortableDomain::Skill,
            Some(ManagedClientId::Opencode),
            "failed",
            ConflictCenterDisposition::Difference(LocalDifferenceKind::Modified),
        ),
        item(
            "ready_item",
            ConflictCenterSource::LocalScan,
            PortableDomain::Skill,
            Some(ManagedClientId::Opencode),
            "ready",
            ConflictCenterDisposition::Difference(LocalDifferenceKind::Added),
        ),
    ]);
    let resolver = RecordingResolver::default();
    *resolver.fail_item.lock().unwrap() = Some("failed_item".to_string());
    let rollbacks = MemoryRollbacks::default();

    let error = resolve_conflict_center_item(
        &[&source],
        &resolver,
        &rollbacks,
        300,
        &ConflictResolutionRequest {
            item_id: "failed_item".to_string(),
            action: ConflictResolutionAction::KeepLocal,
        },
    )
    .unwrap_err();

    assert_eq!(error.code, ConflictCenterErrorCode::Apply);
    assert_eq!(rollbacks.retained.lock().unwrap().as_slice(), &["point-1"]);
    assert!(rollbacks.deleted.lock().unwrap().is_empty());
    let listed = list_conflict_center_items(&[&source], &resolver).unwrap();
    assert_eq!(listed.len(), 2);
}

#[test]
fn stale_or_unsupported_requests_fail_before_rollback_or_apply() {
    let source = FixedSource(vec![item(
        "current_item",
        ConflictCenterSource::LocalScan,
        PortableDomain::Prompt,
        Some(ManagedClientId::Claude),
        "prompt-live",
        ConflictCenterDisposition::Difference(LocalDifferenceKind::Modified),
    )]);
    let resolver = RecordingResolver::default();
    let rollbacks = MemoryRollbacks::default();

    for request in [
        ConflictResolutionRequest {
            item_id: "stale_item".to_string(),
            action: ConflictResolutionAction::AcceptExternal,
        },
        ConflictResolutionRequest {
            item_id: "current_item".to_string(),
            action: ConflictResolutionAction::KeepBoth,
        },
    ] {
        assert!(
            resolve_conflict_center_item(&[&source], &resolver, &rollbacks, 400, &request,)
                .is_err()
        );
    }
    assert!(rollbacks.created_payloads.lock().unwrap().is_empty());
    assert!(resolver.applied.lock().unwrap().is_empty());
}

#[test]
fn domain_and_orchestration_contracts_have_no_ui_database_or_network_dependency() {
    let source = format!(
        "{}\n{}\n{}",
        include_str!("../src/domain/conflict_center.rs"),
        include_str!("../src/ports/conflict_center.rs"),
        include_str!("../src/services/conflict_center.rs")
    )
    .to_ascii_lowercase();
    for forbidden in ["tauri", "rusqlite", "reqwest", "std::fs", "react"] {
        assert!(
            !source.contains(forbidden),
            "conflict-center core gained forbidden dependency: {forbidden}"
        );
    }
}
