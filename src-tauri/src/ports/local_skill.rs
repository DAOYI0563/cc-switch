use serde::{Deserialize, Serialize};

use crate::domain::{skill_tree_is_cloud_eligible, LocalSkill, ManagedClientId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkillFile {
    pub relative_path: String,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkillTree {
    pub directories: Vec<String>,
    pub files: Vec<LocalSkillFile>,
    pub content_hash: String,
    pub total_size_bytes: u64,
    pub file_count: u64,
}

impl LocalSkillTree {
    pub fn file(&self, relative_path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|file| file.relative_path == relative_path)
            .map(|file| file.contents.as_slice())
    }

    pub fn is_cloud_eligible(&self) -> bool {
        let largest_file_size_bytes = self
            .files
            .iter()
            .map(|file| file.contents.len() as u64)
            .max()
            .unwrap_or(0);
        skill_tree_is_cloud_eligible(
            self.total_size_bytes,
            self.file_count,
            largest_file_size_bytes,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkillTreeSnapshot {
    pub client: ManagedClientId,
    pub directory: String,
    pub tree: Option<LocalSkillTree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkillLiveCandidate {
    pub client: ManagedClientId,
    pub directory: String,
    pub path: String,
    pub tree: LocalSkillTree,
}

/// One validly named first-level entry below a fixed client `skills` root.
///
/// Listing intentionally does not read `SKILL.md` or capture the directory
/// tree. A candidate may still be a link or otherwise unsafe; guarded manifest
/// reads and full capture classify those cases without hiding managed copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSkillDirectoryCandidate {
    pub client: ManagedClientId,
    pub directory: String,
    pub path: String,
}

pub trait LocalSkillTreePort {
    fn list_directories(
        &self,
        client: ManagedClientId,
    ) -> Result<Vec<LocalSkillDirectoryCandidate>, LocalSkillTreeError>;

    /// Read only the ordinary, non-link `SKILL.md` for a listed directory.
    /// Missing manifests are returned as `None`; this method never captures the
    /// rest of the tree and never computes a tree digest.
    fn read_manifest(
        &self,
        candidate: &LocalSkillDirectoryCandidate,
    ) -> Result<Option<Vec<u8>>, LocalSkillTreeError>;

    fn capture(
        &self,
        client: ManagedClientId,
        directory: &str,
    ) -> Result<LocalSkillTreeSnapshot, LocalSkillTreeError>;

    fn replace(
        &self,
        client: ManagedClientId,
        directory: &str,
        tree: &LocalSkillTree,
    ) -> Result<(), LocalSkillTreeError>;

    fn restore(&self, snapshot: &LocalSkillTreeSnapshot) -> Result<(), LocalSkillTreeError>;

    fn remove(&self, client: ManagedClientId, directory: &str) -> Result<(), LocalSkillTreeError>;
}

pub trait LocalSkillRepository {
    fn list_local_skills(&self) -> Result<Vec<LocalSkill>, LocalSkillRepositoryError>;

    fn get_local_skill(&self, id: &str) -> Result<Option<LocalSkill>, LocalSkillRepositoryError>;

    /// Persist every record in one SQLite transaction or persist none of them.
    fn save_local_skills(&self, skills: &[LocalSkill]) -> Result<(), LocalSkillRepositoryError>;

    fn delete_local_skill(&self, id: &str) -> Result<bool, LocalSkillRepositoryError>;

    /// Atomically apply one local-authority index refresh and return the complete
    /// post-transaction index. Implementations must not expose partial upserts or
    /// deletes if any statement fails.
    fn reconcile_local_skills(
        &self,
        upserts: &[LocalSkill],
        removed_ids: &[String],
    ) -> Result<Vec<LocalSkill>, LocalSkillRepositoryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSkillTreeErrorCode {
    InvalidPath,
    LinkNotAllowed,
    NotFound,
    InvalidTree,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillTreeError {
    pub code: LocalSkillTreeErrorCode,
    pub message: String,
}

impl LocalSkillTreeError {
    pub fn new(code: LocalSkillTreeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LocalSkillTreeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalSkillTreeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSkillRepositoryError {
    pub message: String,
}

impl LocalSkillRepositoryError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for LocalSkillRepositoryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalSkillRepositoryError {}
