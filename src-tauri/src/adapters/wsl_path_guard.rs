use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::ports::{
    WslPathAccess, WslPathError, WslPathErrorCode, WslPathGuard, WslPathResolver, WslPathScope,
};

use super::wsl_paths::FixedWslPathResolver;

#[derive(Debug, Clone)]
pub struct SafeWslPathGuard<R = FixedWslPathResolver> {
    resolver: R,
}

impl<R> SafeWslPathGuard<R> {
    pub fn new(resolver: R) -> Self {
        Self { resolver }
    }
}

impl<R: WslPathResolver> WslPathGuard for SafeWslPathGuard<R> {
    fn resolve(
        &self,
        scope: WslPathScope,
        relative: &str,
        access: WslPathAccess,
    ) -> Result<PathBuf, WslPathError> {
        if scope == WslPathScope::OpencodeSessionData && access == WslPathAccess::Write {
            return Err(WslPathError::new(
                WslPathErrorCode::ReadOnlyScope,
                "OpenCode session data is read-only",
            ));
        }

        let (root, root_is_file) = match scope {
            WslPathScope::ClientConfig(client) => {
                (self.resolver.client_config_root(client).windows, false)
            }
            WslPathScope::ClaudeStateFile => (self.resolver.claude_state_file().windows, true),
            WslPathScope::OpencodeSessionData => {
                (self.resolver.opencode_session_data_root().windows, false)
            }
        };

        if root_is_file {
            if !relative.is_empty() {
                return Err(WslPathError::new(
                    WslPathErrorCode::ScopeIsFile,
                    "a child path cannot be resolved below a file scope",
                ));
            }
            inspect_no_links(&root)?;
            return Ok(root);
        }

        let relative_path = validate_relative(relative)?;
        let candidate = root.join(relative_path);
        inspect_no_links(&root)?;
        inspect_no_links(&candidate)?;
        verify_canonical_containment(&root, &candidate)?;
        Ok(candidate)
    }
}

fn validate_relative(relative: &str) -> Result<PathBuf, WslPathError> {
    if relative.contains('\\') {
        return Err(invalid_relative());
    }

    let path = Path::new(relative);
    if path.is_absolute() {
        return Err(invalid_relative());
    }

    for component in path.components() {
        match component {
            Component::Normal(part) if valid_component(part) => {}
            Component::CurDir if relative.is_empty() => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_)
            | Component::Normal(_) => return Err(invalid_relative()),
        }
    }

    Ok(path.to_path_buf())
}

fn valid_component(component: &OsStr) -> bool {
    let value = component.to_string_lossy();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains(':')
        && !value.chars().any(char::is_control)
}

fn invalid_relative() -> WslPathError {
    WslPathError::new(
        WslPathErrorCode::InvalidRelativePath,
        "path must be a clean relative path",
    )
}

