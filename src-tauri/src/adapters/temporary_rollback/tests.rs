use std::fs;
use std::sync::{Arc, Barrier};

use sha2::{Digest, Sha256};

use super::*;
use crate::ports::{
    LocalProtectionError, LocalProtectionErrorCode, LocalProtectionPurpose, LocalProtector,
};

#[derive(Debug, Clone)]
struct AuthenticatedTestProtector {
    key: Vec<u8>,
}

impl AuthenticatedTestProtector {
    fn new(key: &[u8]) -> Self {
        Self { key: key.to_vec() }
    }

    fn stream(&self, purpose: LocalProtectionPurpose) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"temporary-rollback-test-protector\0");
        hasher.update(&self.key);
        hasher.update(purpose.as_str().as_bytes());
        hasher.finalize().into()
    }
}

impl LocalProtector for AuthenticatedTestProtector {
    fn protect(
        &self,
        purpose: LocalProtectionPurpose,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        let stream = self.stream(purpose);
        let encrypted: Vec<_> = plaintext
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ stream[index % stream.len()])
            .collect();
        let mut hasher = Sha256::new();
        hasher.update(b"tag\0");
        hasher.update(&self.key);
        hasher.update(purpose.as_str().as_bytes());
        hasher.update(&encrypted);
        let mut output = hasher.finalize().to_vec();
        output.extend(encrypted);
        Ok(output)
    }

    fn unprotect(
        &self,
        purpose: LocalProtectionPurpose,
        protected: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        let (tag, encrypted) = protected.split_at_checked(32).ok_or_else(|| {
            LocalProtectionError::new(
                LocalProtectionErrorCode::UnprotectFailed,
                "test protected value is truncated",
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(b"tag\0");
        hasher.update(&self.key);
        hasher.update(purpose.as_str().as_bytes());
        hasher.update(encrypted);
        if hasher.finalize().as_slice() != tag {
            return Err(LocalProtectionError::new(
                LocalProtectionErrorCode::UnprotectFailed,
                "test protected value authentication failed",
            ));
        }
        let stream = self.stream(purpose);
        Ok(encrypted
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ stream[index % stream.len()])
            .collect())
    }
}

#[derive(Debug, Clone, Copy)]
struct FailingProtector;

impl LocalProtector for FailingProtector {
    fn protect(
        &self,
        _purpose: LocalProtectionPurpose,
        _plaintext: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        Err(LocalProtectionError::new(
            LocalProtectionErrorCode::ProtectFailed,
            "injected protection failure",
        ))
    }

    fn unprotect(
        &self,
        _purpose: LocalProtectionPurpose,
        _protected: &[u8],
    ) -> Result<Vec<u8>, LocalProtectionError> {
        unreachable!("failing protector never persists data")
    }
}

fn store(root: &Path) -> FixedTemporaryRollbackStore<AuthenticatedTestProtector> {
    FixedTemporaryRollbackStore::new(root, AuthenticatedTestProtector::new(b"fixture-key"))
}

fn rollback_files(root: &Path) -> Vec<PathBuf> {
    if !root.is_dir() {
        return Vec::new();
    }
    let mut paths = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some(FILE_EXTENSION))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn creates_encrypted_envelope_and_restores_empty_or_nonempty_payloads() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    let store = store(&root);
    let secret = b"rollback-plaintext-marker-f7a2";

    let point = store
        .create(RollbackPointPurpose::DataMigration, 100, secret)
        .unwrap();
    let empty = store
        .create(RollbackPointPurpose::WebdavSync, 101, b"")
        .unwrap();

    assert_eq!(point.state, RollbackPointState::Pending);
    assert_eq!(point.failed_at_ms, None);
    assert_eq!(point.payload_size_bytes, secret.len() as u64);
    assert_eq!(store.restore(&point.id).unwrap(), secret);
    assert_eq!(store.restore(&empty.id).unwrap(), b"");
    let disk = fs::read(store.path_for(&point.id).unwrap()).unwrap();
    assert!(!disk.windows(secret.len()).any(|window| window == secret));
    assert!(!String::from_utf8_lossy(&disk).contains("rollback-plaintext-marker-f7a2"));

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|item| item.payload_sha256.len() == 64));
}

