use std::fs;
use std::path::{Path, PathBuf};

use crate::ports::{DeviceSettingsError, DeviceSettingsErrorCode, DeviceSettingsStore};

const SETTINGS_FILE: &str = "settings.json";
const MAX_SETTINGS_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FixedDeviceSettingsStore {
    root: PathBuf,
}

impl FixedDeviceSettingsStore {
    pub fn runtime() -> Self {
        Self::new(crate::config::get_app_config_dir())
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.root.join(SETTINGS_FILE)
    }

    fn inspect_root(&self) -> Result<(), DeviceSettingsError> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => Err(link_error(&self.root)),
            Ok(metadata) if !metadata.is_dir() => Err(io_error(
                DeviceSettingsErrorCode::ReadFailed,
                &self.root,
                "device settings root is not a directory",
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(
                DeviceSettingsErrorCode::ReadFailed,
                &self.root,
                format!("failed to inspect device settings root: {error}"),
            )),
        }
    }

    fn inspect_file(&self) -> Result<Option<fs::Metadata>, DeviceSettingsError> {
        self.inspect_root()?;
        let path = self.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata_is_link_or_reparse(&metadata) => Err(link_error(&path)),
            Ok(metadata) if !metadata.is_file() => Err(io_error(
                DeviceSettingsErrorCode::ReadFailed,
                &path,
                "device settings path is not a regular file",
            )),
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(
                DeviceSettingsErrorCode::ReadFailed,
                &path,
                format!("failed to inspect device settings: {error}"),
            )),
        }
    }
}

impl DeviceSettingsStore for FixedDeviceSettingsStore {
    fn read(&self) -> Result<Option<Vec<u8>>, DeviceSettingsError> {
        let Some(metadata) = self.inspect_file()? else {
            return Ok(None);
        };
        if metadata.len() > MAX_SETTINGS_BYTES as u64 {
            return Err(DeviceSettingsError::new(
                DeviceSettingsErrorCode::TooLarge,
                "device settings exceed the migration size limit",
            )
            .with_context("maxBytes", MAX_SETTINGS_BYTES.to_string()));
        }
        let path = self.path();
        fs::read(&path).map(Some).map_err(|error| {
            io_error(
                DeviceSettingsErrorCode::ReadFailed,
                &path,
                format!("failed to read device settings: {error}"),
            )
        })
    }

    fn replace(&self, contents: &[u8]) -> Result<(), DeviceSettingsError> {
        if contents.len() > MAX_SETTINGS_BYTES {
            return Err(DeviceSettingsError::new(
                DeviceSettingsErrorCode::TooLarge,
                "device settings exceed the migration size limit",
            )
            .with_context("maxBytes", MAX_SETTINGS_BYTES.to_string()));
        }
        self.inspect_file()?;
        let path = self.path();
        crate::config::atomic_write(&path, contents).map_err(|error| {
            io_error(
                DeviceSettingsErrorCode::WriteFailed,
                &path,
                format!("failed to atomically replace device settings: {error}"),
            )
        })
    }

    fn delete(&self) -> Result<(), DeviceSettingsError> {
        if self.inspect_file()?.is_none() {
            return Ok(());
        }
        let path = self.path();
        fs::remove_file(&path).map_err(|error| {
            io_error(
                DeviceSettingsErrorCode::DeleteFailed,
                &path,
                format!("failed to delete device settings: {error}"),
            )
        })
    }
}

fn io_error(
    code: DeviceSettingsErrorCode,
    path: &Path,
    message: impl Into<String>,
) -> DeviceSettingsError {
    DeviceSettingsError::new(code, message).with_context("path", path.display().to_string())
}

fn link_error(path: &Path) -> DeviceSettingsError {
    io_error(
        DeviceSettingsErrorCode::LinkNotAllowed,
        path,
        "device settings migration refuses links and Windows reparse points",
    )
}

fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_bytes_roundtrip_and_delete_are_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let store = FixedDeviceSettingsStore::new(temp.path().join("app"));
        assert_eq!(store.read().unwrap(), None);
        store.replace(b"{\"fixture\":true}\n").unwrap();
        assert_eq!(
            store.read().unwrap(),
            Some(b"{\"fixture\":true}\n".to_vec())
        );
        store.delete().unwrap();
        store.delete().unwrap();
        assert_eq!(store.read().unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn linked_settings_file_is_rejected_without_touching_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("app");
        fs::create_dir(&root).unwrap();
        let outside = temp.path().join("outside.json");
        fs::write(&outside, b"original").unwrap();
        symlink(&outside, root.join(SETTINGS_FILE)).unwrap();
        let store = FixedDeviceSettingsStore::new(root);

        assert_eq!(
            store.replace(b"replacement").unwrap_err().code,
            DeviceSettingsErrorCode::LinkNotAllowed
        );
        assert_eq!(fs::read(outside).unwrap(), b"original");
    }
}
