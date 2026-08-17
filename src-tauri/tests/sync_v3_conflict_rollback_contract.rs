use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::json;
use wsl_code_switch_lib::domain::{
    merge_sync_records, ConflictCenterDisposition, ConflictCenterItem, ConflictCenterSource,
    ConflictResolutionAction, ConflictResolutionRequest, LocalConflictKind, PortableDomain,
    PortablePayload, PortableRecordId, RollbackPointMetadata, RollbackPointPurpose,
    RollbackPointState, SyncDeviceId, SyncLocalCommitPlan, SyncMergeBatch, SyncMergeInput,
    SyncMergeSideAction, SyncRecord, SyncSchemaVersion,
};
use wsl_code_switch_lib::ports::{
    ConflictCenterError, ConflictCenterErrorCode, ConflictCenterResolutionPort,
    ConflictCenterSourcePort, SyncLocalApplyPort, TemporaryRollbackError,
    TemporaryRollbackErrorCode, TemporaryRollbackStore,
};
use wsl_code_switch_lib::{
    apply_committed_sync_batch, list_conflict_center_items, WebDavConflictSource,
};

const NOW: i64 = 1_800_000_000_000;
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn live(key: &str, value: &str, owner: &str, counter: u64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        SyncDeviceId::new(owner).unwrap(),
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

fn merge_batch() -> SyncMergeBatch {
    merge_sync_records(SyncMergeInput {
        schema_version: SyncSchemaVersion::V1,
        baselines: Vec::new(),
        local_records: vec![live("conflict", "left", "device-a", 1)],
        remote_records: vec![
            live("conflict", "right", "device-b", 1),
            live("remote-clean", "remote", "device-b", 2),
        ],
    })
    .unwrap()
}

fn local_commit(merge_batch: SyncMergeBatch) -> SyncLocalCommitPlan {
    SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: 1,
        fixed_identity: None,
        devices: Vec::new(),
        merge_batch,
    }
}

struct Resolver;

impl ConflictCenterResolutionPort for Resolver {
    fn supported_actions(
        &self,
        _item: &ConflictCenterItem,
    ) -> Result<Vec<ConflictResolutionAction>, ConflictCenterError> {
        Ok(vec![
            ConflictResolutionAction::AcceptExternal,
            ConflictResolutionAction::KeepLocal,
        ])
    }

    fn capture_rollback(
        &self,
        _item: &ConflictCenterItem,
        _request: &ConflictResolutionRequest,
    ) -> Result<Vec<u8>, ConflictCenterError> {
        unreachable!("listing does not capture rollback data")
    }

    fn apply_and_validate(
        &self,
        _item: &ConflictCenterItem,
        _request: &ConflictResolutionRequest,
    ) -> Result<(), ConflictCenterError> {
        unreachable!("listing does not apply resolutions")
    }
}

#[test]
fn merge_conflicts_enter_the_shared_center_without_blocking_clean_records() {
    let batch = merge_batch();
    assert_eq!(batch.conflicts.len(), 1);
    assert!(batch.resolved.iter().any(|resolution| {
        resolution.record.id.key == "remote-clean"
            && resolution.local_action == SyncMergeSideAction::ApplyMerged
    }));

    let source = WebDavConflictSource::default();
    source.replace_from_merge(&batch).unwrap();
    let listed = list_conflict_center_items(&[&source], &Resolver).unwrap();

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].source, ConflictCenterSource::Webdav);
    assert_eq!(listed[0].domain, PortableDomain::Mcp);
    assert_eq!(listed[0].record_id.as_deref(), Some("conflict"));
    assert_eq!(
        listed[0].disposition,
        ConflictCenterDisposition::Conflict(LocalConflictKind::ConcurrentUpdate)
    );
    let encoded = serde_json::to_string(&listed).unwrap();
    for forbidden in ["left", "right", "serverConfig", "payload", "safe"] {
        assert!(!encoded.contains(forbidden));
    }

    source.clear();
    assert!(source.list_pending().unwrap().is_empty());
}

#[derive(Default)]
struct RecordingApplier {
    captured: Mutex<usize>,
    applied: Mutex<usize>,
    fail: bool,
}

impl SyncLocalApplyPort for RecordingApplier {
    fn capture_rollback(
        &self,
        plan: &wsl_code_switch_lib::domain::SyncLocalCommitPlan,
    ) -> Result<Vec<u8>, ConflictCenterError> {
        assert!(plan.committed_generation > 0);
        assert!(plan.requires_local_write());
        *self.captured.lock().unwrap() += 1;
        Ok(b"encrypted-by-store".to_vec())
    }

