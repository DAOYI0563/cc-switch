use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;
use wsl_code_switch_lib::adapters::FixedSyncCryptoEngine;
use wsl_code_switch_lib::domain::{
    PortableDomain, PortablePayload, PortableRecordId, RollbackPointMetadata, RollbackPointPurpose,
    RollbackPointState, SyncCiphertext, SyncDevice, SyncDeviceId, SyncDeviceStatus,
    SyncEncryptedEnvelope, SyncEtag, SyncObjectIdentity, SyncProtocolVersion, SyncRecord,
    SyncRecordBaseline, SyncRecordIndexEntry, SyncRemoteObject, SyncRemotePath, SyncSchemaVersion,
    SyncV3Manifest, SyncWriteCondition, SyncWriteReceipt,
};
use wsl_code_switch_lib::ports::{
    ConflictCenterError, SyncCryptoError, SyncCryptoPort, SyncCryptoRandom, SyncLocalApplyPort,
    SyncTransportError, SyncTransportErrorCode, SyncTransportFuture, SyncTransportPort,
    TemporaryRollbackError, TemporaryRollbackErrorCode, TemporaryRollbackStore,
};
use wsl_code_switch_lib::{
    sync_manifest_remote_path, sync_record_remote_path, SyncDeviceRetireRequest,
    SyncFirstSyncConfirmRequest, SyncFirstSyncPreviewRequest, SyncRunErrorCode, SyncRunRequest,
    SyncV3Orchestrator, WebDavConflictSource,
};

const NOW: i64 = 1_800_000_000_000;
const PASSPHRASE: &[u8] = b"correct horse battery staple";
const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Default)]
struct CounterRandom(AtomicU8);

impl SyncCryptoRandom for CounterRandom {
    fn fill_bytes(&self, destination: &mut [u8]) -> Result<(), SyncCryptoError> {
        let seed = self.0.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
        for (offset, byte) in destination.iter_mut().enumerate() {
            *byte = seed.wrapping_add(offset as u8);
        }
        Ok(())
    }
}

fn device_id(value: &str) -> SyncDeviceId {
    SyncDeviceId::new(value).unwrap()
}

fn device(value: &str, generation: u64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(value),
        display_name: value.to_string(),
        acknowledged_generation: generation,
        registered_at_ms: NOW - 100,
        last_seen_at_ms: NOW - 100,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

fn live(key: &str, value: &str, owner: &str, counter: u64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, key).unwrap(),
        device_id(owner),
        counter,
        NOW - 50,
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
        generated_at_ms: NOW - 25,
        generated_by_device_id: device_id("device-b"),
        records: indexes,
        devices: vec![
            device("device-a", generation),
            device("device-b", generation),
        ],
    }
}

type StoredObjects = BTreeMap<Vec<String>, (Vec<u8>, Option<SyncEtag>)>;

fn path_key(path: &SyncRemotePath) -> Vec<String> {
    path.segments().to_vec()
}

fn encrypted_snapshot(
    crypto: &FixedSyncCryptoEngine<CounterRandom>,
    profile: &wsl_code_switch_lib::domain::SyncKdfProfile,
    manifest: &SyncV3Manifest,
    records: &[SyncRecord],
    manifest_etag: &str,
) -> StoredObjects {
    let session = crypto.unlock(PASSPHRASE, profile).unwrap();
    let mut objects = BTreeMap::new();
    for record in records {
        let index = SyncRecordIndexEntry::from_record(record).unwrap();
        let identity =
            SyncObjectIdentity::record(record.id.clone(), record.revision.counter).unwrap();
        let envelope = session
            .seal(&identity, &record.to_canonical_json_bytes().unwrap())
            .unwrap();
        objects.insert(
            path_key(&sync_record_remote_path(&index).unwrap()),
            (envelope.to_json_bytes().unwrap(), None),
        );
    }
    let identity = SyncObjectIdentity::manifest(manifest.generation).unwrap();
    let envelope = session
        .seal(&identity, &manifest.to_canonical_json_bytes().unwrap())
        .unwrap();
    objects.insert(
        path_key(&sync_manifest_remote_path().unwrap()),
        (
            envelope.to_json_bytes().unwrap(),
            Some(SyncEtag::new(manifest_etag).unwrap()),
        ),
    );
    objects
}

