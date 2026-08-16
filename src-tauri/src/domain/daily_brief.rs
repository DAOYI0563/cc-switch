use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Timelike, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BRIEF_TEMPLATE_VERSION: &str = "brief-html-v1";
pub const BRIEF_PROMPT_VERSION: &str = "brief-prompt-v1";
pub const MAX_AI_CALLS: usize = 50;
pub const MAX_INPUT_TOKENS: usize = 1_000_000;
pub const TARGET_CHUNK_TOKENS: usize = 24_000;
pub const MAX_RUN_SECONDS: u64 = 300;
pub const CHECKPOINT_TTL_DAYS: i64 = 7;

static API_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(sk-[A-Za-z0-9_-]{12,}|xox[baprs]-[A-Za-z0-9-]{10,}|gh[pousr]_[A-Za-z0-9]{20,})\b",
    )
    .expect("API key regex")
});
static ASSIGNMENT_SECRET_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|password|passwd|cookie|authorization)\b\s*[:=]\s*([^\s,;]+|\"[^\"]*\"|'[^']*')"#)
        .expect("secret assignment regex")
});
static BEARER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{8,}=*").expect("bearer regex"));
static CONNECTION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis)://[^\s]+")
        .expect("connection regex")
});
static PRIVATE_KEY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
        .expect("private key regex")
});
static CODE_FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```[^\n]*\n.*?```").expect("code fence regex"));
static DIFF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:diff --git |@@ |\+\+\+ |--- ).*(?:\n[+\- ].*)*").expect("diff regex")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefInputEvent {
    pub client: String,
    pub session_id: String,
    pub project: String,
    pub occurred_at_ms: i64,
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBriefInput {
    pub date: NaiveDate,
    pub events: Vec<BriefInputEvent>,
    pub source_fingerprint: String,
    pub approximate_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BriefItem {
    pub text: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub session_ids: Vec<String>,
    #[serde(default)]
    pub beijing_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DailyBriefDocument {
    pub daily_summary: String,
    #[serde(default)]
    pub project_work: Vec<BriefItem>,
    #[serde(default)]
    pub completed: Vec<BriefItem>,
    #[serde(default)]
    pub key_decisions: Vec<BriefItem>,
    #[serde(default)]
    pub blockers: Vec<BriefItem>,
    #[serde(default)]
    pub unfinished: Vec<BriefItem>,
    #[serde(default)]
    pub next_suggestions: Vec<BriefItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefRenderMetadata<'a> {
    pub date: NaiveDate,
    pub generated_at_ms: i64,
    pub device_name: &'a str,
    pub device_id: &'a str,
    pub model_name: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DailyBriefSettings {
    #[serde(default)]
    pub api_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub focus: String,
    #[serde(default)]
    pub auto_enabled: bool,
    #[serde(default)]
    pub enabled_at_ms: Option<i64>,
    #[serde(default)]
    pub privacy_confirmation_hash: Option<String>,
    #[serde(default)]
    pub connection_test_hash: Option<String>,
}

impl DailyBriefSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.api_url.len() > 2_048 || self.model.len() > 200 || self.focus.len() > 1_000 {
            return Err("每日简报设置超过长度限制".to_string());
        }
        if !self.api_url.trim().is_empty() {
            let url = url::Url::parse(self.api_url.trim())
                .map_err(|_| "每日简报 API 地址无效".to_string())?;
            let local_http = url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
            if url.scheme() != "https" && !local_http {
                return Err("每日简报 API 必须使用 HTTPS（本机回环地址除外）".to_string());
            }
            if url.username() != "" || url.password().is_some() || url.query().is_some() {
                return Err("每日简报 API 地址不得包含凭据或查询参数".to_string());
            }
        }
        if self.auto_enabled {
            let hash = self.configuration_hash();
            if self.api_url.trim().is_empty()
                || self.model.trim().is_empty()
                || self.enabled_at_ms.is_none()
                || self.privacy_confirmation_hash.as_deref() != Some(hash.as_str())
                || self.connection_test_hash.as_deref() != Some(hash.as_str())
            {
                return Err("自动简报需要完整配置、连通性测试和隐私确认".to_string());
            }
        }
        Ok(())
    }

    pub fn configuration_hash(&self) -> String {
        sha256_hex(format!("{}\0{}", self.api_url.trim(), self.model.trim()).as_bytes())
    }

    pub fn is_privacy_confirmed(&self) -> bool {
        self.privacy_confirmation_hash.as_deref() == Some(self.configuration_hash().as_str())
    }
}

pub fn beijing_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 60 * 60).expect("fixed Beijing offset")
}

