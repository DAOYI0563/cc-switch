use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::domain::{
    LegacyCommonSnippetRecord, LegacyIgnoredCounts, LegacyMcpRecord, LegacyMigrationPreview,
    LegacyMigrationStatus, LegacyPromptRecord, LegacyProviderRecord, LegacyRetainedCounts,
    LegacyRetainedSnapshot, LegacySkillRecord, LegacySourceKind,
};
use crate::ports::{LegacyDataError, LegacyDataErrorCode};

use super::files::{inspect_no_links, inspection_error, path_exists_without_following};
use super::{CONFIG_FILE, LEGACY_MAX_JSON_VERSION, MAX_JSON_BYTES};

pub(super) fn preview_json(
    config_path: &Path,
    skills_path: &Path,
) -> Result<LegacyMigrationPreview, LegacyDataError> {
    let config = read_json_value(config_path)?;
    let root = config.as_object().ok_or_else(|| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidJson,
            "legacy config root must be a JSON object",
        )
        .with_context("file", CONFIG_FILE)
    })?;
    let is_v1 = root.get("providers").is_some_and(Value::is_object)
        && root.get("current").is_some_and(Value::is_string)
        && !root.contains_key("apps");
    let version = if is_v1 {
        1
    } else {
        match root.get("version") {
            Some(Value::Number(number)) => {
                number.as_u64().and_then(|value| u32::try_from(value).ok())
            }
            Some(_) => None,
            None => Some(2),
        }
        .ok_or_else(|| {
            LegacyDataError::new(
                LegacyDataErrorCode::InvalidJson,
                "legacy config version must be a non-negative integer",
            )
            .with_context("file", CONFIG_FILE)
        })?
    };
    if version > LEGACY_MAX_JSON_VERSION {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::UnsupportedVersion,
            format!(
                "legacy JSON version {version} is newer than supported v{LEGACY_MAX_JSON_VERSION}"
            ),
        )
        .with_context("version", version.to_string())
        .with_context("supportedVersion", LEGACY_MAX_JSON_VERSION.to_string()));
    }

    let providers = provider_counts(root, is_v1);
    let prompt_counts = target_prompt_counts(root);
    let mut target_mcp_ids = BTreeSet::new();
    collect_mcp_ids(root, &["claude", "codex", "opencode"], &mut target_mcp_ids);

    let config_skills = nested(&config, &["skills", "skills"]);
    let external_skills = if config_skills.is_none() && path_exists_without_following(skills_path)?
    {
        inspect_no_links(skills_path)?;
        Some(read_json_value(skills_path)?)
    } else {
        None
    };
    let skill_count = config_skills.map(collection_len).unwrap_or_else(|| {
        external_skills
            .as_ref()
            .and_then(|value| nested(value, &["skills"]).or(Some(value)))
            .map(collection_len)
            .unwrap_or(0)
    });

    let snippets = root
        .get("common_config_snippets")
        .or_else(|| root.get("commonConfigSnippets"));
    let legacy_claude_snippet = root.get("claude_common_config_snippet");
    let claude_snippet = snippets.and_then(|value| value.get("claude"));
    let codex_snippet = snippets.and_then(|value| value.get("codex"));
    let common_snippets = u64::from(
        claude_snippet.is_some_and(non_empty_value)
            || legacy_claude_snippet.is_some_and(non_empty_value),
    ) + u64::from(codex_snippet.is_some_and(non_empty_value));

    let retained = LegacyRetainedCounts {
        claude_providers: providers.0,
        codex_providers: providers.1,
        opencode_providers: providers.2,
        mcp_servers: target_mcp_ids.len() as u64,
        claude_prompts: prompt_counts.0,
        codex_prompts: prompt_counts.1,
        opencode_prompts: prompt_counts.2,
        skills: skill_count,
        common_snippets,
    };

    let ignored = LegacyIgnoredCounts {
        non_target_client_records: count_non_target_client_records(root),
        profiles: root.get("profiles").map(collection_len).unwrap_or(0),
        proxy_and_routing: sum_keys(
            root,
            &[
                "proxy",
                "proxy_config",
                "proxyConfig",
                "proxy_settings",
                "proxySettings",
                "routing",
            ],
        ),
        usage_and_pricing: sum_keys(
            root,
            &[
                "model_pricing",
                "modelPricing",
                "pricing",
                "usage",
                "usage_stats",
                "usageStats",
            ],
        ),
        failover: count_json_failover(root),
        online_skill_repositories: nested(&config, &["skills", "repos"])
            .map(collection_len)
            .or_else(|| {
                external_skills
                    .as_ref()
                    .and_then(|value| nested(value, &["repos"]))
                    .map(collection_len)
            })
            .unwrap_or(0),
    };

    Ok(LegacyMigrationPreview {
        status: LegacyMigrationStatus::Ready,
        source: Some(LegacySourceKind::Json),
        source_version: Some(version),
        retained,
        ignored,
        files: Vec::new(),
        directory_fingerprint: None,
    })
}