enum ManifestWriteBehavior {
    ReplaceAndFail(StoredObjects),
    Fail,
}

struct MemoryTransport {
    objects: Mutex<StoredObjects>,
    manifest_behaviors: Mutex<VecDeque<ManifestWriteBehavior>>,
    events: Arc<Mutex<Vec<String>>>,
    writes: Mutex<usize>,
}

impl MemoryTransport {
    fn new(objects: StoredObjects, events: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            objects: Mutex::new(objects),
            manifest_behaviors: Mutex::new(VecDeque::new()),
            events,
            writes: Mutex::new(0),
        }
    }

    fn queue_manifest_behavior(&self, behavior: ManifestWriteBehavior) {
        self.manifest_behaviors.lock().unwrap().push_back(behavior);
    }

    fn write_count(&self) -> usize {
        *self.writes.lock().unwrap()
    }
}

impl SyncTransportPort for MemoryTransport {
    fn ensure_directories<'a>(&'a self, _path: &'a SyncRemotePath) -> SyncTransportFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn read<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        _max_bytes: usize,
    ) -> SyncTransportFuture<'a, Option<SyncRemoteObject>> {
        Box::pin(async move {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .get(&path_key(path))
                .map(|(bytes, etag)| SyncRemoteObject::new(bytes.clone(), etag.clone())))
        })
    }

    fn conditional_write<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        bytes: &'a [u8],
        condition: &'a SyncWriteCondition,
    ) -> SyncTransportFuture<'a, SyncWriteReceipt> {
        Box::pin(async move {
            *self.writes.lock().unwrap() += 1;
            let key = path_key(path);
            let is_manifest = key == path_key(&sync_manifest_remote_path().unwrap());
            if is_manifest {
                self.events
                    .lock()
                    .unwrap()
                    .push("remote_manifest_write".to_string());
                if let Some(behavior) = self.manifest_behaviors.lock().unwrap().pop_front() {
                    if let ManifestWriteBehavior::ReplaceAndFail(replacement) = behavior {
                        *self.objects.lock().unwrap() = replacement;
                    }
                    return Err(SyncTransportError::new(
                        SyncTransportErrorCode::PreconditionFailed,
                        "fixture CAS conflict",
                    ));
                }
            }

            let mut objects = self.objects.lock().unwrap();
            match condition {
                SyncWriteCondition::CreateOnly if objects.contains_key(&key) => {
                    return Err(SyncTransportError::new(
                        SyncTransportErrorCode::PreconditionFailed,
                        "fixture object already exists",
                    ));
                }
                SyncWriteCondition::Match(expected)
                    if objects.get(&key).and_then(|(_, etag)| etag.as_ref()) != Some(expected) =>
                {
                    return Err(SyncTransportError::new(
                        SyncTransportErrorCode::PreconditionFailed,
                        "fixture ETag mismatch",
                    ));
                }
                _ => {}
            }
            let etag = SyncEtag::new(format!("\"write-{}\"", self.write_count())).unwrap();
            objects.insert(key, (bytes.to_vec(), Some(etag.clone())));
            Ok(SyncWriteReceipt::new(Some(etag)))
        })
    }
}

struct RecordingApplier {
    events: Arc<Mutex<Vec<String>>>,
    applied: Mutex<usize>,
    plans: Mutex<Vec<wsl_code_switch_lib::domain::SyncLocalCommitPlan>>,
}

impl SyncLocalApplyPort for RecordingApplier {
    fn capture_rollback(
        &self,
        plan: &wsl_code_switch_lib::domain::SyncLocalCommitPlan,
    ) -> Result<Vec<u8>, ConflictCenterError> {
        assert!(plan.committed_generation > 0);
        self.events.lock().unwrap().push("capture".to_string());
        Ok(b"rollback payload".to_vec())
    }

    fn apply_and_validate(
        &self,
        plan: &wsl_code_switch_lib::domain::SyncLocalCommitPlan,
    ) -> Result<(), ConflictCenterError> {
        assert!(plan.committed_generation > 0);
        self.events.lock().unwrap().push("local_apply".to_string());
        self.plans.lock().unwrap().push(plan.clone());
        *self.applied.lock().unwrap() += 1;
        Ok(())
    }
}

