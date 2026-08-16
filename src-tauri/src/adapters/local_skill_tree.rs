use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::domain::{validate_skill_directory, ManagedClientId};
use crate::ports::{
    LocalSkillFile, LocalSkillLiveCandidate, LocalSkillTree, LocalSkillTreeError,
    LocalSkillTreeErrorCode, LocalSkillTreePort, LocalSkillTreeSnapshot, WslPathAccess,
    WslPathError, WslPathErrorCode, WslPathGuard, WslPathScope,
};

use super::wsl_path_guard::SafeWslPathGuard;
use super::wsl_paths::FixedWslPathResolver;

/// Protected ordinary-file tree access for the three fixed live Skill roots.
#[derive(Debug, Clone)]
pub struct LocalSkillTreeAdapter {
    guard: SafeWslPathGuard<FixedWslPathResolver>,
}

impl LocalSkillTreeAdapter {
    pub fn runtime() -> Self {
        Self {
            guard: SafeWslPathGuard::new(FixedWslPathResolver::runtime()),
        }
    }

    /// Strict scan used by background reconciliation. Unlike the import UI's
    /// tolerant scan, one unsafe or malformed candidate fails the target.
    pub fn scan_strict(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
        self.scan_candidates(client, true)
    }

    fn scan_candidates(
        &self,
        client: ManagedClientId,
        strict: bool,
    ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
        let root = self.resolve(client, "skills", WslPathAccess::Read)?;
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(io_error(&root, error)),
        };
        let mut candidates = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&root, error))?;
            let directory = entry.file_name().into_string().map_err(|_| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 目录名无法表示为 UTF-8",
                )
            })?;
            if validate_skill_directory(&directory).is_err() {
                if strict {
                    return Err(LocalSkillTreeError::new(
                        LocalSkillTreeErrorCode::InvalidPath,
                        "Skill 根目录包含无效目录名",
                    ));
                }
                continue;
            }
            let tree = match self.read_tree(client, &directory) {
                Ok(Some(tree)) => tree,
                Ok(None) => continue,
                Err(error)
                    if !strict
                        && matches!(
                            error.code,
                            LocalSkillTreeErrorCode::LinkNotAllowed
                                | LocalSkillTreeErrorCode::InvalidTree
                        ) =>
                {
                    log::warn!(
                        "跳过不安全或无效的 {} Skill {}: {}",
                        client.as_str(),
                        directory,
                        error
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };
            candidates.push(LocalSkillLiveCandidate {
                client,
                directory,
                path: entry.path().to_string_lossy().to_string(),
                tree,
            });
        }
        candidates.sort_by(|left, right| left.directory.cmp(&right.directory));
        Ok(candidates)
    }

    fn resolve(
        &self,
        client: ManagedClientId,
        relative: &str,
        access: WslPathAccess,
    ) -> Result<PathBuf, LocalSkillTreeError> {
        self.guard
            .resolve(WslPathScope::ClientConfig(client), relative, access)
            .map_err(map_path_error)
    }

    fn skill_relative(directory: &str) -> Result<String, LocalSkillTreeError> {
        validate_skill_directory(directory).map_err(|error| {
            LocalSkillTreeError::new(LocalSkillTreeErrorCode::InvalidPath, error.to_string())
        })?;
        Ok(format!("skills/{directory}"))
    }

    fn read_tree(
        &self,
        client: ManagedClientId,
        directory: &str,
    ) -> Result<Option<LocalSkillTree>, LocalSkillTreeError> {
        let base_relative = Self::skill_relative(directory)?;
        let base = self.resolve(client, &base_relative, WslPathAccess::Read)?;
        let metadata = match fs::symlink_metadata(&base) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(&base, error)),
        };
        if !metadata.is_dir() {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                format!("Skill 路径不是目录: {}", base.display()),
            ));
        }

        let mut directories = Vec::new();
        let mut files = Vec::new();
        self.read_entries(
            client,
            &base_relative,
            &base,
            Path::new(""),
            &mut directories,
            &mut files,
        )?;
        directories.sort();
        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        if !files.iter().any(|file| file.relative_path == "SKILL.md") {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                format!("Skill 目录缺少 SKILL.md: {}", base.display()),
            ));
        }

        let total_size_bytes = files.iter().try_fold(0_u64, |total, file| {
            total
                .checked_add(file.contents.len() as u64)
                .ok_or_else(|| {
                    LocalSkillTreeError::new(
                        LocalSkillTreeErrorCode::InvalidTree,
                        "Skill 文件总大小溢出",
                    )
                })
        })?;
        let file_count = files.len() as u64;
        let content_hash = hash_tree(&directories, &files);
        Ok(Some(LocalSkillTree {
            directories,
            files,
            content_hash,
            total_size_bytes,
            file_count,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn read_entries(
        &self,
        client: ManagedClientId,
        base_relative: &str,
        current: &Path,
        relative: &Path,
        directories: &mut Vec<String>,
        files: &mut Vec<LocalSkillFile>,
    ) -> Result<(), LocalSkillTreeError> {
        let mut entries = fs::read_dir(current)
            .map_err(|error| io_error(current, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(current, error))?;
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let name = entry.file_name().into_string().map_err(|_| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 包含无法表示为 UTF-8 的路径",
                )
            })?;
            if should_ignore_entry_name(&name) {
                continue;
            }
            let child_relative = relative.join(&name);
            let child_relative_string = portable_relative(&child_relative)?;
            let guarded_relative = format!("{base_relative}/{child_relative_string}");
            let guarded_path = self.resolve(client, &guarded_relative, WslPathAccess::Read)?;
            let metadata = fs::symlink_metadata(&guarded_path)
                .map_err(|error| io_error(&guarded_path, error))?;
            if metadata.is_dir() {
                directories.push(child_relative_string);
                self.read_entries(
                    client,
                    base_relative,
                    &guarded_path,
                    &child_relative,
                    directories,
                    files,
                )?;
            } else if metadata.is_file() {
                files.push(LocalSkillFile {
                    relative_path: child_relative_string,
                    contents: fs::read(&guarded_path)
                        .map_err(|error| io_error(&guarded_path, error))?,
                });
            } else {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    format!("Skill 包含不支持的文件类型: {}", guarded_path.display()),
                ));
            }
        }
        Ok(())
    }

    fn validate_tree(tree: &LocalSkillTree) -> Result<(), LocalSkillTreeError> {
        if tree.file("SKILL.md").is_none() {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                "Skill 文件树缺少 SKILL.md",
            ));
        }
        for relative in tree
            .directories
            .iter()
            .map(String::as_str)
            .chain(tree.files.iter().map(|file| file.relative_path.as_str()))
        {
            validate_tree_relative(relative)?;
            if relative.split('/').any(should_ignore_entry_name) {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    format!("Skill 文件树包含应忽略的路径: {relative}"),
                ));
            }
        }
        if hash_tree(&tree.directories, &tree.files) != tree.content_hash
            || tree.file_count != tree.files.len() as u64
            || tree.total_size_bytes
                != tree
                    .files
                    .iter()
                    .map(|file| file.contents.len() as u64)
                    .sum::<u64>()
        {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                "Skill 文件树摘要不匹配",
            ));
        }
        Ok(())
    }

    fn build_tree_at(
        &self,
        client: ManagedClientId,
        base_relative: &str,
        tree: &LocalSkillTree,
    ) -> Result<PathBuf, LocalSkillTreeError> {
        let base = self.resolve(client, base_relative, WslPathAccess::Write)?;
        fs::create_dir_all(&base).map_err(|error| io_error(&base, error))?;
        self.resolve(client, base_relative, WslPathAccess::Write)?;

        let mut directories = tree.directories.clone();
        directories.sort_by_key(|relative| relative.matches('/').count());
        for relative in directories {
            let path_relative = format!("{base_relative}/{relative}");
            let path = self.resolve(client, &path_relative, WslPathAccess::Write)?;
            fs::create_dir_all(&path).map_err(|error| io_error(&path, error))?;
            self.resolve(client, &path_relative, WslPathAccess::Write)?;
        }
        for file in &tree.files {
            let path_relative = format!("{base_relative}/{}", file.relative_path);
            let path = self.resolve(client, &path_relative, WslPathAccess::Write)?;
            let parent = path.parent().ok_or_else(|| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 文件没有父目录",
                )
            })?;
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
            self.resolve(client, &path_relative, WslPathAccess::Write)?;
            crate::config::atomic_write(&path, &file.contents).map_err(|error| {
                LocalSkillTreeError::new(LocalSkillTreeErrorCode::Io, error.to_string())
            })?;
        }
        Ok(base)
    }

    fn remove_existing(
        &self,
        client: ManagedClientId,
        relative: &str,
    ) -> Result<(), LocalSkillTreeError> {
        let path = self.resolve(client, relative, WslPathAccess::Write)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                // Reading validates every descendant before recursive removal.
                let directory =
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .ok_or_else(|| {
                            LocalSkillTreeError::new(
                                LocalSkillTreeErrorCode::InvalidPath,
                                "Skill 目录名无法表示为 UTF-8",
                            )
                        })?;
                self.read_tree(client, directory)?;
                self.validate_removal_entries(client, relative, &path, Path::new(""))?;
                self.resolve(client, relative, WslPathAccess::Write)?;
                fs::remove_dir_all(&path).map_err(|error| io_error(&path, error))
            }
            Ok(_) => Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                format!("Skill 目标不是目录: {}", path.display()),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(&path, error)),
        }
    }

    fn replace_without_snapshot(
        &self,
        client: ManagedClientId,
        directory: &str,
        tree: &LocalSkillTree,
    ) -> Result<(), LocalSkillTreeError> {
        Self::validate_tree(tree)?;
        let target_relative = Self::skill_relative(directory)?;
        let target = self.resolve(client, &target_relative, WslPathAccess::Write)?;
        match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => {
                self.remove_managed_entries(client, &target_relative, &target, Path::new(""))?;
            }
            Ok(_) => {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    format!("Skill 目标不是目录: {}", target.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(&target, error)),
        }
        self.build_tree_at(client, &target_relative, tree)?;
        Ok(())
    }

    fn remove_managed_entries(
        &self,
        client: ManagedClientId,
        base_relative: &str,
        current: &Path,
        relative: &Path,
    ) -> Result<(), LocalSkillTreeError> {
        let mut entries = fs::read_dir(current)
            .map_err(|error| io_error(current, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(current, error))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().into_string().map_err(|_| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 包含无法表示为 UTF-8 的路径",
                )
            })?;
            if should_ignore_entry_name(&name) {
                continue;
            }
            let child_relative = relative.join(&name);
            let child_relative_string = portable_relative(&child_relative)?;
            let guarded_relative = format!("{base_relative}/{child_relative_string}");
            let guarded_path = self.resolve(client, &guarded_relative, WslPathAccess::Write)?;
            let metadata = fs::symlink_metadata(&guarded_path)
                .map_err(|error| io_error(&guarded_path, error))?;
            if metadata.is_dir() {
                self.remove_managed_entries(client, base_relative, &guarded_path, &child_relative)?;
                match fs::remove_dir(&guarded_path) {
                    Ok(()) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                        ) => {}
                    Err(error) => return Err(io_error(&guarded_path, error)),
                }
            } else if metadata.is_file() {
                fs::remove_file(&guarded_path).map_err(|error| io_error(&guarded_path, error))?;
            } else {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    format!("Skill 包含不支持的文件类型: {}", guarded_path.display()),
                ));
            }
        }
        Ok(())
    }

    fn validate_removal_entries(
        &self,
        client: ManagedClientId,
        base_relative: &str,
        current: &Path,
        relative: &Path,
    ) -> Result<(), LocalSkillTreeError> {
        let entries = fs::read_dir(current)
            .map_err(|error| io_error(current, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| io_error(current, error))?;
        for entry in entries {
            let name = entry.file_name().into_string().map_err(|_| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 包含无法表示为 UTF-8 的路径",
                )
            })?;
            let child_relative = relative.join(&name);
            let child_relative_string = portable_relative(&child_relative)?;
            let guarded_relative = format!("{base_relative}/{child_relative_string}");
            let guarded_path = self.resolve(client, &guarded_relative, WslPathAccess::Write)?;
            let metadata = fs::symlink_metadata(&guarded_path)
                .map_err(|error| io_error(&guarded_path, error))?;
            if metadata.is_dir() {
                self.validate_removal_entries(
                    client,
                    base_relative,
                    &guarded_path,
                    &child_relative,
                )?;
            } else if !metadata.is_file() {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    format!("Skill 包含不支持的文件类型: {}", guarded_path.display()),
                ));
            }
        }
        Ok(())
    }
}

