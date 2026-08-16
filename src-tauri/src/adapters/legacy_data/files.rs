use std::collections::BTreeMap;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::domain::LegacyFileSummary;
use crate::ports::{LegacyDataError, LegacyDataErrorCode};

use super::KNOWN_FILES;

pub(super) fn reject_pending_database_changes(root: &Path) -> Result<(), LegacyDataError> {
    for name in ["cc-switch.db-wal", "cc-switch.db-journal"] {
        let path = root.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata_is_link_or_reparse(&metadata) {
                    return Err(link_error(&path));
                }
                if !metadata.is_file() {
                    return Err(LegacyDataError::new(
                        LegacyDataErrorCode::InspectionFailed,
                        "database sidecar is not a regular file",
                    )
                    .with_context("path", path.display().to_string()));
                }
                if metadata.len() > 0 {
                    return Err(LegacyDataError::new(
                        LegacyDataErrorCode::PendingDatabaseChanges,
                        "legacy database has pending journal data",
                    )
                    .with_context("file", name)
                    .with_context("sizeBytes", metadata.len().to_string()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(inspection_error(&path, "inspect database sidecar", error)),
        }
    }
    Ok(())
}

pub(super) fn collect_known_files(
    root: &Path,
) -> Result<BTreeMap<String, LegacyFileSummary>, LegacyDataError> {
    let mut files = BTreeMap::new();
    for name in KNOWN_FILES {
        let path = root.join(name);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(inspection_error(&path, "inspect legacy file", error)),
        };
        if metadata_is_link_or_reparse(&metadata) {
            return Err(link_error(&path));
        }
        if !metadata.is_file() {
            return Err(LegacyDataError::new(
                LegacyDataErrorCode::InspectionFailed,
                "recognized legacy source is not a regular file",
            )
            .with_context("path", path.display().to_string()));
        }
        files.insert(
            (*name).to_string(),
            LegacyFileSummary {
                name: (*name).to_string(),
                size_bytes: metadata.len(),
                sha256: hash_file(&path)?,
            },
        );
    }
    Ok(files)
}

fn hash_file(path: &Path) -> Result<String, LegacyDataError> {
    let mut file = File::open(path)
        .map_err(|error| inspection_error(path, "open legacy file for hashing", error))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| inspection_error(path, "hash legacy file", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn directory_fingerprint(files: &[LegacyFileSummary]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wsl-code-switch-legacy-preview-v1\0");
    for file in files {
        hasher.update(file.name.as_bytes());
        hasher.update([0]);
        hasher.update(file.size_bytes.to_be_bytes());
        hasher.update(file.sha256.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn path_exists_without_following(path: &Path) -> Result<bool, LegacyDataError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(inspection_error(path, "inspect legacy path", error)),
    }
}

pub(super) fn inspect_no_links(path: &Path) -> Result<(), LegacyDataError> {
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
                return Err(inspection_error(
                    candidate,
                    "inspect legacy path component",
                    error,
                ));
            }
        }
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    false
}

fn link_error(path: &Path) -> LegacyDataError {
    LegacyDataError::new(
        LegacyDataErrorCode::LinkNotAllowed,
        "links and Windows reparse points are not allowed in the legacy source path",
    )
    .with_context("path", path.display().to_string())
}

pub(super) fn inspection_error(
    path: &Path,
    action: &str,
    error: std::io::Error,
) -> LegacyDataError {
    LegacyDataError::new(
        LegacyDataErrorCode::InspectionFailed,
        format!("failed to {action}: {error}"),
    )
    .with_context("path", path.display().to_string())
}
