#![allow(non_snake_case)]

mod cli_status;
mod config;
mod conflict_center;
mod daily_brief;
mod local_scan;
mod mcp;
mod misc;
mod model_fetch;
mod prompt;
mod provider;
mod session_manager;
mod settings;
pub mod skill;
mod webdav_sync;

use crate::app_config::LegacyAppType;
use crate::domain::ManagedClientId;

pub(crate) fn parse_managed_app_type(app: &str) -> Result<LegacyAppType, String> {
    app.parse::<ManagedClientId>()
        .map(LegacyAppType::from)
        .map_err(|error| error.to_string())
}

pub(crate) fn parse_managed_client_id(app: &str) -> Result<ManagedClientId, String> {
    app.parse::<ManagedClientId>()
        .map_err(|error| error.to_string())
}

pub use cli_status::*;
pub use config::*;
pub use conflict_center::*;
pub use daily_brief::*;
pub use local_scan::*;
pub use mcp::*;
pub use misc::*;
pub use model_fetch::*;
pub use prompt::*;
pub use provider::*;
pub use session_manager::*;
pub use settings::*;
pub use skill::*;
pub use webdav_sync::*;

#[cfg(test)]
mod tests {
    use super::{parse_managed_app_type, parse_managed_client_id};
    use crate::app_config::LegacyAppType;
    use crate::domain::ManagedClientId;

    #[test]
    fn production_command_boundary_accepts_exactly_three_clients() {
        assert_eq!(
            parse_managed_client_id("claude").unwrap(),
            ManagedClientId::Claude
        );
        assert_eq!(
            parse_managed_app_type("codex").unwrap(),
            LegacyAppType::Codex
        );
        assert_eq!(
            parse_managed_client_id(" OpenCode ").unwrap(),
            ManagedClientId::Opencode
        );
        for unsupported in [
            "claude-desktop",
            "gemini",
            "grokbuild",
            "openclaw",
            "hermes",
        ] {
            assert!(parse_managed_client_id(unsupported).is_err());
        }
    }
}
