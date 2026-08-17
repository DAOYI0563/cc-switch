use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::json;
use serial_test::serial;
use sha2::{Digest, Sha256};
use wsl_code_switch_lib::domain::{
    FixedSyncDeviceIdentity, LocalSkill, ManagedClientApps, ManagedClientId, PortableDomain,
    PortablePayload, PortableRecordId, RollbackPointMetadata, RollbackPointPurpose,
    RollbackPointState, SyncDevice, SyncDeviceId, SyncDeviceStatus, SyncLocalCommitPlan,
    SyncMergeBatch, SyncMergeResolution, SyncMergeSideAction, SyncRecord, SyncSchemaVersion,
};
use wsl_code_switch_lib::ports::{
    SyncLocalApplyPort, TemporaryRollbackError, TemporaryRollbackErrorCode, TemporaryRollbackStore,
};
use wsl_code_switch_lib::{
    apply_committed_sync_batch, AppState, Database, McpServer, RuntimeSyncLocalAdapter,
};

#[path = "support.rs"]
mod support;
use support::{ensure_test_home, reset_test_fs, test_mutex};

const NOW: i64 = 1_800_000_000_000;

fn device_id() -> SyncDeviceId {
    SyncDeviceId::new("device-a").unwrap()
}

fn device(generation: u64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(),
        display_name: "Workstation".to_string(),
        acknowledged_generation: generation,
        registered_at_ms: NOW,
        last_seen_at_ms: NOW,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

fn skill_dir(home: &Path, client: ManagedClientId, directory: &str) -> PathBuf {
    match client {
        ManagedClientId::Claude => home.join(".claude/skills").join(directory),
        ManagedClientId::Codex => home.join(".codex/skills").join(directory),
        ManagedClientId::Opencode => home.join(".config/opencode/skills").join(directory),
    }
}

fn tree_hash(path: &str, contents: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"F\0");
    hasher.update((path.len() as u64).to_le_bytes());
    hasher.update(path.as_bytes());
    hasher.update((contents.len() as u64).to_le_bytes());
    hasher.update(contents);
    format!("{:x}", hasher.finalize())
}

fn skill_record(id: &str, apps: ManagedClientApps, contents: &[u8]) -> LocalSkill {
    LocalSkill {
        id: id.to_string(),
        name: "Synced Skill".to_string(),
        description: None,
        directory: "synced-skill".to_string(),
        content_hash: Some(tree_hash("SKILL.md", contents)),
        total_size_bytes: contents.len() as u64,
        file_count: 1,
        apps,
        cloud_eligible: true,
        created_at_ms: NOW,
        updated_at_ms: NOW,
    }
}

fn live_skill_record(skill: &LocalSkill, contents: &[u8], counter: u64) -> SyncRecord {
    SyncRecord::live(
        PortableRecordId::new(PortableDomain::Skill, skill.id.clone()).unwrap(),
        device_id(),
        counter,
        NOW + counter as i64,
        PortablePayload::new(
            PortableDomain::Skill,
            json!({
                "id": skill.id,
                "name": skill.name,
                "directory": skill.directory,
                "contentHash": skill.content_hash,
                "totalSizeBytes": skill.total_size_bytes,
                "fileCount": skill.file_count,
                "apps": skill.apps,
                "cloudEligible": skill.cloud_eligible,
                "createdAtMs": skill.created_at_ms,
                "updatedAtMs": skill.updated_at_ms,
                "files": {
                    "directories": [],
                    "entries": [{
                        "path": "SKILL.md",
                        "contentBase64": BASE64.encode(contents)
                    }]
                }
            }),
        )
        .unwrap(),
    )
    .unwrap()
}

fn skill_plan(record: SyncRecord, generation: u64) -> SyncLocalCommitPlan {
    SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: generation,
        fixed_identity: None,
        devices: vec![device(generation)],
        merge_batch: SyncMergeBatch {
            schema_version: SyncSchemaVersion::V1,
            resolved: vec![SyncMergeResolution {
                schema_version: SyncSchemaVersion::V1,
                record,
                local_action: SyncMergeSideAction::ApplyMerged,
                remote_action: SyncMergeSideAction::Unchanged,
            }],
            conflicts: Vec::new(),
        },
    }
}

#[derive(Default)]
struct FailingCleanupRollbacks {
    point: Mutex<Option<RollbackPointMetadata>>,
    deleted: Mutex<usize>,
    retained: Mutex<usize>,
}

impl TemporaryRollbackStore for FailingCleanupRollbacks {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let point = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id: "runtime-sync-point".to_string(),
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: "a".repeat(64),
        };
        *self.point.lock().unwrap() = Some(point.clone());
        Ok(point)
    }

    fn restore(&self, _id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        Ok(Vec::new())
    }

    fn delete_after_success(&self, _id: &str) -> Result<(), TemporaryRollbackError> {
        *self.deleted.lock().unwrap() += 1;
        Err(TemporaryRollbackError::new(
            TemporaryRollbackErrorCode::Io,
            "injected runtime sync rollback delete failure",
        ))
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        *self.retained.lock().unwrap() += 1;
        let mut point = self.point.lock().unwrap();
        let point = point.as_mut().ok_or_else(|| {
            TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::NotFound,
                "missing runtime sync rollback point",
            )
        })?;
        assert_eq!(point.id, id);
        point.state = RollbackPointState::Failed;
        point.failed_at_ms = Some(failed_at_ms);
        Ok(point.clone())
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        Ok(self.point.lock().unwrap().iter().cloned().collect())
    }
}

