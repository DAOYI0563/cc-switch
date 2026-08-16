//! Pure validation and sanitization for portable Claude/Codex config snippets.

use serde_json::Value;
use toml_edit::{DocumentMut, TableLike};

use super::ManagedClientId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedCommonSnippet {
    pub text: String,
    pub removed_paths: Vec<String>,
}

pub fn validate_common_snippet(client: ManagedClientId, snippet: &str) -> Result<(), String> {
    let sanitized = sanitize_common_snippet(client, snippet)?;
    if sanitized.removed_paths.is_empty() {
        return Ok(());
    }

    Err(format!(
        "forbidden common config fields: {}",
        sanitized.removed_paths.join(", ")
    ))
}

pub fn extract_common_snippet(client: ManagedClientId, settings: &Value) -> Result<String, String> {
    match client {
        ManagedClientId::Claude => {
            sanitize_claude_value(settings.clone()).map(|result| result.text)
        }
        ManagedClientId::Codex => {
            let config = settings
                .get("config")
                .and_then(Value::as_str)
                .unwrap_or_default();
            sanitize_codex(config).map(|result| result.text)
        }
        ManagedClientId::Opencode => {
            Err("common config snippets are supported only for claude and codex".to_string())
        }
    }
}

pub fn sanitize_common_snippet(
    client: ManagedClientId,
    snippet: &str,
) -> Result<SanitizedCommonSnippet, String> {
    if snippet.trim().is_empty() {
        return Ok(SanitizedCommonSnippet {
            text: String::new(),
            removed_paths: Vec::new(),
        });
    }

    match client {
        ManagedClientId::Claude => {
            let value = serde_json::from_str::<Value>(snippet)
                .map_err(|error| format!("Invalid Claude common config JSON: {error}"))?;
            sanitize_claude_value(value)
        }
        ManagedClientId::Codex => sanitize_codex(snippet),
        ManagedClientId::Opencode => {
            Err("common config snippets are supported only for claude and codex".to_string())
        }
    }
}

pub fn is_sensitive_config_key(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    const SENSITIVE_SUFFIXES: &[&str] = &[
        "_KEY",
        "_API_KEY",
        "_ACCESS_KEY",
        "_ACCESS_KEY_ID",
        "_KEY_ID",
        "_PRIVATE_KEY",
        "_APIKEY",
        "_ACCESSKEY",
        "_SECRETKEY",
        "_APITOKEN",
        "_AUTH_TOKEN",
        "_TOKEN",
        "_PAT",
        "_PWD",
        "_PASS",
        "_PASSPHRASE",
        "_CREDS",
    ];
    const SENSITIVE_EXACT: &[&str] = &[
        "APIKEY",
        "API_KEY",
        "AUTH",
        "AUTHORIZATION",
        "COOKIE",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIALS",
    ];
    const SENSITIVE_CONTAINS: &[&str] = &[
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE_KEY",
        "BEARER_TOKEN",
        "AUTHORIZATION",
    ];

    SENSITIVE_EXACT.contains(&upper.as_str())
        || SENSITIVE_SUFFIXES
            .iter()
            .any(|suffix| upper.ends_with(suffix))
        || SENSITIVE_CONTAINS
            .iter()
            .any(|fragment| upper.contains(fragment))
}