fn provider_counts(root: &Map<String, Value>, is_v1: bool) -> (u64, u64, u64) {
    if is_v1 {
        return (root.get("providers").map(collection_len).unwrap_or(0), 0, 0);
    }
    (
        provider_count_for(root, "claude"),
        provider_count_for(root, "codex"),
        provider_count_for(root, "opencode"),
    )
}

fn provider_count_for(root: &Map<String, Value>, client: &str) -> u64 {
    manager_for(root, client)
        .and_then(|manager| manager.get("providers"))
        .map(collection_len)
        .unwrap_or(0)
}

fn manager_for<'a>(root: &'a Map<String, Value>, client: &str) -> Option<&'a Value> {
    root.get("apps")
        .and_then(Value::as_object)
        .and_then(|apps| apps.get(client))
        .or_else(|| root.get(client))
}

fn target_prompt_counts(root: &Map<String, Value>) -> (u64, u64, u64) {
    let prompts = root.get("prompts");
    let count = |client: &str| {
        prompts
            .and_then(|value| value.get(client))
            .and_then(|value| value.get("prompts"))
            .map(collection_len)
            .unwrap_or(0)
    };
    (count("claude"), count("codex"), count("opencode"))
}

fn collect_mcp_ids(root: &Map<String, Value>, clients: &[&str], ids: &mut BTreeSet<String>) {
    let Some(mcp) = root.get("mcp").and_then(Value::as_object) else {
        return;
    };
    if let Some(servers) = mcp.get("servers").and_then(Value::as_object) {
        ids.extend(servers.keys().cloned());
    }
    for client in clients {
        if let Some(servers) = mcp
            .get(*client)
            .and_then(|value| value.get("servers"))
            .and_then(Value::as_object)
        {
            ids.extend(servers.keys().cloned());
        }
    }
}

fn count_non_target_client_records(root: &Map<String, Value>) -> u64 {
    let target = ["claude", "codex", "opencode"];
    let mut count = 0;
    let mut seen = BTreeSet::new();
    if let Some(apps) = root.get("apps").and_then(Value::as_object) {
        for (client, manager) in apps {
            seen.insert(client.as_str());
            if !target.contains(&client.as_str()) {
                count += manager.get("providers").map(collection_len).unwrap_or(0);
            }
        }
    }
    for (client, manager) in root {
        if seen.contains(client.as_str()) || target.contains(&client.as_str()) {
            continue;
        }
        if manager.get("providers").is_some() && manager.get("current").is_some() {
            count += manager.get("providers").map(collection_len).unwrap_or(0);
        }
    }
    if let Some(prompts) = root.get("prompts").and_then(Value::as_object) {
        for (client, prompt_config) in prompts {
            if !target.contains(&client.as_str()) {
                count += prompt_config
                    .get("prompts")
                    .map(collection_len)
                    .unwrap_or(0);
            }
        }
    }
    if let Some(mcp) = root.get("mcp").and_then(Value::as_object) {
        for (client, mcp_config) in mcp {
            if client != "servers" && !target.contains(&client.as_str()) {
                count += mcp_config.get("servers").map(collection_len).unwrap_or(0);
            }
        }
    }
    if let Some(snippets) = root
        .get("common_config_snippets")
        .or_else(|| root.get("commonConfigSnippets"))
        .and_then(Value::as_object)
    {
        for (client, value) in snippets {
            if !["claude", "codex"].contains(&client.as_str()) && non_empty_value(value) {
                count += 1;
            }
        }
    }
    count
}

