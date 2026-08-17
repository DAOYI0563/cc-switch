use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::{RollbackPointMetadata, RollbackPointPurpose, RollbackPointState};
use crate::ports::{
    LocalProtectionPurpose, LocalProtector, TemporaryRollbackError, TemporaryRollbackErrorCode,
    TemporaryRollbackStore,
};

use super::local_protection::WindowsDpapiProtector;

const DIRECTORY_NAME: &str = "temporary-rollbacks";
const FILE_EXTENSION: &str = "rollback";
const ENVELOPE_SCHEMA_VERSION: u32 = 1;
const PROTECTED_PAYLOAD_SCHEMA_VERSION: u32 = 1;
const MAX_ROLLBACK_POINTS: usize = 3;
const MAX_ENVELOPE_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RollbackEnvelope {
    schema_version: u32,
    protected_payload_sha256: String,
    protected_payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtectedRollbackPayload {
    schema_version: u32,
    metadata: RollbackPointMetadata,
    payload: String,
}

#[derive(Debug, Clone)]
pub struct FixedTemporaryRollbackStore<P = WindowsDpapiProtector> {
    root: PathBuf,
    protector: P,
}

impl FixedTemporaryRollbackStore<WindowsDpapiProtector> {
    pub fn runtime() -> Self {
        Self::new(
            crate::config::get_app_config_dir().join(DIRECTORY_NAME),
            WindowsDpapiProtector,
        )
    }
}

impl<P> FixedTemporaryRollbackStore<P> {
    pub fn new(root: impl Into<PathBuf>, protector: P) -> Self {
        Self {
            root: root.into(),
            protector,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl<P: LocalProtector> FixedTemporaryRollbackStore<P> {
    fn path_for(&self, id: &str) -> Result<PathBuf, TemporaryRollbackError> {
        validate_id(id)?;
        Ok(self.root.join(format!("{id}.{FILE_EXTENSION}")))
    }

    fn encode(
        &self,
        metadata: RollbackPointMetadata,
        payload: &[u8],
    ) -> Result<Vec<u8>, TemporaryRollbackError> {
        metadata.validate().map_err(|error| {
            TemporaryRollbackError::new(TemporaryRollbackErrorCode::InvalidState, error.to_string())
        })?;
        let protected_input = serde_json::to_vec(&ProtectedRollbackPayload {
            schema_version: PROTECTED_PAYLOAD_SCHEMA_VERSION,
            metadata,
            payload: BASE64.encode(payload),
        })
        .map_err(serialization_error)?;
        let protected = self
            .protector
            .protect(LocalProtectionPurpose::TemporaryRollback, &protected_input)
            .map_err(protection_error)?;
        let envelope = RollbackEnvelope {
            schema_version: ENVELOPE_SCHEMA_VERSION,
            protected_payload_sha256: sha256(&protected),
            protected_payload: BASE64.encode(protected),
        };
        serde_json::to_vec(&envelope).map_err(serialization_error)
    }

    fn decode(
        &self,
        expected_id: &str,
        bytes: &[u8],
    ) -> Result<(RollbackPointMetadata, Vec<u8>), TemporaryRollbackError> {
        let envelope: RollbackEnvelope =
            serde_json::from_slice(bytes).map_err(serialization_error)?;
        if envelope.schema_version != ENVELOPE_SCHEMA_VERSION {
            return Err(integrity_error("unsupported rollback envelope version"));
        }
        validate_sha256(&envelope.protected_payload_sha256)?;
        let protected = BASE64
            .decode(envelope.protected_payload)
            .map_err(|_| integrity_error("rollback envelope contains invalid protected data"))?;
        if sha256(&protected) != envelope.protected_payload_sha256 {
            return Err(integrity_error("rollback protected data digest mismatch"));
        }
        let unprotected = self
            .protector
            .unprotect(LocalProtectionPurpose::TemporaryRollback, &protected)
            .map_err(protection_error)?;
        let inner: ProtectedRollbackPayload =
            serde_json::from_slice(&unprotected).map_err(serialization_error)?;
        if inner.schema_version != PROTECTED_PAYLOAD_SCHEMA_VERSION {
            return Err(integrity_error(
                "unsupported protected rollback payload version",
            ));
        }
        inner.metadata.validate().map_err(|error| {
            TemporaryRollbackError::new(TemporaryRollbackErrorCode::Integrity, error.to_string())
        })?;
        if inner.metadata.id != expected_id {
            return Err(integrity_error("rollback point identity mismatch"));
        }
        let payload = BASE64
            .decode(inner.payload)
            .map_err(|_| integrity_error("rollback payload is not valid base64"))?;
        if payload.len() as u64 != inner.metadata.payload_size_bytes
            || sha256(&payload) != inner.metadata.payload_sha256
        {
            return Err(integrity_error("rollback payload integrity check failed"));
        }
        Ok((inner.metadata, payload))
    }

    fn read(&self, id: &str) -> Result<(RollbackPointMetadata, Vec<u8>), TemporaryRollbackError> {
        let path = self.path_for(id)?;
        inspect_no_links(&self.root)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                TemporaryRollbackError::new(
                    TemporaryRollbackErrorCode::NotFound,
                    "temporary rollback point was not found",
                )
                .with_context("id", id)
            } else {
                io_error(&path, "inspect rollback point", error)
            }
        })?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(link_error(&path));
        }
        if !metadata.is_file() {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::Io,
                "temporary rollback point is not a regular file",
            )
            .with_context("id", id));
        }
        if metadata.len() > MAX_ENVELOPE_BYTES {
            return Err(integrity_error("rollback envelope exceeds its size limit"));
        }
        let bytes =
            fs::read(&path).map_err(|error| io_error(&path, "read rollback point", error))?;
        self.decode(id, &bytes)
    }

    fn write(
        &self,
        metadata: RollbackPointMetadata,
        payload: &[u8],
    ) -> Result<(), TemporaryRollbackError> {
        let id = metadata.id.clone();
        let bytes = self.encode(metadata, payload)?;
        self.write_encoded(&id, &bytes)
    }

    fn write_encoded(&self, id: &str, bytes: &[u8]) -> Result<(), TemporaryRollbackError> {
        let path = self.path_for(id)?;
        ensure_safe_directory(&self.root)?;
        match fs::symlink_metadata(&path) {
            Ok(existing) if metadata_is_link_or_reparse(&existing) => {
                return Err(link_error(&path));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(&path, "inspect rollback destination", error)),
        }
        crate::config::atomic_write(&path, bytes).map_err(|error| {
            TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::Io,
                format!("failed to atomically write temporary rollback point: {error}"),
            )
            .with_context("id", id)
        })?;
        inspect_no_links(&path)
    }

    fn remove(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        let path = self.path_for(id)?;
        inspect_no_links(&self.root)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                TemporaryRollbackError::new(
                    TemporaryRollbackErrorCode::NotFound,
                    "temporary rollback point was not found",
                )
                .with_context("id", id)
            } else {
                io_error(&path, "inspect rollback point before deletion", error)
            }
        })?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(link_error(&path));
        }
        if !metadata.is_file() {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::Io,
                "temporary rollback point is not a regular file",
            ));
        }
        fs::remove_file(&path).map_err(|error| io_error(&path, "delete rollback point", error))
    }

    fn reserve_create_slot(&self) -> Result<(), TemporaryRollbackError> {
        let points = self.list()?;
        let mut remaining = points.len();
        if remaining < MAX_ROLLBACK_POINTS {
            return Ok(());
        }

        let mut failed: Vec<_> = points
            .iter()
            .filter(|metadata| metadata.state == RollbackPointState::Failed)
            .collect();
        failed.sort_by(|left, right| {
            left.failed_at_ms
                .cmp(&right.failed_at_ms)
                .then_with(|| left.created_at_ms.cmp(&right.created_at_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        for metadata in failed {
            if remaining < MAX_ROLLBACK_POINTS {
                break;
            }
            self.remove(&metadata.id)?;
            remaining -= 1;
        }

        if remaining >= MAX_ROLLBACK_POINTS {
            let pending = points
                .iter()
                .filter(|metadata| metadata.state == RollbackPointState::Pending)
                .count();
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::InvalidState,
                "temporary rollback capacity is occupied by pending points",
            )
            .with_context("pendingPoints", pending.to_string())
            .with_context("maximumPoints", MAX_ROLLBACK_POINTS.to_string()));
        }

        Ok(())
    }

    fn prune_excess_points(&self) -> Result<(), TemporaryRollbackError> {
        let points = self.list()?;
        let mut remaining = points.len();
        if remaining <= MAX_ROLLBACK_POINTS {
            return Ok(());
        }

        let mut failed: Vec<_> = points
            .iter()
            .filter(|metadata| metadata.state == RollbackPointState::Failed)
            .collect();
        failed.sort_by(|left, right| {
            left.failed_at_ms
                .cmp(&right.failed_at_ms)
                .then_with(|| left.created_at_ms.cmp(&right.created_at_ms))
                .then_with(|| left.id.cmp(&right.id))
        });
        for metadata in failed {
            if remaining <= MAX_ROLLBACK_POINTS {
                break;
            }
            self.remove(&metadata.id)?;
            remaining -= 1;
        }

        if remaining > MAX_ROLLBACK_POINTS {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::InvalidState,
                "temporary rollback capacity is exceeded by pending points",
            )
            .with_context("remainingPoints", remaining.to_string())
            .with_context("maximumPoints", MAX_ROLLBACK_POINTS.to_string()));
        }

        Ok(())
    }
}

