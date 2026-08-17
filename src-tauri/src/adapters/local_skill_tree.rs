use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::domain::{validate_skill_directory, ManagedClientId};
use crate::ports::{
    LocalSkillDirectoryCandidate, LocalSkillFile, LocalSkillLiveCandidate, LocalSkillTree,
    LocalSkillTreeError, LocalSkillTreeErrorCode, LocalSkillTreePort, LocalSkillTreeSnapshot,
    WslPathAccess, WslPathError, WslPathErrorCode, WslPathGuard, WslPathScope,
};
// `open_checked_manifest` is `#[cfg(windows)]`, so this trait is only used on Windows.
// On other hosts it is an unused import; that is expected and harmless.
#[cfg(windows)]
use crate::ports::WslPathResolver;
#[cfg(not(windows))]
#[allow(unused_imports)]
use crate::ports::WslPathResolver;

use super::wsl_path_guard::SafeWslPathGuard;
use super::wsl_paths::FixedWslPathResolver;

/// Protected ordinary-file tree access for the three fixed live Skill roots.
#[derive(Debug, Clone)]
pub struct LocalSkillTreeAdapter {
    resolver: FixedWslPathResolver,
    guard: SafeWslPathGuard<FixedWslPathResolver>,
}

impl LocalSkillTreeAdapter {
    pub fn runtime() -> Self {
        let resolver = FixedWslPathResolver::runtime();
        Self {
            guard: SafeWslPathGuard::new(resolver.clone()),
            resolver,
        }
    }