    fn apply_and_validate(
        &self,
        plan: &wsl_code_switch_lib::domain::SyncLocalCommitPlan,
    ) -> Result<(), ConflictCenterError> {
        assert!(plan.committed_generation > 0);
        *self.applied.lock().unwrap() += 1;
        if self.fail {
            Err(ConflictCenterError::new(
                ConflictCenterErrorCode::Apply,
                "fixture apply failed",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct MemoryRollbacks {
    points: Mutex<HashMap<String, RollbackPointMetadata>>,
    created_purposes: Mutex<Vec<RollbackPointPurpose>>,
    deleted: Mutex<usize>,
    retained: Mutex<usize>,
    fail_delete: bool,
    fail_retain: bool,
}

impl TemporaryRollbackStore for MemoryRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let id = format!("point-{}", self.points.lock().unwrap().len() + 1);
        let metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: id.clone(),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: DIGEST.to_string(),
        };
        self.created_purposes.lock().unwrap().push(purpose);
        self.points.lock().unwrap().insert(id, metadata.clone());
        Ok(metadata)
    }

    fn restore(&self, _id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        Ok(Vec::new())
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        *self.deleted.lock().unwrap() += 1;
        if self.fail_delete {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::Io,
                "injected rollback delete failure with sensitive details",
            ));
        }
        self.points.lock().unwrap().remove(id).ok_or_else(missing)?;
        Ok(())
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        *self.retained.lock().unwrap() += 1;
        if self.fail_retain {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::Protection,
                "injected rollback retention failure with sensitive details",
            ));
        }
        let mut points = self.points.lock().unwrap();
        let point = points.get_mut(id).ok_or_else(missing)?;
        point.state = RollbackPointState::Failed;
        point.failed_at_ms = Some(failed_at_ms);
        Ok(point.clone())
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self.points.lock().unwrap().values().cloned().collect())
    }
}

fn missing() -> TemporaryRollbackError {
    TemporaryRollbackError::new(TemporaryRollbackErrorCode::NotFound, "missing rollback")
}

#[test]
fn committed_clean_records_use_webdav_rollback_lifecycle() {
    let batch = merge_batch();

    let success = RecordingApplier::default();
    let success_rollbacks = MemoryRollbacks::default();
    apply_committed_sync_batch(
        &success,
        &success_rollbacks,
        NOW,
        &local_commit(batch.clone()),
    )
    .unwrap();
    assert_eq!(*success.captured.lock().unwrap(), 1);
    assert_eq!(*success.applied.lock().unwrap(), 1);
    assert_eq!(
        success_rollbacks
            .created_purposes
            .lock()
            .unwrap()
            .as_slice(),
        &[RollbackPointPurpose::WebdavSync]
    );
    assert_eq!(*success_rollbacks.deleted.lock().unwrap(), 1);
    assert_eq!(*success_rollbacks.retained.lock().unwrap(), 0);

    let failed = RecordingApplier {
        fail: true,
        ..Default::default()
    };
    let failed_rollbacks = MemoryRollbacks::default();
    let error = apply_committed_sync_batch(&failed, &failed_rollbacks, NOW, &local_commit(batch))
        .unwrap_err();
    assert_eq!(error.code, ConflictCenterErrorCode::Apply);
    assert_eq!(*failed_rollbacks.deleted.lock().unwrap(), 0);
    assert_eq!(*failed_rollbacks.retained.lock().unwrap(), 1);
}

#[test]
fn committed_sync_batch_survives_rollback_cleanup_failure_and_marks_the_point() {
    let applier = RecordingApplier::default();
    let rollbacks = MemoryRollbacks {
        fail_delete: true,
        ..Default::default()
    };

    apply_committed_sync_batch(&applier, &rollbacks, NOW, &local_commit(merge_batch()))
        .expect("committed sync apply must survive rollback cleanup failure");

    assert_eq!(*applier.captured.lock().unwrap(), 1);
    assert_eq!(*applier.applied.lock().unwrap(), 1);
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 1);
    assert_eq!(*rollbacks.retained.lock().unwrap(), 1);
    let points = rollbacks.points.lock().unwrap();
    assert_eq!(points["point-1"].state, RollbackPointState::Failed);
    assert_eq!(points["point-1"].failed_at_ms, Some(NOW));
}

#[test]
fn rollback_mark_failure_does_not_replace_sync_apply_error() {
    let applier = RecordingApplier {
        fail: true,
        ..Default::default()
    };
    let rollbacks = MemoryRollbacks {
        fail_retain: true,
        ..Default::default()
    };

    let error = apply_committed_sync_batch(&applier, &rollbacks, NOW, &local_commit(merge_batch()))
        .unwrap_err();

    assert_eq!(error.code, ConflictCenterErrorCode::Apply);
    assert_eq!(error.message, "fixture apply failed");
    assert_eq!(*rollbacks.retained.lock().unwrap(), 1);
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 0);
}

#[test]
fn remote_commit_without_business_apply_still_updates_local_sync_metadata() {
    let batch = merge_sync_records(SyncMergeInput {
        schema_version: SyncSchemaVersion::V1,
        baselines: Vec::new(),
        local_records: vec![live("local-only", "local", "device-a", 1)],
        remote_records: Vec::new(),
    })
    .unwrap();
    let applier = RecordingApplier::default();
    let rollbacks = MemoryRollbacks::default();

    apply_committed_sync_batch(&applier, &rollbacks, NOW, &local_commit(batch)).unwrap();

    assert_eq!(*applier.captured.lock().unwrap(), 1);
    assert_eq!(*applier.applied.lock().unwrap(), 1);
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 1);
}

#[test]
fn empty_local_commit_skips_rollback_and_apply() {
    let applier = RecordingApplier::default();
    let rollbacks = MemoryRollbacks::default();
    let plan = local_commit(SyncMergeBatch {
        schema_version: SyncSchemaVersion::V1,
        resolved: Vec::new(),
        conflicts: Vec::new(),
    });

    apply_committed_sync_batch(&applier, &rollbacks, NOW, &plan).unwrap();

    assert_eq!(*applier.captured.lock().unwrap(), 0);
    assert_eq!(*applier.applied.lock().unwrap(), 0);
    assert!(rollbacks.created_purposes.lock().unwrap().is_empty());
}
