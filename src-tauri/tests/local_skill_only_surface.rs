use std::fs;
use std::path::{Path, PathBuf};

fn manifest_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    fs::read_to_string(manifest_path(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

#[test]
fn production_skill_ipc_exposes_only_the_local_core() {
    let command = read("src/commands/skill.rs");
    let registry = read("src/lib.rs");

    for required in [
        "get_installed_skills",
        "uninstall_skill_unified",
        "toggle_skill_app",
        "sync_skill_from_live",
        "scan_unmanaged_skills",
        "import_skills_from_apps",
    ] {
        assert!(
            command.contains(&format!("fn {required}")),
            "local Skill command is missing: {required}"
        );
        assert!(
            registry.contains(&format!("commands::{required}")),
            "local Skill command is not registered: {required}"
        );
    }

    for removed in [
        "get_skill_backups",
        "delete_skill_backup",
        "install_skill_unified",
        "restore_skill_backup",
        "discover_available_skills",
        "check_skill_updates",
        "update_skill",
        "migrate_skill_storage",
        "search_skills_sh",
        "get_skills_for_app",
        "install_skill_for_app",
        "uninstall_skill_for_app",
        "get_skill_repos",
        "add_skill_repo",
        "remove_skill_repo",
        "open_zip_file_dialog",
        "install_skills_from_zip",
    ] {
        assert!(
            !command.contains(&format!("fn {removed}")),
            "removed Skill command is still implemented: {removed}"
        );
        assert!(
            !registry.contains(&format!("commands::{removed}")),
            "removed Skill command is still registered: {removed}"
        );
    }
}

#[test]
fn filesystem_heavy_skill_commands_run_off_the_async_runtime() {
    let command = read("src/commands/skill.rs");

    for required in [
        "uninstall_skill_unified",
        "toggle_skill_app",
        "sync_skill_from_live",
        "scan_unmanaged_skills",
        "import_skills_from_apps",
    ] {
        assert!(
            command.contains(&format!("pub async fn {required}")),
            "blocking Skill command must be asynchronous: {required}"
        );
    }
    assert!(
        command.matches("spawn_blocking").count() >= 5,
        "every filesystem-heavy Skill command must leave the async runtime unblocked"
    );
}

#[test]
fn authoritative_skill_refresh_waits_for_the_committing_blocking_task() {
    let command = read("src/commands/skill.rs");
    let start = command
        .find("pub async fn scan_unmanaged_skills")
        .expect("scan command");
    let end = command[start..]
        .find("pub async fn import_skills_from_apps")
        .map(|offset| start + offset)
        .expect("next command");
    let scan_command = &command[start..end];

    assert!(scan_command.contains("spawn_blocking"));
    assert!(!scan_command.contains("tokio::time::timeout"));
    assert!(scan_command.contains("restart_target_observation"));
}

#[test]
fn unmanaged_skill_ipc_contract_contains_no_absolute_path_field() {
    let domain = read("src/domain/skill.rs");
    let start = domain
        .find("pub struct UnmanagedLocalSkill")
        .expect("unmanaged Skill contract");
    let end = domain[start..]
        .find("pub enum LocalSkillScanIssueKind")
        .map(|offset| start + offset)
        .expect("next Skill contract");
    assert!(!domain[start..end].contains("pub path:"));
}

#[test]
fn legacy_skill_service_repository_and_deep_link_are_absent() {
    for relative in [
        "src/services/skill.rs",
        "src/deeplink/skill.rs",
        "src/deeplink/mod.rs",
        "src/services/provider/live.rs",
        "src/services/profile.rs",
        "tests/skill_sync.rs",
    ] {
        assert!(
            !manifest_path(relative).exists(),
            "removed Skill implementation still exists: {relative}"
        );
    }

    let services = read("src/services/mod.rs");
    let dao = read("src/database/dao/skills.rs");
    let settings = read("src/settings.rs");
    let app_config = read("src/app_config.rs");
    for (name, source, forbidden) in [
        ("services", services.as_str(), "mod skill"),
        ("services", services.as_str(), "pub use skill::"),
        ("Skill DAO", dao.as_str(), "skill_repos"),
        ("Skill DAO", dao.as_str(), "FROM skills"),
        ("Skill DAO", dao.as_str(), "INTO skills"),
        ("settings", settings.as_str(), "skill_storage_location"),
        ("settings", settings.as_str(), "skill_sync_method"),
        ("app config", app_config.as_str(), "SkillStore"),
        ("app config", app_config.as_str(), "InstalledSkill"),
    ] {
        assert!(
            !source.contains(forbidden),
            "{name} still contains removed Skill activity: {forbidden}"
        );
    }
}
