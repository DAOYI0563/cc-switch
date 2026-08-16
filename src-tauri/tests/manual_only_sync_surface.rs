use std::fs;
use std::path::{Path, PathBuf};

fn manifest_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    manifest_root()
        .parent()
        .expect("Tauri crate must live under the repository root")
        .to_path_buf()
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path.as_ref())
        .unwrap_or_else(|error| panic!("read {}: {error}", path.as_ref().display()))
}

#[test]
fn legacy_archive_auto_sync_and_s3_modules_are_physically_removed() {
    let root = manifest_root();
    for relative in [
        "src/commands/s3_sync.rs",
        "src/services/s3.rs",
        "src/services/s3_auto_sync.rs",
        "src/services/s3_sync.rs",
        "src/services/sync_protocol.rs",
        "src/services/webdav.rs",
        "src/services/webdav_auto_sync.rs",
        "src/services/webdav_sync.rs",
        "src/services/webdav_sync/archive.rs",
    ] {
        assert!(
            !root.join(relative).exists(),
            "obsolete sync module still exists: {relative}"
        );
    }

    let services = read(root.join("src/services/mod.rs"));
    let commands = read(root.join("src/commands/mod.rs"));
    for forbidden in [
        "pub mod s3",
        "pub mod s3_auto_sync",
        "pub mod s3_sync",
        "pub mod sync_protocol",
        "pub mod webdav;",
        "pub mod webdav_auto_sync",
        "pub mod webdav_sync",
    ] {
        assert!(
            !services.contains(forbidden),
            "service module still registers {forbidden}"
        );
    }
    for forbidden in ["mod s3_sync", "pub use s3_sync"] {
        assert!(
            !commands.contains(forbidden),
            "command module still registers {forbidden}"
        );
    }
}

#[test]
fn product_surface_is_manual_sync_v3_only() {
    let root = repo_root();
    let settings = read(root.join("src-tauri/src/settings.rs"));
    for forbidden in [
        "S3SyncSettings",
        "s3_sync",
        "auto_sync",
        "update_webdav_sync_status",
    ] {
        assert!(
            !settings.contains(forbidden),
            "settings retain deleted sync field or API: {forbidden}"
        );
    }
    let command = read(root.join("src-tauri/src/commands/webdav_sync.rs"));
    for required in [
        "webdav_test_connection",
        "webdav_sync_save_settings",
        "webdav_sync_preview_first",
        "webdav_sync_confirm_first",
        "webdav_sync_now",
        "webdav_sync_list_devices",
        "webdav_sync_retire_device",
        "ReqwestSyncWebDavTransport",
        "RuntimeSyncLocalAdapter",
        "FixedSyncCryptoEngine",
    ] {
        assert!(
            command.contains(required),
            "retained WebDAV boundary is missing: {required}"
        );
    }
    for forbidden in [
        "webdav_sync_upload",
        "webdav_sync_download",
        "webdav_sync_fetch_remote_info",
        "services::webdav_sync",
        "webdav_auto_sync",
    ] {
        assert!(
            !command.contains(forbidden),
            "legacy WebDAV command remains: {forbidden}"
        );
    }

    let frontend = [
        root.join("src/components/settings/WebdavSyncSection.tsx"),
        root.join("src/lib/api/settings.ts"),
        root.join("src/types.ts"),
        root.join("src/App.tsx"),
    ]
    .into_iter()
    .map(read)
    .collect::<Vec<_>>()
    .join("\n");
    for forbidden in [
        "S3SyncSettings",
        "s3Sync",
        "s3-sync",
        "autoSync",
        "webdavSyncUpload",
        "webdavSyncDownload",
        "webdavSyncFetchRemoteInfo",
        "db.sql",
        "skills.zip",
    ] {
        assert!(
            !frontend.contains(forbidden),
            "frontend retains deleted sync surface: {forbidden}"
        );
    }
    for required in [
        "webdavSyncPreviewFirst",
        "webdavSyncConfirmFirst",
        "webdavSyncNow",
        "webdavSyncListDevices",
        "webdavSyncRetireDevice",
        "syncPassphrase",
        "firstSyncPreview",
        "deviceManagement",
    ] {
        assert!(
            frontend.contains(required),
            "frontend is missing manual sync-v3 surface: {required}"
        );
    }

    let manual = read(root.join("docs/user-manual/README.md"));
    for required in ["手动 WebDAV 同步", "立即同步", "不存在定时任务"] {
        assert!(
            manual.contains(required),
            "user manual is missing the manual-only sync contract: {required}"
        );
    }
    for forbidden in [
        "Uses **v2 protocol**",
        "使用 **v2 协议**",
        "**v2 プロトコル**",
        "#### Auto-Sync",
        "#### 自动同步",
        "#### 自動同期",
    ] {
        assert!(
            !manual.contains(forbidden),
            "user manual advertises removed archive/auto sync behavior: {forbidden}"
        );
    }

    let cargo = read(root.join("src-tauri/Cargo.toml"));
    assert!(
        !cargo
            .lines()
            .any(|line| line.trim_start().starts_with("zip =")),
        "legacy archive-only zip dependency remains"
    );

    let database_backup = read(root.join("src-tauri/src/database/backup.rs"));
    for forbidden in [
        "export_sql_string_for_sync",
        "import_sql_string_for_sync",
        "SYNC_SKIP_TABLES",
        "SYNC_PRESERVE_TABLES",
    ] {
        assert!(
            !database_backup.contains(forbidden),
            "database backup retains legacy archive-sync branch: {forbidden}"
        );
    }

    assert!(root.join("src-tauri/src/adapters/sync_webdav.rs").is_file());
}
