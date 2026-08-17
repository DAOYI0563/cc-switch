use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use unicode_casefold::UnicodeCaseFold;

use crate::adapters::local_skill_tree::LocalSkillTreeAdapter;
use crate::adapters::temporary_rollback::FixedTemporaryRollbackStore;
use crate::domain::{
    LocalScanDomain, LocalScanTarget, LocalSkill, LocalSkillImport, LocalSkillScanIssue,
    LocalSkillScanIssueKind, LocalSkillScanResult, ManagedClientApps, ManagedClientId,
    RollbackPointPurpose, UnmanagedLocalSkill,
};
use crate::error::AppError;
use crate::ports::{
    LocalSkillRepository, LocalSkillTree, LocalSkillTreeError, LocalSkillTreeErrorCode,
    LocalSkillTreePort, LocalSkillTreeSnapshot, TemporaryRollbackError, TemporaryRollbackStore,
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

#[derive(Debug)]
struct PreparedLocalSkillScan {
    before: Vec<LocalSkill>,
    unmanaged: Vec<UnmanagedLocalSkill>,
    issues: Vec<LocalSkillScanIssue>,
    upserts: Vec<LocalSkill>,
    removed_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillIndexRollbackPayload<'a> {
    schema_version: u32,
    skills: &'a [LocalSkill],
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

    pub fn scan_unmanaged(state: &AppState) -> Result<LocalSkillScanResult, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        let rollbacks = FixedTemporaryRollbackStore::runtime();
        Self::scan_unmanaged_with_rollback(
            state.db.as_ref(),
            &adapter,
            &rollbacks,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    pub fn import_from_live(
        state: &AppState,
        imports: Vec<LocalSkillImport>,
    ) -> Result<Vec<LocalSkill>, AppError> {
        let _guard = mutation_lock().lock()?;
        let adapter = LocalSkillTreeAdapter::runtime();
        // The selected source enters managed scope even though import does not
        // rewrite its tree, so every enabled client needs a DB-aware expectation.
        let written_clients: Vec<_> = imports
            .iter()
            .flat_map(|import| import.apps.enabled_clients())
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
        record_skill_writes(state, skill.apps.enabled_clients());
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

    #[cfg(test)]
    fn scan_unmanaged_with<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
    ) -> Result<LocalSkillScanResult, AppError> {
        let prepared =
            Self::prepare_scan(repository, trees, chrono::Utc::now().timestamp_millis())?;
        Self::apply_prepared_scan(repository, &prepared)
    }

    fn scan_unmanaged_with_rollback<
        R: LocalSkillRepository,
        T: LocalSkillTreePort,
        B: TemporaryRollbackStore,
    >(
        repository: &R,
        trees: &T,
        rollbacks: &B,
        now_ms: i64,
    ) -> Result<LocalSkillScanResult, AppError> {
        let prepared = Self::prepare_scan(repository, trees, now_ms)?;
        if prepared.upserts.is_empty() && prepared.removed_ids.is_empty() {
            return Self::apply_prepared_scan(repository, &prepared);
        }
        let payload = serde_json::to_vec(&SkillIndexRollbackPayload {
            schema_version: 1,
            skills: &prepared.before,
        })
        .map_err(|error| AppError::JsonSerialize { source: error })?;
        let rollback = rollbacks
            .create(RollbackPointPurpose::SkillIndexRefresh, now_ms, &payload)
            .map_err(rollback_error)?;
        match Self::apply_prepared_scan(repository, &prepared) {
            Ok(result) => {
                if let Err(delete) = rollbacks.delete_after_success(&rollback.id) {
                    log::warn!(
                        "Skill 索引刷新已提交，但临时回滚点清理失败: kind={:?}",
                        delete.code
                    );
                    if let Err(retain) = rollbacks.retain_after_failure(&rollback.id, now_ms) {
                        log::warn!(
                            "Skill 索引刷新已提交，但临时回滚点保留失败: kind={:?}",
                            retain.code
                        );
                    }
                }
                Ok(result)
            }
            Err(primary) => match rollbacks.retain_after_failure(&rollback.id, now_ms) {
                Ok(_) => Err(primary),
                Err(retain) => Err(AppError::Message(format!(
                    "{primary}; Skill 索引刷新回滚点保留也失败: {retain}"
                ))),
            },
        }
    }

    fn prepare_scan<R: LocalSkillRepository, T: LocalSkillTreePort>(
        repository: &R,
        trees: &T,
        now_ms: i64,
    ) -> Result<PreparedLocalSkillScan, AppError> {
        let before = repository.list_local_skills().map_err(repository_error)?;
        let mut listed = HashMap::new();
        for client in ManagedClientId::ALL {
            // Every fixed root is listed before any capture or database write so
            // an inaccessible home/root can never be mistaken for mass removal.
            listed.insert(client, trees.list_directories(client).map_err(tree_error)?);
        }

        let mut managed_spellings: HashMap<String, HashSet<String>> = HashMap::new();
        let mut managed_counts: HashMap<String, usize> = HashMap::new();
        for skill in &before {
            let key = skill_directory_identity(&skill.directory);
            managed_spellings
                .entry(key.clone())
                .or_default()
                .insert(skill.directory.clone());
            *managed_counts.entry(key).or_default() += 1;
        }
        let managed_keys: HashSet<_> = managed_counts.keys().cloned().collect();
        let mut live_spellings: HashMap<String, HashSet<String>> = HashMap::new();
        let mut case_clients: HashMap<String, Vec<ManagedClientId>> = HashMap::new();
        for client in ManagedClientId::ALL {
            for candidate in &listed[&client] {
                let key = skill_directory_identity(&candidate.directory);
                live_spellings
                    .entry(key.clone())
                    .or_default()
                    .insert(candidate.directory.clone());
                let clients = case_clients.entry(key).or_default();
                if !clients.contains(&client) {
                    clients.push(client);
                }
            }
        }
        let mut case_collisions: HashSet<_> = live_spellings
            .iter()
            .filter(|(_, spellings)| spellings.len() > 1)
            .map(|(key, _)| key.clone())
            .collect();
        for (key, count) in &managed_counts {
            let database_collision = *count > 1;
            let live_mismatch = live_spellings.get(key).is_some_and(|live| {
                managed_spellings
                    .get(key)
                    .is_none_or(|managed| live.iter().any(|spelling| !managed.contains(spelling)))
            });
            if database_collision || live_mismatch {
                case_collisions.insert(key.clone());
            }
        }

        let mut upserts = Vec::new();
        let mut removed_ids = Vec::new();
        let mut issues = Vec::new();
        for original in &before {
            let case_key = skill_directory_identity(&original.directory);
            if case_collisions.contains(&case_key) {
                issues.push(LocalSkillScanIssue {
                    directory: original.directory.clone(),
                    clients: case_clients.get(&case_key).cloned().unwrap_or_default(),
                    kind: LocalSkillScanIssueKind::CaseCollision,
                });
                continue;
            }

            let mut copies = Vec::new();
            let mut invalid_clients = Vec::new();
            for client in ManagedClientId::ALL {
                let exact: Vec<_> = listed[&client]
                    .iter()
                    .filter(|candidate| candidate.directory == original.directory)
                    .collect();
                if exact.len() > 1 {
                    invalid_clients.push(client);
                    continue;
                }
                let Some(_) = exact.first() else {
                    continue;
                };
                match trees.capture(client, &original.directory) {
                    Ok(snapshot) => match snapshot.tree {
                        Some(tree) if parse_metadata(&tree, &original.directory).is_ok() => {
                            copies.push((client, tree));
                        }
                        Some(_) | None => invalid_clients.push(client),
                    },
                    Err(error) => {
                        log::warn!(
                            "无法安全刷新 {} Skill {}: kind={:?}",
                            client,
                            original.directory,
                            error.code
                        );
                        invalid_clients.push(client);
                    }
                }
            }
            if !invalid_clients.is_empty() {
                issues.push(LocalSkillScanIssue {
                    directory: original.directory.clone(),
                    clients: invalid_clients,
                    kind: LocalSkillScanIssueKind::InvalidCopy,
                });
                continue;
            }
            if copies.is_empty() {
                removed_ids.push(original.id.clone());
                continue;
            }

            let mut actual_apps = ManagedClientApps::default();
            for (client, _) in &copies {
                actual_apps.set_enabled_for(*client, true);
            }
            let canonical = &copies[0].1;
            // 已以“分歧接受”导入的 Skill（content_hash 为 None）不再把跨客户端
            // 内容差异当作错误：差异是各端配置不同导致的合法状态。
            let diverged = original.content_hash.is_none();
            if !diverged
                && copies
                    .iter()
                    .any(|(_, tree)| tree.content_hash != canonical.content_hash)
            {
                let mut refreshed = original.clone();
                refreshed.apps = actual_apps;
                if refreshed.apps != original.apps {
                    refreshed.updated_at_ms = now_ms.max(original.updated_at_ms);
                    upserts.push(refreshed);
                }
                issues.push(LocalSkillScanIssue {
                    directory: original.directory.clone(),
                    clients: copies.iter().map(|(client, _)| *client).collect(),
                    kind: LocalSkillScanIssueKind::DivergentCopies,
                });
                continue;
            }

            let (name, description) = parse_metadata(canonical, &original.directory)?;
            let mut refreshed = original.clone();
            refreshed.name = name;
            refreshed.description = description;
            refreshed.content_hash = if diverged {
                None
            } else {
                Some(canonical.content_hash.clone())
            };
            refreshed.total_size_bytes = canonical.total_size_bytes;
            refreshed.file_count = canonical.file_count;
            refreshed.apps = actual_apps;
            refreshed.cloud_eligible = if diverged {
                false
            } else {
                canonical.is_cloud_eligible()
            };
            if refreshed != *original {
                refreshed.updated_at_ms = now_ms.max(original.updated_at_ms);
                upserts.push(refreshed);
            }
        }

        let mut unmanaged_collision_keys: Vec<_> = case_collisions
            .iter()
            .filter(|key| !managed_keys.contains(*key))
            .cloned()
            .collect();
        unmanaged_collision_keys.sort();
        for key in unmanaged_collision_keys {
            let directory = live_spellings[&key]
                .iter()
                .min()
                .cloned()
                .unwrap_or(key.clone());
            issues.push(LocalSkillScanIssue {
                directory,
                clients: case_clients.get(&key).cloned().unwrap_or_default(),
                kind: LocalSkillScanIssueKind::CaseCollision,
            });
        }

        let mut unmanaged: HashMap<String, UnmanagedLocalSkill> = HashMap::new();
        for client in ManagedClientId::ALL {
            for candidate in &listed[&client] {
                let key = skill_directory_identity(&candidate.directory);
                if managed_keys.contains(&key) || case_collisions.contains(&key) {
                    continue;
                }
                let contents = match trees.read_manifest(candidate) {
                    Ok(Some(contents)) => contents,
                    Ok(None) => continue,
                    Err(error)
                        if matches!(
                            error.code,
                            LocalSkillTreeErrorCode::InvalidPath
                                | LocalSkillTreeErrorCode::LinkNotAllowed
                                | LocalSkillTreeErrorCode::InvalidTree
                        ) =>
                    {
                        log::warn!(
                            "跳过 manifest 无法安全读取的 {} Skill {}: kind={:?}",
                            client,
                            candidate.directory,
                            error.code
                        );
                        continue;
                    }
                    Err(error) => return Err(tree_error(error)),
                };
                let (name, description) =
                    match parse_manifest_metadata(&contents, &candidate.directory) {
                        Ok(metadata) => metadata,
                        Err(_) => {
                            log::warn!("跳过元数据无效的 {} Skill {}", client, candidate.directory);
                            continue;
                        }
                    };
                let skill = unmanaged.entry(key).or_insert_with(|| UnmanagedLocalSkill {
                    directory: candidate.directory.clone(),
                    name,
                    description,
                    found_in: Vec::new(),
                });
                if !skill.found_in.contains(&client) {
                    skill.found_in.push(client);
                }
            }
        }
        let mut unmanaged: Vec<_> = unmanaged.into_values().collect();
        unmanaged.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.directory.cmp(&right.directory))
        });
        issues.sort_by(|left, right| {
            left.directory
                .to_lowercase()
                .cmp(&right.directory.to_lowercase())
                .then_with(|| left.directory.cmp(&right.directory))
        });
        Ok(PreparedLocalSkillScan {
            before,
            unmanaged,
            issues,
            upserts,
            removed_ids,
        })
    }

    fn apply_prepared_scan<R: LocalSkillRepository>(
        repository: &R,
        prepared: &PreparedLocalSkillScan,
    ) -> Result<LocalSkillScanResult, AppError> {
        let updated_count = prepared.upserts.len() as u64;
        let removed_count = prepared.removed_ids.len() as u64;
        let mut installed = if updated_count == 0 && removed_count == 0 {
            prepared.before.clone()
        } else {
            repository
                .reconcile_local_skills(&prepared.upserts, &prepared.removed_ids)
                .map_err(repository_error)?
        };
        installed.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(LocalSkillScanResult {
            installed,
            unmanaged: prepared.unmanaged.clone(),
            issues: prepared.issues.clone(),
            updated_count,
            removed_count,
        })
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
            if !requested_directories.insert(skill_directory_identity(&import.directory)) {
                return Err(AppError::InvalidInput(format!(
                    "同一批导入中重复选择了 Skill 目录: {}",
                    import.directory
                )));
            }
        }
        let managed = repository.list_local_skills().map_err(repository_error)?;
        for import in &imports {
            if managed.iter().any(|skill| {
                skill_directory_identity(&skill.directory)
                    == skill_directory_identity(&import.directory)
            }) {
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
            let mut diverged = false;
            for target in import.apps.enabled_clients() {
                if target == import.source_client {
                    continue;
                }
                let snapshot = trees
                    .capture(target, &import.directory)
                    .map_err(tree_error)?;
                if let Some(target_tree) = &snapshot.tree {
                    if target_tree.content_hash != tree.content_hash {
                        // 同名副本的内容按客户端而异（例如各端的细微配置差异），
                        // 视为同一 Skill 的合法差异：原样接管，不覆盖也不报错。
                        diverged = true;
                    }
                    // 已存在的副本（一致或不一致）均原样接管，不写入。
                } else {
                    // 目标端尚无副本：将来源树写入该客户端。
                    writes.push((target, import.directory.clone(), tree.clone()));
                    snapshots.push(snapshot);
                }
            }
            let record = LocalSkill {
                id: format!("local-{}", uuid::Uuid::new_v4().simple()),
                name,
                description,
                directory: import.directory.clone(),
                content_hash: if diverged {
                    None
                } else {
                    Some(tree.content_hash.clone())
                },
                total_size_bytes: tree.total_size_bytes,
                file_count: tree.file_count,
                apps: import.apps.clone(),
                cloud_eligible: !diverged && tree.is_cloud_eligible(),
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            record
                .validate()
                .map_err(|error| AppError::InvalidInput(error.to_string()))?;
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
                // 分歧接受的 Skill 各端内容本就可能不同，手动同步时直接以
                // 所选来源覆盖各端，不再以此报错。
                if !target_is_source && !target_is_baseline && skill.content_hash.is_some() {
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
        // 分歧接受的 Skill（content_hash 为 None）各端内容本就可能不同，
        // 删除时不再以单一基线校验每个副本，直接删除受管目录。
        let baseline = skill.content_hash.as_ref();
        let mut snapshots = Vec::new();
        for client in skill.apps.enabled_clients() {
            let snapshot = trees
                .capture(client, &skill.directory)
                .map_err(tree_error)?;
            if let Some(baseline) = baseline {
                if snapshot
                    .tree
                    .as_ref()
                    .is_some_and(|tree| tree.content_hash != *baseline)
                {
                    return Err(external_change_error(&skill.directory, client));
                }
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
            // 分歧接受的 Skill 各端内容本就可能不同，启用时不以此阻断。
            if skill.content_hash.is_some()
                && snapshot
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
            // 分歧接受的 Skill 各端内容本就可能不同，禁用时不以此阻断。
            if let Some(baseline) = skill.content_hash.as_ref() {
                if snapshot
                    .tree
                    .as_ref()
                    .is_some_and(|target| target.content_hash != *baseline)
                {
                    return Err(external_change_error(&skill.directory, target_client));
                }
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
    crate::services::record_database_local_writes(
        &state.local_scan_writes,
        state.db.clone(),
        clients.into_iter().map(|client_id| LocalScanTarget {
            domain: LocalScanDomain::Skill,
            client_id,
        }),
    );
}

fn skill_directory_identity(directory: &str) -> String {
    directory.case_fold().collect()
}

fn parse_metadata(
    tree: &LocalSkillTree,
    fallback_name: &str,
) -> Result<(String, Option<String>), AppError> {
    let contents = tree
        .file("SKILL.md")
        .ok_or_else(|| AppError::InvalidInput("Skill 目录缺少 SKILL.md".to_string()))?;
    parse_manifest_metadata(contents, fallback_name)
}

fn parse_manifest_metadata(
    contents: &[u8],
    fallback_name: &str,
) -> Result<(String, Option<String>), AppError> {
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
    LocalSkill {
        id: "local-scan-metadata".to_string(),
        name: name.clone(),
        description: description.clone(),
        directory: fallback_name.to_string(),
        content_hash: None,
        total_size_bytes: 0,
        file_count: 0,
        apps: ManagedClientApps::only(ManagedClientId::Claude),
        cloud_eligible: false,
        created_at_ms: 0,
        updated_at_ms: 0,
    }
    .validate()
    .map_err(|error| AppError::InvalidInput(format!("SKILL.md 元数据无效: {error}")))?;
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
        .filter_map(|snapshot| {
            trees
                .restore(snapshot)
                .err()
                .map(|error| format!("kind={:?}", error.code))
        })
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

fn tree_error(error: LocalSkillTreeError) -> AppError {
    AppError::Message(format!("Skill live 文件树操作失败: kind={:?}", error.code))
}

fn repository_error(_error: impl std::fmt::Display) -> AppError {
    AppError::Database("Skill 本地核心持久化失败".to_string())
}

fn rollback_error(error: TemporaryRollbackError) -> AppError {
    AppError::Message(format!(
        "Skill 索引刷新临时回滚点失败: kind={:?}",
        error.code
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RollbackPointMetadata, RollbackPointState};
    use crate::ports::{
        LocalSkillDirectoryCandidate, LocalSkillFile, LocalSkillRepositoryError,
        LocalSkillTreeError, TemporaryRollbackError, TemporaryRollbackErrorCode,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct FailingRepository {
        records: Mutex<Vec<LocalSkill>>,
        reconcile_calls: Mutex<usize>,
        fail_save: bool,
        fail_delete: bool,
        fail_reconcile: bool,
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

        fn reconcile_local_skills(
            &self,
            upserts: &[LocalSkill],
            removed_ids: &[String],
        ) -> Result<Vec<LocalSkill>, LocalSkillRepositoryError> {
            *self.reconcile_calls.lock().unwrap() += 1;
            if self.fail_reconcile {
                return Err(LocalSkillRepositoryError::new("injected reconcile failure"));
            }
            let mut records = self.records.lock().unwrap();
            let mut reconciled = records.clone();
            for skill in upserts {
                if let Some(existing) = reconciled.iter_mut().find(|item| item.id == skill.id) {
                    *existing = skill.clone();
                } else {
                    reconciled.push(skill.clone());
                }
            }
            reconciled.retain(|record| !removed_ids.contains(&record.id));
            reconciled.sort_by(|left, right| {
                left.name
                    .to_lowercase()
                    .cmp(&right.name.to_lowercase())
                    .then_with(|| left.id.cmp(&right.id))
            });
            *records = reconciled.clone();
            Ok(reconciled)
        }
    }

    #[derive(Default)]
    struct MemoryRollbacks {
        created: Mutex<Vec<(RollbackPointPurpose, i64, Vec<u8>)>>,
        deleted: Mutex<Vec<String>>,
        retained: Mutex<Vec<(String, i64)>>,
        fail_create: bool,
        fail_delete: bool,
    }

    impl TemporaryRollbackStore for MemoryRollbacks {
        fn create(
            &self,
            purpose: RollbackPointPurpose,
            created_at_ms: i64,
            payload: &[u8],
        ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
            if self.fail_create {
                return Err(TemporaryRollbackError::new(
                    TemporaryRollbackErrorCode::Protection,
                    "injected rollback create failure",
                ));
            }
            let mut created = self.created.lock().unwrap();
            let id = format!("rollback-{}", created.len() + 1);
            created.push((purpose, created_at_ms, payload.to_vec()));
            Ok(RollbackPointMetadata {
                schema_version: RollbackPointMetadata::SCHEMA_VERSION,
                id,
                purpose,
                state: RollbackPointState::Pending,
                created_at_ms,
                failed_at_ms: None,
                payload_size_bytes: payload.len() as u64,
                payload_sha256: "a".repeat(64),
            })
        }

        fn restore(&self, id: &str) -> Result<Vec<u8>, TemporaryRollbackError> {
            let index = id
                .strip_prefix("rollback-")
                .and_then(|value| value.parse::<usize>().ok())
                .and_then(|value| value.checked_sub(1))
                .ok_or_else(missing_rollback)?;
            self.created
                .lock()
                .unwrap()
                .get(index)
                .map(|(_, _, payload)| payload.clone())
                .ok_or_else(missing_rollback)
        }

        fn delete_after_success(&self, id: &str) -> Result<(), TemporaryRollbackError> {
            self.deleted.lock().unwrap().push(id.to_string());
            if self.fail_delete {
                return Err(TemporaryRollbackError::new(
                    TemporaryRollbackErrorCode::Io,
                    "injected rollback delete failure",
                ));
            }
            Ok(())
        }

        fn retain_after_failure(
            &self,
            id: &str,
            failed_at_ms: i64,
        ) -> Result<RollbackPointMetadata, TemporaryRollbackError> {
            self.retained
                .lock()
                .unwrap()
                .push((id.to_string(), failed_at_ms));
            Ok(RollbackPointMetadata {
                schema_version: RollbackPointMetadata::SCHEMA_VERSION,
                id: id.to_string(),
                purpose: RollbackPointPurpose::SkillIndexRefresh,
                state: RollbackPointState::Failed,
                created_at_ms: failed_at_ms,
                failed_at_ms: Some(failed_at_ms),
                payload_size_bytes: 0,
                payload_sha256: "a".repeat(64),
            })
        }

        fn list(&self) -> Result<Vec<RollbackPointMetadata>, TemporaryRollbackError> {
            Ok(Vec::new())
        }
    }

    fn missing_rollback() -> TemporaryRollbackError {
        TemporaryRollbackError::new(
            TemporaryRollbackErrorCode::NotFound,
            "missing fixture rollback point",
        )
    }

    #[derive(Default)]
    struct MemoryTrees {
        trees: Mutex<HashMap<(ManagedClientId, String), LocalSkillTree>>,
        manifest_reads: Mutex<Vec<(ManagedClientId, String)>>,
        capture_reads: Mutex<Vec<(ManagedClientId, String)>>,
        fail_list: Mutex<Option<LocalSkillTreeError>>,
        fail_manifest: Mutex<Option<LocalSkillTreeError>>,
        fail_capture: Mutex<HashMap<(ManagedClientId, String), LocalSkillTreeError>>,
        fail_replace: Mutex<Option<(ManagedClientId, String)>>,
    }

    impl LocalSkillTreePort for MemoryTrees {
        fn list_directories(
            &self,
            client: ManagedClientId,
        ) -> Result<Vec<LocalSkillDirectoryCandidate>, LocalSkillTreeError> {
            if let Some(error) = self.fail_list.lock().unwrap().clone() {
                return Err(error);
            }
            let mut candidates: Vec<_> = self
                .trees
                .lock()
                .unwrap()
                .keys()
                .filter(|(candidate_client, _)| *candidate_client == client)
                .map(|(_, directory)| LocalSkillDirectoryCandidate {
                    client,
                    directory: directory.clone(),
                    path: format!("/fixture/{directory}"),
                })
                .collect();
            candidates.sort_by(|left, right| left.directory.cmp(&right.directory));
            Ok(candidates)
        }

        fn read_manifest(
            &self,
            candidate: &LocalSkillDirectoryCandidate,
        ) -> Result<Option<Vec<u8>>, LocalSkillTreeError> {
            if let Some(error) = self.fail_manifest.lock().unwrap().clone() {
                return Err(error);
            }
            self.manifest_reads
                .lock()
                .unwrap()
                .push((candidate.client, candidate.directory.clone()));
            Ok(self
                .trees
                .lock()
                .unwrap()
                .get(&(candidate.client, candidate.directory.clone()))
                .and_then(|tree| tree.file("SKILL.md"))
                .map(ToOwned::to_owned))
        }

        fn capture(
            &self,
            client: ManagedClientId,
            directory: &str,
        ) -> Result<LocalSkillTreeSnapshot, LocalSkillTreeError> {
            self.capture_reads
                .lock()
                .unwrap()
                .push((client, directory.to_string()));
            if let Some(error) = self
                .fail_capture
                .lock()
                .unwrap()
                .get(&(client, directory.to_string()))
                .cloned()
            {
                return Err(error);
            }
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
        record_for("fixture", content_hash, apps)
    }

    fn record_for(
        directory: &str,
        content_hash: Option<String>,
        apps: crate::domain::ManagedClientApps,
    ) -> LocalSkill {
        LocalSkill {
            id: format!("local-{directory}"),
            name: directory.to_string(),
            description: None,
            directory: directory.to_string(),
            content_hash,
            total_size_bytes: 0,
            file_count: 0,
            apps,
            cloud_eligible: false,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    fn record_from_tree(
        directory: &str,
        tree: &LocalSkillTree,
        apps: crate::domain::ManagedClientApps,
    ) -> LocalSkill {
        let (name, description) = parse_metadata(tree, directory).unwrap();
        let mut record = record_for(directory, Some(tree.content_hash.clone()), apps);
        record.name = name;
        record.description = description;
        record.total_size_bytes = tree.total_size_bytes;
        record.file_count = tree.file_count;
        record.cloud_eligible = tree.is_cloud_eligible();
        record
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
    fn managed_refresh_creates_no_rollback_point_for_an_unchanged_index() {
        let live = tree(b"same");
        let original = record_from_tree(
            "fixture",
            &live,
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees
            .trees
            .lock()
            .unwrap()
            .insert((ManagedClientId::Claude, "fixture".to_string()), live);
        let rollbacks = MemoryRollbacks::default();

        let scanned =
            LocalSkillService::scan_unmanaged_with_rollback(&repository, &trees, &rollbacks, 10)
                .expect("unchanged refresh succeeds");

        assert_eq!(scanned.installed, vec![original]);
        assert!(rollbacks.created.lock().unwrap().is_empty());
        assert!(rollbacks.deleted.lock().unwrap().is_empty());
        assert!(rollbacks.retained.lock().unwrap().is_empty());
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 0);
    }

    #[test]
    fn managed_refresh_deletes_rollback_after_successful_index_change() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        let rollbacks = MemoryRollbacks::default();

        let scanned =
            LocalSkillService::scan_unmanaged_with_rollback(&repository, &trees, &rollbacks, 10)
                .expect("changed refresh succeeds");

        assert_eq!(scanned.removed_count, 1);
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 1);
        let created = rollbacks.created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].0, RollbackPointPurpose::SkillIndexRefresh);
        assert_eq!(created[0].1, 10);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&created[0].2).unwrap(),
            serde_json::json!({
                "schemaVersion": 1,
                "skills": [original]
            })
        );
        assert_eq!(rollbacks.deleted.lock().unwrap().as_slice(), ["rollback-1"]);
        assert!(rollbacks.retained.lock().unwrap().is_empty());
    }

    #[test]
    fn managed_refresh_retains_rollback_when_database_reconcile_fails() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            fail_reconcile: true,
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        let rollbacks = MemoryRollbacks::default();

        LocalSkillService::scan_unmanaged_with_rollback(&repository, &trees, &rollbacks, 10)
            .expect_err("database failure must retain rollback");

        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 1);
        assert_eq!(rollbacks.created.lock().unwrap().len(), 1);
        assert!(rollbacks.deleted.lock().unwrap().is_empty());
        assert_eq!(
            rollbacks.retained.lock().unwrap().as_slice(),
            &[("rollback-1".to_string(), 10)]
        );
    }

    #[test]
    fn managed_refresh_returns_committed_result_when_rollback_cleanup_fails() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        let rollbacks = MemoryRollbacks {
            fail_delete: true,
            ..Default::default()
        };

        let scanned =
            LocalSkillService::scan_unmanaged_with_rollback(&repository, &trees, &rollbacks, 10)
                .expect("committed refresh survives rollback cleanup failure");

        assert_eq!(scanned.removed_count, 1);
        assert!(repository.records.lock().unwrap().is_empty());
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 1);
        assert_eq!(rollbacks.created.lock().unwrap().len(), 1);
        assert_eq!(rollbacks.deleted.lock().unwrap().as_slice(), ["rollback-1"]);
        assert_eq!(
            rollbacks.retained.lock().unwrap().as_slice(),
            &[("rollback-1".to_string(), 10)]
        );
    }

    #[test]
    fn rollback_create_failure_prevents_any_database_reconcile() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        let rollbacks = MemoryRollbacks {
            fail_create: true,
            ..Default::default()
        };

        LocalSkillService::scan_unmanaged_with_rollback(&repository, &trees, &rollbacks, 10)
            .expect_err("rollback protection failure must stop before database writes");

        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 0);
        assert!(rollbacks.created.lock().unwrap().is_empty());
        assert!(rollbacks.deleted.lock().unwrap().is_empty());
        assert!(rollbacks.retained.lock().unwrap().is_empty());
    }

    #[test]
    fn managed_refresh_removes_a_record_when_all_three_copies_are_gone() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("empty live roots remove the managed record");

        assert!(scanned.installed.is_empty());
        assert_eq!(scanned.removed_count, 1);
        assert_eq!(scanned.updated_count, 0);
        assert!(scanned.issues.is_empty());
    }

    #[test]
    fn managed_refresh_disables_a_client_whose_copy_was_deleted() {
        let live = tree(b"same");
        let original = record_from_tree("fixture", &live, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        for client in [ManagedClientId::Claude, ManagedClientId::Codex] {
            trees
                .trees
                .lock()
                .unwrap()
                .insert((client, "fixture".to_string()), live.clone());
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("actual copies refresh enabled apps");

        assert_eq!(scanned.updated_count, 1);
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(
            scanned.installed[0].apps,
            ManagedClientApps {
                claude: true,
                codex: true,
                opencode: false,
            }
        );
    }

    #[test]
    fn managed_refresh_enables_a_previously_disabled_client_that_now_has_a_copy() {
        let live = tree(b"same");
        let original = record_from_tree(
            "fixture",
            &live,
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        for client in [ManagedClientId::Claude, ManagedClientId::Opencode] {
            trees
                .trees
                .lock()
                .unwrap()
                .insert((client, "fixture".to_string()), live.clone());
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("a safe disabled-client copy is locally authoritative");

        assert_eq!(scanned.updated_count, 1);
        assert_eq!(
            scanned.installed[0].apps,
            ManagedClientApps {
                claude: true,
                codex: false,
                opencode: true,
            }
        );
    }

    #[test]
    fn managed_refresh_adopts_identical_live_metadata_and_measurements() {
        let live = tree_from_contents(
            b"---\nname: Refreshed\ndescription: Live metadata\n---\nbody".to_vec(),
        );
        let original = record(
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        for client in [ManagedClientId::Claude, ManagedClientId::Codex] {
            trees
                .trees
                .lock()
                .unwrap()
                .insert((client, "fixture".to_string()), live.clone());
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("identical copies refresh canonical metadata");
        let refreshed = &scanned.installed[0];

        assert_eq!(scanned.updated_count, 1);
        assert_eq!(refreshed.name, "Refreshed");
        assert_eq!(refreshed.description.as_deref(), Some("Live metadata"));
        assert_eq!(
            refreshed.content_hash.as_deref(),
            Some(live.content_hash.as_str())
        );
        assert_eq!(refreshed.total_size_bytes, live.total_size_bytes);
        assert_eq!(refreshed.file_count, live.file_count);
        assert!(refreshed.cloud_eligible);
    }

    #[test]
    fn divergent_managed_copies_preserve_metadata_but_refresh_actual_apps() {
        let original = record(Some("a".repeat(64)), all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree_from_contents(b"---\nname: Claude Changed\n---\nclaude".to_vec()),
        );
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Codex, "fixture".to_string()),
            tree_from_contents(b"---\nname: Codex Changed\n---\ncodex".to_vec()),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("divergence is reported without choosing a canonical copy");
        let refreshed = &scanned.installed[0];

        assert_eq!(scanned.updated_count, 1);
        assert_eq!(refreshed.name, original.name);
        assert_eq!(refreshed.description, original.description);
        assert_eq!(refreshed.content_hash, original.content_hash);
        assert_eq!(refreshed.total_size_bytes, original.total_size_bytes);
        assert_eq!(refreshed.file_count, original.file_count);
        assert_eq!(refreshed.cloud_eligible, original.cloud_eligible);
        assert_eq!(
            refreshed.apps,
            ManagedClientApps {
                claude: true,
                codex: true,
                opencode: false,
            }
        );
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(
            scanned.issues[0].kind,
            LocalSkillScanIssueKind::DivergentCopies
        );
    }

    #[test]
    fn divergent_accepted_copies_are_not_reported_as_divergent() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        for client in ManagedClientId::ALL {
            trees.trees.lock().unwrap().insert(
                (client, "fixture".to_string()),
                tree(format!("{}-config", client.as_str()).as_bytes()),
            );
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("accepted divergence is not reported");
        let refreshed = &scanned.installed[0];

        assert_eq!(scanned.updated_count, 1);
        assert_eq!(refreshed.content_hash, None);
        assert!(!refreshed.cloud_eligible);
        assert_eq!(scanned.issues.len(), 0);
        assert_eq!(
            refreshed.apps,
            ManagedClientApps {
                claude: true,
                codex: true,
                opencode: true,
            }
        );
    }

    #[test]
    fn invalid_managed_copy_preserves_the_original_record() {
        let original = record(Some("a".repeat(64)), all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree_from_contents(b"---\nname: [\n---\ninvalid".to_vec()),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("invalid copy is isolated from database reconciliation");

        assert_eq!(scanned.installed, vec![original]);
        assert_eq!(scanned.updated_count, 0);
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(scanned.issues[0].kind, LocalSkillScanIssueKind::InvalidCopy);
        assert_eq!(scanned.issues[0].clients, vec![ManagedClientId::Claude]);
    }

    #[test]
    fn skill_directory_identity_uses_full_unicode_case_folding() {
        assert_eq!(skill_directory_identity("Σ"), skill_directory_identity("ς"));
        assert_eq!(skill_directory_identity("Ä"), skill_directory_identity("ä"));
        assert_eq!(
            skill_directory_identity("Straße"),
            skill_directory_identity("STRASSE")
        );
    }

    #[test]
    fn live_only_case_spellings_are_reported_and_not_offered_for_import() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        trees
            .trees
            .lock()
            .unwrap()
            .insert((ManagedClientId::Claude, "Foo".to_string()), tree(b"upper"));
        trees
            .trees
            .lock()
            .unwrap()
            .insert((ManagedClientId::Codex, "foo".to_string()), tree(b"lower"));

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("live spelling collision is isolated");

        assert!(scanned.installed.is_empty());
        assert!(scanned.unmanaged.is_empty());
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(scanned.issues[0].directory, "Foo");
        assert_eq!(
            scanned.issues[0].kind,
            LocalSkillScanIssueKind::CaseCollision
        );
        assert_eq!(
            scanned.issues[0].clients,
            vec![ManagedClientId::Claude, ManagedClientId::Codex]
        );
        assert!(trees.capture_reads.lock().unwrap().is_empty());
        assert!(trees.manifest_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn unicode_case_only_live_spelling_change_preserves_database_identity() {
        let original = record_for(
            "Σ",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "ς".to_string()),
            tree(b"unicode case changed"),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("Unicode case-only spelling mismatch is isolated");

        assert_eq!(scanned.installed, vec![original]);
        assert!(scanned.unmanaged.is_empty());
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(
            scanned.issues[0].kind,
            LocalSkillScanIssueKind::CaseCollision
        );
    }

    #[test]
    fn case_only_live_spelling_change_preserves_database_identity_and_is_not_unmanaged() {
        let original = record_for(
            "Foo",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "foo".to_string()),
            tree(b"case changed"),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("case-only spelling mismatch is isolated");

        assert_eq!(scanned.installed, vec![original]);
        assert!(scanned.unmanaged.is_empty());
        assert_eq!(scanned.updated_count, 0);
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(scanned.issues[0].directory, "Foo");
        assert_eq!(
            scanned.issues[0].kind,
            LocalSkillScanIssueKind::CaseCollision
        );
        assert_eq!(scanned.issues[0].clients, vec![ManagedClientId::Claude]);
        assert!(trees.capture_reads.lock().unwrap().is_empty());
        assert!(trees.manifest_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn case_insensitive_database_duplicates_preserve_both_records() {
        let upper = record_for(
            "Foo",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let mut lower = record_for(
            "foo",
            Some("b".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Codex),
        );
        lower.id = "local-foo-lower".to_string();
        let repository = FailingRepository {
            records: Mutex::new(vec![upper.clone(), lower.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("database collision must not delete either identity");

        assert_eq!(scanned.installed, vec![upper, lower]);
        assert_eq!(scanned.updated_count, 0);
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(scanned.issues.len(), 2);
        assert!(scanned
            .issues
            .iter()
            .all(|issue| issue.kind == LocalSkillScanIssueKind::CaseCollision));
        assert_eq!(*repository.reconcile_calls.lock().unwrap(), 0);
    }

    #[test]
    fn case_collision_preserves_managed_record_and_suppresses_unmanaged_import() {
        let original = record(
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree(b"lower"),
        );
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Codex, "Fixture".to_string()),
            tree(b"upper"),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("case collision is an isolated managed issue");

        assert_eq!(scanned.installed, vec![original]);
        assert!(scanned.unmanaged.is_empty());
        assert_eq!(scanned.updated_count, 0);
        assert_eq!(scanned.removed_count, 0);
        assert_eq!(scanned.issues.len(), 1);
        assert_eq!(
            scanned.issues[0].kind,
            LocalSkillScanIssueKind::CaseCollision
        );
        assert_eq!(
            scanned.issues[0].clients,
            vec![ManagedClientId::Claude, ManagedClientId::Codex]
        );
    }

    #[test]
    fn directory_rename_is_old_managed_removal_plus_new_unmanaged_discovery() {
        let original = record_for(
            "old-name",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "new-name".to_string()),
            tree_from_contents(b"---\nname: Renamed\n---\nbody".to_vec()),
        );

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("renames are not silently adopted into managed identity");

        assert!(scanned.installed.is_empty());
        assert_eq!(scanned.removed_count, 1);
        assert_eq!(scanned.updated_count, 0);
        assert_eq!(scanned.unmanaged.len(), 1);
        assert_eq!(scanned.unmanaged[0].directory, "new-name");
        assert_eq!(scanned.unmanaged[0].name, "Renamed");
    }

    #[test]
    fn repository_reconcile_failure_keeps_updates_and_deletes_uncommitted() {
        let changed = tree_from_contents(b"---\nname: Changed\n---\nbody".to_vec());
        let update_original = record_for(
            "update",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let remove_original = record_for(
            "remove",
            Some("b".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Codex),
        );
        let originals = vec![update_original, remove_original];
        let repository = FailingRepository {
            records: Mutex::new(originals.clone()),
            fail_reconcile: true,
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees
            .trees
            .lock()
            .unwrap()
            .insert((ManagedClientId::Claude, "update".to_string()), changed);

        LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect_err("reconcile failure must reject the whole planned batch");

        assert_eq!(repository.records.lock().unwrap().as_slice(), originals);
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

        assert_eq!(scanned.unmanaged.len(), 1);
        assert_eq!(scanned.unmanaged[0].directory, "working");
        assert_eq!(scanned.unmanaged[0].name, "Working Skill");
        assert!(trees.capture_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn unmanaged_scan_separates_managed_full_capture_from_unmanaged_manifest_read() {
        let repository = FailingRepository {
            records: Mutex::new(vec![record(None, all_apps())]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert(
                (ManagedClientId::Claude, "fixture".to_string()),
                tree(b"managed must not be read"),
            );
            stored.insert(
                (ManagedClientId::Claude, "unmanaged".to_string()),
                tree_from_contents(b"---\nname: Unmanaged\n---\n".to_vec()),
            );
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("scan unmanaged manifests");

        assert_eq!(scanned.installed.len(), 1);
        assert_eq!(scanned.unmanaged.len(), 1);
        assert_eq!(scanned.unmanaged[0].directory, "unmanaged");
        assert_eq!(
            trees.manifest_reads.lock().unwrap().as_slice(),
            &[(ManagedClientId::Claude, "unmanaged".to_string())]
        );
        assert_eq!(
            trees.capture_reads.lock().unwrap().as_slice(),
            &[(ManagedClientId::Claude, "fixture".to_string())]
        );
    }

    #[test]
    fn unmanaged_scan_propagates_directory_listing_io_failures_without_database_changes() {
        let original = record(None, all_apps());
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        let sensitive_path = r"\\wsl.localhost\Ubuntu\home\zhldm\.claude\skills";
        *trees.fail_list.lock().unwrap() = Some(LocalSkillTreeError::new(
            LocalSkillTreeErrorCode::Io,
            format!("injected directory listing failure: {sensitive_path}"),
        ));

        let error = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect_err("directory listing failures must not look like an empty scan");

        assert!(error.to_string().contains("kind=Io"));
        assert!(!error.to_string().contains(sensitive_path));
        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
    }

    #[test]
    fn unmanaged_scan_propagates_manifest_io_failures() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "unmanaged".to_string()),
            tree(b"manifest"),
        );
        let sensitive_path = r"\\wsl.localhost\Ubuntu\home\zhldm\.claude\skills\unmanaged\SKILL.md";
        *trees.fail_manifest.lock().unwrap() = Some(LocalSkillTreeError::new(
            LocalSkillTreeErrorCode::Io,
            format!("injected manifest I/O failure: {sensitive_path}"),
        ));

        let error = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect_err("manifest I/O failures must not be skipped as invalid metadata");

        assert!(error.to_string().contains("kind=Io"));
        assert!(!error.to_string().contains(sensitive_path));
    }

    #[test]
    fn unmanaged_scan_aggregates_the_same_directory_across_three_clients() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        {
            let mut stored = trees.trees.lock().unwrap();
            stored.insert(
                (ManagedClientId::Claude, "shared".to_string()),
                tree_from_contents(b"---\nname: Shared Skill\n---\nclaude".to_vec()),
            );
            stored.insert(
                (ManagedClientId::Codex, "shared".to_string()),
                tree_from_contents(b"---\nname: Shared Skill\n---\ncodex".to_vec()),
            );
            stored.insert(
                (ManagedClientId::Opencode, "shared".to_string()),
                tree_from_contents(b"---\nname: Shared Skill\n---\nopencode".to_vec()),
            );
        }

        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("scan must aggregate by directory");

        assert_eq!(scanned.unmanaged.len(), 1);
        assert_eq!(scanned.unmanaged[0].directory, "shared");
        assert_eq!(scanned.unmanaged[0].name, "Shared Skill");
        assert_eq!(
            scanned.unmanaged[0].found_in,
            vec![
                ManagedClientId::Claude,
                ManagedClientId::Codex,
                ManagedClientId::Opencode,
            ]
        );
        assert!(trees.capture_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn import_rejects_unicode_casefold_alias_of_a_managed_directory() {
        let original = record_for(
            "Ä",
            Some("a".repeat(64)),
            ManagedClientApps::only(ManagedClientId::Claude),
        );
        let repository = FailingRepository {
            records: Mutex::new(vec![original.clone()]),
            ..Default::default()
        };
        let trees = MemoryTrees::default();
        trees
            .trees
            .lock()
            .unwrap()
            .insert((ManagedClientId::Codex, "ä".to_string()), tree(b"alias"));

        LocalSkillService::import_with(
            &repository,
            &trees,
            vec![LocalSkillImport {
                directory: "ä".to_string(),
                source_client: ManagedClientId::Codex,
                apps: ManagedClientApps::only(ManagedClientId::Codex),
            }],
            2,
        )
        .expect_err("Unicode alias of managed directory must not be imported");

        assert_eq!(repository.records.lock().unwrap().as_slice(), &[original]);
        assert!(trees.capture_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn import_rejects_unicode_casefold_aliases_in_the_same_batch() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "Straße".to_string()),
            tree(b"first"),
        );
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Codex, "STRASSE".to_string()),
            tree(b"second"),
        );

        LocalSkillService::import_with(
            &repository,
            &trees,
            vec![
                LocalSkillImport {
                    directory: "Straße".to_string(),
                    source_client: ManagedClientId::Claude,
                    apps: ManagedClientApps::only(ManagedClientId::Claude),
                },
                LocalSkillImport {
                    directory: "STRASSE".to_string(),
                    source_client: ManagedClientId::Codex,
                    apps: ManagedClientApps::only(ManagedClientId::Codex),
                },
            ],
            2,
        )
        .expect_err("Unicode case-fold aliases in one batch must be rejected");

        assert!(repository.records.lock().unwrap().is_empty());
        assert!(trees.capture_reads.lock().unwrap().is_empty());
    }

    #[test]
    fn import_recaptures_the_source_after_manifest_discovery() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            tree_from_contents(b"---\nname: Before Scan\n---\nold".to_vec()),
        );
        let scanned = LocalSkillService::scan_unmanaged_with(&repository, &trees)
            .expect("discover the initial manifest");
        assert_eq!(scanned.unmanaged[0].name, "Before Scan");
        assert!(trees.capture_reads.lock().unwrap().is_empty());

        let changed = tree_from_contents(b"---\nname: After Scan\n---\nnew".to_vec());
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            changed.clone(),
        );
        let imported = LocalSkillService::import_with(
            &repository,
            &trees,
            vec![LocalSkillImport {
                directory: "fixture".to_string(),
                source_client: ManagedClientId::Claude,
                apps: crate::domain::ManagedClientApps::only(ManagedClientId::Claude),
            }],
            1,
        )
        .expect("import must recapture the changed source");

        assert_eq!(imported[0].name, "After Scan");
        assert_eq!(
            imported[0].content_hash.as_deref(),
            Some(changed.content_hash.as_str())
        );
        assert_eq!(
            trees.capture_reads.lock().unwrap().as_slice(),
            &[(ManagedClientId::Claude, "fixture".to_string())]
        );
    }

    #[test]
    fn import_adopts_divergent_same_named_copies_without_error() {
        let repository = FailingRepository::default();
        let trees = MemoryTrees::default();
        let source = tree(b"claude-content");
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Claude, "fixture".to_string()),
            source.clone(),
        );
        // Codex has a same-named copy whose content differs (e.g. per-agent config).
        let diverged = tree(b"codex-config");
        trees.trees.lock().unwrap().insert(
            (ManagedClientId::Codex, "fixture".to_string()),
            diverged.clone(),
        );

        let imported = LocalSkillService::import_with(
            &repository,
            &trees,
            vec![LocalSkillImport {
                directory: "fixture".to_string(),
                source_client: ManagedClientId::Claude,
                apps: crate::domain::ManagedClientApps {
                    claude: true,
                    codex: true,
                    opencode: false,
                },
            }],
            1,
        )
        .expect("divergent same-named copies are adopted as-is");

        assert_eq!(imported[0].content_hash, None);
        assert!(!imported[0].cloud_eligible);
        // The diverged codex copy is left untouched (not overwritten).
        assert_eq!(
            trees
                .trees
                .lock()
                .unwrap()
                .get(&(ManagedClientId::Codex, "fixture".to_string())),
            Some(&diverged)
        );
        assert_eq!(repository.records.lock().unwrap().len(), 1);
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
    fn delete_without_a_baseline_removes_the_managed_copy() {
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

        // 分歧接受（无统一基线）的 Skill 仍然可以删除：直接移除受管目录。
        let removed = LocalSkillService::remove_with(&repository, &trees, "local-fixture")
            .expect("a Skill without a unified baseline can still be removed");
        assert!(removed);

        assert_eq!(
            trees
                .trees
                .lock()
                .unwrap()
                .get(&(ManagedClientId::Claude, "fixture".to_string())),
            None
        );
        assert_eq!(repository.records.lock().unwrap().len(), 0);
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