impl<P: LocalProtector> TemporaryRollbackStore for FixedTemporaryRollbackStore<P> {
    fn create(
        &self,
        purpose: RollbackPointPurpose,
        created_at_ms: i64,
        payload: &[u8],
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let metadata = RollbackPointMetadata {
            schema_version: RollbackPointMetadata::SCHEMA_VERSION,
            id,
            purpose,
            state: RollbackPointState::Pending,
            created_at_ms,
            failed_at_ms: None,
            payload_size_bytes: payload.len() as u64,
            payload_sha256: sha256(payload),
        };
        let bytes = self.encode(metadata.clone(), payload)?;
        let _mutation = mutation_lock()?;
        self.reserve_create_slot()?;
        self.write_encoded(&metadata.id, &bytes)?;
        Ok(metadata)
    }

    fn restore(&self, id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
        self.read(id).map(|(_, payload)| payload)
    }

    fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
        let _mutation = mutation_lock()?;
        self.remove(id)
    }

    fn retain_after_failure(
        &self,
        id: &str,
        failed_at_ms: i64,
    ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
        let _mutation = mutation_lock()?;
        let (mut metadata, payload) = self.read(id)?;
        if failed_at_ms < metadata.created_at_ms {
            return Err(TemporaryRollbackError::new(
                TemporaryRollbackErrorCode::InvalidState,
                "rollback failure time precedes its creation time",
            ));
        }
        metadata.state = RollbackPointState::Failed;
        metadata.failed_at_ms = Some(failed_at_ms);
        self.write(metadata.clone(), &payload)?;
        self.prune_excess_points()?;
        Ok(metadata)
    }

    fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error(&self.root, "inspect rollback directory", error)),
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                return Err(link_error(&self.root));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(TemporaryRollbackError::new(
                    TemporaryRollbackErrorCode::Io,
                    "temporary rollback path is not a directory",
                ));
            }
            Ok(_) => {}
        }
        inspect_no_links(&self.root)?;
        let mut entries = fs::read_dir(&self.root)
            .map_err(|error| io_error(&self.root, "read rollback directory", error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(&self.root, "read rollback directory entry", error))?;
        entries.sort_by_key(|entry| entry.file_name());

        let mut metadata = Vec::new();
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some(FILE_EXTENSION) {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| integrity_error("rollback filename is not valid UTF-8"))?;
            validate_id(id)?;
            metadata.push(self.read(id)?.0);
        }
        metadata.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(metadata)
    }
}