#[test]
fn successful_operation_deletes_its_rollback_point_immediately() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    let point = store
        .create(RollbackPointPurpose::RestoreOperation, 100, b"before")
        .unwrap();

    store.delete_after_success(&point.id).unwrap();

    assert!(store.list().unwrap().is_empty());
    assert_eq!(
        store.restore(&point.id).unwrap_err().code,
        TemporaryRollbackErrorCode::NotFound
    );
}

#[test]
fn failed_operations_keep_only_the_three_most_recent_failed_points() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    let mut ids = Vec::new();
    for timestamp in 100..104 {
        let point = store
            .create(
                RollbackPointPurpose::ConflictResolution,
                timestamp,
                format!("payload-{timestamp}").as_bytes(),
            )
            .unwrap();
        store
            .retain_after_failure(&point.id, timestamp + 1000)
            .unwrap();
        ids.push(point.id);
    }

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), MAX_ROLLBACK_POINTS);
    assert!(listed
        .iter()
        .all(|point| point.state == RollbackPointState::Failed));
    assert_eq!(
        store.restore(&ids[0]).unwrap_err().code,
        TemporaryRollbackErrorCode::NotFound
    );
    for id in &ids[1..] {
        assert!(store.restore(id).is_ok(), "retained {id}");
    }
}

#[test]
fn create_counts_pending_and_failed_points_and_evicts_the_oldest_failed_point() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    let oldest_failed = store
        .create(RollbackPointPurpose::DataMigration, 100, b"oldest-failed")
        .unwrap();
    store
        .retain_after_failure(&oldest_failed.id, 1_000)
        .unwrap();
    let crash_left_pending = store
        .create(RollbackPointPurpose::WebdavSync, 200, b"pending")
        .unwrap();
    let newest_failed = store
        .create(
            RollbackPointPurpose::ConflictResolution,
            300,
            b"newest-failed",
        )
        .unwrap();
    store
        .retain_after_failure(&newest_failed.id, 1_300)
        .unwrap();

    let replacement = store
        .create(RollbackPointPurpose::RestoreOperation, 400, b"replacement")
        .unwrap();

    let listed = store.list().unwrap();
    assert_eq!(listed.len(), MAX_ROLLBACK_POINTS);
    assert!(listed.iter().any(|point| point.id == crash_left_pending.id));
    assert!(listed.iter().any(|point| point.id == newest_failed.id));
    assert!(listed.iter().any(|point| point.id == replacement.id));
    assert_eq!(
        store.restore(&oldest_failed.id).unwrap_err().code,
        TemporaryRollbackErrorCode::NotFound
    );
    assert_eq!(rollback_files(store.root()).len(), MAX_ROLLBACK_POINTS);
}

#[test]
fn crash_left_pending_points_fill_capacity_and_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    let original = store(&root);
    let mut pending_ids = Vec::new();
    for timestamp in 100..103 {
        pending_ids.push(
            original
                .create(
                    RollbackPointPurpose::DataMigration,
                    timestamp,
                    format!("pending-{timestamp}").as_bytes(),
                )
                .unwrap()
                .id,
        );
    }
    drop(original);

    let reopened = store(&root);
    let error = reopened
        .create(RollbackPointPurpose::WebdavSync, 200, b"must-not-persist")
        .unwrap_err();

    assert_eq!(error.code, TemporaryRollbackErrorCode::InvalidState);
    assert_eq!(error.context.get("pendingPoints"), Some(&"3".to_string()));
    let listed = reopened.list().unwrap();
    assert_eq!(listed.len(), MAX_ROLLBACK_POINTS);
    assert!(listed
        .iter()
        .all(|point| point.state == RollbackPointState::Pending));
    assert!(pending_ids
        .iter()
        .all(|id| listed.iter().any(|point| &point.id == id)));
    assert_eq!(rollback_files(&root).len(), MAX_ROLLBACK_POINTS);
}

#[test]
fn concurrent_creates_across_store_instances_never_exceed_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    let barrier = Arc::new(Barrier::new(9));
    let mut threads = Vec::new();
    for timestamp in 100..108 {
        let root = root.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(std::thread::spawn(move || {
            let store = store(&root);
            barrier.wait();
            store.create(
                RollbackPointPurpose::ConflictResolution,
                timestamp,
                format!("concurrent-{timestamp}").as_bytes(),
            )
        }));
    }
    barrier.wait();

    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 3);
    assert!(results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .all(|error| error.code == TemporaryRollbackErrorCode::InvalidState));

    let reopened = store(&root);
    let listed = reopened.list().unwrap();
    assert_eq!(listed.len(), MAX_ROLLBACK_POINTS);
    assert!(listed
        .iter()
        .all(|point| point.state == RollbackPointState::Pending));
    assert_eq!(rollback_files(&root).len(), MAX_ROLLBACK_POINTS);
}