impl Default for LocalSkillTreeAdapter {
    fn default() -> Self {
        Self::runtime()
    }
}

impl LocalSkillTreePort for LocalSkillTreeAdapter {
    fn scan(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
        self.scan_candidates(client, false)
    }

    fn capture(
        &self,
        client: ManagedClientId,
        directory: &str,
    ) -> Result<LocalSkillTreeSnapshot, LocalSkillTreeError> {
        Ok(LocalSkillTreeSnapshot {
            client,
            directory: directory.to_string(),
            tree: self.read_tree(client, directory)?,
        })
    }

    fn replace(
        &self,
        client: ManagedClientId,
        directory: &str,
        tree: &LocalSkillTree,
    ) -> Result<(), LocalSkillTreeError> {
        let original = self.capture(client, directory)?;
        if let Err(primary) = self.replace_without_snapshot(client, directory, tree) {
            return match self.restore(&original) {
                Ok(()) => Err(primary),
                Err(rollback) => Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::Io,
                    format!("{primary}; Skill 回滚也失败: {rollback}"),
                )),
            };
        }
        Ok(())
    }

    fn restore(&self, snapshot: &LocalSkillTreeSnapshot) -> Result<(), LocalSkillTreeError> {
        match &snapshot.tree {
            Some(tree) => self.replace_without_snapshot(snapshot.client, &snapshot.directory, tree),
            None => self.remove(snapshot.client, &snapshot.directory),
        }
    }

    fn remove(&self, client: ManagedClientId, directory: &str) -> Result<(), LocalSkillTreeError> {
        let relative = Self::skill_relative(directory)?;
        self.remove_existing(client, &relative)
    }
}