pub fn beijing_date(timestamp_ms: i64) -> Result<NaiveDate, String> {
    let utc = DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .ok_or_else(|| "timestamp is outside the supported range".to_string())?;
    Ok(utc.with_timezone(&beijing_offset()).date_naive())
}

pub fn scheduled_target_date(now_ms: i64) -> Result<Option<NaiveDate>, String> {
    let utc = DateTime::<Utc>::from_timestamp_millis(now_ms)
        .ok_or_else(|| "timestamp is outside the supported range".to_string())?;
    let beijing = utc.with_timezone(&beijing_offset());
    if beijing.hour() < 8 {
        return Ok(None);
    }
    Ok(Some(beijing.date_naive() - Duration::days(1)))
}

pub fn due_dates(
    now_ms: i64,
    enabled_at_ms: i64,
    existing_dates: &BTreeSet<NaiveDate>,
) -> Result<Vec<NaiveDate>, String> {
    let Some(latest) = scheduled_target_date(now_ms)? else {
        return Ok(Vec::new());
    };
    let enabled_date = beijing_date(enabled_at_ms)?;
    let earliest = (latest - Duration::days(6)).max(enabled_date);
    let mut dates = Vec::new();
    let mut cursor = earliest;
    while cursor <= latest {
        if !existing_dates.contains(&cursor) {
            dates.push(cursor);
        }
        cursor += Duration::days(1);
    }
    Ok(dates)
}

pub fn prepare_input(
    date: NaiveDate,
    events: impl IntoIterator<Item = BriefInputEvent>,
) -> Result<PreparedBriefInput, String> {
    let mut unique = BTreeMap::new();
    for mut event in events {
        if !matches!(event.client.as_str(), "claude" | "codex" | "opencode") {
            continue;
        }
        if beijing_date(event.occurred_at_ms)? != date {
            continue;
        }
        let role = event.role.trim().to_ascii_lowercase();
        if !matches!(role.as_str(), "user" | "assistant" | "tool") {
            continue;
        }
        event.content = redact_text(&event.content);
        if event.content.len() > 32_000 {
            event.content.truncate(32_000);
            event.content.push_str("\n[CONTENT_TRUNCATED]");
        }
        if event.content.trim().is_empty() {
            continue;
        }
        event.role = role;
        let canonical = serde_json::to_vec(&event)
            .map_err(|_| "failed to serialize normalized brief input".to_string())?;
        unique.insert(sha256_hex(&canonical), event);
    }
    let events: Vec<_> = unique.into_values().collect();
    let canonical =
        serde_json::to_vec(&events).map_err(|_| "failed to serialize brief input".to_string())?;
    let approximate_tokens = canonical.len().div_ceil(4);
    Ok(PreparedBriefInput {
        date,
        events,
        source_fingerprint: sha256_hex(&canonical),
        approximate_tokens,
    })
}

pub fn redact_text(input: &str) -> String {
    let mut output = PRIVATE_KEY_RE
        .replace_all(input, "[PRIVATE_KEY_REDACTED]")
        .into_owned();
    output = API_KEY_RE
        .replace_all(&output, "[API_KEY_REDACTED]")
        .into_owned();
    output = BEARER_RE
        .replace_all(&output, "Bearer [TOKEN_REDACTED]")
        .into_owned();
    output = ASSIGNMENT_SECRET_RE
        .replace_all(&output, "$1=[SECRET_REDACTED]")
        .into_owned();
    output = CONNECTION_RE
        .replace_all(&output, "[CONNECTION_STRING_REDACTED]")
        .into_owned();
    output = CODE_FENCE_RE
        .replace_all(&output, "[CODE_BLOCK_OMITTED]")
        .into_owned();
    DIFF_RE.replace_all(&output, "[DIFF_OMITTED]").into_owned()
}

