use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::domain::{
    LocalScanDomain, LocalScanTarget, LocalSkill, LocalSkillImport, ManagedClientId,
    UnmanagedLocalSkill, UnmanagedSkillCopy,
};
use crate::error::AppError;
use crate::ports::{
    LocalSkillRepository, LocalSkillTree, LocalSkillTreePort, LocalSkillTreeSnapshot,
    WslPathResolver,
};
use crate::store::AppState;

pub struct LocalSkillService;

#[derive(Debug, Deserialize)]
struct SkillFrontMatter {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Debug)]
struct PreparedImport {
    record: LocalSkill,
}

/// Read-only SKILL.md preview payload for the skill detail dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDocumentRead {
    pub source_client: ManagedClientId,
    pub size_bytes: u64,
    pub content: String,
}

fn mutation_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

impl LocalSkillService {
    pub fn get_all(state: &AppState) -> Result<Vec<LocalSkill>, AppError> {
        state.db.list_core_skills()
    }

    /// Read-only SKILL.md preview from the skill's first enabled live copy.
    pub fn read_skill_markdown(state: &AppState, id: &str) -> Result<SkillDocumentRead, AppError> {
        let adapter = LocalSkillTreeAdapter::runtime();
        Self::read_markdown_with(state.db.as_ref(), &adapter, id)
    }