fn inspect_no_links(path: &Path) -> Result<(), WslPathError> {
    let mut ancestors: Vec<_> = path.ancestors().collect();
    ancestors.reverse();

    for candidate in ancestors {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata_is_link_or_reparse(&metadata) {
                    return Err(WslPathError::new(
                        WslPathErrorCode::LinkNotAllowed,
                        format!(
                            "links are not allowed in managed paths: {}",
                            candidate.display()
                        ),
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(WslPathError::new(
                    WslPathErrorCode::InspectionFailed,
                    format!("failed to inspect {}: {error}", candidate.display()),
                ));
            }
        }
    }
    Ok(())
}

fn metadata_is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
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

fn verify_canonical_containment(root: &Path, candidate: &Path) -> Result<(), WslPathError> {
    let Some(existing_root) = nearest_existing(root)? else {
        return Ok(());
    };
    let Some(existing_candidate) = nearest_existing(candidate)? else {
        return Ok(());
    };

    let canonical_root = std::fs::canonicalize(existing_root).map_err(|error| {
        WslPathError::new(
            WslPathErrorCode::InspectionFailed,
            format!("failed to canonicalize managed root: {error}"),
        )
    })?;
    let canonical_candidate = std::fs::canonicalize(existing_candidate).map_err(|error| {
        WslPathError::new(
            WslPathErrorCode::InspectionFailed,
            format!("failed to canonicalize managed path: {error}"),
        )
    })?;

    // When the managed root does not exist yet, both paths resolve to the same
    // trusted ancestor. Creation is rechecked component-by-component by the
    // file adapter before any write.
    if root.exists() && !canonical_candidate.starts_with(&canonical_root) {
        return Err(WslPathError::new(
            WslPathErrorCode::PathEscape,
            "resolved path escapes its managed root",
        ));
    }
    Ok(())
}

fn nearest_existing(path: &Path) -> Result<Option<&Path>, WslPathError> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        match std::fs::symlink_metadata(candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                current = candidate.parent();
            }
            Err(error) => {
                return Err(WslPathError::new(
                    WslPathErrorCode::InspectionFailed,
                    format!("failed to inspect {}: {error}", candidate.display()),
                ));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ManagedClientId;
    use crate::ports::WslPathPair;

    #[derive(Clone)]
    struct TestResolver {
        home: PathBuf,
    }

    impl WslPathResolver for TestResolver {
        fn client_config_root(&self, client: ManagedClientId) -> WslPathPair {
            let relative = match client {
                ManagedClientId::Claude => ".claude",
                ManagedClientId::Codex => ".codex",
                ManagedClientId::Opencode => ".config/opencode",
            };
            WslPathPair {
                windows: self.home.join(relative),
                wsl: format!("/fixture/{relative}"),
            }
        }

        fn claude_state_file(&self) -> WslPathPair {
            WslPathPair {
                windows: self.home.join(".claude.json"),
                wsl: "/fixture/.claude.json".to_string(),
            }
        }

        fn opencode_session_data_root(&self) -> WslPathPair {
            WslPathPair {
                windows: self.home.join(".local/share/opencode"),
                wsl: "/fixture/.local/share/opencode".to_string(),
            }
        }
    }

    fn guard(home: &Path) -> SafeWslPathGuard<TestResolver> {
        SafeWslPathGuard::new(TestResolver {
            home: home.to_path_buf(),
        })
    }

    #[test]
    fn resolves_clean_client_relative_paths() {
        let temp = tempfile::tempdir().unwrap();
        let path = guard(temp.path())
            .resolve(
                WslPathScope::ClientConfig(ManagedClientId::Codex),
                "sessions/2026/session.jsonl",
                WslPathAccess::Read,
            )
            .unwrap();

        assert_eq!(path, temp.path().join(".codex/sessions/2026/session.jsonl"));
    }

    #[test]
    fn rejects_parent_absolute_and_separator_smuggling() {
        let temp = tempfile::tempdir().unwrap();
        let guard = guard(temp.path());
        for input in ["../outside", "/outside", r"..\outside", r"child\outside"] {
            let error = guard
                .resolve(
                    WslPathScope::ClientConfig(ManagedClientId::Claude),
                    input,
                    WslPathAccess::Read,
                )
                .unwrap_err();
            assert_eq!(error.code, WslPathErrorCode::InvalidRelativePath, "{input}");
        }
    }

    #[test]
    fn enforces_file_and_read_only_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let guard = guard(temp.path());
        assert_eq!(
            guard
                .resolve(WslPathScope::ClaudeStateFile, "child", WslPathAccess::Read,)
                .unwrap_err()
                .code,
            WslPathErrorCode::ScopeIsFile
        );
        assert_eq!(
            guard
                .resolve(
                    WslPathScope::OpencodeSessionData,
                    "opencode.db",
                    WslPathAccess::Write,
                )
                .unwrap_err()
                .code,
            WslPathErrorCode::ReadOnlyScope
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_and_cycle() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".claude");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(root.join("cycle-b"), root.join("cycle-a")).unwrap();
        symlink(root.join("cycle-a"), root.join("cycle-b")).unwrap();

        for input in ["escape/secret", "cycle-a/item"] {
            let error = guard(temp.path())
                .resolve(
                    WslPathScope::ClientConfig(ManagedClientId::Claude),
                    input,
                    WslPathAccess::Read,
                )
                .unwrap_err();
            assert_eq!(error.code, WslPathErrorCode::LinkNotAllowed, "{input}");
        }
    }

    #[test]
    fn permits_missing_descendants_below_a_link_free_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
        let path = guard(temp.path())
            .resolve(
                WslPathScope::ClientConfig(ManagedClientId::Codex),
                "new/deep/config.toml",
                WslPathAccess::Write,
            )
            .unwrap();
        assert_eq!(path, temp.path().join(".codex/new/deep/config.toml"));
    }
}
