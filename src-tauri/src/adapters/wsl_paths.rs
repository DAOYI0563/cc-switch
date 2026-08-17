use std::path::{Path, PathBuf};

use crate::domain::ManagedClientId;
use crate::ports::{WslPathPair, WslPathResolver};

pub const WSL_DISTRIBUTION: &str = "Ubuntu";
pub const WSL_USER: &str = "zhldm";

/// The only production path resolver supported by WSL Code Switch.
#[derive(Debug, Clone)]
pub struct FixedWslPathResolver {
    windows_home: PathBuf,
}

impl FixedWslPathResolver {
    pub fn production() -> Self {
        Self {
            windows_home: PathBuf::from(format!(
                r"\\wsl.localhost\{WSL_DISTRIBUTION}\home\{WSL_USER}"
            )),
        }
    }

    /// Existing tests isolate file writes through this explicit override.
    /// Release builds never inspect the override and always use the fixed UNC.
    pub fn runtime() -> Self {
        #[cfg(debug_assertions)]
        if let Some(home) = test_home_override() {
            return Self { windows_home: home };
        }

        Self::production()
    }

    pub(crate) fn windows_home(&self) -> &Path {
        &self.windows_home
    }

    fn pair(&self, windows_relative: &Path, wsl_relative: &str) -> WslPathPair {
        WslPathPair {
            windows: self.windows_home.join(windows_relative),
            wsl: format!("/home/{WSL_USER}/{wsl_relative}"),
        }
    }
}

impl Default for FixedWslPathResolver {
    fn default() -> Self {
        Self::production()
    }
}

impl WslPathResolver for FixedWslPathResolver {
    fn client_config_root(&self, client: ManagedClientId) -> WslPathPair {
        match client {
            ManagedClientId::Claude => self.pair(Path::new(".claude"), ".claude"),
            ManagedClientId::Codex => self.pair(Path::new(".codex"), ".codex"),
            ManagedClientId::Opencode => {
                self.pair(Path::new(".config/opencode"), ".config/opencode")
            }
        }
    }

    fn claude_state_file(&self) -> WslPathPair {
        self.pair(Path::new(".claude.json"), ".claude.json")
    }

    fn opencode_session_data_root(&self) -> WslPathPair {
        self.pair(Path::new(".local/share/opencode"), ".local/share/opencode")
    }
}

#[cfg(debug_assertions)]
fn test_home_override() -> Option<PathBuf> {
    std::env::var_os("CC_SWITCH_TEST_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portable(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn production_paths_are_fixed_to_ubuntu_and_zhldm() {
        let resolver = FixedWslPathResolver::production();

        let claude = resolver.client_config_root(ManagedClientId::Claude);
        let codex = resolver.client_config_root(ManagedClientId::Codex);
        let opencode = resolver.client_config_root(ManagedClientId::Opencode);

        assert_eq!(
            portable(&claude.windows),
            "//wsl.localhost/Ubuntu/home/zhldm/.claude"
        );
        assert_eq!(claude.wsl, "/home/zhldm/.claude");
        assert_eq!(
            portable(&codex.windows),
            "//wsl.localhost/Ubuntu/home/zhldm/.codex"
        );
        assert_eq!(codex.wsl, "/home/zhldm/.codex");
        assert_eq!(
            portable(&opencode.windows),
            "//wsl.localhost/Ubuntu/home/zhldm/.config/opencode"
        );
        assert_eq!(opencode.wsl, "/home/zhldm/.config/opencode");
    }

    #[test]
    fn exceptional_paths_are_explicit_and_do_not_open_the_whole_home() {
        let resolver = FixedWslPathResolver::production();
        let claude_state = resolver.claude_state_file();
        let opencode_sessions = resolver.opencode_session_data_root();

        assert_eq!(
            portable(&claude_state.windows),
            "//wsl.localhost/Ubuntu/home/zhldm/.claude.json"
        );
        assert_eq!(claude_state.wsl, "/home/zhldm/.claude.json");
        assert_eq!(
            portable(&opencode_sessions.windows),
            "//wsl.localhost/Ubuntu/home/zhldm/.local/share/opencode"
        );
        assert_eq!(opencode_sessions.wsl, "/home/zhldm/.local/share/opencode");
    }

    #[test]
    fn platform_constants_are_the_only_supported_runtime() {
        assert_eq!(WSL_DISTRIBUTION, "Ubuntu");
        assert_eq!(WSL_USER, "zhldm");
    }
}
