use std::path::Path;

use crate::ports::{WslFileError, WslFileSystem, WslPathAccess, WslPathGuard, WslPathScope};

use super::wsl_path_guard::SafeWslPathGuard;
use super::wsl_paths::FixedWslPathResolver;

#[derive(Debug, Clone)]
pub struct WslFileAdapter {
    guard: SafeWslPathGuard<FixedWslPathResolver>,
}

impl WslFileAdapter {
    pub fn runtime() -> Self {
        Self {
            guard: SafeWslPathGuard::new(FixedWslPathResolver::runtime()),
        }
    }

    fn ensure_parent(&self, scope: WslPathScope, relative: &str) -> Result<(), WslFileError> {
        let path = self
            .guard
            .resolve(scope, relative, WslPathAccess::Write)
            .map_err(WslFileError::from_path)?;
        let parent = path
            .parent()
            .ok_or_else(|| WslFileError::io("managed file has no parent directory"))?;
        std::fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;

        // A newly created component or a concurrent replacement must still be
        // link-free before the file is opened.
        self.guard
            .resolve(scope, relative, WslPathAccess::Write)
            .map_err(WslFileError::from_path)?;
        Ok(())
    }
}

impl Default for WslFileAdapter {
    fn default() -> Self {
        Self::runtime()
    }
}

impl WslFileSystem for WslFileAdapter {
    fn read(&self, scope: WslPathScope, relative: &str) -> Result<Vec<u8>, WslFileError> {
        let path = self
            .guard
            .resolve(scope, relative, WslPathAccess::Read)
            .map_err(WslFileError::from_path)?;
        std::fs::read(&path).map_err(|error| io_error(&path, error))
    }

    fn read_optional(
        &self,
        scope: WslPathScope,
        relative: &str,
    ) -> Result<Option<Vec<u8>>, WslFileError> {
        let path = self
            .guard
            .resolve(scope, relative, WslPathAccess::Read)
            .map_err(WslFileError::from_path)?;
        match std::fs::read(&path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(&path, error)),
        }
    }

    fn atomic_write(
        &self,
        scope: WslPathScope,
        relative: &str,
        contents: &[u8],
    ) -> Result<(), WslFileError> {
        self.ensure_parent(scope, relative)?;
        let path = self
            .guard
            .resolve(scope, relative, WslPathAccess::Write)
            .map_err(WslFileError::from_path)?;
        crate::config::atomic_write(&path, contents)
            .map_err(|error| WslFileError::io(error.to_string()))?;

        self.guard
            .resolve(scope, relative, WslPathAccess::Write)
            .map_err(WslFileError::from_path)?;
        Ok(())
    }

    fn remove_file(&self, scope: WslPathScope, relative: &str) -> Result<(), WslFileError> {
        let path = self
            .guard
            .resolve(scope, relative, WslPathAccess::Write)
            .map_err(WslFileError::from_path)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.guard
                    .resolve(scope, relative, WslPathAccess::Write)
                    .map_err(WslFileError::from_path)?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&path, error)),
        }
    }
}

fn io_error(path: &Path, error: std::io::Error) -> WslFileError {
    WslFileError::io(format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ManagedClientId;
    use serial_test::serial;

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
    fn atomic_write_roundtrip_uses_managed_scope() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let adapter = WslFileAdapter::runtime();
        let scope = WslPathScope::ClientConfig(ManagedClientId::Claude);

        adapter
            .atomic_write(scope, "nested/settings.json", b"first")
            .unwrap();
        adapter
            .atomic_write(scope, "nested/settings.json", b"second")
            .unwrap();

        assert_eq!(
            adapter.read(scope, "nested/settings.json").unwrap(),
            b"second"
        );
        assert_eq!(
            adapter
                .read_optional(scope, "nested/settings.json")
                .unwrap(),
            Some(b"second".to_vec())
        );
        adapter.remove_file(scope, "nested/settings.json").unwrap();
        assert_eq!(
            adapter
                .read_optional(scope, "nested/settings.json")
                .unwrap(),
            None
        );
        assert_eq!(
            std::fs::read_dir(temp.path().join(".claude/nested"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    #[serial]
    fn rejected_path_does_not_create_or_modify_files() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, b"original").unwrap();

        let error = WslFileAdapter::runtime()
            .atomic_write(
                WslPathScope::ClientConfig(ManagedClientId::Codex),
                "../outside.txt",
                b"replacement",
            )
            .unwrap_err();

        assert_eq!(error.code, crate::ports::WslFileErrorCode::InvalidPath);
        assert_eq!(std::fs::read(outside).unwrap(), b"original");
        assert!(!temp.path().join(".codex").exists());
    }

    #[test]
    #[serial]
    fn read_only_session_scope_never_writes() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let error = WslFileAdapter::runtime()
            .atomic_write(
                WslPathScope::OpencodeSessionData,
                "opencode.db",
                b"not allowed",
            )
            .unwrap_err();

        assert_eq!(error.code, crate::ports::WslFileErrorCode::InvalidPath);
        assert!(!temp.path().join(".local").exists());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn symlink_parent_rejection_preserves_external_target() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let root = temp.path().join(".claude");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("settings.json"), b"original").unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        let result = WslFileAdapter::runtime().atomic_write(
            WslPathScope::ClientConfig(ManagedClientId::Claude),
            "linked/settings.json",
            b"replacement",
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(outside.join("settings.json")).unwrap(),
            b"original"
        );
    }

    #[cfg(windows)]
    #[test]
    #[serial]
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR and CC_SWITCH_TEST_HOME on WSL2 UNC paths"]
    fn managed_atomic_write_roundtrip_on_wsl_unc() {
        let root = std::path::PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = std::path::PathBuf::from(
            std::env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set"),
        );
        assert!(home.starts_with(&root));

        let adapter = WslFileAdapter::runtime();
        let scope = WslPathScope::ClientConfig(ManagedClientId::Claude);
        let relative = "phase1-managed-atomic-write.json";
        adapter.atomic_write(scope, relative, b"first").unwrap();
        adapter.atomic_write(scope, relative, b"second").unwrap();
        assert_eq!(adapter.read(scope, relative).unwrap(), b"second");

        std::fs::remove_file(home.join(".claude").join(relative)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[serial]
    #[ignore = "requires a phase1-link symlink prepared below CC_SWITCH_TEST_HOME on WSL2"]
    fn managed_write_rejects_wsl_unc_symlink_escape() {
        let root = std::path::PathBuf::from(
            std::env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = std::path::PathBuf::from(
            std::env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set"),
        );
        let outside = root.join("phase1-outside").join("guard.txt");
        let link = home.join(".claude").join("phase1-link");
        std::fs::symlink_metadata(&link).unwrap_or_else(|error| {
            panic!(
                "WSL symlink fixture must be inspectable at {}: {error}",
                link.display()
            )
        });
        assert_eq!(std::fs::read(&outside).unwrap(), b"original");

        let error = WslFileAdapter::runtime()
            .atomic_write(
                WslPathScope::ClientConfig(ManagedClientId::Claude),
                "phase1-link/guard.txt",
                b"replacement",
            )
            .unwrap_err();

        assert_eq!(error.code, crate::ports::WslFileErrorCode::InvalidPath);
        assert_eq!(std::fs::read(outside).unwrap(), b"original");
    }
}
