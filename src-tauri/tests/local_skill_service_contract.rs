use std::fs;
use std::path::{Path, PathBuf};

use serial_test::serial;
use wsl_code_switch_lib::domain::{LocalSkillImport, ManagedClientApps, ManagedClientId};
use wsl_code_switch_lib::LocalSkillService;

#[path = "support.rs"]
mod support;
use support::{create_test_state, ensure_test_home, reset_test_fs, test_mutex};

fn skill_dir(home: &Path, client: ManagedClientId, directory: &str) -> PathBuf {
    match client {
        ManagedClientId::Claude => home.join(".claude/skills").join(directory),
        ManagedClientId::Codex => home.join(".codex/skills").join(directory),
        ManagedClientId::Opencode => home.join(".config/opencode/skills").join(directory),
    }
}

fn write_skill(home: &Path, client: ManagedClientId, directory: &str, body: &str) {
    let root = skill_dir(home, client, directory);
    fs::create_dir_all(root.join("references")).expect("create skill tree");
    fs::write(
        root.join("SKILL.md"),
        format!("---\nname: {directory}\ndescription: Local fixture\n---\n{body}\n"),
    )
    .expect("write SKILL.md");
    fs::write(root.join("references/guide.md"), body).expect("write nested fixture");
}

fn all_apps() -> ManagedClientApps {
    ManagedClientApps {
        claude: true,
        codex: true,
        opencode: true,
    }
}

#[test]
#[serial]
fn imports_from_the_explicit_live_source_and_copies_plain_trees_to_three_clients() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let home = ensure_test_home();
    write_skill(
        home,
        ManagedClientId::Claude,
        "local-fixture",
        "claude source",
    );
    let source_before =
        fs::read(skill_dir(home, ManagedClientId::Claude, "local-fixture").join("SKILL.md"))
            .expect("read source before import");
    let state = create_test_state().expect("create state");

    let imported = LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "local-fixture".to_string(),
            source_client: ManagedClientId::Claude,
            apps: all_apps(),
        }],
    )
    .expect("import local Skill");

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].directory, "local-fixture");
    assert_eq!(imported[0].apps, all_apps());
    assert!(imported[0].content_hash.is_some());
    assert!(imported[0].cloud_eligible);
    for client in ManagedClientId::ALL {
        let root = skill_dir(home, client, "local-fixture");
        assert!(root.join("SKILL.md").is_file());
        assert_eq!(
            fs::read(root.join("references/guide.md")).expect("read copied nested file"),
            b"claude source"
        );
        assert!(
            !fs::symlink_metadata(root)
                .expect("inspect copied root")
                .file_type()
                .is_symlink(),
            "managed copies must be ordinary directories"
        );
    }
    assert_eq!(
        fs::read(skill_dir(home, ManagedClientId::Claude, "local-fixture").join("SKILL.md"))
            .expect("read source after import"),
        source_before,
        "import must not rewrite its selected source"
    );
    assert_eq!(
        state.db.list_core_skills().expect("list core Skills"),
        imported
    );
    assert_eq!(state.local_scan_writes.pending_count(), 2);
    assert_eq!(state.local_scan_writes.last_generation(), 2);
}

#[test]
#[serial]
fn over_file_count_limit_remains_local_but_is_not_cloud_eligible() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let home = ensure_test_home();
    let root = skill_dir(home, ManagedClientId::Claude, "large-local");
    fs::create_dir_all(&root).expect("create Skill root");
    fs::write(root.join("SKILL.md"), "---\nname: large-local\n---\n").expect("write SKILL.md");
    for index in 0..500 {
        fs::write(root.join(format!("file-{index:03}.txt")), b"x").expect("write limit fixture");
    }
    let state = create_test_state().expect("create state");

    let imported = LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "large-local".to_string(),
            source_client: ManagedClientId::Claude,
            apps: ManagedClientApps::only(ManagedClientId::Claude),
        }],
    )
    .expect("over-limit Skill remains locally manageable");

    assert_eq!(imported[0].file_count, 501);
    assert!(!imported[0].cloud_eligible);
    assert!(root.join("file-499.txt").is_file());
    assert_eq!(
        state.db.list_core_skills().expect("list core Skills"),
        imported
    );
}