    /// Strict full-tree scan used by background reconciliation. One unsafe or
    /// malformed candidate fails the target so reconciliation cannot accept a
    /// partial summary.
    pub fn scan_strict(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
        self.ensure_home_accessible()?;
        let root = self.resolve(client, "skills", WslPathAccess::Read)?;
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.ensure_home_accessible()?;
                return Ok(Vec::new());
            }
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
            validate_skill_directory(&directory).map_err(|_| {
                LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidPath,
                    "Skill 根目录包含无效目录名",
                )
            })?;
            let Some(tree) = self.read_tree(client, &directory)? else {
                continue;
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

    /// Reads only database-known directories for background reconciliation.
    /// Unknown first-level entries are deliberately never enumerated here; they
    /// remain visible exclusively through the explicit import flow.
    pub fn scan_managed(
        &self,
        client: ManagedClientId,
        directories: impl IntoIterator<Item = String>,
    ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
        self.ensure_home_accessible()?;
        let mut directories: Vec<_> = directories.into_iter().collect();
        directories.sort();
        directories.dedup();

        let mut candidates = Vec::with_capacity(directories.len());
        for directory in directories {
            let Some(tree) = self.read_tree(client, &directory)? else {
                continue;
            };
            let relative = Self::skill_relative(&directory)?;
            let path = self.resolve(client, &relative, WslPathAccess::Read)?;
            candidates.push(LocalSkillLiveCandidate {
                client,
                directory,
                path: path.to_string_lossy().to_string(),
                tree,
            });
        }
        Ok(candidates)
    }

    /// First-level listing shared by managed refresh and explicit import.
    /// No manifest or descendant is read here; validly named unsafe entries stay
    /// visible so managed reconciliation can report them instead of deleting.
    fn list_safe_directories(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillDirectoryCandidate>, LocalSkillTreeError> {
        self.ensure_home_accessible()?;
        let root = self.resolve(client, "skills", WslPathAccess::Read)?;
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.ensure_home_accessible()?;
                return Ok(Vec::new());
            }
            Err(error) => return Err(io_error(&root, error)),
        };
        let mut candidates = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&root, error))?;
            let directory = match entry.file_name().into_string() {
                Ok(directory) => directory,
                Err(_) => {
                    log::warn!("跳过名称不是 UTF-8 的 {} Skill 目录项", client.as_str());
                    continue;
                }
            };
            if validate_skill_directory(&directory).is_err() {
                log::warn!(
                    "跳过名称无效的 {} Skill 目录: {}",
                    client.as_str(),
                    directory
                );
                continue;
            }
            // Retain every valid first-level name. Managed reconciliation must
            // see linked or otherwise invalid entries and preserve the database
            // record with an InvalidCopy issue instead of treating it as deleted.
            // Unknown entries are still guarded by the bounded manifest read.
            candidates.push(LocalSkillDirectoryCandidate {
                client,
                directory,
                path: entry.path().to_string_lossy().to_string(),
            });
        }
        candidates.sort_by(|left, right| left.directory.cmp(&right.directory));
        Ok(candidates)
    }

    fn ensure_home_accessible(&self) -> Result<(), LocalSkillTreeError> {
        let home = self.resolver.windows_home();
        let metadata = fs::symlink_metadata(home).map_err(|error| io_error(home, error))?;
        if metadata_is_link_or_reparse(&metadata) {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::LinkNotAllowed,
                "WSL 用户目录不得是链接或重解析点",
            ));
        }
        if !metadata.is_dir() {
            return Err(LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidTree,
                "WSL 用户目录不是普通目录",
            ));
        }
        Ok(())
    }

    fn read_manifest_only(
        &self,
        candidate: &LocalSkillDirectoryCandidate,
    ) -> Result<Option<Vec<u8>>, LocalSkillTreeError> {
        let base_relative = Self::skill_relative(&candidate.directory)?;
        let manifest_relative = format!("{base_relative}/SKILL.md");
        let manifest = self.resolve(candidate.client, &manifest_relative, WslPathAccess::Read)?;
        let mut file =
            match self.open_checked_manifest(candidate.client, &manifest_relative, &manifest) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                    return Err(LocalSkillTreeError::new(
                        LocalSkillTreeErrorCode::InvalidTree,
                        "Skill manifest 无法安全读取",
                    ));
                }
                Err(error) => return Err(io_error(&manifest, error)),
            };
        match read_manifest_metadata_prefix(&mut file) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::InvalidTree,
                    "Skill manifest 元数据无效",
                ))
            }
            Err(error) => Err(io_error(&manifest, error)),
        }
    }

    #[cfg(not(windows))]
    fn open_checked_manifest(
        &self,
        _client: ManagedClientId,
        _manifest_relative: &str,
        manifest: &Path,
    ) -> std::io::Result<fs::File> {
        let metadata = fs::symlink_metadata(manifest)?;
        if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Skill manifest 不是普通的非链接文件",
            ));
        }
        fs::File::open(manifest)
    }

    #[cfg(windows)]
    fn open_checked_manifest(
        &self,
        client: ManagedClientId,
        manifest_relative: &str,
        manifest: &Path,
    ) -> std::io::Result<fs::File> {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let home = self.resolver.windows_home();
        let home_handle = fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(home)?;
        if home_handle.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "WSL 用户目录不得是重解析点",
            ));
        }
        let final_home = windows_final_path(&home_handle)?;
        let config_root = self.resolver.client_config_root(client).windows;
        let config_relative = config_root.strip_prefix(home).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Skill 客户端根目录不在固定 WSL 用户目录内",
            )
        })?;
        let expected = final_home.join(config_relative).join(manifest_relative);

        let file = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(manifest)?;
        if !file.metadata()?.is_file()
            || !windows_paths_equal(&windows_final_path(&file)?, &expected)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Skill manifest 解析后不在固定目录或不是普通文件",
            ));
        }
        Ok(file)
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
        validate_skill_directory(directory).map_err(|_| {
            LocalSkillTreeError::new(
                LocalSkillTreeErrorCode::InvalidPath,
                "Skill 目录名不符合安全边界",
            )
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
                "Skill 路径不是目录",
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
                "Skill 目录缺少 SKILL.md",
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
                    "Skill 包含不支持的文件类型",
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
            crate::config::atomic_write(&path, &file.contents).map_err(|_| {
                LocalSkillTreeError::new(LocalSkillTreeErrorCode::Io, "Skill live 原子写入失败")
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
                "Skill 目标不是目录",
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
                    "Skill 目标不是目录",
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
                    "Skill 包含不支持的文件类型",
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
                    "Skill 包含不支持的文件类型",
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
    fn list_directories(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillDirectoryCandidate>, LocalSkillTreeError> {
        self.list_safe_directories(client)
    }

    fn read_manifest(
        &self,
        candidate: &LocalSkillDirectoryCandidate,
    ) -> Result<Option<Vec<u8>>, LocalSkillTreeError> {
        self.read_manifest_only(candidate)
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
                Err(_) => Err(LocalSkillTreeError::new(
                    LocalSkillTreeErrorCode::Io,
                    format!("Skill live 替换与回滚失败: primary_kind={:?}", primary.code),
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

const MAX_MANIFEST_METADATA_BYTES: usize = 256 * 1024;

fn read_manifest_metadata_prefix(file: &mut fs::File) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(file);
    let mut contents = Vec::new();
    let first_start = read_bounded_line(&mut reader, &mut contents)?;
    if trim_line_ending(&contents[first_start..]) != b"---" {
        return Ok(contents);
    }

    loop {
        let line_start = read_bounded_line(&mut reader, &mut contents)?;
        if line_start == contents.len() || trim_line_ending(&contents[line_start..]) == b"---" {
            return Ok(contents);
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, contents: &mut Vec<u8>) -> std::io::Result<usize> {
    if contents.len() >= MAX_MANIFEST_METADATA_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Skill manifest 元数据超过 256 KiB",
        ));
    }
    let start = contents.len();
    reader
        .take((MAX_MANIFEST_METADATA_BYTES - contents.len()) as u64)
        .read_until(b'\n', contents)?;
    if contents.len() == MAX_MANIFEST_METADATA_BYTES && !contents[start..].ends_with(b"\n") {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Skill manifest 元数据超过 256 KiB",
        ));
    }
    Ok(start)
}

fn trim_line_ending(mut value: &[u8]) -> &[u8] {
    if value.ends_with(b"\n") {
        value = &value[..value.len() - 1];
    }
    if value.ends_with(b"\r") {
        value = &value[..value.len() - 1];
    }
    value
}

fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
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

#[cfg(windows)]
fn windows_final_path(file: &fs::File) -> std::io::Result<PathBuf> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    let handle = file.as_raw_handle() as HANDLE;
    let required = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            std::ptr::null_mut(),
            0,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if required == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buffer = vec![0_u16; required as usize + 1];
    let written = unsafe {
        GetFinalPathNameByHandleW(
            handle,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if written == 0 || written as usize >= buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    buffer.truncate(written as usize);
    let value = String::from_utf16(&buffer)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(PathBuf::from(
        value
            .strip_prefix(r"\\?\UNC\")
            .map(|suffix| format!(r"\\{suffix}"))
            .or_else(|| value.strip_prefix(r"\\?\").map(str::to_string))
            .unwrap_or(value),
    ))
}

#[cfg(windows)]
fn windows_paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy().replace('/', "\\") == right.to_string_lossy().replace('/', "\\")
}

fn hash_tree(directories: &[String], files: &[LocalSkillFile]) -> String {
    let mut directories: Vec<_> = directories.iter().map(String::as_str).collect();
    directories.sort();
    let mut files: Vec<_> = files.iter().collect();
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
    LocalSkillTreeError::new(
        code,
        format!("Skill live 路径校验失败: kind={:?}", error.code),
    )
}

fn io_error(_path: &Path, error: std::io::Error) -> LocalSkillTreeError {
    LocalSkillTreeError::new(
        LocalSkillTreeErrorCode::Io,
        format!("Skill live 文件树 I/O 失败: kind={:?}", error.kind()),
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
    #[serial]
    fn manual_scan_reports_an_inaccessible_fixed_home_instead_of_an_empty_result() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("missing-home");
        let _home = TestHomeGuard::set(&missing);
        let adapter = LocalSkillTreeAdapter::runtime();

        let error = adapter
            .list_directories(ManagedClientId::Claude)
            .expect_err("an inaccessible fixed home must not look like an empty skills root");

        assert_eq!(error.code, LocalSkillTreeErrorCode::Io);
    }

    #[test]
    #[serial]
    fn manual_scan_treats_a_missing_skills_root_as_an_empty_client() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let adapter = LocalSkillTreeAdapter::runtime();

        let candidates = adapter
            .list_directories(ManagedClientId::Claude)
            .expect("an accessible home with no skills root is an empty client");

        assert!(candidates.is_empty());
    }

    #[test]
    fn manifest_discovery_reads_only_the_front_matter_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(
            &path,
            [
                b"---\nname: bounded\ndescription: preview\n---\n".as_slice(),
                &vec![b'x'; MAX_MANIFEST_METADATA_BYTES],
            ]
            .concat(),
        )
        .unwrap();
        let mut file = fs::File::open(path).unwrap();

        assert_eq!(
            read_manifest_metadata_prefix(&mut file).unwrap(),
            b"---\nname: bounded\ndescription: preview\n---\n"
        );
    }

    #[test]
    fn manifest_discovery_rejects_an_unbounded_front_matter() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        fs::write(
            &path,
            [
                b"---\nname: ".as_slice(),
                &vec![b'x'; MAX_MANIFEST_METADATA_BYTES],
            ]
            .concat(),
        )
        .unwrap();
        let mut file = fs::File::open(path).unwrap();

        assert_eq!(
            read_manifest_metadata_prefix(&mut file).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn hash_tree_matches_the_v1_protocol_golden_value_and_ignores_input_order() {
        let skill = LocalSkillFile {
            relative_path: "SKILL.md".to_string(),
            contents: b"---\nname: Golden\n---\n".to_vec(),
        };
        let data = LocalSkillFile {
            relative_path: "assets/data.bin".to_string(),
            contents: vec![0, 1, 2, 255],
        };
        let directories = vec!["empty".to_string(), "assets".to_string()];
        let files = vec![data.clone(), skill.clone()];

        assert_eq!(
            hash_tree(&directories, &files),
            "e2756857e34c5eafc94f16762f1f3741156848918c4dcf5a3c8b83aac8a9e6b4"
        );
        assert_eq!(
            hash_tree(&directories, &files),
            hash_tree(&["assets".to_string(), "empty".to_string()], &[skill, data])
        );
    }

    #[test]
    #[serial]
    fn manual_scan_reads_only_manifest_while_strict_scan_captures_the_full_tree() {
        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let source = temp.path().join(".claude/skills/example");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("SKILL.md"), b"---\nname: example\n---\n").unwrap();
        fs::write(source.join("nested/data.bin"), vec![7_u8; 1024]).unwrap();
        let adapter = LocalSkillTreeAdapter::runtime();

        let candidates = adapter.list_directories(ManagedClientId::Claude).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            adapter.read_manifest(&candidates[0]).unwrap().unwrap(),
            b"---\nname: example\n---\n"
        );

        let strict = adapter.scan_strict(ManagedClientId::Claude).unwrap();
        assert_eq!(strict.len(), 1);
        assert_eq!(strict[0].tree.file_count, 2);
        assert_eq!(strict[0].tree.file("nested/data.bin").unwrap().len(), 1024);
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn manual_scan_lists_linked_entries_but_guarded_manifest_reads_reject_them() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let _home = TestHomeGuard::set(temp.path());
        let root = temp.path().join(".claude/skills");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("SKILL.md"), b"---\nname: outside\n---\n").unwrap();
        symlink(&outside, root.join("linked-directory")).unwrap();
        fs::create_dir_all(root.join("linked-manifest")).unwrap();
        symlink(
            outside.join("SKILL.md"),
            root.join("linked-manifest/SKILL.md"),
        )
        .unwrap();
        let adapter = LocalSkillTreeAdapter::runtime();

        let candidates = adapter.list_directories(ManagedClientId::Claude).unwrap();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].directory, "linked-directory");
        assert_eq!(candidates[1].directory, "linked-manifest");
        for candidate in &candidates {
            assert_eq!(
                adapter.read_manifest(candidate).unwrap_err().code,
                LocalSkillTreeErrorCode::LinkNotAllowed
            );
        }
    }

    #[test]
    fn windows_zone_identifier_sidecars_are_ignored() {
        assert!(should_ignore_entry_name("download.example:Zone.Identifier"));
        assert!(should_ignore_entry_name(
            "download.example:Zone.Identifier:$DATA"
        ));
    }
}
