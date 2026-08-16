//! Fetch a model list from one explicit OpenAI-compatible endpoint.

use reqwest::header::{HeaderValue, USER_AGENT};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const ERROR_BODY_MAX_CHARS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FetchedModel {
    pub id: String,
    pub owned_by: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Option<Vec<ModelEntry>>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
    owned_by: Option<String>,
}

pub async fn fetch_models(
    base_url: &str,
    api_key: &str,
    is_full_url: bool,
    models_url_override: Option<&str>,
    user_agent: Option<HeaderValue>,
) -> Result<Vec<FetchedModel>, String> {
    if api_key.trim().is_empty() {
        return Err("API Key is required to fetch models".to_string());
    }

    let target = build_models_url(base_url, is_full_url, models_url_override)?;
    let client = crate::adapters::retained_http::get()?;
    let mut request = client
        .get(target)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .timeout(FETCH_TIMEOUT);
    if let Some(user_agent) = user_agent {
        request = request.header(USER_AGENT, user_agent);
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = truncate_body(response.text().await.unwrap_or_default());
        return Err(format!("HTTP {status}: {body}"));
    }

    let response: ModelsResponse = response
        .json()
        .await
        .map_err(|error| format!("Failed to parse response: {error}"))?;
    let mut models = response
        .data
        .unwrap_or_default()
        .into_iter()
        .map(|model| FetchedModel {
            id: model.id,
            owned_by: model.owned_by,
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    Ok(models)
}

/// Resolve exactly one endpoint. An explicit models URL always wins.
pub fn build_models_url(
    base_url: &str,
    is_full_url: bool,
    models_url_override: Option<&str>,
) -> Result<Url, String> {
    if let Some(explicit) = models_url_override
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        return parse_http_url(explicit, "Models URL");
    }

    let base = parse_http_url(base_url.trim(), "Base URL")?;
    let path = base.path().trim_end_matches('/');
    let target_path = if is_full_url {
        let prefix = path
            .find("/v1/")
            .map(|index| &path[..index])
            .or_else(|| path.rfind('/').map(|index| &path[..index]))
            .ok_or_else(|| "Cannot derive models endpoint from full URL".to_string())?;
        format!("{prefix}/v1/models")
    } else if ends_with_version_segment(path) {
        format!("{path}/models")
    } else {
        format!("{path}/v1/models")
    };

    let mut target = base;
    target.set_path(&target_path);
    target.set_query(None);
    target.set_fragment(None);
    Ok(target)
}

fn parse_http_url(raw: &str, label: &str) -> Result<Url, String> {
    if raw.is_empty() {
        return Err(format!("{label} is empty"));
    }
    let url = Url::parse(raw).map_err(|error| format!("Invalid {label}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("{label} must use HTTP or HTTPS"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{label} must not contain credentials"));
    }
    Ok(url)
}

fn ends_with_version_segment(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .unwrap_or("")
        .strip_prefix('v')
        .is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn truncate_body(body: String) -> String {
    if body.chars().count() <= ERROR_BODY_MAX_CHARS {
        return body;
    }
    let mut truncated = body.chars().take(ERROR_BODY_MAX_CHARS).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_one_deterministic_models_url() {
        assert_eq!(
            build_models_url("https://api.example.com", false, None)
                .unwrap()
                .as_str(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            build_models_url("https://api.example.com/v1", false, None)
                .unwrap()
                .as_str(),
            "https://api.example.com/v1/models"
        );
        assert_eq!(
            build_models_url("https://api.example.com/paas/v4", false, None)
                .unwrap()
                .as_str(),
            "https://api.example.com/paas/v4/models"
        );
        assert_eq!(
            build_models_url(
                "https://proxy.example.com/v1/chat/completions?ignored=true",
                true,
                None,
            )
            .unwrap()
            .as_str(),
            "https://proxy.example.com/v1/models"
        );
        assert_eq!(
            build_models_url(
                "https://api.example.com/anthropic",
                false,
                Some("https://api.example.com/models"),
            )
            .unwrap()
            .as_str(),
            "https://api.example.com/models"
        );
    }

    #[test]
    fn rejects_invalid_targets_before_network() {
        for target in ["", "file:///tmp/models", "https://u:p@example.com/models"] {
            assert!(build_models_url(target, false, None).is_err());
        }
        assert!(build_models_url(
            "https://api.example.com",
            false,
            Some("javascript:alert(1)"),
        )
        .is_err());
    }

    #[tokio::test]
    async fn fetches_once_and_sorts_unique_models() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let read = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(request.starts_with("GET /v1/models HTTP/1.1"));
            assert!(request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-key"));
            let body = r#"{"data":[{"id":"z","owned_by":"vendor"},{"id":"a"},{"id":"a"}]}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });

        let models = fetch_models(&format!("http://{address}"), "test-key", false, None, None)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(
            models,
            vec![
                FetchedModel {
                    id: "a".to_string(),
                    owned_by: None,
                },
                FetchedModel {
                    id: "z".to_string(),
                    owned_by: Some("vendor".to_string()),
                },
            ]
        );
    }

    #[tokio::test]
    async fn does_not_fall_back_after_not_found() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let read = socket.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..read])
                .starts_with("GET /anthropic/v1/models HTTP/1.1"));
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let error = fetch_models(
            &format!("http://{address}/anthropic"),
            "test-key",
            false,
            None,
            None,
        )
        .await
        .unwrap_err();
        server.await.unwrap();
        assert!(error.contains("HTTP 404"));
    }
}
