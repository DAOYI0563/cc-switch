use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSummaryRequest {
    pub system_prompt: String,
    pub input_json: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSummaryErrorCode {
    InvalidConfiguration,
    Authentication,
    RateLimited,
    Timeout,
    Unavailable,
    InvalidResponse,
    RequestRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSummaryError {
    pub code: AiSummaryErrorCode,
    pub message: String,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
}

impl AiSummaryError {
    pub fn new(code: AiSummaryErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            retry_after_ms: None,
        }
    }
}

pub type AiSummaryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<String, AiSummaryError>> + Send + 'a>>;

pub trait AiSummaryClient: Send + Sync {
    fn summarize<'a>(&'a self, request: &'a AiSummaryRequest) -> AiSummaryFuture<'a>;
}