#[derive(Default)]
struct MemoryRollbacks(Mutex<HashMap<String, RollbackPointMetadata>>);

impl TemporaryRollbackStore for MemoryRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let point = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: "rollback-1".to_string(),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: DIGEST.to_string(),
        };
        self.0
            .lock()
            .unwrap()
            .insert(point.id.clone(), point.clone());
        Ok(point)
    }

    fn restore(&self, _id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        Ok(Vec::new())
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        self.0.lock().unwrap().remove(id).ok_or_else(missing)?;
        Ok(())
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let mut points = self.0.lock().unwrap();
        let point = points.get_mut(id).ok_or_else(missing)?;
        point.state = RollbackPointState::Failed;
        point.failed_at_ms = Some(failed_at_ms);
        Ok(point.clone())
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }
}

fn missing() -> TemporaryRollbackError {
    TemporaryRollbackError::new(TemporaryRollbackErrorCode::NotFound, "missing rollback")
}

fn request(local_records: Vec<SyncRecord>) -> SyncRunRequest {
    SyncRunRequest {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id("device-a"),
        now_ms: NOW,
        baselines: Vec::new(),
        local_records,
    }
}

#[tokio::test]
async fn remote_manifest_commit_precedes_conflict_routing_and_local_apply() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let profile = crypto.create_profile().unwrap();
    let remote_records = vec![
        live("conflict", "remote", "device-b", 1),
        live("remote-clean", "remote", "device-b", 2),
    ];
    let objects = encrypted_snapshot(
        &crypto,
        &profile,
        &manifest(1, &remote_records),
        &remote_records,
        "\"etag-1\"",
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let transport = MemoryTransport::new(objects, events.clone());
    let applier = RecordingApplier {
        events: events.clone(),
        applied: Mutex::new(0),
        plans: Mutex::new(Vec::new()),
    };
    let rollbacks = MemoryRollbacks::default();
    let conflicts = WebDavConflictSource::default();
    let service = SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);

    let result = service
        .synchronize(
            PASSPHRASE,
            request(vec![
                live("conflict", "local", "device-a", 1),
                live("local-clean", "local", "device-a", 2),
            ]),
        )
        .await
        .unwrap();

    assert_eq!(result.committed_generation, 2);
    assert_eq!(result.attempts, 1);
    assert_eq!(result.conflicts, 1);
    assert_eq!(*applier.applied.lock().unwrap(), 1);
    let events = events.lock().unwrap();
    let commit = events
        .iter()
        .position(|event| event == "remote_manifest_write")
        .unwrap();
    let local = events
        .iter()
        .position(|event| event == "local_apply")
        .unwrap();
    assert!(commit < local, "local state changed before remote commit");
}