#[test]
fn tampered_envelope_fails_before_any_plaintext_is_returned() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    let point = store
        .create(RollbackPointPurpose::RemoteReset, 100, b"original")
        .unwrap();
    let path = store.path_for(&point.id).unwrap();
    let bytes = fs::read(&path).unwrap();
    let mut envelope: RollbackEnvelope = serde_json::from_slice(&bytes).unwrap();
    let mut protected = BASE64.decode(&envelope.protected_payload).unwrap();
    let middle = protected.len() / 2;
    protected[middle] ^= 1;
    envelope.protected_payload = BASE64.encode(protected);
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();

    assert_eq!(
        store.restore(&point.id).unwrap_err().code,
        TemporaryRollbackErrorCode::Integrity
    );
}

#[test]
fn a_different_protector_key_cannot_restore_the_point() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    let original = store(&root);
    let point = original
        .create(RollbackPointPurpose::DataMigration, 100, b"original")
        .unwrap();
    let wrong = FixedTemporaryRollbackStore::new(
        &root,
        AuthenticatedTestProtector::new(b"wrong-fixture-key"),
    );

    assert_eq!(
        wrong.restore(&point.id).unwrap_err().code,
        TemporaryRollbackErrorCode::Protection
    );
}

#[test]
fn traversal_and_malformed_ids_are_rejected_without_filesystem_access() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    for id in ["../outside", "", "A123", "0123456789abcdef"] {
        assert_eq!(
            store.restore(id).unwrap_err().code,
            TemporaryRollbackErrorCode::InvalidId,
            "{id}"
        );
    }
    assert!(!store.root().exists());
}

#[test]
fn protection_or_write_failure_leaves_no_partial_rollback_file() {
    let temp = tempfile::tempdir().unwrap();
    let protection_root = temp.path().join("protect-failure");
    let failing = FixedTemporaryRollbackStore::new(&protection_root, FailingProtector);
    assert_eq!(
        failing
            .create(RollbackPointPurpose::DataMigration, 100, b"payload")
            .unwrap_err()
            .code,
        TemporaryRollbackErrorCode::Protection
    );
    assert!(!protection_root.exists());

    let file_root = temp.path().join("root-is-file");
    fs::write(&file_root, b"existing").unwrap();
    let unwritable = store(&file_root);
    assert_eq!(
        unwritable
            .create(RollbackPointPurpose::DataMigration, 100, b"payload")
            .unwrap_err()
            .code,
        TemporaryRollbackErrorCode::Io
    );
    assert_eq!(fs::read(&file_root).unwrap(), b"existing");
    assert!(rollback_files(temp.path()).is_empty());
}

#[test]
fn invalid_failure_timestamp_preserves_the_pending_point() {
    let temp = tempfile::tempdir().unwrap();
    let store = store(&temp.path().join(DIRECTORY_NAME));
    let point = store
        .create(RollbackPointPurpose::DataMigration, 100, b"payload")
        .unwrap();

    assert_eq!(
        store.retain_after_failure(&point.id, 99).unwrap_err().code,
        TemporaryRollbackErrorCode::InvalidState
    );
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, RollbackPointState::Pending);
    assert_eq!(store.restore(&point.id).unwrap(), b"payload");
}

#[cfg(unix)]
#[test]
fn symlinked_rollback_directory_is_rejected() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    symlink(outside.path(), &root).unwrap();
    let store = store(&root);

    assert_eq!(
        store
            .create(RollbackPointPurpose::DataMigration, 100, b"payload")
            .unwrap_err()
            .code,
        TemporaryRollbackErrorCode::LinkNotAllowed
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[cfg(target_os = "windows")]
#[test]
fn windows_junction_rollback_directory_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = temp.path().join(DIRECTORY_NAME);
    let output = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&root)
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let store = store(&root);

    assert_eq!(
        store
            .create(RollbackPointPurpose::DataMigration, 100, b"payload")
            .unwrap_err()
            .code,
        TemporaryRollbackErrorCode::LinkNotAllowed
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
