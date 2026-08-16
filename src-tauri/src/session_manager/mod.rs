pub mod providers;
pub mod terminal;

use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::path::Path;

use providers::{claude, codex, opencode};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub provider_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    #[serde(skip)]
    pub source_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSessionEvent {
    pub sequence: usize,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurred_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPage<T> {
    pub items: Vec<T>,
    pub offset: usize,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionSearchRequest {
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub from_ms: Option<i64>,
    #[serde(default)]
    pub to_ms: Option<i64>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub trait SessionSource: Send + Sync {
    fn provider_id(&self) -> &'static str;
    fn scan(&self) -> Vec<SessionMeta>;
    fn read_messages(&self, source_path: &str) -> Result<Vec<SessionMessage>, String>;
}

struct ClaudeSessionSource;
struct CodexSessionSource;
struct OpenCodeSessionSource;

impl SessionSource for ClaudeSessionSource {
    fn provider_id(&self) -> &'static str {
        "claude"
    }

    fn scan(&self) -> Vec<SessionMeta> {
        claude::scan_sessions()
    }

    fn read_messages(&self, source_path: &str) -> Result<Vec<SessionMessage>, String> {
        claude::load_messages(Path::new(source_path))
    }
}

impl SessionSource for CodexSessionSource {
    fn provider_id(&self) -> &'static str {
        "codex"
    }

    fn scan(&self) -> Vec<SessionMeta> {
        codex::scan_sessions()
    }

    fn read_messages(&self, source_path: &str) -> Result<Vec<SessionMessage>, String> {
        codex::load_messages(Path::new(source_path))
    }
}

impl SessionSource for OpenCodeSessionSource {
    fn provider_id(&self) -> &'static str {
        "opencode"
    }

    fn scan(&self) -> Vec<SessionMeta> {
        opencode::scan_sessions()
    }

    fn read_messages(&self, source_path: &str) -> Result<Vec<SessionMessage>, String> {
        if source_path.starts_with("sqlite:") {
            opencode::load_messages_sqlite(source_path)
        } else {
            opencode::load_messages(Path::new(source_path))
        }
    }
}

fn managed_sources() -> Vec<Box<dyn SessionSource>> {
    vec![
        Box::new(ClaudeSessionSource),
        Box::new(CodexSessionSource),
        Box::new(OpenCodeSessionSource),
    ]
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    scan_sessions_from(&managed_sources())
}

fn scan_sessions_from(sources: &[Box<dyn SessionSource>]) -> Vec<SessionMeta> {
    let mut sessions = Vec::new();
    for source in sources {
        sessions.extend(
            source
                .scan()
                .into_iter()
                .filter(|session| session.provider_id == source.provider_id()),
        );
    }
    sessions.sort_by_key(|session| Reverse(session_timestamp(session)));
    sessions
}

pub fn search_sessions(request: &SessionSearchRequest) -> Result<SessionPage<SessionMeta>, String> {
    search_sessions_from(&managed_sources(), request)
}

fn search_sessions_from(
    sources: &[Box<dyn SessionSource>],
    request: &SessionSearchRequest,
) -> Result<SessionPage<SessionMeta>, String> {
    validate_search_request(request)?;
    let project = normalized_filter(request.project.as_deref());
    let keyword = normalized_filter(request.keyword.as_deref());
    let mut matches = Vec::new();

    for source in sources {
        if request
            .provider_id
            .as_deref()
            .is_some_and(|provider_id| provider_id != "all" && provider_id != source.provider_id())
        {
            continue;
        }

        for session in source.scan() {
            if session.provider_id != source.provider_id() {
                continue;
            }
            let timestamp = session_timestamp(&session);
            if request.from_ms.is_some_and(|from| timestamp < from)
                || request.to_ms.is_some_and(|to| timestamp > to)
            {
                continue;
            }
            if project.as_ref().is_some_and(|needle| {
                !session
                    .project_dir
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(needle)
            }) {
                continue;
            }
            if let Some(needle) = keyword.as_ref() {
                let metadata = [
                    session.session_id.as_str(),
                    session.title.as_deref().unwrap_or_default(),
                    session.summary.as_deref().unwrap_or_default(),
                    session.project_dir.as_deref().unwrap_or_default(),
                ]
                .join(" ")
                .to_lowercase();
                let content_match = if metadata.contains(needle) {
                    true
                } else {
                    session
                        .source_path
                        .as_deref()
                        .and_then(|path| source.read_messages(path).ok())
                        .is_some_and(|events| {
                            events
                                .iter()
                                .any(|event| event.content.to_lowercase().contains(needle))
                        })
                };
                if !content_match {
                    continue;
                }
            }
            matches.push(session);
        }
    }

    matches.sort_by_key(|session| Reverse(session_timestamp(session)));
    Ok(page(matches, request.offset, page_limit(request.limit)?))
}