#[tokio::test]
async fn wrong_passphrase_and_invalid_manifest_stop_before_any_write() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let profile = crypto.create_profile().unwrap();
    let remote_records = vec![live("remote", "remote", "device-b", 1)];
    let objects = encrypted_snapshot(
        &crypto,
        &profile,
        &manifest(1, &remote_records),
        &remote_records,
        "\"etag-1\"",
    );

    #[derive(Clone, Copy)]
    enum FailureCase {
        WrongPassphrase,
        InvalidManifest,
        TamperedCiphertext,
    }

    for failure_case in [
        FailureCase::WrongPassphrase,
        FailureCase::InvalidManifest,
        FailureCase::TamperedCiphertext,
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let transport = MemoryTransport::new(objects.clone(), events.clone());
        match failure_case {
            FailureCase::InvalidManifest => {
                let session = crypto.unlock(PASSPHRASE, &profile).unwrap();
                let mut invalid = manifest(1, &remote_records);
                invalid.generation = 0;
                let envelope = session
                    .seal(
                        &SyncObjectIdentity::manifest(1).unwrap(),
                        &serde_json::to_vec(&invalid).unwrap(),
                    )
                    .unwrap();
                transport.objects.lock().unwrap().insert(
                    path_key(&sync_manifest_remote_path().unwrap()),
                    (
                        envelope.to_json_bytes().unwrap(),
                        Some(SyncEtag::new("\"etag-invalid\"").unwrap()),
                    ),
                );
            }
            FailureCase::TamperedCiphertext => {
                let path = path_key(&sync_manifest_remote_path().unwrap());
                let mut stored = transport.objects.lock().unwrap();
                let (bytes, _) = stored.get_mut(&path).unwrap();
                let envelope = SyncEncryptedEnvelope::from_json_bytes(bytes).unwrap();
                let mut ciphertext = envelope.ciphertext().as_bytes().to_vec();
                ciphertext[0] ^= 0x01;
                *bytes = envelope
                    .with_ciphertext(SyncCiphertext::new(ciphertext).unwrap())
                    .unwrap()
                    .to_json_bytes()
                    .unwrap();
            }
            FailureCase::WrongPassphrase => {}
        }
        let applier = RecordingApplier {
            events,
            applied: Mutex::new(0),
            plans: Mutex::new(Vec::new()),
        };
        let rollbacks = MemoryRollbacks::default();
        let conflicts = WebDavConflictSource::default();
        let service =
            SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);
        let (passphrase, expected_code) = match failure_case {
            FailureCase::WrongPassphrase => (
                &b"wrong passphrase"[..],
                SyncRunErrorCode::AuthenticationFailed,
            ),
            FailureCase::InvalidManifest => (PASSPHRASE, SyncRunErrorCode::InvalidRemote),
            FailureCase::TamperedCiphertext => (PASSPHRASE, SyncRunErrorCode::AuthenticationFailed),
        };

        let error = service
            .synchronize(passphrase, request(Vec::new()))
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code);
        assert_eq!(transport.write_count(), 0);
        assert_eq!(*applier.applied.lock().unwrap(), 0);
    }
}

#[tokio::test]
async fn exactly_one_cas_remerge_is_allowed_before_stopping_without_local_apply() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let profile = crypto.create_profile().unwrap();
    let first_records = vec![live("remote", "v1", "device-b", 1)];
    let second_records = vec![live("remote", "v2", "device-b", 2)];
    let first = encrypted_snapshot(
        &crypto,
        &profile,
        &manifest(1, &first_records),
        &first_records,
        "\"etag-1\"",
    );
    let second = encrypted_snapshot(
        &crypto,
        &profile,
        &manifest(2, &second_records),
        &second_records,
        "\"etag-2\"",
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let transport = MemoryTransport::new(first, events.clone());
    transport.queue_manifest_behavior(ManifestWriteBehavior::ReplaceAndFail(second));
    transport.queue_manifest_behavior(ManifestWriteBehavior::Fail);
    let applier = RecordingApplier {
        events: events.clone(),
        applied: Mutex::new(0),
        plans: Mutex::new(Vec::new()),
    };
    let rollbacks = MemoryRollbacks::default();
    let conflicts = WebDavConflictSource::default();
    let service = SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);

    let error = service
        .synchronize(
            PASSPHRASE,
            request(vec![live("local", "local", "device-a", 1)]),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, SyncRunErrorCode::ConcurrentWrite);
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.as_str() == "remote_manifest_write")
            .count(),
        2
    );
    assert_eq!(*applier.applied.lock().unwrap(), 0);
}