    fn read_markdown_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        id: &str,
    ) -> Result<SkillDocumentRead, AppError> {
        let skill = repository
            .get_local_skill(id)
            .map_err(repository_error)?
            .ok_or_else(|| AppError::InvalidInput(format!("未找到 Skill: {id}")))?;
        let source = skill.apps.enabled_clients().next().ok_or_else(|| {
            AppError::InvalidInput("Skill 未启用任何客户端，无法读取内容".to_string())
        })?;
        let snapshot = trees
            .capture(source, &skill.directory)
            .map_err(tree_error)?;
        let tree = snapshot.tree.ok_or_else(|| {
            AppError::InvalidInput(format!(
                "来源客户端 {source} 中不存在 Skill: {}",
                skill.directory
            ))
        })?;
        let contents = tree
            .file("SKILL.md")
            .ok_or_else(|| AppError::InvalidInput("Skill 目录缺少 SKILL.md".to_string()))?;
        let content = std::str::from_utf8(contents)
            .map_err(|_| AppError::InvalidInput("SKILL.md 不是有效 UTF-8".to_string()))?
            .to_string();
        Ok(SkillDocumentRead {
            source_client: source,
            size_bytes: contents.len() as u64,
            content,
        })
    }

    /// Windows explorer path of one live skill directory, restricted to the
    /// fixed per-client skill roots and the stored single-segment name.
    pub fn skill_directory_windows_path(
        state: &AppState,
        id: &str,
        client: ManagedClientId,
    ) -> Result<String, AppError> {
        let resolver = crate::adapters::wsl_paths::FixedWslPathResolver::runtime();
        Self::skill_directory_path_with(state.db.as_ref(), &resolver, id, client)
    }

    fn skill_directory_path_with<R: LocalSkillRepository>(
        repository: &R,
        resolver: &crate::adapters::wsl_paths::FixedWslPathResolver,
        id: &str,
        client: ManagedClientId,
    ) -> Result<String, AppError> {
        let skill = repository
            .get_local_skill(id)
            .map_err(repository_error)?
            .ok_or_else(|| AppError::InvalidInput(format!("未找到 Skill: {id}")))?;
        crate::domain::validate_skill_directory(&skill.directory)
            .map_err(|error| AppError::InvalidInput(error.to_string()))?;
        let path = resolver
            .client_config_root(client)
            .windows
            .join("skills")
            .join(&skill.directory);
        Ok(path.to_string_lossy().to_string())
    }

    pub fn scan_unmanaged(state: &AppState) -> Result<Vec<UnmanagedLocalSkill>, AppError> {
        let adapter = LocalSkillTreeAdapter::runtime();
        Self::scan_unmanaged_with(state.db.as_ref(), &adapter)
    }

    pub fn import_from_live(
        state: &AppState,
        imports: Vec<LocalSkillImport>,
    ) -> Result<Vec<LocalSkill>, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        let written_clients: Vec<_> = imports
            .iter()
            .flat_map(|import| {
                import
                    .apps
                    .enabled_clients()
                    .filter(move |client| *client != import.source_client)
            })
            .collect();
        let records = Self::import_with(
            state.db.as_ref(),
            &adapter,
            imports,
            chrono::Utc::now().timestamp_millis(),
        )?;
        record_skill_writes(state, written_clients);
        Ok(records)
    }

    pub fn sync_from_live(
        state: &AppState,
        id: &str,
        source_client: ManagedClientId,
    ) -> Result<LocalSkill, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        let skill = Self::sync_with(
            state.db.as_ref(),
            &adapter,
            id,
            source_client,
            chrono::Utc::now().timestamp_millis(),
        )?;
        record_skill_writes(
            state,
            skill
                .apps
                .enabled_clients()
                .filter(|client| *client != source_client),
        );
        Ok(skill)
    }

    pub fn toggle_app(
        state: &AppState,
        id: &str,
        source_client: ManagedClientId,
        target_client: ManagedClientId,
        enabled: bool,
    ) -> Result<LocalSkill, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        let changed = state
            .db
            .get_core_skill(id)?
            .is_some_and(|skill| skill.apps.is_enabled_for(target_client) != enabled);
        let skill = Self::toggle_with(
            state.db.as_ref(),
            &adapter,
            id,
            source_client,
            target_client,
            enabled,
            chrono::Utc::now().timestamp_millis(),
        )?;
        if changed {
            record_skill_writes(state, [target_client]);
        }
        Ok(skill)
    }

    pub fn remove_managed(state: &AppState, id: &str) -> Result<bool, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        let enabled_clients: Vec<_> = state
            .db
            .get_core_skill(id)?
            .into_iter()
            .flat_map(|skill| skill.apps.enabled_clients().collect::<Vec<_>>())
            .collect();
        let removed = Self::remove_with(state.db.as_ref(), &adapter, id)?;
        if removed {
            record_skill_writes(state, enabled_clients);
        }
        Ok(removed)
    }

    fn scan_unmanaged_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
    ) -> Result<Vec<UnmanagedLocalSkill>, AppError> {
        let managed: HashSet<_> = repository
            .list_local_skills()
            .map_err(repository_error)?
            .into_iter()
            .map(|skill| skill.directory.to_lowercase())
            .collect();
        let mut found: HashMap<String, (UnmanagedLocalSkill, HashMap<ManagedClientId, String>)> =
            HashMap::new();
        for client in ManagedClientId::ALL {
            for candidate in trees.scan(client).map_err(tree_error)? {
                let key = candidate.directory.to_lowercase();
                if managed.contains(&key) {
                    continue;
                }
                let (name, description) =
                    match parse_metadata(&candidate.tree, &candidate.directory) {
                        Ok(metadata) => metadata,
                        Err(error) => {
                            log::warn!(
                                "跳过元数据无效的 {} Skill {}: {}",
                                client,
                                candidate.directory,
                                error
                            );
                            continue;
                        }
                    };
                let (skill, hashes) = found.entry(key).or_insert_with(|| {
                    (
                        UnmanagedLocalSkill {
                            directory: candidate.directory.clone(),
                            name: name.clone(),
                            description: description.clone(),
                            found_in: Vec::new(),
                            copies: Vec::new(),
                            path: candidate.path.clone(),
                        },
                        HashMap::new(),
                    )
                });
                if !skill.found_in.contains(&client) {
                    skill.found_in.push(client);
                }
                hashes.insert(client, candidate.tree.content_hash.clone());
            }
        }
        let mut result: Vec<_> = found
            .into_values()
            .map(|(mut skill, hashes)| {
                skill.found_in.sort_by_key(|client| match client {
                    ManagedClientId::Claude => 0,
                    ManagedClientId::Codex => 1,
                    ManagedClientId::Opencode => 2,
                });
                skill.copies = skill
                    .found_in
                    .iter()
                    .map(|client| UnmanagedSkillCopy {
                        client: *client,
                        content_hash: hashes.get(client).cloned().unwrap_or_default(),
                    })
                    .collect();
                skill
            })
            .collect();
        result.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.directory.cmp(&right.directory))
        });
        Ok(result)
    }

    fn import_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        imports: Vec<LocalSkillImport>,
        now_ms: i64,
    ) -> Result<Vec<LocalSkill>, AppError> {
        if imports.is_empty() {
            return Ok(Vec::new());
        }
        let mut requested_directories = HashSet::new();
        for import in &imports {
            import
                .validate()
                .map_err(|error| AppError::InvalidInput(error.to_string()))?;
            if !requested_directories.insert(import.directory.to_lowercase()) {
                return Err(AppError::InvalidInput(format!(
                    "同一批导入中重复选择了 Skill 目录: {}",
                    import.directory
                )));
            }
        }
        let managed = repository.list_local_skills().map_err(repository_error)?;
        for import in &imports {
            if managed
                .iter()
                .any(|skill| skill.directory.eq_ignore_ascii_case(&import.directory))
            {
                return Err(AppError::InvalidInput(format!(
                    "Skill 已由本地核心管理: {}",
                    import.directory
                )));
            }
        }

        let mut prepared = Vec::with_capacity(imports.len());
        let mut snapshots = Vec::new();
        let mut writes = Vec::new();
        for import in imports {
            let source = trees
                .capture(import.source_client, &import.directory)
                .map_err(tree_error)?;
            let tree = source.tree.ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "来源客户端 {} 中不存在 Skill: {}",
                    import.source_client, import.directory
                ))
            })?;
            let (name, description) = parse_metadata(&tree, &import.directory)?;
            let record = LocalSkill {
                id: format!("local-{}", uuid::Uuid::new_v4().simple()),
                name,
                description,
                directory: import.directory.clone(),
                content_hash: Some(tree.content_hash.clone()),
                total_size_bytes: tree.total_size_bytes,
                file_count: tree.file_count,
                apps: import.apps.clone(),
                cloud_eligible: tree.is_cloud_eligible(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            record
                .validate()
                .map_err(|error| AppError::InvalidInput(error.to_string()))?;

            for target in import.apps.enabled_clients() {
                if target == import.source_client {
                    continue;
                }
                let snapshot = trees
                    .capture(target, &import.directory)
                    .map_err(tree_error)?;
                if snapshot
                    .tree
                    .as_ref()
                    .is_some_and(|target_tree| target_tree.content_hash != tree.content_hash)
                {
                    return Err(import_target_conflict_error(
                        &import.directory,
                        import.source_client,
                        target,
                    ));
                }
                if snapshot.tree.is_none() {
                    writes.push((target, import.directory.clone(), tree.clone()));
                    snapshots.push(snapshot);
                }
            }
            prepared.push(PreparedImport { record });
        }

        let mut applied = 0;
        for (client, directory, tree) in &writes {
            if let Err(primary) = trees.replace(*client, directory, tree) {
                return Err(rollback_trees(
                    trees,
                    &snapshots[..applied],
                    tree_error(primary),
                ));
            }
            applied += 1;
        }
        let records: Vec<_> = prepared.into_iter().map(|item| item.record).collect();
        if let Err(primary) = repository.save_local_skills(&records) {
            return Err(rollback_trees(
                trees,
                &snapshots[..applied],
                repository_error(primary),
            ));
        }
        Ok(records)
    }

    fn sync_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        id: &str,
        source_client: ManagedClientId,
        now_ms: i64,
    ) -> Result<LocalSkill, AppError> {
        let mut skill = repository
            .get_local_skill(id)
            .map_err(repository_error)?
            .ok_or_else(|| AppError::InvalidInput(format!("未找到 Skill: {id}")))?;
        if !skill.apps.is_enabled_for(source_client) {
            return Err(AppError::InvalidInput(format!(
                "{} 不是 Skill {} 的已启用来源客户端",
                source_client, skill.directory
            )));
        }
        let source = trees
            .capture(source_client, &skill.directory)
            .map_err(tree_error)?;
        let tree = source.tree.ok_or_else(|| {
            AppError::InvalidInput(format!(
                "来源客户端 {} 中不存在 Skill: {}",
                source_client, skill.directory
            ))
        })?;
        let (name, description) = parse_metadata(&tree, &skill.directory)?;
        let mut snapshots = Vec::new();
        for target in skill.apps.enabled_clients() {
            if target == source_client {
                continue;
            }
            let snapshot = trees
                .capture(target, &skill.directory)
                .map_err(tree_error)?;
            if let Some(target_tree) = &snapshot.tree {
                let target_is_source = target_tree.content_hash == tree.content_hash;
                let target_is_baseline = skill
                    .content_hash
                    .as_ref()
                    .is_some_and(|baseline| target_tree.content_hash == *baseline);
                if !target_is_source && !target_is_baseline {
                    return Err(external_change_error(&skill.directory, target));
                }
                if target_is_source {
                    continue;
                }
            }
            snapshots.push(snapshot);
        }

        let mut applied = 0;
        for snapshot in &snapshots {
            if let Err(primary) = trees.replace(snapshot.client, &snapshot.directory, &tree) {
                return Err(rollback_trees(
                    trees,
                    &snapshots[..applied],
                    tree_error(primary),
                ));
            }
            applied += 1;
        }
        skill.name = name;
        skill.description = description;
        skill.content_hash = Some(tree.content_hash.clone());
        skill.total_size_bytes = tree.total_size_bytes;
        skill.file_count = tree.file_count;
        skill.cloud_eligible = tree.is_cloud_eligible();
        skill.updated_at_ms = now_ms.max(skill.created_at_ms);
        if let Err(primary) = repository.save_local_skills(&[skill.clone()]) {
            return Err(rollback_trees(
                trees,
                &snapshots[..applied],
                repository_error(primary),
            ));
        }
        Ok(skill)
    }

    fn remove_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        id: &str,
    ) -> Result<bool, AppError> {
        let Some(skill) = repository.get_local_skill(id).map_err(repository_error)? else {
            return Ok(false);
        };
        let baseline = skill.content_hash.as_ref().ok_or_else(|| {
            AppError::InvalidInput(format!(
                "Skill {} 尚无内容基线，请先从一个 live 客户端手动同步",
                skill.directory
            ))
        })?;
        let mut snapshots = Vec::new();
        for client in skill.apps.enabled_clients() {
            let snapshot = trees
                .capture(client, &skill.directory)
                .map_err(tree_error)?;
            if snapshot
                .tree
                .as_ref()
                .is_some_and(|tree| tree.content_hash != *baseline)
            {
                return Err(external_change_error(&skill.directory, client));
            }
            snapshots.push(snapshot);
        }

        let mut applied = 0;
        for snapshot in &snapshots {
            if snapshot.tree.is_some() {
                if let Err(primary) = trees.remove(snapshot.client, &snapshot.directory) {
                    return Err(rollback_trees(
                        trees,
                        &snapshots[..applied],
                        tree_error(primary),
                    ));
                }
            }
            applied += 1;
        }
        if let Err(primary) = repository.delete_local_skill(id) {
            return Err(rollback_trees(
                trees,
                &snapshots[..applied],
                repository_error(primary),
            ));
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn toggle_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        id: &str,
        source_client: ManagedClientId,
        target_client: ManagedClientId,
        enabled: bool,
        now_ms: i64,
    ) -> Result<LocalSkill, AppError> {
        let mut skill = repository
            .get_local_skill(id)
            .map_err(repository_error)?
            .ok_or_else(|| AppError::InvalidInput(format!("未找到 Skill: {id}")))?;
        if skill.apps.is_enabled_for(target_client) == enabled {
            return Ok(skill);
        }

        let snapshot = trees
            .capture(target_client, &skill.directory)
            .map_err(tree_error)?;
        let mut refreshed_cloud_eligibility = None;
        if enabled {
            if !skill.apps.is_enabled_for(source_client) {
                return Err(AppError::InvalidInput(format!(
                    "复制来源 {} 尚未启用 Skill {}",
                    source_client, skill.directory
                )));
            }
            let source = trees
                .capture(source_client, &skill.directory)
                .map_err(tree_error)?;
            let tree = source.tree.ok_or_else(|| {
                AppError::InvalidInput(format!(
                    "复制来源 {} 中不存在 Skill {}",
                    source_client, skill.directory
                ))
            })?;
            refreshed_cloud_eligibility = Some(tree.is_cloud_eligible());
            if skill
                .content_hash
                .as_ref()
                .is_some_and(|baseline| tree.content_hash != *baseline)
            {
                return Err(AppError::InvalidInput(format!(
                    "来源 Skill {} 已发生外部修改，请先执行手动同步",
                    skill.directory
                )));
            }
            if snapshot
                .tree
                .as_ref()
                .is_some_and(|target| target.content_hash != tree.content_hash)
            {
                return Err(external_change_error(&skill.directory, target_client));
            }
            if snapshot.tree.is_none() {
                trees
                    .replace(target_client, &skill.directory, &tree)
                    .map_err(tree_error)?;
            }
        } else {
            let mut updated_apps = skill.apps.clone();
            updated_apps.set_enabled_for(target_client, false);
            if updated_apps.is_empty() {
                return Err(AppError::InvalidInput(
                    "不能禁用 Skill 的最后一个 live 副本".to_string(),
                ));
            }
            if snapshot.tree.as_ref().is_some_and(|target| {
                skill
                    .content_hash
                    .as_ref()
                    .is_none_or(|baseline| target.content_hash != *baseline)
            }) {
                return Err(external_change_error(&skill.directory, target_client));
            }
            trees
                .remove(target_client, &skill.directory)
                .map_err(tree_error)?;
        }

        skill.apps.set_enabled_for(target_client, enabled);
        if let Some(cloud_eligible) = refreshed_cloud_eligibility {
            skill.cloud_eligible = skill.content_hash.is_some() && cloud_eligible;
        }
        skill.updated_at_ms = now_ms.max(skill.created_at_ms);
        if let Err(primary) = repository.save_local_skills(&[skill.clone()]) {
            return Err(rollback_trees(
                trees,
                &[snapshot],
                repository_error(primary),
            ));
        }
        Ok(skill)
    }
}

