//! Pure domain contracts shared by application use cases and adapters.
//!
//! This module intentionally has no Tauri, filesystem, SQLite, HTTP, or
//! Windows dependencies. Platform-specific code maps to and from these types at
//! the adapter boundary.

mod common_snippet;
mod conflict_center;
mod contracts;
mod daily_brief;
mod local_reconciliation;
mod local_scan;
mod mcp;
mod prompt;
mod retained_migration;
mod skill;
mod sync_cas;
mod sync_crypto;
mod sync_device;
mod sync_local;
mod sync_merge;
mod sync_tombstone;
mod sync_transport;
mod sync_v3;

pub use common_snippet::*;
pub use conflict_center::*;
pub use contracts::*;
pub use daily_brief::*;
pub use local_reconciliation::*;
pub use local_scan::*;
pub use mcp::*;
pub use prompt::*;
pub use retained_migration::*;
pub use skill::*;
pub use sync_cas::*;
pub use sync_crypto::*;
pub use sync_device::*;
pub use sync_local::*;
pub use sync_merge::*;
pub use sync_tombstone::*;
pub use sync_transport::*;
pub use sync_v3::*;