pub fn load_messages_page(
    provider_id: &str,
    session_id: &str,
    offset: usize,
    limit: Option<usize>,
) -> Result<SessionPage<NormalizedSessionEvent>, String> {
    let sources = managed_sources();
    let source = sources
        .iter()
        .find(|source| source.provider_id() == provider_id)
        .ok_or_else(|| format!("Unsupported provider: {provider_id}"))?;
    let session = source
        .scan()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .ok_or_else(|| "Session not found".to_string())?;
    let source_path = session
        .source_path
        .as_deref()
        .ok_or_else(|| "Session source is unavailable".to_string())?;
    let events = source
        .read_messages(source_path)?
        .into_iter()
        .enumerate()
        .map(|(sequence, message)| NormalizedSessionEvent {
            sequence,
            role: message.role,
            content: message.content,
            occurred_at: message.ts,
        })
        .collect();
    Ok(page(events, offset, page_limit(limit)?))
}

pub fn find_session(provider_id: &str, session_id: &str) -> Result<SessionMeta, String> {
    let sources = managed_sources();
    let source = sources
        .iter()
        .find(|source| source.provider_id() == provider_id)
        .ok_or_else(|| format!("Unsupported provider: {provider_id}"))?;
    source
        .scan()
        .into_iter()
        .find(|session| session.session_id == session_id)
        .ok_or_else(|| "Session not found".to_string())
}