#[tokio::test]
async fn first_sync_preview_is_read_only_and_confirmation_fixes_identity_after_remote_commit() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let transport = MemoryTransport::new(BTreeMap::new(), events.clone());
    let applier = RecordingApplier {
        events,
        applied: Mutex::new(0),
        plans: Mutex::new(Vec::new()),
    };
    let rollbacks = MemoryRollbacks::default();
    let conflicts = WebDavConflictSource::default();
    let service = SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);
    let local_records = vec![live("local", "local", "device-new", 1)];

    let preview = service
        .preview_first_sync(
            PASSPHRASE,
            SyncFirstSyncPreviewRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-new"),
                display_name: "Workstation".to_string(),
                observed_at_ms: NOW,
                baselines: Vec::new(),
                local_records: local_records.clone(),
            },
        )
        .await
        .unwrap();

    assert_eq!(preview.remote_generation, 0);
    assert_eq!(preview.changes.additions, 1);
    assert_eq!(transport.write_count(), 0);
    assert_eq!(*applier.applied.lock().unwrap(), 0);

    let result = service
        .confirm_first_sync(
            PASSPHRASE,
            SyncFirstSyncConfirmRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-new"),
                display_name: "Workstation".to_string(),
                observed_at_ms: NOW,
                baselines: Vec::new(),
                local_records,
                expected_preview_token: preview.preview_token,
                existing_identity: None,
                confirmed_at_ms: NOW + 1,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.committed_generation, 1);
    assert!(transport.write_count() >= 2);
    let plans = applier.plans.lock().unwrap();
    assert_eq!(plans.len(), 1);
    let identity = plans[0].fixed_identity.as_ref().unwrap();
    assert_eq!(identity.device_id, device_id("device-new"));
    assert_eq!(plans[0].devices.len(), 1);
    assert_eq!(plans[0].devices[0].acknowledged_generation, 1);
}

#[tokio::test]
async fn device_listing_is_read_only_and_retirement_commits_target_bound_registry_change() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let profile = crypto.create_profile().unwrap();
    let objects = encrypted_snapshot(&crypto, &profile, &manifest(1, &[]), &[], "\"etag-1\"");
    let events = Arc::new(Mutex::new(Vec::new()));
    let transport = MemoryTransport::new(objects, events.clone());
    let applier = RecordingApplier {
        events,
        applied: Mutex::new(0),
        plans: Mutex::new(Vec::new()),
    };
    let rollbacks = MemoryRollbacks::default();
    let conflicts = WebDavConflictSource::default();
    let service = SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);

    let listed = service.list_devices(PASSPHRASE).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(transport.write_count(), 0);

    let result = service
        .retire_device(
            PASSPHRASE,
            SyncDeviceRetireRequest {
                schema_version: SyncSchemaVersion::V1,
                writer_device_id: device_id("device-a"),
                target_device_id: device_id("device-b"),
                confirmed_target_device_id: device_id("device-b"),
                retired_at_ms: NOW + 1,
            },
        )
        .await
        .unwrap();

    assert_eq!(result.committed_generation, 2);
    let listed = service.list_devices(PASSPHRASE).await.unwrap();
    let retired = listed
        .iter()
        .find(|device| device.device_id == device_id("device-b"))
        .unwrap();
    assert_eq!(retired.status, SyncDeviceStatus::Retired);
    assert_eq!(retired.retired_at_ms, Some(NOW + 1));
    let plans = applier.plans.lock().unwrap();
    assert_eq!(plans.len(), 1);
    assert!(plans[0].fixed_identity.is_none());
    assert!(plans[0].merge_batch.resolved.is_empty());
}

fn committed_baselines(
    plan: &wsl_code_switch_lib::domain::SyncLocalCommitPlan,
) -> Vec<SyncRecordBaseline> {
    plan.merge_batch
        .resolved
        .iter()
        .map(|resolution| SyncRecordBaseline {
            schema_version: SyncSchemaVersion::V1,
            confirmed_generation: plan.committed_generation,
            record: resolution.record.clone(),
        })
        .collect()
}