#[test]
fn mcp_snapshot_omits_credentials_and_remote_apply_preserves_local_credentials() {
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_mcp_server(&McpServer {
            id: "server-a".to_string(),
            name: "Before".to_string(),
            server: json!({
                "command": "uvx",
                "args": ["fixture"],
                "env": {"PRIVATE_TOKEN": "never-upload"},
                "headers": {"Authorization": "Bearer never-upload"}
            }),
            apps: ManagedClientApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        })
        .unwrap();
    let state = AppState::new(database.clone());
    let adapter = RuntimeSyncLocalAdapter::new(&state);

    let snapshot = adapter.snapshot(&device_id(), NOW).unwrap();
    assert!(snapshot
        .local_records
        .iter()
        .any(|record| record.id.domain == PortableDomain::Mcp && record.id.key == "server-a"));
    let encoded = serde_json::to_string(&snapshot.local_records).unwrap();
    assert!(!encoded.contains("never-upload"));
    assert!(!encoded.contains("\"env\""));
    assert!(!encoded.contains("\"headers\""));

    let incoming = SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, "server-a").unwrap(),
        device_id(),
        2,
        NOW + 1,
        PortablePayload::new(
            PortableDomain::Mcp,
            json!({
                "id": "server-a",
                "name": "After",
                "serverConfig": {"command": "uvx", "args": ["updated"]},
                "apps": {"claude": false, "codex": false, "opencode": false}
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let plan = SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: 2,
        fixed_identity: None,
        devices: vec![device(2)],
        merge_batch: SyncMergeBatch {
            schema_version: SyncSchemaVersion::V1,
            resolved: vec![SyncMergeResolution {
                schema_version: SyncSchemaVersion::V1,
                record: incoming,
                local_action: SyncMergeSideAction::ApplyMerged,
                remote_action: SyncMergeSideAction::Unchanged,
            }],
            conflicts: Vec::new(),
        },
    };

    adapter.apply_and_validate(&plan).unwrap();

    let stored = database
        .get_all_mcp_servers()
        .unwrap()
        .shift_remove("server-a")
        .unwrap();
    assert_eq!(stored.name, "After");
    assert_eq!(stored.server["args"], json!(["updated"]));
    assert_eq!(stored.server["env"]["PRIVATE_TOKEN"], "never-upload");
    assert_eq!(
        stored.server["headers"]["Authorization"],
        "Bearer never-upload"
    );
    assert_eq!(database.load_sync_baselines().unwrap().len(), 1);
}

#[test]
#[serial]
fn synchronized_skill_registers_old_and_new_enabled_client_union_after_success() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = ensure_test_home();
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database.clone());
    let old_contents = b"---\nname: Synced Skill\n---\nold\n";
    let old = skill_record(
        "sync-skill",
        ManagedClientApps {
            claude: true,
            codex: true,
            opencode: false,
        },
        old_contents,
    );
    database
        .save_core_skills(std::slice::from_ref(&old))
        .unwrap();
    for client in old.apps.enabled_clients() {
        let root = skill_dir(home, client, &old.directory);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), old_contents).unwrap();
    }
    let new_contents = b"---\nname: Synced Skill\n---\nnew\n";
    let incoming = skill_record(
        "sync-skill",
        ManagedClientApps {
            claude: false,
            codex: true,
            opencode: true,
        },
        new_contents,
    );
    let plan = skill_plan(live_skill_record(&incoming, new_contents, 2), 2);

    RuntimeSyncLocalAdapter::new(&state)
        .apply_and_validate(&plan)
        .unwrap();

    assert_eq!(state.local_scan_writes.pending_count(), 3);
    assert_eq!(state.local_scan_writes.last_generation(), 3);
    assert!(!skill_dir(home, ManagedClientId::Claude, "synced-skill").exists());
    assert!(skill_dir(home, ManagedClientId::Codex, "synced-skill").is_dir());
    assert!(skill_dir(home, ManagedClientId::Opencode, "synced-skill").is_dir());
}

