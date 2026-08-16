mod claude;
mod codex;
mod opencode;

pub use claude::ClaudeLiveProviderConfigAdapter;
pub use codex::CodexLiveProviderConfigAdapter;
pub use opencode::OpenCodeLiveProviderConfigAdapter;

use crate::domain::ManagedClientId;
use crate::ports::LiveProviderConfigPort;

pub fn runtime_adapter(client_id: ManagedClientId) -> Box<dyn LiveProviderConfigPort> {
    match client_id {
        ManagedClientId::Claude => Box::new(ClaudeLiveProviderConfigAdapter::runtime()),
        ManagedClientId::Codex => Box::new(CodexLiveProviderConfigAdapter::runtime()),
        ManagedClientId::Opencode => Box::new(OpenCodeLiveProviderConfigAdapter::runtime()),
    }
}
