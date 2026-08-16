use std::sync::Arc;

use serde_json::json;
use wsl_code_switch_lib::domain::{
    ManagedClientApps, PortableDomain, PortablePayload, PortableRecordId, SyncDevice, SyncDeviceId,
    SyncDeviceStatus, SyncLocalCommitPlan, SyncMergeBatch, SyncMergeResolution,
    SyncMergeSideAction, SyncRecord, SyncSchemaVersion,
};
use wsl_code_switch_lib::ports::SyncLocalApplyPort;
use wsl_code_switch_lib::{AppState, Database, McpServer, RuntimeSyncLocalAdapter};

const NOW: i64 = 1_800_000_000_000;

fn device_id() -> SyncDeviceId {
    SyncDeviceId::new("device-a").unwrap()
}

fn device(generation: u64) -> SyncDevice {
    SyncDevice {
        schema_version: SyncSchemaVersion::V1,
        device_id: device_id(),
        display_name: "Workstation".to_string(),
        acknowledged_generation: generation,
        registered_at_ms: NOW,
        last_seen_at_ms: NOW,
        status: SyncDeviceStatus::Active,
        retired_at_ms: None,
    }
}

#[test]
fn mcp_snapshot_omits_credentials_and_remote_apply_preserves_local_credentials() {
    let database = Arc::new(Database::memory().unwrap());
    database
        .save_mcp_server(&McpServer {
            id: "server-a".to_string(),
            name: "Before".to_string(),
            server: json!({
                "command": "uvx",
                "args": ["fixture"],
                "env": {"PRIVATE_TOKEN": "never-upload"},
                "headers": {"Authorization": "Bearer never-upload"}
            }),
            apps: ManagedClientApps::default(),
            description: None,
            homepage: None,
            docs: None,
            tags: Vec::new(),
        })
        .unwrap();
    let state = AppState::new(database.clone());
    let adapter = RuntimeSyncLocalAdapter::new(&state);

    let snapshot = adapter.snapshot(&device_id(), NOW).unwrap();
    assert!(snapshot
        .local_records
        .iter()
        .any(|record| record.id.domain == PortableDomain::Mcp && record.id.key == "server-a"));
    let encoded = serde_json::to_string(&snapshot.local_records).unwrap();
    assert!(!encoded.contains("never-upload"));
    assert!(!encoded.contains("\"env\""));
    assert!(!encoded.contains("\"headers\""));

    let incoming = SyncRecord::live(
        PortableRecordId::new(PortableDomain::Mcp, "server-a").unwrap(),
        device_id(),
        2,
        NOW + 1,
        PortablePayload::new(
            PortableDomain::Mcp,
            json!({
                "id": "server-a",
                "name": "After",
                "serverConfig": {"command": "uvx", "args": ["updated"]},
                "apps": {"claude": false, "codex": false, "opencode": false}
            }),
        )
        .unwrap(),
    )
    .unwrap();
    let plan = SyncLocalCommitPlan {
        schema_version: SyncSchemaVersion::V1,
        committed_generation: 2,
        fixed_identity: None,
        devices: vec![device(2)],
        merge_batch: SyncMergeBatch {
            schema_version: SyncSchemaVersion::V1,
            resolved: vec![SyncMergeResolution {
                schema_version: SyncSchemaVersion::V1,
                record: incoming,
                local_action: SyncMergeSideAction::ApplyMerged,
                remote_action: SyncMergeSideAction::Unchanged,
            }],
            conflicts: Vec::new(),
        },
    };

    adapter.apply_and_validate(&plan).unwrap();

    let stored = database
        .get_all_mcp_servers()
        .unwrap()
        .shift_remove("server-a")
        .unwrap();
    assert_eq!(stored.name, "After");
    assert_eq!(stored.server["args"], json!(["updated"]));
    assert_eq!(stored.server["env"]["PRIVATE_TOKEN"], "never-upload");
    assert_eq!(
        stored.server["headers"]["Authorization"],
        "Bearer never-upload"
    );
    assert_eq!(database.load_sync_baselines().unwrap().len(), 1);
}