fn count_json_failover(root: &Map<String, Value>) -> u64 {
    let mut count = sum_keys(root, &["failover", "failover_queue", "failoverQueue"]);
    let inspect_manager = |manager: &Value| {
        manager
            .get("providers")
            .and_then(Value::as_object)
            .map(|providers| {
                providers
                    .values()
                    .filter(|provider| {
                        provider
                            .get("in_failover_queue")
                            .or_else(|| provider.get("inFailoverQueue"))
                            .and_then(Value::as_bool)
                            == Some(true)
                    })
                    .count() as u64
            })
            .unwrap_or(0)
    };
    if let Some(apps) = root.get("apps").and_then(Value::as_object) {
        count += apps.values().map(inspect_manager).sum::<u64>();
    } else {
        count += root.values().map(inspect_manager).sum::<u64>();
    }
    count
}

fn nested<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
}

fn collection_len(value: &Value) -> u64 {
    match value {
        Value::Null => 0,
        Value::Array(values) => values.len() as u64,
        Value::Object(values) => values.len() as u64,
        _ => 1,
    }
}

fn non_empty_value(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

fn sum_keys(root: &Map<String, Value>, keys: &[&str]) -> u64 {
    keys.iter()
        .filter_map(|key| root.get(*key))
        .map(collection_len)
        .sum()
}

pub(super) fn read_json_document(path: &Path) -> Result<String, LegacyDataError> {
    let value = read_json_value(path)?;
    if !value.is_object() {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::InvalidJson,
            "legacy JSON document root must be an object",
        )
        .with_context("path", path.display().to_string()));
    }
    serde_json::to_string(&value).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidJson,
            format!("failed to normalize legacy JSON document: {error}"),
        )
    })
}

fn read_json_value(path: &Path) -> Result<Value, LegacyDataError> {
    inspect_no_links(path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| inspection_error(path, "inspect legacy JSON", error))?;
    if !metadata.is_file() || metadata.len() > MAX_JSON_BYTES {
        return Err(LegacyDataError::new(
            LegacyDataErrorCode::InvalidJson,
            "legacy JSON source is not a regular file or exceeds the size limit",
        )
        .with_context("path", path.display().to_string())
        .with_context("maxBytes", MAX_JSON_BYTES.to_string()));
    }
    let bytes =
        fs::read(path).map_err(|error| inspection_error(path, "read legacy JSON", error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        LegacyDataError::new(
            LegacyDataErrorCode::InvalidJson,
            format!("legacy JSON is invalid: {error}"),
        )
        .with_context(
            "file",
            path.file_name().unwrap_or_default().to_string_lossy(),
        )
    })
}