fn record_skill_writes(state: &AppState, clients: impl IntoIterator<Item = ManagedClientId>) {
    crate::services::record_runtime_local_writes(
        &state.local_scan_writes,
        clients.into_iter().map(|client_id| LocalScanTarget {
            domain: LocalScanDomain::Skill,
            client_id,
        }),
    );
}

fn parse_metadata(
    tree: &LocalSkillTree,
    fallback_name: &str,
) -> Result<(String, Option<String>), AppError> {
    let contents = tree
        .file("SKILL.md")
        .ok_or_else(|| AppError::InvalidInput("Skill 目录缺少 SKILL.md".to_string()))?;
    let text = std::str::from_utf8(contents)
        .map_err(|error| AppError::InvalidInput(format!("SKILL.md 不是有效 UTF-8: {error}")))?;
    let front_matter = text
        .strip_prefix("---")
        .and_then(|tail| tail.strip_prefix(['\r', '\n']))
        .and_then(|tail| tail.split_once("\n---"))
        .map(|(yaml, _)| yaml.trim());
    let metadata = front_matter
        .map(serde_yaml::from_str::<SkillFrontMatter>)
        .transpose()
        .map_err(|error| AppError::InvalidInput(format!("SKILL.md 元数据无效: {error}")))?;
    let name = metadata
        .as_ref()
        .and_then(|metadata| metadata.name.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let description = metadata
        .and_then(|metadata| metadata.description)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    Ok((name, description))
}

fn rollback_trees<T: LocalSkillTreePort>(
    trees: &T,
    snapshots: &[LocalSkillTreeSnapshot],
    primary: AppError,
) -> AppError {
    let failures: Vec<_> = snapshots
        .iter()
        .rev()
        .filter_map(|snapshot| trees.restore(snapshot).err().map(|error| error.to_string()))
        .collect();
    if failures.is_empty() {
        primary
    } else {
        AppError::Message(format!(
            "{primary}; Skill live 回滚也失败: {}",
            failures.join("; ")
        ))
    }
}

fn external_change_error(directory: &str, client: ManagedClientId) -> AppError {
    AppError::InvalidInput(format!(
        "{} 的 Skill {} 包含未确认的外部修改，已停止覆盖",
        client, directory
    ))
}

fn import_target_conflict_error(
    directory: &str,
    source: ManagedClientId,
    target: ManagedClientId,
) -> AppError {
    AppError::InvalidInput(format!(
        "{} 中的 Skill {} 与所选 {} 来源内容不同；请取消选择 {}，或改选 {} 为内容来源",
        client_display_name(target),
        directory,
        client_display_name(source),
        client_display_name(target),
        client_display_name(target),
    ))
}

fn client_display_name(client: ManagedClientId) -> &'static str {
    match client {
        ManagedClientId::Claude => "Claude",
        ManagedClientId::Codex => "Codex",
        ManagedClientId::Opencode => "OpenCode",
    }
}