pub fn validate_and_redact_document(
    mut document: DailyBriefDocument,
) -> Result<DailyBriefDocument, String> {
    document.daily_summary = bounded_redacted(&document.daily_summary, 8_000, "dailySummary")?;
    for (label, items) in [
        ("projectWork", &mut document.project_work),
        ("completed", &mut document.completed),
        ("keyDecisions", &mut document.key_decisions),
        ("blockers", &mut document.blockers),
        ("unfinished", &mut document.unfinished),
        ("nextSuggestions", &mut document.next_suggestions),
    ] {
        if items.len() > 500 {
            return Err(format!("{label} contains too many items"));
        }
        for item in items {
            item.text = bounded_redacted(&item.text, 4_000, label)?;
            item.project = bounded_redacted(&item.project, 300, label)?;
            item.beijing_time = bounded_redacted(&item.beijing_time, 80, label)?;
            item.sources
                .retain(|source| matches!(source.as_str(), "claude" | "codex" | "opencode"));
            item.sources.sort();
            item.sources.dedup();
            if item.session_ids.len() > 50 {
                return Err(format!("{label} contains too many session IDs"));
            }
            for session_id in &mut item.session_ids {
                *session_id = bounded_redacted(session_id, 200, label)?;
            }
        }
    }
    Ok(document)
}

fn bounded_redacted(value: &str, limit: usize, field: &str) -> Result<String, String> {
    if value.len() > limit {
        return Err(format!("brief field {field} exceeds its size limit"));
    }
    Ok(redact_text(value.trim()))
}

pub fn render_complete_html(
    metadata: &BriefRenderMetadata<'_>,
    document: &DailyBriefDocument,
) -> Result<String, String> {
    let document = validate_and_redact_document(document.clone())?;
    let generated = beijing_datetime(metadata.generated_at_ms)?;
    let mut sections = String::new();
    for (title, items) in [
        ("按项目归类的工作内容", &document.project_work),
        ("已完成事项", &document.completed),
        ("关键决策", &document.key_decisions),
        ("问题与阻塞", &document.blockers),
        ("未完成事项", &document.unfinished),
        ("次日建议", &document.next_suggestions),
    ] {
        sections.push_str(&render_section(title, items));
    }
    let html = format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{date} 每日工作简报</title><style>{css}</style></head><body><main><header><p class=\"eyebrow\">WSL Code Switch · 每日工作简报</p><h1>{date}</h1><dl><div><dt>生成时间</dt><dd>{generated}</dd></div><div><dt>设备</dt><dd>{device}</dd></div><div><dt>模型</dt><dd>{model}</dd></div><div><dt>版本</dt><dd>{template} / {prompt}</dd></div></dl></header><section><h2>每日摘要</h2><p>{summary}</p></section>{sections}</main></body></html>",
        date = metadata.date,
        css = BRIEF_CSS,
        generated = escape_html(&generated),
        device = escape_html(&format!("{} ({})", metadata.device_name, metadata.device_id)),
        model = escape_html(metadata.model_name),
        template = BRIEF_TEMPLATE_VERSION,
        prompt = BRIEF_PROMPT_VERSION,
        summary = escape_html(&document.daily_summary),
    );
    validate_html(&html)?;
    Ok(html)
}

pub fn render_failed_html(date: NaiveDate, failed_sessions: &[String], reason: &str) -> String {
    let sessions = failed_sessions
        .iter()
        .take(100)
        .map(|session| format!("<li>{}</li>", escape_html(session)))
        .collect::<String>();
    format!(
        "<!doctype html><html lang=\"zh-CN\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{date} 简报未完成</title><style>{BRIEF_CSS}</style></head><body><main><header><p class=\"eyebrow\">WSL Code Switch · 每日工作简报</p><h1>{date}</h1></header><section><h2>未完成</h2><p>{reason}</p><ul>{sessions}</ul></section></main></body></html>",
        reason = escape_html(&redact_text(reason)),
    )
}

