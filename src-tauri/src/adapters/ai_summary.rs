use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

use crate::ports::{
    AiSummaryClient, AiSummaryError, AiSummaryErrorCode, AiSummaryFuture, AiSummaryRequest,
};

pub struct DirectOpenAiSummaryClient {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: Zeroizing<String>,
}

impl std::fmt::Debug for DirectOpenAiSummaryClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DirectOpenAiSummaryClient")
            .field("endpoint", &"[redacted]")
            .field("model", &self.model)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

impl DirectOpenAiSummaryClient {
    pub fn new(api_url: &str, model: &str, api_key: String) -> Result<Self, AiSummaryError> {
        let api_url = api_url.trim();
        let model = model.trim();
        if api_url.is_empty() || model.is_empty() || api_key.trim().is_empty() {
            return Err(AiSummaryError::new(
                AiSummaryErrorCode::InvalidConfiguration,
                "每日简报 AI 配置不完整",
                false,
            ));
        }
        let parsed = url::Url::parse(api_url).map_err(|_| {
            AiSummaryError::new(
                AiSummaryErrorCode::InvalidConfiguration,
                "每日简报 API 地址无效",
                false,
            )
        })?;
        let local_http = parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
        if parsed.scheme() != "https" && !local_http {
            return Err(AiSummaryError::new(
                AiSummaryErrorCode::InvalidConfiguration,
                "每日简报 API 必须使用 HTTPS（本机回环地址除外）",
                false,
            ));
        }
        let endpoint = if api_url.ends_with("/chat/completions") {
            api_url.to_string()
        } else if api_url.ends_with("/v1") {
            format!("{api_url}/chat/completions")
        } else {
            format!("{}/v1/chat/completions", api_url.trim_end_matches('/'))
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| {
                AiSummaryError::new(
                    AiSummaryErrorCode::InvalidConfiguration,
                    "每日简报 HTTP 客户端初始化失败",
                    false,
                )
            })?;
        Ok(Self {
            client,
            endpoint,
            model: model.to_string(),
            api_key: Zeroizing::new(api_key),
        })
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

impl AiSummaryClient for DirectOpenAiSummaryClient {
    fn summarize<'a>(&'a self, request: &'a AiSummaryRequest) -> AiSummaryFuture<'a> {
        Box::pin(async move {
            let response = self
                .client
                .post(&self.endpoint)
                .bearer_auth(self.api_key.as_str())
                .json(&json!({
                    "model": self.model,
                    "messages": [
                        {"role": "system", "content": request.system_prompt},
                        {"role": "user", "content": request.input_json}
                    ],
                    "response_format": {"type": "json_object"},
                    "tools": [],
                    "tool_choice": "none",
                    "store": false,
                    "stream": false,
                    "temperature": 0.2
                }))
                .send()
                .await
                .map_err(|error| {
                    let timeout = error.is_timeout();
                    AiSummaryError::new(
                        if timeout {
                            AiSummaryErrorCode::Timeout
                        } else {
                            AiSummaryErrorCode::Unavailable
                        },
                        if timeout {
                            "每日简报 AI 请求超时"
                        } else {
                            "每日简报 AI 服务不可用"
                        },
                        true,
                    )
                })?;
            let status = response.status();
            if !status.is_success() {
                let (code, retryable) = match status {
                    StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                        (AiSummaryErrorCode::Authentication, false)
                    }
                    StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS => {
                        (AiSummaryErrorCode::RateLimited, true)
                    }
                    status if status.is_server_error() => (AiSummaryErrorCode::Unavailable, true),
                    _ => (AiSummaryErrorCode::RequestRejected, false),
                };
                let mut error = AiSummaryError::new(
                    code,
                    format!("每日简报 AI 请求失败（HTTP {}）", status.as_u16()),
                    retryable,
                );
                error.retry_after_ms = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .map(|seconds| seconds.saturating_mul(1_000).min(30_000));
                return Err(error);
            }
            let response = response.json::<ChatResponse>().await.map_err(|_| {
                AiSummaryError::new(
                    AiSummaryErrorCode::InvalidResponse,
                    "每日简报 AI 返回格式无效",
                    false,
                )
            })?;
            response
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.message.content)
                .filter(|content| !content.trim().is_empty())
                .ok_or_else(|| {
                    AiSummaryError::new(
                        AiSummaryErrorCode::InvalidResponse,
                        "每日简报 AI 返回了空内容",
                        false,
                    )
                })
        })
    }
}