#[test]
#[serial]
fn selected_source_is_not_inferred_from_another_matching_client_directory() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let home = ensure_test_home();
    write_skill(home, ManagedClientId::Claude, "shared-name", "claude bytes");
    write_skill(home, ManagedClientId::Codex, "shared-name", "codex bytes");
    let claude_before =
        fs::read(skill_dir(home, ManagedClientId::Claude, "shared-name").join("SKILL.md"))
            .expect("read Claude fixture");
    let state = create_test_state().expect("create state");

    let imported = LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "shared-name".to_string(),
            source_client: ManagedClientId::Codex,
            apps: ManagedClientApps::only(ManagedClientId::Codex),
        }],
    )
    .expect("import selected Codex source");

    assert_eq!(
        imported[0].apps,
        ManagedClientApps::only(ManagedClientId::Codex)
    );
    assert_eq!(
        fs::read(skill_dir(home, ManagedClientId::Claude, "shared-name").join("SKILL.md"))
            .expect("read untouched Claude fixture"),
        claude_before
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn linked_source_is_rejected_without_database_or_target_writes() {
    use std::os::unix::fs::symlink;

    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let home = ensure_test_home();
    let outside = home.join("outside-linked-skill");
    fs::create_dir_all(&outside).expect("create outside directory");
    fs::write(outside.join("SKILL.md"), "---\nname: linked\n---\n").expect("write outside Skill");
    let linked = skill_dir(home, ManagedClientId::Claude, "linked");
    fs::create_dir_all(linked.parent().expect("linked parent")).expect("create live root");
    symlink(&outside, &linked).expect("create linked source");
    let state = create_test_state().expect("create state");

    LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "linked".to_string(),
            source_client: ManagedClientId::Claude,
            apps: all_apps(),
        }],
    )
    .expect_err("linked source must fail closed");

    assert!(state
        .db
        .list_core_skills()
        .expect("list core Skills")
        .is_empty());
    assert!(!skill_dir(home, ManagedClientId::Codex, "linked").exists());
    assert!(!skill_dir(home, ManagedClientId::Opencode, "linked").exists());
    assert_eq!(
        fs::read(outside.join("SKILL.md")).expect("read outside Skill"),
        b"---\nname: linked\n---\n"
    );
    assert_eq!(state.local_scan_writes.pending_count(), 0);
    assert_eq!(state.local_scan_writes.last_generation(), 0);
}

#[test]
#[serial]
fn conflicting_target_is_rejected_before_any_other_target_is_written() {
    let _guard = test_mutex().lock().expect("acquire test mutex");
    reset_test_fs();
    let home = ensure_test_home();
    write_skill(
        home,
        ManagedClientId::Claude,
        "conflict",
        "authoritative source",
    );
    write_skill(
        home,
        ManagedClientId::Opencode,
        "conflict",
        "external target edit",
    );
    let external_before =
        fs::read(skill_dir(home, ManagedClientId::Opencode, "conflict").join("SKILL.md"))
            .expect("read external target");
    let state = create_test_state().expect("create state");

    let error = LocalSkillService::import_from_live(
        &state,
        vec![LocalSkillImport {
            directory: "conflict".to_string(),
            source_client: ManagedClientId::Claude,
            apps: all_apps(),
        }],
    )
    .expect_err("different target content requires conflict resolution");

    let message = error.to_string();
    assert!(message.contains("与所选 Claude 来源内容不同"), "{message}");
    assert!(!message.contains("外部修改"), "{message}");

    assert!(state
        .db
        .list_core_skills()
        .expect("list core Skills")
        .is_empty());
    assert!(
        !skill_dir(home, ManagedClientId::Codex, "conflict").exists(),
        "full preflight must run before the first target write"
    );
    assert_eq!(
        fs::read(skill_dir(home, ManagedClientId::Opencode, "conflict").join("SKILL.md"))
            .expect("read preserved external target"),
        external_before
    );
}