#[tokio::test]
async fn two_devices_add_modify_delete_and_converge_with_ciphertext_only_remote_objects() {
    let crypto = FixedSyncCryptoEngine::new(CounterRandom::default());
    let events = Arc::new(Mutex::new(Vec::new()));
    let transport = MemoryTransport::new(BTreeMap::new(), events.clone());
    let applier = RecordingApplier {
        events,
        applied: Mutex::new(0),
        plans: Mutex::new(Vec::new()),
    };
    let rollbacks = MemoryRollbacks::default();
    let conflicts = WebDavConflictSource::default();
    let service = SyncV3Orchestrator::new(&transport, &crypto, &applier, &rollbacks, &conflicts);
    let a1 = live("alpha", "alpha-private-v1", "device-a", 1);
    let b1 = live("beta", "beta-private-v1", "device-b", 1);

    let preview_a = service
        .preview_first_sync(
            PASSPHRASE,
            SyncFirstSyncPreviewRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-a"),
                display_name: "Device A".to_string(),
                observed_at_ms: NOW,
                baselines: Vec::new(),
                local_records: vec![a1.clone()],
            },
        )
        .await
        .unwrap();
    service
        .confirm_first_sync(
            PASSPHRASE,
            SyncFirstSyncConfirmRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-a"),
                display_name: "Device A".to_string(),
                observed_at_ms: NOW,
                baselines: Vec::new(),
                local_records: vec![a1.clone()],
                expected_preview_token: preview_a.preview_token,
                existing_identity: None,
                confirmed_at_ms: NOW + 1,
            },
        )
        .await
        .unwrap();
    let baseline_a = committed_baselines(&applier.plans.lock().unwrap()[0]);

    let preview_b = service
        .preview_first_sync(
            PASSPHRASE,
            SyncFirstSyncPreviewRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-b"),
                display_name: "Device B".to_string(),
                observed_at_ms: NOW + 2,
                baselines: Vec::new(),
                local_records: vec![b1.clone()],
            },
        )
        .await
        .unwrap();
    service
        .confirm_first_sync(
            PASSPHRASE,
            SyncFirstSyncConfirmRequest {
                schema_version: SyncSchemaVersion::V1,
                candidate_device_id: device_id("device-b"),
                display_name: "Device B".to_string(),
                observed_at_ms: NOW + 2,
                baselines: Vec::new(),
                local_records: vec![b1.clone()],
                expected_preview_token: preview_b.preview_token,
                existing_identity: None,
                confirmed_at_ms: NOW + 3,
            },
        )
        .await
        .unwrap();
    let baseline_b = committed_baselines(&applier.plans.lock().unwrap()[1]);

    let a2 = live("alpha", "alpha-private-v2", "device-a", 2);
    let result_a = service
        .synchronize(
            PASSPHRASE,
            SyncRunRequest {
                schema_version: SyncSchemaVersion::V1,
                device_id: device_id("device-a"),
                now_ms: NOW + 4,
                baselines: baseline_a,
                local_records: vec![a2.clone()],
            },
        )
        .await
        .unwrap();
    assert_eq!(result_a.conflicts, 0);
    let baseline_a = committed_baselines(&applier.plans.lock().unwrap()[2]);

    let beta_deleted =
        SyncRecord::deleted(b1.id.clone(), device_id("device-b"), 2, NOW + 5, 3).unwrap();
    let result_b = service
        .synchronize(
            PASSPHRASE,
            SyncRunRequest {
                schema_version: SyncSchemaVersion::V1,
                device_id: device_id("device-b"),
                now_ms: NOW + 5,
                baselines: baseline_b,
                local_records: vec![a1, beta_deleted.clone()],
            },
        )
        .await
        .unwrap();
    assert_eq!(result_b.conflicts, 0);

    let final_result = service
        .synchronize(
            PASSPHRASE,
            SyncRunRequest {
                schema_version: SyncSchemaVersion::V1,
                device_id: device_id("device-a"),
                now_ms: NOW + 6,
                baselines: baseline_a,
                local_records: vec![a2, b1],
            },
        )
        .await
        .unwrap();
    assert_eq!(final_result.conflicts, 0);
    assert_eq!(final_result.committed_generation, 5);
    let plans = applier.plans.lock().unwrap();
    let final_records = &plans.last().unwrap().merge_batch.resolved;
    assert!(final_records.iter().any(|resolution| {
        resolution.record.id.key == "beta" && resolution.record == beta_deleted
    }));
    drop(plans);

    for (bytes, _) in transport.objects.lock().unwrap().values() {
        let encoded = String::from_utf8_lossy(bytes);
        for forbidden in [
            "alpha-private-v1",
            "alpha-private-v2",
            "beta-private-v1",
            "serverConfig",
            "rawSession",
            "credentials",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }
}

#[test]
fn orchestration_source_stays_outside_tauri_database_and_settings() {
    let source = include_str!("../src/services/sync_v3.rs").to_ascii_lowercase();
    for forbidden in ["tauri", "rusqlite", "database", "appstate", "settings"] {
        assert!(
            !source.contains(forbidden),
            "sync-v3 orchestration gained infrastructure dependency: {forbidden}"
        );
    }
}