fn should_ignore_entry_name(value: &str) -> bool {
    let normalized = value.to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "out"
            | "coverage"
            | ".next"
            | ".nuxt"
            | ".svelte-kit"
            | ".turbo"
            | ".vite"
            | ".parcel-cache"
            | ".cache"
            | "cache"
            | "__pycache__"
            | ".pytest_cache"
            | ".mypy_cache"
            | ".ruff_cache"
            | ".tox"
            | ".gradle"
            | "tmp"
            | "temp"
            | ".tmp"
            | ".temp"
            | ".ds_store"
            | "thumbs.db"
            | ".coverage"
            | ".eslintcache"
            | ".stylelintcache"
    ) || normalized.ends_with(".tmp")
        || normalized.ends_with(".temp")
        || normalized.ends_with(".swp")
        || normalized.ends_with(".swo")
        || normalized.ends_with('~')
        || normalized.ends_with(":zone.identifier")
        || normalized.ends_with(":zone.identifier:$data")
}

fn portable_relative(path: &Path) -> Result<String, LocalSkillTreeError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str().ok_or_else(|| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 路径无法表示为 UTF-8",
                )
            })?),
            _ => {
                return Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 文件必须使用干净的相对路径",
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn validate_tree_relative(value: &str) -> Result<(), LocalSkillTreeError> {
    if value.is_empty()
        || value.contains('\\')
        || Path::new(value).is_absolute()
        || value.split('/').any(|part| {
            part.is_empty()
                || part == "."
                || part == ".."
                || part.contains(':')
                || part.chars().any(char::is_control)
        })
    {
        Err(LocalSkillTreeError::new(
            LocalSkillTreeErrorCode::InvalidPath,
            format!("无效的 Skill 树相对路径: {value}"),
        ))
    } else {
        Ok(())
    }
}

fn hash_tree(directories: &[String], files: &[LocalSkillFile]) -> String {
    let mut directories = directories.to_vec();
    directories.sort();
    let mut files = files.to_vec();
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let mut hasher = Sha256::new();
    for directory in directories {
        hasher.update(b"D\0");
        hasher.update((directory.len() as u64).to_le_bytes());
        hasher.update(directory.as_bytes());
    }
    for file in files {
        hasher.update(b"F\0");
        hasher.update((file.relative_path.len() as u64).to_le_bytes());
        hasher.update(file.relative_path.as_bytes());
        hasher.update((file.contents.len() as u64).to_le_bytes());
        hasher.update(&file.contents);
    }
    format!("{:x}", hasher.finalize())
}

fn map_path_error(error: WslPathError) -> LocalSkillTreeError {
    let code = match error.code {
        WslPathErrorCode::LinkNotAllowed => LocalSkillTreeErrorCode::LinkNotAllowed,
        WslPathErrorCode::InvalidRelativePath
        | WslPathErrorCode::ScopeIsFile
        | WslPathErrorCode::ReadOnlyScope
        | WslPathErrorCode::PathEscape => LocalSkillTreeErrorCode::InvalidPath,
        WslPathErrorCode::InspectionFailed => LocalSkillTreeErrorCode::Io,
    };
    LocalSkillTreeError::new(code, error.to_string())
}

fn io_error(path: &Path, error: std::io::Error) -> LocalSkillTreeError {
    LocalSkillTreeError::new(
        LocalSkillTreeErrorCode::Io,
        format!("{}: {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn capture_replace_and_restore_preserve_nested_plain_files() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let source = temp.path().join(".claude/skills/example");
        fs::create_dir_all(source.join("empty")).unwrap();
        fs::write(source.join("SKILL.md"), b"---\nname: example\n---\n").unwrap();
        let adapter = LocalSkillTreeAdapter::runtime();
        let source = adapter
            .capture(ManagedClientId::Claude, "example")
            .unwrap()
            .tree
            .unwrap();
        let original = adapter.capture(ManagedClientId::Codex, "example").unwrap();

        adapter
            .replace(ManagedClientId::Codex, "example", &source)
            .unwrap();
        assert!(temp.path().join(".codex/skills/example/SKILL.md").is_file());
        assert!(temp.path().join(".codex/skills/example/empty").is_dir());
        adapter.restore(&original).unwrap();
        assert!(!temp.path().join(".codex/skills/example").exists());
    }

    #[test]
    #[serial]
    fn ignored_entries_do_not_affect_hash_or_copy_and_existing_ignored_entries_survive_replace() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let source = temp.path().join(".claude/skills/example");
        fs::create_dir_all(source.join("references")).unwrap();
        fs::create_dir_all(source.join(".git")).unwrap();
        fs::create_dir_all(source.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(source.join("dist")).unwrap();
        fs::create_dir_all(source.join(".cache")).unwrap();
        fs::create_dir_all(source.join("tmp")).unwrap();
        fs::write(source.join("SKILL.md"), b"---\nname: example\n---\n").unwrap();
        fs::write(source.join("references/keep.md"), b"keep").unwrap();
        fs::write(source.join(".git/config"), b"source git").unwrap();
        fs::write(source.join("node_modules/pkg/index.js"), b"dependency").unwrap();
        fs::write(source.join("dist/bundle.js"), b"build").unwrap();
        fs::write(source.join(".cache/state"), b"cache").unwrap();
        fs::write(source.join("tmp/work"), b"temporary").unwrap();
        fs::write(source.join("editor.swp"), b"swap").unwrap();
        fs::write(source.join("partial.tmp"), b"partial").unwrap();

        let target = temp.path().join(".codex/skills/example");
        fs::create_dir_all(target.join(".git")).unwrap();
        fs::write(target.join("SKILL.md"), b"---\nname: old\n---\n").unwrap();
        fs::write(target.join(".git/config"), b"target git").unwrap();
        fs::write(target.join("local.tmp"), b"target temporary").unwrap();

        let adapter = LocalSkillTreeAdapter::runtime();
        let first = adapter
            .capture(ManagedClientId::Claude, "example")
            .unwrap()
            .tree
            .unwrap();
        assert_eq!(first.file_count, 2);
        assert_eq!(
            first.total_size_bytes,
            b"---\nname: example\n---\n".len() as u64 + b"keep".len() as u64
        );
        assert!(first.file(".git/config").is_none());
        assert!(first.file("node_modules/pkg/index.js").is_none());
        assert!(first.file("dist/bundle.js").is_none());
        assert!(first.file("editor.swp").is_none());

        fs::write(source.join(".git/config"), b"changed ignored bytes").unwrap();
        fs::write(source.join("partial.tmp"), b"changed ignored temporary").unwrap();
        let second = adapter
            .capture(ManagedClientId::Claude, "example")
            .unwrap()
            .tree
            .unwrap();
        assert_eq!(second.content_hash, first.content_hash);

        adapter
            .replace(ManagedClientId::Codex, "example", &second)
            .unwrap();
        assert_eq!(fs::read(target.join(".git/config")).unwrap(), b"target git");
        assert_eq!(
            fs::read(target.join("local.tmp")).unwrap(),
            b"target temporary"
        );

        adapter
            .replace(ManagedClientId::Opencode, "example", &second)
            .unwrap();
        let copied = temp.path().join(".config/opencode/skills/example");
        assert!(!copied.join(".git").exists());
        assert!(!copied.join("node_modules").exists());
        assert!(!copied.join("partial.tmp").exists());
    }

    #[test]
    fn windows_zone_identifier_sidecars_are_ignored() {
        assert!(should_ignore_entry_name("download.example:Zone.Identifier"));
        assert!(should_ignore_entry_name(
            "download.example:Zone.Identifier:$DATA"
        ));
    }
}
