use crate::domain::ManagedClientId;
use crate::error::AppError;
use crate::ports::{WslFileSystem, WslPathScope};

use super::wsl_files::WslFileAdapter;

/// Exact-byte access to the three managed MCP live files.
///
/// Path selection and link/reparse-point defense stay in this adapter so MCP
/// application policy never manipulates absolute Windows or WSL paths.
#[derive(Debug, Clone, Default)]
pub struct McpLiveFileAdapter {
    files: WslFileAdapter,
}

#[derive(Debug, Clone)]
pub struct McpLiveFileSnapshot {
    client: ManagedClientId,
    contents: Option<Vec<u8>>,
}

impl McpLiveFileAdapter {
    pub fn runtime() -> Self {
        Self {
            files: WslFileAdapter::runtime(),
        }
    }

    pub fn read_optional(&self, client: ManagedClientId) -> Result<Option<Vec<u8>>, AppError> {
        let (scope, relative) = location(client);
        self.files
            .read_optional(scope, relative)
            .map_err(adapter_error)
    }

    pub fn write(&self, client: ManagedClientId, contents: &[u8]) -> Result<(), AppError> {
        let (scope, relative) = location(client);
        self.files
            .atomic_write(scope, relative, contents)
            .map_err(adapter_error)
    }

    pub fn remove(&self, client: ManagedClientId) -> Result<(), AppError> {
        let (scope, relative) = location(client);
        self.files
            .remove_file(scope, relative)
            .map_err(adapter_error)
    }

    pub fn capture(&self, client: ManagedClientId) -> Result<McpLiveFileSnapshot, AppError> {
        Ok(McpLiveFileSnapshot {
            client,
            contents: self.read_optional(client)?,
        })
    }

    pub fn restore(&self, snapshot: &McpLiveFileSnapshot) -> Result<(), AppError> {
        match &snapshot.contents {
            Some(contents) => self.write(snapshot.client, contents),
            None => self.remove(snapshot.client),
        }
    }
}

fn location(client: ManagedClientId) -> (WslPathScope, &'static str) {
    match client {
        ManagedClientId::Claude => (WslPathScope::ClaudeStateFile, ""),
        ManagedClientId::Codex => (
            WslPathScope::ClientConfig(ManagedClientId::Codex),
            "config.toml",
        ),
        ManagedClientId::Opencode => (
            WslPathScope::ClientConfig(ManagedClientId::Opencode),
            "opencode.json",
        ),
    }
}

fn adapter_error(error: crate::ports::WslFileError) -> AppError {
    AppError::Config(format!("MCP live file access failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::Path;

    struct TestHomeGuard(Option<std::ffi::OsString>);

    impl TestHomeGuard {
        #[allow(deprecated)]
        fn set(home: &Path) -> Self {
            let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
            std::env::set_var("CC_SWITCH_TEST_HOME", home);
            Self(previous)
        }
    }

    impl Drop for TestHomeGuard {
        #[allow(deprecated)]
        fn drop(&mut self) {
            match self.0.take() {
                Some(previous) => std::env::set_var("CC_SWITCH_TEST_HOME", previous),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[test]
    #[serial]
    fn snapshot_restore_is_exact_for_all_three_clients() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let adapter = McpLiveFileAdapter::runtime();

        for client in ManagedClientId::ALL {
            let original = format!("original-{}", client.as_str()).into_bytes();
            adapter.write(client, &original).unwrap();
            let snapshot = adapter.capture(client).unwrap();
            adapter.write(client, b"changed").unwrap();
            adapter.restore(&snapshot).unwrap();
            assert_eq!(adapter.read_optional(client).unwrap(), Some(original));
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn capture_rejects_a_linked_live_file_without_reading_its_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let outside = temp.path().join("outside.toml");
        std::fs::write(&outside, b"outside-secret").unwrap();
        std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
        symlink(&outside, temp.path().join(".codex/config.toml")).unwrap();

        assert!(McpLiveFileAdapter::runtime()
            .capture(ManagedClientId::Codex)
            .is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"outside-secret");
    }
}