pub fn validate_html(html: &str) -> Result<(), String> {
    let lower = html.to_ascii_lowercase();
    for forbidden in [
        "<script",
        "javascript:",
        "<link",
        "<img",
        " url(",
        "@import",
        "<iframe",
        "<object",
    ] {
        if lower.contains(forbidden) {
            return Err(format!(
                "brief HTML contains forbidden content: {forbidden}"
            ));
        }
    }
    if !lower.starts_with("<!doctype html>") || !lower.contains("<meta charset=\"utf-8\">") {
        return Err("brief HTML is missing its required document structure".to_string());
    }
    Ok(())
}

pub fn content_hash(html: &str) -> String {
    sha256_hex(html.as_bytes())
}

pub fn brief_day_bounds_ms(date: NaiveDate) -> Result<(i64, i64), String> {
    let start = beijing_offset()
        .from_local_datetime(
            &date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| "invalid brief date".to_string())?,
        )
        .single()
        .ok_or_else(|| "invalid Beijing date boundary".to_string())?;
    let end = start + Duration::days(1) - Duration::milliseconds(1);
    Ok((start.timestamp_millis(), end.timestamp_millis()))
}

pub fn beijing_datetime(timestamp_ms: i64) -> Result<String, String> {
    let utc = DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .ok_or_else(|| "timestamp is outside the supported range".to_string())?;
    Ok(utc
        .with_timezone(&beijing_offset())
        .format("%Y-%m-%d %H:%M:%S %:z")
        .to_string())
}