pub fn collect_brief_events(
    from_ms: i64,
    to_ms: i64,
) -> Result<Vec<crate::domain::BriefInputEvent>, String> {
    if from_ms > to_ms {
        return Err("Brief session range is invalid".to_string());
    }
    let mut events = Vec::new();
    for source in managed_sources() {
        for session in source.scan() {
            let fallback_timestamp = session_timestamp(&session);
            let session_started = session.created_at.unwrap_or(fallback_timestamp);
            if session_started > to_ms || fallback_timestamp < from_ms {
                continue;
            }
            let source_path = session
                .source_path
                .as_deref()
                .ok_or_else(|| format!("Session {} has no readable source", session.session_id))?;
            let messages = source
                .read_messages(source_path)
                .map_err(|_| format!("Session {} could not be read", session.session_id))?;
            for message in messages {
                let occurred_at_ms = message.ts.unwrap_or(fallback_timestamp);
                if occurred_at_ms < from_ms || occurred_at_ms > to_ms {
                    continue;
                }
                events.push(crate::domain::BriefInputEvent {
                    client: session.provider_id.clone(),
                    session_id: session.session_id.clone(),
                    project: session.project_dir.clone().unwrap_or_default(),
                    occurred_at_ms,
                    role: message.role,
                    content: message.content,
                });
            }
        }
    }
    events.sort_by(|left, right| {
        left.occurred_at_ms
            .cmp(&right.occurred_at_ms)
            .then_with(|| left.client.cmp(&right.client))
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    Ok(events)
}

fn validate_search_request(request: &SessionSearchRequest) -> Result<(), String> {
    if let (Some(from), Some(to)) = (request.from_ms, request.to_ms) {
        if from > to {
            return Err("Session date range is invalid".to_string());
        }
    }
    if let Some(provider_id) = request.provider_id.as_deref() {
        if !matches!(provider_id, "all" | "claude" | "codex" | "opencode") {
            return Err(format!("Unsupported provider: {provider_id}"));
        }
    }
    page_limit(request.limit).map(|_| ())
}

fn normalized_filter(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
}

fn page_limit(limit: Option<usize>) -> Result<usize, String> {
    let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(format!(
            "Session page size must be between 1 and {MAX_PAGE_SIZE}"
        ));
    }
    Ok(limit)
}

fn page<T>(items: Vec<T>, offset: usize, limit: usize) -> SessionPage<T> {
    let total = items.len();
    let end = offset.saturating_add(limit).min(total);
    let items = if offset >= total {
        Vec::new()
    } else {
        items.into_iter().skip(offset).take(limit).collect()
    };
    SessionPage {
        items,
        offset,
        total,
        next_offset: (end < total).then_some(end),
    }
}

fn session_timestamp(session: &SessionMeta) -> i64 {
    session.last_active_at.or(session.created_at).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureSource {
        provider_id: &'static str,
        sessions: Vec<SessionMeta>,
        messages: Vec<SessionMessage>,
    }

    impl SessionSource for FixtureSource {
        fn provider_id(&self) -> &'static str {
            self.provider_id
        }

        fn scan(&self) -> Vec<SessionMeta> {
            self.sessions.clone()
        }

        fn read_messages(&self, _source_path: &str) -> Result<Vec<SessionMessage>, String> {
            Ok(self.messages.clone())
        }
    }

    fn fixture_session(id: &str, project: &str, timestamp: i64) -> SessionMeta {
        SessionMeta {
            provider_id: "claude".to_string(),
            session_id: id.to_string(),
            title: Some(format!("title {id}")),
            summary: None,
            project_dir: Some(project.to_string()),
            created_at: Some(timestamp),
            last_active_at: Some(timestamp),
            source_path: Some(format!("/{id}.jsonl")),
            resume_command: Some(format!("claude --resume {id}")),
        }
    }

    #[test]
    fn search_filters_content_and_paginates_without_exposing_source_paths() {
        let sources: Vec<Box<dyn SessionSource>> = vec![Box::new(FixtureSource {
            provider_id: "claude",
            sessions: vec![
                fixture_session("old", "/work/a", 100),
                fixture_session("new", "/work/b", 200),
            ],
            messages: vec![SessionMessage {
                role: "user".to_string(),
                content: "needle in original session".to_string(),
                ts: Some(200),
            }],
        })];
        let result = search_sessions_from(
            &sources,
            &SessionSearchRequest {
                keyword: Some("needle".to_string()),
                offset: 0,
                limit: Some(1),
                ..SessionSearchRequest::default()
            },
        )
        .expect("search");

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.total, 2);
        assert_eq!(result.next_offset, Some(1));
        let json = serde_json::to_value(&result).expect("serialize");
        assert!(json.to_string().find("sourcePath").is_none());
    }

    #[test]
    fn search_rejects_non_target_clients_and_invalid_ranges() {
        let sources: Vec<Box<dyn SessionSource>> = Vec::new();
        for request in [
            SessionSearchRequest {
                provider_id: Some("gemini".to_string()),
                ..SessionSearchRequest::default()
            },
            SessionSearchRequest {
                from_ms: Some(20),
                to_ms: Some(10),
                ..SessionSearchRequest::default()
            },
        ] {
            assert!(search_sessions_from(&sources, &request).is_err());
        }
    }

    #[test]
    fn active_session_manager_is_read_only_and_three_client_only() {
        let source = include_str!("mod.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .map(|(production, _)| production)
            .expect("session manager test boundary");

        for forbidden in [
            "delete_session",
            "remove_file",
            "remove_dir_all",
            "gemini::",
            "grokbuild::",
            "openclaw::",
            "hermes::",
        ] {
            assert!(!production.contains(forbidden), "found {forbidden}");
        }
        for required in [
            "ClaudeSessionSource",
            "CodexSessionSource",
            "OpenCodeSessionSource",
        ] {
            assert!(production.contains(required), "missing {required}");
        }
    }
}