#[test]
#[serial]
fn committed_runtime_sync_survives_rollback_cleanup_failure_after_self_write_registration() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = ensure_test_home();
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database.clone());
    let contents = b"---\nname: Synced Skill\n---\ncommitted\n";
    let incoming = skill_record(
        "sync-committed",
        ManagedClientApps::only(ManagedClientId::Claude),
        contents,
    );
    let plan = skill_plan(live_skill_record(&incoming, contents, 1), 1);
    let rollbacks = FailingCleanupRollbacks::default();

    apply_committed_sync_batch(
        &RuntimeSyncLocalAdapter::new(&state),
        &rollbacks,
        NOW,
        &plan,
    )
    .expect("committed runtime sync must survive rollback cleanup failure");

    assert!(database.get_core_skill("sync-committed").unwrap().is_some());
    assert_eq!(database.load_sync_baselines().unwrap().len(), 1);
    assert_eq!(state.local_scan_writes.pending_count(), 1);
    assert_eq!(state.local_scan_writes.last_generation(), 1);
    assert!(skill_dir(home, ManagedClientId::Claude, "synced-skill").is_dir());
    assert_eq!(*rollbacks.deleted.lock().unwrap(), 1);
    assert_eq!(*rollbacks.retained.lock().unwrap(), 1);
    let point = rollbacks.point.lock().unwrap();
    let point = point.as_ref().unwrap();
    assert_eq!(point.state, RollbackPointState::Failed);
    assert_eq!(point.failed_at_ms, Some(NOW));
}

#[test]
#[serial]
fn synchronized_skill_deletion_registers_every_previously_enabled_client() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let home = ensure_test_home();
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database.clone());
    let contents = b"---\nname: Synced Skill\n---\nold\n";
    let old = skill_record(
        "sync-delete",
        ManagedClientApps {
            claude: true,
            codex: false,
            opencode: true,
        },
        contents,
    );
    database
        .save_core_skills(std::slice::from_ref(&old))
        .unwrap();
    for client in old.apps.enabled_clients() {
        let root = skill_dir(home, client, &old.directory);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), contents).unwrap();
    }
    let deleted = SyncRecord::deleted(
        PortableRecordId::new(PortableDomain::Skill, old.id.clone()).unwrap(),
        device_id(),
        2,
        NOW + 2,
        2,
    )
    .unwrap();

    RuntimeSyncLocalAdapter::new(&state)
        .apply_and_validate(&skill_plan(deleted, 2))
        .unwrap();

    assert_eq!(state.local_scan_writes.pending_count(), 2);
    assert_eq!(state.local_scan_writes.last_generation(), 2);
    assert!(database.get_core_skill(&old.id).unwrap().is_none());
    assert!(!skill_dir(home, ManagedClientId::Claude, "synced-skill").exists());
    assert!(!skill_dir(home, ManagedClientId::Opencode, "synced-skill").exists());
}

#[test]
#[serial]
fn failed_post_commit_validation_registers_no_skill_expectations() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database);
    let first_contents = b"---\nname: Synced Skill\n---\nfirst\n";
    let first = skill_record(
        "sync-duplicate",
        ManagedClientApps::only(ManagedClientId::Claude),
        first_contents,
    );
    let second_contents = b"---\nname: Synced Skill\n---\nsecond\n";
    let second = skill_record(
        "sync-duplicate",
        ManagedClientApps::only(ManagedClientId::Codex),
        second_contents,
    );
    let mut plan = skill_plan(live_skill_record(&first, first_contents, 1), 2);
    plan.merge_batch.resolved.push(SyncMergeResolution {
        schema_version: SyncSchemaVersion::V1,
        record: live_skill_record(&second, second_contents, 2),
        local_action: SyncMergeSideAction::ApplyMerged,
        remote_action: SyncMergeSideAction::Unchanged,
    });

    RuntimeSyncLocalAdapter::new(&state)
        .apply_and_validate(&plan)
        .expect_err("duplicate resolved identity must fail final metadata validation");

    assert_eq!(state.local_scan_writes.pending_count(), 0);
    assert_eq!(state.local_scan_writes.last_generation(), 0);
}

#[test]
#[serial]
fn failed_sync_metadata_commit_registers_no_skill_expectations() {
    let _guard = test_mutex().lock().unwrap();
    reset_test_fs();
    let database = Arc::new(Database::memory().unwrap());
    let state = AppState::new(database.clone());
    let fixed = FixedSyncDeviceIdentity {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(),
        display_name: "Original".to_string(),
        fixed_at_ms: NOW,
    };
    let seed = SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: 1,
        fixed_identity: Some(fixed),
        devices: vec![device(1)],
        merge_batch: SyncMergeBatch {
            schema_version: SyncSchemaVersion::V1,
            resolved: Vec::new(),
            conflicts: Vec::new(),
        },
    };
    database.commit_sync_metadata(&seed).unwrap();

    let contents = b"---\nname: Synced Skill\n---\nincoming\n";
    let incoming = skill_record(
        "sync-failed",
        ManagedClientApps::only(ManagedClientId::Claude),
        contents,
    );
    let mut plan = skill_plan(live_skill_record(&incoming, contents, 2), 2);
    plan.fixed_identity = Some(FixedSyncDeviceIdentity {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(),
        display_name: "Replacement".to_string(),
        fixed_at_ms: NOW,
    });

    RuntimeSyncLocalAdapter::new(&state)
        .apply_and_validate(&plan)
        .expect_err("fixed identity replacement must fail after local apply");

    assert_eq!(state.local_scan_writes.pending_count(), 0);
    assert_eq!(state.local_scan_writes.last_generation(), 0);
}