fn mutation_lock() -> Result<MutexGuard<'static, ()>, TemporaryRollbackError> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().map_err(|error| {
        TemporaryRollbackError::new(
            TemporaryRollbackErrorCode::InvalidState,
            format!("temporary rollback mutation lock is poisoned: {error}"),
        )
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_sha256(value: &str) -> Result<(), TemporaryRollbackError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(integrity_error("rollback digest is invalid"))
    }
}

fn validate_id(id: &str) -> Result<(), TemporaryRollbackError> {
    if id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(TemporaryRollbackError::new(
            TemporaryRollbackErrorCode::InvalidId,
            "invalid temporary rollback point id",
        ))
    }
}

fn ensure_safe_directory(root: &Path) -> Result<(), TemporaryRollbackError> {
    inspect_no_links(root)?;
    fs::create_dir_all(root).map_err(|error| io_error(root, "create rollback directory", error))?;
    inspect_no_links(root)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| io_error(root, "inspect rollback directory", error))?;
    if !metadata.is_dir() {
        return Err(TemporaryRollbackError::new(
            TemporaryRollbackErrorCode::Io,
            "temporary rollback path is not a directory",
        ));
    }
    Ok(())
}

fn inspect_no_links(path: &Path) -> Result<(), TemporaryRollbackError> {
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();
    for candidate in ancestors {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => {
                return Err(link_error(candidate));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(io_error(
                    candidate,
                    "inspect rollback path component",
                    error,
                ));
            }
        }
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(target_os = "windows"))]
    false
}

fn link_error(path: &Path) -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::LinkNotAllowed,
        "links and Windows reparse points are not allowed in the rollback path",
    )
    .with_context("path", path.display().to_string())
}

fn io_error(path: &Path, action: &str, error: std::io::Error) -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::Io,
        format!("failed to {action}: {error}"),
    )
    .with_context("path", path.display().to_string())
}

fn serialization_error(error: serde_json::Error) -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::Serialization,
        format!("invalid temporary rollback data: {error}"),
    )
}

fn integrity_error(message: &str) -> TemporaryRollbackError {
    TemporaryRollbackError::new(TemporaryRollbackErrorCode::Integrity, message)
}

fn protection_error(error: crate::ports::LocalProtectionError) -> TemporaryRollbackError {
    TemporaryRollbackError::new(
        TemporaryRollbackErrorCode::Protection,
        "local rollback protection failed",
    )
    .with_context("protectionCode", format!("{:?}", error.code))
}

#[cfg(test)]
mod tests;