pub(super) fn load_retained_json(
    config_path: &Path,
    skills_path: &Path,
    source_fingerprint: &str,
) -> Result<LegacyRetainedSnapshot, LegacyDataError> {
    let value = read_json_value(config_path)?;
    let root = value
        .as_object()
        .ok_or_else(|| invalid_record("legacy config root is invalid"))?;
    let is_v1 = root.get("providers").is_some_and(Value::is_object)
        && root.get("current").is_some_and(Value::is_string)
        && !root.contains_key("apps");
    let source_version = if is_v1 {
        1
    } else {
        root.get("version")
            .and_then(Value::as_u64)
            .and_then(|version| u32::try_from(version).ok())
            .unwrap_or(2)
    };

    let mut providers = Vec::new();
    if is_v1 {
        collect_json_providers(root, "claude", root, &mut providers)?;
    } else {
        for client in ["claude", "codex", "opencode"] {
            if let Some(manager) = manager_for(root, client).and_then(Value::as_object) {
                collect_json_providers(root, client, manager, &mut providers)?;
            }
        }
    }

    let mut mcp_by_id = std::collections::BTreeMap::<String, LegacyMcpRecord>::new();
    if let Some(mcp) = root.get("mcp").and_then(Value::as_object) {
        if let Some(servers) = mcp.get("servers").and_then(Value::as_object) {
            for (id, server) in servers {
                let object = server
                    .as_object()
                    .ok_or_else(|| invalid_record("legacy unified MCP record must be an object"))?;
                let config = object.get("server").unwrap_or(server);
                mcp_by_id.insert(
                    id.clone(),
                    LegacyMcpRecord {
                        id: id.clone(),
                        name: object
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(id)
                            .to_string(),
                        server_config_json: normalize_json(config)?,
                        description: optional_string(object, "description"),
                        homepage: optional_string(object, "homepage"),
                        docs: optional_string(object, "docs"),
                        tags_json: normalize_json(
                            object.get("tags").unwrap_or(&Value::Array(vec![])),
                        )?,
                        enabled_claude: app_enabled(object, "claude"),
                        enabled_codex: app_enabled(object, "codex"),
                        enabled_opencode: app_enabled(object, "opencode"),
                    },
                );
            }
        }
        for client in ["claude", "codex", "opencode"] {
            let Some(servers) = mcp
                .get(client)
                .and_then(|entry| entry.get("servers"))
                .and_then(Value::as_object)
            else {
                continue;
            };
            for (id, config) in servers {
                let server_config_json = normalize_json(config)?;
                let record = mcp_by_id.entry(id.clone()).or_insert(LegacyMcpRecord {
                    id: id.clone(),
                    name: id.clone(),
                    server_config_json,
                    description: None,
                    homepage: None,
                    docs: None,
                    tags_json: "[]".to_string(),
                    enabled_claude: false,
                    enabled_codex: false,
                    enabled_opencode: false,
                });
                match client {
                    "claude" => record.enabled_claude = true,
                    "codex" => record.enabled_codex = true,
                    "opencode" => record.enabled_opencode = true,
                    _ => unreachable!(),
                }
            }
        }
    }

    let mut prompts = Vec::new();
    for client in ["claude", "codex", "opencode"] {
        let Some(entries) = root
            .get("prompts")
            .and_then(|all| all.get(client))
            .and_then(|entry| entry.get("prompts"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (id, value) in entries {
            let object = value
                .as_object()
                .ok_or_else(|| invalid_record("legacy prompt record must be an object"))?;
            prompts.push(LegacyPromptRecord {
                id: id.clone(),
                client_id: client.to_string(),
                name: required_string_or(object, "name", id)?,
                content: required_string_or(object, "content", "")?,
                description: optional_string(object, "description"),
                enabled: object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                created_at_ms: object.get("createdAt").and_then(Value::as_i64).unwrap_or(0),
                updated_at_ms: object.get("updatedAt").and_then(Value::as_i64).unwrap_or(0),
            });
        }
    }

    let skills_value = if let Some(embedded) = nested(&value, &["skills", "skills"]).cloned() {
        Some(embedded)
    } else if path_exists_without_following(skills_path)? {
        let external = read_json_value(skills_path)?;
        Some(nested(&external, &["skills"]).cloned().unwrap_or(external))
    } else {
        None
    };
    let mut skills = Vec::new();
    if let Some(entries) = skills_value.as_ref().and_then(Value::as_object) {
        for (directory, state) in entries {
            let installed = state
                .get("installed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            skills.push(LegacySkillRecord {
                id: format!("local:{directory}"),
                name: directory.clone(),
                description: None,
                directory: directory.clone(),
                content_hash: None,
                enabled_claude: installed,
                enabled_codex: false,
                enabled_opencode: false,
                created_at_ms: 0,
                updated_at_ms: 0,
            });
        }
    }

    let snippets = root
        .get("common_config_snippets")
        .or_else(|| root.get("commonConfigSnippets"));
    let mut common_snippets = Vec::new();
    for client in ["claude", "codex"] {
        let value = snippets.and_then(|all| all.get(client)).or_else(|| {
            (client == "claude")
                .then(|| root.get("claude_common_config_snippet"))
                .flatten()
        });
        if let Some(content) = value
            .and_then(Value::as_str)
            .filter(|content| !content.trim().is_empty())
        {
            common_snippets.push(LegacyCommonSnippetRecord {
                id: format!("legacy-common-{client}"),
                client_id: client.to_string(),
                content: content.to_string(),
            });
        }
    }

    Ok(LegacyRetainedSnapshot {
        source: LegacySourceKind::Json,
        source_version,
        source_fingerprint: source_fingerprint.to_string(),
        providers,
        mcp_servers: mcp_by_id.into_values().collect(),
        prompts,
        skills,
        common_snippets,
        legacy_settings_json: None,
    })
}

fn collect_json_providers(
    _root: &Map<String, Value>,
    client: &str,
    manager: &Map<String, Value>,
    output: &mut Vec<LegacyProviderRecord>,
) -> Result<(), LegacyDataError> {
    let current = manager.get("current").and_then(Value::as_str);
    let Some(providers) = manager.get("providers").and_then(Value::as_object) else {
        return Ok(());
    };
    for (id, value) in providers {
        let object = value
            .as_object()
            .ok_or_else(|| invalid_record("legacy provider record must be an object"))?;
        let settings = object.get("settingsConfig").unwrap_or(value);
        output.push(LegacyProviderRecord {
            id: id.clone(),
            client_id: client.to_string(),
            name: required_string_or(object, "name", id)?,
            settings_config_json: normalize_json(settings)?,
            website_url: optional_string(object, "websiteUrl"),
            category: optional_string(object, "category"),
            created_at_ms: object.get("createdAt").and_then(Value::as_i64).unwrap_or(0),
            sort_index: object.get("sortIndex").and_then(Value::as_i64).unwrap_or(0),
            notes: optional_string(object, "notes"),
            icon: optional_string(object, "icon"),
            icon_color: optional_string(object, "iconColor"),
            meta_json: normalize_json(object.get("meta").unwrap_or(&Value::Object(Map::new())))?,
            is_current: current == Some(id.as_str()),
        });
    }
    Ok(())
}

fn app_enabled(object: &Map<String, Value>, client: &str) -> bool {
    object
        .get("apps")
        .and_then(|apps| apps.get(client))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

fn required_string_or(
    object: &Map<String, Value>,
    key: &str,
    fallback: &str,
) -> Result<String, LegacyDataError> {
    let value = object.get(key).and_then(Value::as_str).unwrap_or(fallback);
    if value.trim().is_empty() && key == "name" {
        return Err(invalid_record("legacy record name must not be empty"));
    }
    Ok(value.to_string())
}

fn normalize_json(value: &Value) -> Result<String, LegacyDataError> {
    serde_json::to_string(value)
        .map_err(|error| invalid_record(format!("failed to normalize legacy record JSON: {error}")))
}

fn invalid_record(message: impl Into<String>) -> LegacyDataError {
    LegacyDataError::new(LegacyDataErrorCode::InvalidRecord, message)
}