fn tree_error(error: impl std::fmt::Display) -> AppError {
    AppError::Message(format!("Skill live 文件树操作失败: {error}"))
}

fn repository_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(format!("Skill 本地核心持久化失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        LocalSkillFile, LocalSkillLiveCandidate, LocalSkillRepositoryError, LocalSkillTreeError,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct FailingRepository {
        records: Mutex<Vec<LocalSkill>>,
        fail_save: bool,
        fail_delete: bool,
    }

    impl LocalSkillRepository for FailingRepository {
        fn list_local_skills(&self) -> Result<Vec<LocalSkill>, LocalSkillRepositoryError> {
            Ok(self.records.lock().unwrap().clone())
        }

        fn get_local_skill(
            &self,
            id: &str,
        ) -> Result<Option<LocalSkill>, LocalSkillRepositoryError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .find(|record| record.id == id)
                .cloned())
        }

        fn save_local_skills(
            &self,
            skills: &[LocalSkill],
        ) -> Result<(), LocalSkillRepositoryError> {
            if self.fail_save {
                return Err(LocalSkillRepositoryError::new("injected save failure"));
            }
            *self.records.lock().unwrap() = skills.to_vec();
            Ok(())
        }

        fn delete_local_skill(&self, id: &str) -> Result<bool, LocalSkillRepositoryError> {
            if self.fail_delete {
                return Err(LocalSkillRepositoryError::new("injected delete failure"));
            }
            let mut records = self.records.lock().unwrap();
            let before = records.len();
            records.retain(|record| record.id != id);
            Ok(records.len() != before)
        }
    }

    #[derive(Default)]
    struct MemoryTrees {
        trees: Mutex<HashMap<(ManagedClientId, String), LocalSkillTree>>,
        fail_replace: Mutex<Option<(ManagedClientId, String)>>,
    }

    impl LocalSkillTreePort for MemoryTrees {
        fn scan(
            &self,
            client: ManagedClientId,
        ) -> Result<Vec<LocalSkillLiveCandidate>, LocalSkillTreeError> {
            Ok(self
                .trees
                .lock()
                .unwrap()
                .iter()
                .filter(|((candidate_client, _), _)| *candidate_client == client)
                .map(|((_, directory), tree)| LocalSkillLiveCandidate {
                    client,
                    directory: directory.clone(),
                    path: format!("/fixture/{directory}"),
                    tree: tree.clone(),
                })
                .collect())
        }

        fn capture(
            &self,
            client: ManagedClientId,
            directory: &str,
        ) -> Result<LocalSkillTreeSnapshot, LocalSkillTreeError> {
            Ok(LocalSkillTreeSnapshot {
                client,
                directory: directory.to_string(),
                tree: self
                    .trees
                    .lock()
                    .unwrap()
                    .get(&(client, directory.to_string()))
                    .cloned(),
            })
        }

        fn replace(
            &self,
            client: ManagedClientId,
            directory: &str,
            tree: &LocalSkillTree,
        ) -> Result<(), LocalSkillTreeError> {
            let key = (client, directory.to_string());
            if self.fail_replace.lock().unwrap().as_ref() == Some(&key) {
                return Err(LocalSkillTreeError::new(
                    crate::ports::LocalSkillTreeErrorCode::Io,
                    "injected replace failure",
                ));
            }
            self.trees.lock().unwrap().insert(key, tree.clone());
            Ok(())
        }

        fn restore(&self, snapshot: &LocalSkillTreeSnapshot) -> Result<(), LocalSkillTreeError> {
            let key = (snapshot.client, snapshot.directory.clone());
            match &snapshot.tree {
                Some(tree) => {
                    self.trees.lock().unwrap().insert(key, tree.clone());
                }
                None => {
                    self.trees.lock().unwrap().remove(&key);
                }
            }
            Ok(())
        }

        fn remove(
            &self,
            client: ManagedClientId,
            directory: &str,
        ) -> Result<(), LocalSkillTreeError> {
            self.trees
                .lock()
                .unwrap()
                .remove(&(client, directory.to_string()));
            Ok(())
        }
    }

    fn tree_from_contents(contents: Vec<u8>) -> LocalSkillTree {
        use sha2::{Digest, Sha256};
        let file = LocalSkillFile {
            relative_path: "SKILL.md".to_string(),
            contents,
        };
        let mut hasher = Sha256::new();
        hasher.update(b"F\0");
        hasher.update((file.relative_path.len() as u64).to_le_bytes());
        hasher.update(file.relative_path.as_bytes());
        hasher.update((file.contents.len() as u64).to_le_bytes());
        hasher.update(&file.contents);
        LocalSkillTree {
            directories: Vec::new(),
            total_size_bytes: file.contents.len() as u64,
            file_count: 1,
            files: vec![file],
            content_hash: format!("{:x}", hasher.finalize()),
        }
    }

    fn tree(body: &[u8]) -> LocalSkillTree {
        tree_from_contents([b"---\nname: fixture\n---\n".as_slice(), body].concat())
    }

    fn record(content_hash: Option<String>, apps: crate::domain::ManagedClientApps) -> LocalSkill {
        LocalSkill {
            id: "local-fixture".to_string(),
            name: "fixture".to_string(),
            description: None,
            directory: "fixture".to_string(),
            content_hash,
            total_size_bytes: 0,
            file_count: 0,
            apps,
            cloud_eligible: false,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn all_apps() -> crate::domain::ManagedClientApps {
        crate::domain::ManagedClientApps {
            claude: true,
            codex: true,
            opencode: true,
        }
    }

    #[test]
    fn database_failure_restores_every_written_target() {
        let repository = FailingRepository {
            fail_save: true,
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree(b"source"),
        );

        LocalSkillService::import_with(
            &repository,
            &trees,
            vec![LocalSkillImport {
                directory: "fixture".to_string(),
                source_client: ManagedClientId::Claude,
                apps: all_apps(),
            }],
            1,
        )
        .expect_err("injected database failure");

        let stored = trees.trees.lock().unwrap();
        assert!(stored.contains_key(&(ManagedClientId::Claude, "fixture".to_string())));
        assert!(!stored.contains_key(&(ManagedClientId::Codex, "fixture".to_string())));
        assert!(!stored.contains_key(&(ManagedClientId::Opencode, "fixture".to_string())));
    }

    #[test]
    fn unmanaged_scan_keeps_valid_skills_when_one_manifest_is_malformed() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert(
                (ManagedClientId::Claude, "broken".to_string()),
                tree_from_contents(b"---\nname: [\n---\n".to_vec()),
            );
            stored.insert(
                (ManagedClientId::Claude, "working".to_string()),
                tree_from_contents(b"---\nname: Working Skill\n---\n".to_vec()),
            );
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("one malformed manifest must not hide valid Skills");

        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].directory, "working");
        assert_eq!(scanned[0].name, "Working Skill");
    }

    #[test]
    fn unmanaged_scan_reports_per_client_content_hashes() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        let consistent = tree(b"same");
        let solo_tree = tree(b"claude-only");
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert(
                (ManagedClientId::Claude, "shared".to_string()),
                consistent.clone(),
            );
            stored.insert(
                (ManagedClientId::Codex, "shared".to_string()),
                consistent.clone(),
            );
            stored.insert(
                (ManagedClientId::Opencode, "shared".to_string()),
                tree(b"divergent"),
            );
            stored.insert(
                (ManagedClientId::Claude, "solo".to_string()),
                solo_tree.clone(),
            );
        }

        let mut scanned =
            LocalSkillService::scan_unmanaged_with(&repository, &trees).expect("scan must succeed");
        scanned.sort_by(|left, right| left.directory.cmp(&right.directory));

        assert_eq!(scanned.len(), 2);
        let solo = &scanned[1];
        assert_eq!(solo.directory, "solo");
        assert_eq!(solo.copies.len(), 1);
        assert_eq!(solo.copies[0].client, ManagedClientId::Claude);
        assert_eq!(solo.copies[0].content_hash, solo_tree.content_hash);

        let shared = &scanned[0];
        assert_eq!(shared.directory, "shared");
        assert_eq!(
            shared
                .copies
                .iter()
                .map(|copy| copy.client)
                .collect::<Vec<_>>(),
            vec![
                ManagedClientId::Claude,
                ManagedClientId::Codex,
                ManagedClientId::Opencode,
            ]
        );
        assert_eq!(shared.copies[0].content_hash, consistent.content_hash);
        assert_eq!(shared.copies[1].content_hash, consistent.content_hash);
        assert_ne!(shared.copies[2].content_hash, consistent.content_hash);
    }

    #[test]
    fn file_failure_restores_targets_written_before_the_failure() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree(b"source"),
        );
        *trees.fail_replace.lock().unwrap() =
            Some((ManagedClientId::Opencode, "fixture".to_string()));

        LocalSkillService::import_with(
            &repository,
            &trees,
            vec![LocalSkillImport {
                directory: "fixture".to_string(),
                source_client: ManagedClientId::Claude,
                apps: all_apps(),
            }],
            1,
        )
        .expect_err("second target failure must surface");

        let stored = trees.trees.lock().unwrap();
        assert!(stored.contains_key(&(ManagedClientId::Claude, "fixture".to_string())));
        assert!(!stored.contains_key(&(ManagedClientId::Codex, "fixture".to_string())));
        assert!(!stored.contains_key(&(ManagedClientId::Opencode, "fixture".to_string())));
        assert!(repository.records.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_source_metadata_is_rejected_before_any_sync_write() {
        let baseline = tree(b"baseline");
        let invalid = tree_from_contents(b"---\nname: [\n---\ninvalid".to_vec());
        let repository = FailingRepository {
            records: Mutex::new(vec![record(
                Some(baseline.content_hash.clone()),
                all_apps(),
            )]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert((ManagedClientId::Claude, "fixture".to_string()), invalid);
            stored.insert(
                (ManagedClientId::Codex, "fixture".to_string()),
                baseline.clone(),
            );
            stored.insert(
                (ManagedClientId::Opencode, "fixture".to_string()),
                baseline.clone(),
            );
        }

        LocalSkillService::sync_with(
            &repository,
            &trees,
            "local-fixture",
            ManagedClientId::Claude,
            2,
        )
        .expect_err("invalid front matter must fail before writes");

        let stored = trees.trees.lock().unwrap();
        assert_eq!(
            stored.get(&(ManagedClientId::Codex, "fixture".to_string())),
            Some(&baseline)
        );
        assert_eq!(
            stored.get(&(ManagedClientId::Opencode, "fixture".to_string())),
            Some(&baseline)
        );
    }

    #[test]
    fn manual_sync_establishes_a_missing_migration_baseline() {
        let source = tree(b"adopted");
        let apps = crate::domain::ManagedClientApps {
            claude: true,
            codex: true,
            opencode: false,
        };
        let repository = FailingRepository {
            records: Mutex::new(vec![record(None, apps)]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            source.clone(),
        );

        let synced = LocalSkillService::sync_with(
            &repository,
            &trees,
            "local-fixture",
            ManagedClientId::Claude,
            2,
        )
        .expect("manual sync establishes the baseline");

        assert_eq!(
            synced.content_hash.as_deref(),
            Some(source.content_hash.as_str())
        );
        assert_eq!(
            trees
                .trees
                .lock()
                .unwrap()
                .get(&(ManagedClientId::Codex, "fixture".to_string())),
            Some(&source)
        );
        assert_eq!(repository.records.lock().unwrap().as_slice(), &[synced]);
    }

    #[test]
    fn sync_database_failure_restores_all_target_trees() {
        let baseline = tree(b"baseline");
        let source = tree(b"changed source");
        let original = record(Some(baseline.content_hash.clone()), all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            fail_save: true,
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert((ManagedClientId::Claude, "fixture".to_string()), source);
            stored.insert(
                (ManagedClientId::Codex, "fixture".to_string()),
                baseline.clone(),
            );
            stored.insert(
                (ManagedClientId::Opencode, "fixture".to_string()),
                baseline.clone(),
            );
        }

        LocalSkillService::sync_with(
            &repository,
            &trees,
            "local-fixture",
            ManagedClientId::Claude,
            2,
        )
        .expect_err("database failure must roll back target trees");

        let stored = trees.trees.lock().unwrap();
        assert_eq!(
            stored.get(&(ManagedClientId::Codex, "fixture".to_string())),
            Some(&baseline)
        );
        assert_eq!(
            stored.get(&(ManagedClientId::Opencode, "fixture".to_string())),
            Some(&baseline)
        );
        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
    }

    #[test]
    fn delete_without_a_baseline_is_zero_write_rejected() {
        let source = tree(b"unconfirmed");
        let repository = FailingRepository {
            records: Mutex::new(vec![record(
                None,
                crate::domain::ManagedClientApps::only(ManagedClientId::Claude),
            )]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            source.clone(),
        );

        LocalSkillService::remove_with(&repository, &trees, "local-fixture")
            .expect_err("unconfirmed migrated Skill cannot be deleted");

        assert_eq!(
            trees
                .trees
                .lock()
                .unwrap()
                .get(&(ManagedClientId::Claude, "fixture".to_string())),
            Some(&source)
        );
        assert_eq!(repository.records.lock().unwrap().len(), 1);
    }

    #[test]
    fn delete_database_failure_restores_every_removed_live_copy() {
        let source = tree(b"baseline");
        let original = record(Some(source.content_hash.clone()), all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            fail_delete: true,
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        for client in ManagedClientId::ALL {
            trees
                .trees
                .lock()
                .unwrap()
                .insert((client, "fixture".to_string()), source.clone());
        }

        LocalSkillService::remove_with(&repository, &trees, "local-fixture")
            .expect_err("database failure must restore deleted live copies");

        let stored = trees.trees.lock().unwrap();
        for client in ManagedClientId::ALL {
            assert_eq!(stored.get(&(client, "fixture".to_string())), Some(&source));
        }
        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
    }

    #[test]
    fn parses_crlf_front_matter_without_losing_metadata() {
        let source = tree_from_contents(
            b"---\r\nname: Windows Skill\r\ndescription: Native CRLF\r\n---\r\nbody".to_vec(),
        );

        assert_eq!(
            parse_metadata(&source, "fallback").expect("parse CRLF front matter"),
            ("Windows Skill".to_string(), Some("Native CRLF".to_string()))
        );
    }

    #[test]
    fn read_skill_markdown_uses_the_first_enabled_live_copy() {
        let repository = FailingRepository {
            records: Mutex::new(vec![record(
                None,
                crate::domain::ManagedClientApps {
                    claude: true,
                    codex: true,
                    opencode: false,
                },
            )]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree(b"preview body"),
        );

        let document = LocalSkillService::read_markdown_with(&repository, &trees, "local-fixture")
            .expect("read SKILL.md");
        assert_eq!(document.source_client, ManagedClientId::Claude);
        assert!(document.content.contains("preview body"));
        assert!(document.size_bytes > 0);

        LocalSkillService::read_markdown_with(&repository, &trees, "missing")
            .expect_err("unknown skill id must fail");
    }

    #[test]
    fn skill_directory_path_is_restricted_to_fixed_roots() {
        let repository = FailingRepository {
            records: Mutex::new(vec![record(None, all_apps())]),
            ..Default::default()
        };
        let resolver = crate::adapters::wsl_paths::FixedWslPathResolver::production();

        let path = LocalSkillService::skill_directory_path_with(
            &repository,
            &resolver,
            "local-fixture",
            ManagedClientId::Opencode,
        )
        .expect("resolve skill directory");
        assert_eq!(
            path,
            r"\\wsl.localhost\Ubuntu\home\zhldm\.config/opencode\skills\fixture"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::env;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;

    use serial_test::serial;

    use super::*;
    use crate::database::Database;
    use crate::domain::ManagedClientApps;

    struct NativeFixtureCleanup {
        home: PathBuf,
        outside: PathBuf,
        distro: String,
        link_linux: String,
    }

    impl Drop for NativeFixtureCleanup {
        fn drop(&mut self) {
            let _ = Command::new("wsl.exe")
                .arg("-d")
                .arg(&self.distro)
                .arg("--")
                .arg("rm")
                .arg("-f")
                .arg(&self.link_linux)
                .status();
            remove_path(&self.home);
            remove_path(&self.outside);
        }
    }

    fn remove_path(path: &Path) {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => {
                let _ = fs::remove_dir_all(path);
            }
            Ok(_) => {
                let _ = fs::remove_file(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("inspect cleanup path {}: {error}", path.display()),
        }
    }

    fn skill_dir(home: &Path, client: ManagedClientId, directory: &str) -> PathBuf {
        match client {
            ManagedClientId::Claude => home.join(".claude/skills").join(directory),
            ManagedClientId::Codex => home.join(".codex/skills").join(directory),
            ManagedClientId::Opencode => home.join(".config/opencode/skills").join(directory),
        }
    }

    fn unc_to_wsl(path: &Path) -> (String, String) {
        let portable = path.to_string_lossy().replace('\\', "/");
        let mut parts = portable.trim_start_matches('/').split('/');
        let server = parts.next().expect("UNC server");
        assert!(
            server.eq_ignore_ascii_case("wsl.localhost") || server.eq_ignore_ascii_case("wsl$"),
            "path must use a WSL UNC server: {portable}"
        );
        let distro = parts.next().expect("WSL distribution").to_string();
        let linux = format!("/{}", parts.collect::<Vec<_>>().join("/"));
        (distro, linux)
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
    #[ignore = "requires CC_SWITCH_WSL_TEST_DIR and CC_SWITCH_TEST_HOME on isolated WSL2 UNC paths"]
    fn local_skill_copy_sync_conflict_and_link_rejection_on_wsl_unc() {
        let root = PathBuf::from(
            env::var_os("CC_SWITCH_WSL_TEST_DIR").expect("CC_SWITCH_WSL_TEST_DIR must be set"),
        );
        let home = PathBuf::from(
            env::var_os("CC_SWITCH_TEST_HOME").expect("CC_SWITCH_TEST_HOME must be set"),
        );
        let portable_root = root.to_string_lossy().replace('\\', "/");
        assert!(
            portable_root.starts_with("//wsl.localhost/") || portable_root.starts_with("//wsl$/"),
            "test root must be a WSL UNC path: {}",
            root.display()
        );
        assert!(home.starts_with(&root));

        remove_path(&home);
        let outside = root.join("local-skill-native-outside");
        remove_path(&outside);
        let link = skill_dir(&home, ManagedClientId::Claude, "linked-native");
        let (distro, link_linux) = unc_to_wsl(&link);
        let _cleanup = NativeFixtureCleanup {
            home: home.clone(),
            outside: outside.clone(),
            distro: distro.clone(),
            link_linux: link_linux.clone(),
        };

        let source = skill_dir(&home, ManagedClientId::Claude, "native-skill");
        fs::create_dir_all(source.join("nested")).expect("create native source tree");
        fs::create_dir_all(source.join(".git")).expect("create ignored git metadata");
        fs::create_dir_all(source.join("node_modules/pkg"))
            .expect("create ignored dependency tree");
        let initial = b"---\r\nname: Native Skill\r\ndescription: Initial\r\n---\r\nbody";
        fs::write(source.join("SKILL.md"), initial).expect("seed native SKILL.md");
        fs::write(source.join("nested/data.bin"), [0_u8, 1, 2, 255])
            .expect("seed native binary file");
        fs::write(source.join(".git/config"), b"source git metadata")
            .expect("seed ignored git metadata");
        fs::write(
            source.join("node_modules/pkg/index.js"),
            b"ignored dependency",
        )
        .expect("seed ignored dependency");
        fs::write(source.join("partial.tmp"), b"ignored temporary file")
            .expect("seed ignored temporary file");

        let state = AppState::new(Arc::new(Database::memory().expect("in-memory database")));
        let imported = LocalSkillService::import_from_live(
            &state,
            vec![LocalSkillImport {
                directory: "native-skill".to_string(),
                source_client: ManagedClientId::Claude,
                apps: all_apps(),
            }],
        )
        .expect("import and copy native Skill");
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].file_count, 2);
        assert!(imported[0].cloud_eligible);
        assert_eq!(fs::read(source.join("SKILL.md")).unwrap(), initial);
        for client in ManagedClientId::ALL {
            let target = skill_dir(&home, client, "native-skill");
            assert!(target.is_dir());
            assert!(!fs::symlink_metadata(&target)
                .expect("inspect copied Skill directory")
                .file_type()
                .is_symlink());
            assert_eq!(fs::read(target.join("SKILL.md")).unwrap(), initial);
            assert_eq!(
                fs::read(target.join("nested/data.bin")).unwrap(),
                [0, 1, 2, 255]
            );
            if client != ManagedClientId::Claude {
                assert!(!target.join(".git").exists());
                assert!(!target.join("node_modules").exists());
                assert!(!target.join("partial.tmp").exists());
            }
        }

        let codex_root = skill_dir(&home, ManagedClientId::Codex, "native-skill");
        fs::create_dir_all(codex_root.join(".git")).expect("create target ignored metadata");
        fs::write(codex_root.join(".git/config"), b"target git metadata")
            .expect("seed target ignored metadata");
        fs::write(codex_root.join("local.tmp"), b"target temporary bytes")
            .expect("seed target ignored temporary file");
        let adopted = b"---\r\nname: Native Skill Updated\r\ndescription: Adopted\r\n---\r\nnew";
        fs::write(source.join("SKILL.md"), adopted).expect("modify selected live source");
        let synced =
            LocalSkillService::sync_from_live(&state, &imported[0].id, ManagedClientId::Claude)
                .expect("explicitly adopt and copy selected live source");
        assert_eq!(synced.name, "Native Skill Updated");
        for client in [ManagedClientId::Codex, ManagedClientId::Opencode] {
            assert_eq!(
                fs::read(skill_dir(&home, client, "native-skill").join("SKILL.md")).unwrap(),
                adopted
            );
        }
        assert_eq!(
            fs::read(codex_root.join(".git/config")).unwrap(),
            b"target git metadata"
        );
        assert_eq!(
            fs::read(codex_root.join("local.tmp")).unwrap(),
            b"target temporary bytes"
        );

        let codex_path = skill_dir(&home, ManagedClientId::Codex, "native-skill").join("SKILL.md");
        let opencode_path =
            skill_dir(&home, ManagedClientId::Opencode, "native-skill").join("SKILL.md");
        let codex_before = fs::read(&codex_path).expect("capture first target before conflict");
        let external = b"---\nname: External OpenCode\n---\nexternal";
        fs::write(&opencode_path, external).expect("seed last-target external change");
        fs::write(
            source.join("SKILL.md"),
            b"---\nname: Blocked Source\n---\nblocked",
        )
        .expect("modify source before conflict");
        LocalSkillService::sync_from_live(&state, &imported[0].id, ManagedClientId::Claude)
            .expect_err("last-target external change must block every target write");
        assert_eq!(fs::read(&codex_path).unwrap(), codex_before);
        assert_eq!(fs::read(&opencode_path).unwrap(), external);
        assert_eq!(
            state.db.get_core_skill(&imported[0].id).unwrap(),
            Some(synced)
        );

        fs::create_dir_all(&outside).expect("create outside Skill target");
        let outside_bytes = b"---\nname: Outside\n---\nuntouched";
        fs::write(outside.join("SKILL.md"), outside_bytes).expect("seed outside Skill");
        fs::create_dir_all(link.parent().expect("link parent")).expect("create link parent");
        let (_, outside_linux) = unc_to_wsl(&outside);
        let status = Command::new("wsl.exe")
            .arg("-d")
            .arg(&distro)
            .arg("--")
            .arg("ln")
            .arg("-s")
            .arg(&outside_linux)
            .arg(&link_linux)
            .status()
            .expect("invoke wsl.exe to create native link fixture");
        assert!(status.success(), "create WSL link fixture");
        fs::symlink_metadata(&link).expect("Windows must inspect WSL link fixture");

        LocalSkillService::import_from_live(
            &state,
            vec![LocalSkillImport {
                directory: "linked-native".to_string(),
                source_client: ManagedClientId::Claude,
                apps: ManagedClientApps::only(ManagedClientId::Claude),
            }],
        )
        .expect_err("WSL symlink/reparse source must be rejected");
        assert_eq!(fs::read(outside.join("SKILL.md")).unwrap(), outside_bytes);

        let large = skill_dir(&home, ManagedClientId::Claude, "large-native");
        fs::create_dir_all(&large).expect("create oversized local Skill");
        fs::write(large.join("SKILL.md"), b"---\nname: Large Native\n---\n")
            .expect("seed oversized Skill metadata");
        fs::write(
            large.join("large.bin"),
            vec![0_u8; crate::domain::MAX_SKILL_FILE_SIZE_BYTES as usize + 1],
        )
        .expect("seed one-byte-over file");
        let oversized = LocalSkillService::import_from_live(
            &state,
            vec![LocalSkillImport {
                directory: "large-native".to_string(),
                source_client: ManagedClientId::Claude,
                apps: ManagedClientApps::only(ManagedClientId::Claude),
            }],
        )
        .expect("oversized Skill must remain locally manageable");
        assert_eq!(oversized.len(), 1);
        assert!(!oversized[0].cloud_eligible);
        assert!(large.join("large.bin").is_file());
        assert_eq!(state.db.list_core_skills().unwrap().len(), 2);
    }
}