fn normalized_key(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_managed_resource_key(name: &str) -> bool {
    let key = normalized_key(name);
    key == "mcp"
        || key.contains("mcpserver")
        || matches!(
            key.as_str(),
            "prompt" | "prompts" | "systemprompt" | "skill" | "skills"
        )
}

fn is_provider_specific_key(name: &str, at_root: bool, in_env: bool) -> bool {
    let key = normalized_key(name);
    if key.contains("baseurl")
        || key.contains("modelsurl")
        || key == "endpoint"
        || key.ends_with("endpoint")
        || key == "headers"
        || key.ends_with("headers")
        || matches!(
            key.as_str(),
            "modelprovider"
                | "modelproviders"
                | "modelcatalog"
                | "modelcatalogjson"
                | "provider"
                | "providerid"
                | "providertype"
                | "experimentalbearertoken"
        )
    {
        return true;
    }

    if at_root
        && matches!(
            key.as_str(),
            "model" | "models" | "primarymodel" | "smallfastmodel" | "wireapi" | "profiles"
        )
    {
        return true;
    }

    if in_env {
        let upper = name.to_ascii_uppercase();
        return upper == "ANTHROPIC_MODEL"
            || upper == "ANTHROPIC_REASONING_MODEL"
            || upper == "CLAUDE_CODE_SUBAGENT_MODEL"
            || upper == "CLAUDE_CODE_MAX_CONTEXT_TOKENS"
            || upper == "CLAUDE_CODE_AUTO_COMPACT_WINDOW"
            || (upper.starts_with("ANTHROPIC_DEFAULT_")
                && (upper.ends_with("_MODEL") || upper.ends_with("_MODEL_NAME")));
    }

    false
}

fn sanitize_claude_value(mut value: Value) -> Result<SanitizedCommonSnippet, String> {
    if !value.is_object() {
        return Err("Claude common config must be a JSON object".to_string());
    }

    let mut removed_paths = Vec::new();
    sanitize_json_node(&mut value, "", true, false, &mut removed_paths);
    let text = if value.as_object().is_none_or(serde_json::Map::is_empty) {
        "{}".to_string()
    } else {
        serde_json::to_string_pretty(&value)
            .map_err(|error| format!("Failed to serialize Claude common config: {error}"))?
    };
    Ok(SanitizedCommonSnippet {
        text,
        removed_paths,
    })
}

fn sanitize_json_node(
    value: &mut Value,
    path: &str,
    at_root: bool,
    in_env: bool,
    removed_paths: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if is_sensitive_config_key(&key)
                    || is_managed_resource_key(&key)
                    || is_provider_specific_key(&key, at_root, in_env)
                {
                    map.remove(&key);
                    removed_paths.push(child_path);
                    continue;
                }

                if let Some(child) = map.get_mut(&key) {
                    sanitize_json_node(
                        child,
                        &child_path,
                        false,
                        at_root && normalized_key(&key) == "env",
                        removed_paths,
                    );
                }
            }
            map.retain(|_, child| !child.as_object().is_some_and(serde_json::Map::is_empty));
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                sanitize_json_node(
                    item,
                    &format!("{path}[{index}]"),
                    false,
                    false,
                    removed_paths,
                );
            }
        }
        _ => {}
    }
}

fn sanitize_codex(snippet: &str) -> Result<SanitizedCommonSnippet, String> {
    let mut document = snippet
        .parse::<DocumentMut>()
        .map_err(|error| format!("Invalid Codex common config TOML: {error}"))?;
    let mut removed_paths = Vec::new();

    if document.get("web_search").and_then(|item| item.as_str()) == Some("disabled") {
        document.remove("web_search");
        removed_paths.push("web_search".to_string());
    }
    sanitize_toml_table(document.as_table_mut(), "", true, &mut removed_paths);

    let mut cleaned = String::new();
    let mut blank_run = 0usize;
    for line in document.to_string().lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                cleaned.push('\n');
            }
        } else {
            blank_run = 0;
            cleaned.push_str(line);
            cleaned.push('\n');
        }
    }

    Ok(SanitizedCommonSnippet {
        text: cleaned.trim().to_string(),
        removed_paths,
    })
}

fn sanitize_toml_table(
    table: &mut dyn TableLike,
    path: &str,
    at_root: bool,
    removed_paths: &mut Vec<String>,
) {
    let keys: Vec<String> = table.iter().map(|(key, _)| key.to_string()).collect();
    for key in keys {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        if is_sensitive_config_key(&key)
            || is_managed_resource_key(&key)
            || is_provider_specific_key(&key, at_root, false)
        {
            table.remove(&key);
            removed_paths.push(child_path);
            continue;
        }

        let Some(item) = table.get_mut(&key) else {
            continue;
        };
        if let Some(child) = item.as_table_like_mut() {
            sanitize_toml_table(child, &child_path, false, removed_paths);
        } else if let Some(array) = item.as_array_of_tables_mut() {
            for (index, child) in array.iter_mut().enumerate() {
                sanitize_toml_table(
                    child,
                    &format!("{child_path}[{index}]"),
                    false,
                    removed_paths,
                );
            }
        }
    }
}