fn render_section(title: &str, items: &[BriefItem]) -> String {
    let body = if items.is_empty() {
        "<p class=\"empty\">无</p>".to_string()
    } else {
        let rows = items
            .iter()
            .map(|item| {
                let mut metadata = Vec::new();
                if !item.project.is_empty() {
                    metadata.push(item.project.clone());
                }
                if !item.sources.is_empty() {
                    metadata.push(item.sources.join(" / "));
                }
                if !item.beijing_time.is_empty() {
                    metadata.push(item.beijing_time.clone());
                }
                if !item.session_ids.is_empty() {
                    metadata.push(item.session_ids.join(", "));
                }
                let meta = metadata
                    .iter()
                    .map(|value| escape_html(value))
                    .collect::<Vec<_>>()
                    .join(" · ");
                format!(
                    "<li><p>{}</p>{}</li>",
                    escape_html(&item.text),
                    if meta.is_empty() {
                        String::new()
                    } else {
                        format!("<small>{meta}</small>")
                    }
                )
            })
            .collect::<String>();
        format!("<ul>{rows}</ul>")
    };
    format!("<section><h2>{}</h2>{body}</section>", escape_html(title))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

const BRIEF_CSS: &str = "*{box-sizing:border-box}body{margin:0;background:#f6f7f9;color:#18181b;font:15px/1.65 system-ui,-apple-system,Segoe UI,sans-serif}main{width:min(900px,calc(100% - 32px));margin:32px auto 64px}header{border-bottom:2px solid #18181b;padding:12px 0 20px}h1{font-size:32px;margin:4px 0 18px;letter-spacing:0}.eyebrow{color:#52525b;font-size:12px;margin:0}dl{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:8px 24px;margin:0}dl div{display:flex;gap:8px}dt{color:#71717a}dd{margin:0;overflow-wrap:anywhere}section{border-bottom:1px solid #d4d4d8;padding:20px 0}h2{font-size:17px;margin:0 0 10px;letter-spacing:0}p{margin:0;white-space:pre-wrap;overflow-wrap:anywhere}ul{margin:0;padding-left:22px}li+li{margin-top:12px}small{display:block;color:#71717a;margin-top:2px;overflow-wrap:anywhere}.empty{color:#a1a1aa}@media(max-width:640px){main{width:min(100% - 24px,900px);margin-top:16px}h1{font-size:26px}dl{grid-template-columns:1fr}}@media(prefers-color-scheme:dark){body{background:#18181b;color:#f4f4f5}header{border-color:#f4f4f5}section{border-color:#3f3f46}dt,small,.eyebrow{color:#a1a1aa}}";

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(date: &str, hour: u32) -> i64 {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        beijing_offset()
            .from_local_datetime(&date.and_hms_opt(hour, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn schedule_uses_beijing_0800_and_limits_backfill_to_seven_days() {
        assert_eq!(
            scheduled_target_date(timestamp("2026-08-15", 7)).unwrap(),
            None
        );
        assert_eq!(
            scheduled_target_date(timestamp("2026-08-15", 8)).unwrap(),
            Some(NaiveDate::from_ymd_opt(2026, 8, 14).unwrap())
        );
        let dates = due_dates(
            timestamp("2026-08-15", 9),
            timestamp("2026-07-01", 0),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(dates.len(), 7);
        assert_eq!(dates[0], NaiveDate::from_ymd_opt(2026, 8, 8).unwrap());
        assert_eq!(dates[6], NaiveDate::from_ymd_opt(2026, 8, 14).unwrap());
    }

    #[test]
    fn input_is_split_by_beijing_day_deduplicated_and_redacted() {
        let date = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        let event = BriefInputEvent {
            client: "codex".to_string(),
            session_id: "session-1".to_string(),
            project: "project".to_string(),
            occurred_at_ms: timestamp("2026-08-14", 23),
            role: "user".to_string(),
            content: "Authorization: Bearer secret-token-123 and sk-abcdefghijklmnop\n```rs\nsecret code\n```".to_string(),
        };
        let prepared = prepare_input(date, [event.clone(), event]).unwrap();
        assert_eq!(prepared.events.len(), 1);
        let text = &prepared.events[0].content;
        assert!(!text.contains("secret-token"));
        assert!(!text.contains("sk-abcdef"));
        assert!(!text.contains("secret code"));
        assert!(text.contains("[CODE_BLOCK_OMITTED]"));
    }

    #[test]
    fn html_escapes_model_output_and_has_no_script_or_external_resource() {
        let document = DailyBriefDocument {
            daily_summary: "<script>alert('x')</script> password=hunter2".to_string(),
            project_work: vec![BriefItem {
                text: "<img src=https://evil.example/x>".to_string(),
                project: "alpha".to_string(),
                sources: vec!["codex".to_string()],
                session_ids: vec!["session-1".to_string()],
                beijing_time: "10:00".to_string(),
            }],
            completed: Vec::new(),
            key_decisions: Vec::new(),
            blockers: Vec::new(),
            unfinished: Vec::new(),
            next_suggestions: Vec::new(),
        };
        let html = render_complete_html(
            &BriefRenderMetadata {
                date: NaiveDate::from_ymd_opt(2026, 8, 14).unwrap(),
                generated_at_ms: timestamp("2026-08-15", 8),
                device_name: "DESKTOP",
                device_id: "device-1",
                model_name: "model-a",
            },
            &document,
        )
        .unwrap();
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("password=[SECRET_REDACTED]"));
        assert!(html.contains("&lt;img src=https://evil.example/x&gt;"));
        validate_html(&html).unwrap();
    }

    #[test]
    fn strict_document_rejects_unknown_fields_and_oversized_content() {
        assert!(serde_json::from_str::<DailyBriefDocument>(
            r#"{"dailySummary":"ok","unknown":true}"#
        )
        .is_err());
        let document = DailyBriefDocument {
            daily_summary: "x".repeat(8_001),
            project_work: Vec::new(),
            completed: Vec::new(),
            key_decisions: Vec::new(),
            blockers: Vec::new(),
            unfinished: Vec::new(),
            next_suggestions: Vec::new(),
        };
        assert!(validate_and_redact_document(document).is_err());
    }
}
