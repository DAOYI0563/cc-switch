use std::fmt;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::{HeaderMap, CONTENT_LENGTH, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH};
use reqwest::redirect::Policy;
use reqwest::{Client, Method, RequestBuilder, Response, StatusCode, Url};
use zeroize::Zeroizing;

use crate::domain::{
    SyncEtag, SyncRemoteObject, SyncRemotePath, SyncWriteCondition, SyncWriteReceipt,
};
use crate::ports::{
    SyncTransportError, SyncTransportErrorCode, SyncTransportFuture, SyncTransportPort,
    MAX_SYNC_REMOTE_OBJECT_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncWebDavTransportOptions {
    pub connect_timeout: Duration,
    pub request_timeout: Duration,
    pub max_object_bytes: usize,
}

impl Default for SyncWebDavTransportOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(300),
            max_object_bytes: MAX_SYNC_REMOTE_OBJECT_BYTES,
        }
    }
}

pub struct ReqwestSyncWebDavTransport {
    client: Client,
    base_url: Url,
    username: Option<String>,
    password: Zeroizing<String>,
    options: SyncWebDavTransportOptions,
}

impl ReqwestSyncWebDavTransport {
    pub fn new(base_url: &str, username: &str, password: &str) -> Result<Self, SyncTransportError> {
        Self::with_options(
            base_url,
            username,
            password,
            SyncWebDavTransportOptions::default(),
        )
    }

    pub fn with_options(
        base_url: &str,
        username: &str,
        password: &str,
        options: SyncWebDavTransportOptions,
    ) -> Result<Self, SyncTransportError> {
        validate_options(options)?;
        let mut parsed = Url::parse(base_url.trim()).map_err(|_| invalid_configuration())?;
        if !matches!(parsed.scheme(), "http" | "https")
            || parsed.cannot_be_a_base()
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
        {
            return Err(invalid_configuration());
        }
        parsed.set_fragment(None);

        let username = username.trim();
        if username.is_empty() && !password.is_empty() {
            return Err(invalid_configuration());
        }
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .build()
            .map_err(|_| invalid_configuration())?;

        Ok(Self {
            client,
            base_url: parsed,
            username: (!username.is_empty()).then(|| username.to_string()),
            password: Zeroizing::new(password.to_string()),
            options,
        })
    }

    pub async fn test_connection(&self) -> Result<(), SyncTransportError> {
        let response = self
            .authenticated(
                self.client
                    .request(method_propfind(), self.base_url.clone())
                    .header("Depth", "0"),
            )
            .send()
            .await
            .map_err(|error| map_reqwest_error("propfind", &error))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(map_status("propfind", response.status()))
        }
    }

    fn authenticated(&self, builder: RequestBuilder) -> RequestBuilder {
        match &self.username {
            Some(username) => builder.basic_auth(username, Some(self.password.as_str())),
            None => builder,
        }
    }

    fn remote_url(
        &self,
        path: &SyncRemotePath,
        collection: bool,
    ) -> Result<Url, SyncTransportError> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| invalid_configuration())?;
            segments.pop_if_empty();
            for segment in path.segments() {
                segments.push(segment);
            }
            if collection {
                segments.push("");
            }
        }
        Ok(url)
    }

    async fn collection_exists(&self, url: Url) -> Result<bool, SyncTransportError> {
        let response = self
            .authenticated(
                self.client
                    .request(method_propfind(), url)
                    .header("Depth", "0"),
            )
            .send()
            .await
            .map_err(|error| map_reqwest_error("propfind", &error))?;
        match response.status() {
            status if status.is_success() || status == StatusCode::MULTI_STATUS => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => Err(map_status("propfind", status)),
        }
    }

    fn validate_read_limit(&self, max_bytes: usize) -> Result<(), SyncTransportError> {
        if max_bytes == 0 || max_bytes > self.options.max_object_bytes {
            return Err(SyncTransportError::new(
                SyncTransportErrorCode::InvalidInput,
                "sync WebDAV read limit is invalid",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for ReqwestSyncWebDavTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReqwestSyncWebDavTransport([redacted])")
    }
}

impl SyncTransportPort for ReqwestSyncWebDavTransport {
    fn ensure_directories<'a>(&'a self, path: &'a SyncRemotePath) -> SyncTransportFuture<'a, ()> {
        Box::pin(async move {
            for depth in 1..=path.segments().len() {
                let prefix = SyncRemotePath::new(path.segments()[..depth].iter().cloned())
                    .map_err(|_| invalid_input("sync WebDAV directory path is invalid"))?;
                let url = self.remote_url(&prefix, true)?;
                let response = self
                    .authenticated(self.client.request(method_mkcol(), url.clone()))
                    .send()
                    .await
                    .map_err(|error| map_reqwest_error("mkcol", &error))?;
                match response.status() {
                    status if status.is_success() => {}
                    StatusCode::METHOD_NOT_ALLOWED => {
                        if !self.collection_exists(url).await? {
                            return Err(map_status("mkcol", StatusCode::METHOD_NOT_ALLOWED));
                        }
                    }
                    status => return Err(map_status("mkcol", status)),
                }
            }
            Ok(())
        })
    }

    fn read<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        max_bytes: usize,
    ) -> SyncTransportFuture<'a, Option<SyncRemoteObject>> {
        Box::pin(async move {
            self.validate_read_limit(max_bytes)?;
            let url = self.remote_url(path, false)?;
            let response = self
                .authenticated(self.client.get(url))
                .send()
                .await
                .map_err(|error| map_reqwest_error("get", &error))?;
            if response.status() == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if !response.status().is_success() {
                return Err(map_status("get", response.status()));
            }
            enforce_content_length(response.headers(), max_bytes)?;
            let etag = response_etag(&response)?;
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|error| map_reqwest_error("read", &error))?;
                if bytes.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(limit_exceeded());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Some(SyncRemoteObject::new(bytes, etag)))
        })
    }

    fn conditional_write<'a>(
        &'a self,
        path: &'a SyncRemotePath,
        bytes: &'a [u8],
        condition: &'a SyncWriteCondition,
    ) -> SyncTransportFuture<'a, SyncWriteReceipt> {
        Box::pin(async move {
            if bytes.len() > self.options.max_object_bytes {
                return Err(limit_exceeded());
            }
            let url = self.remote_url(path, false)?;
            let builder = self
                .client
                .put(url)
                .header(CONTENT_TYPE, "application/json")
                .body(bytes.to_vec());
            let builder = match condition {
                SyncWriteCondition::Match(etag) => builder.header(IF_MATCH, etag.as_str()),
                SyncWriteCondition::CreateOnly => builder.header(IF_NONE_MATCH, "*"),
            };
            let response = self
                .authenticated(builder)
                .send()
                .await
                .map_err(|error| map_reqwest_error("put", &error))?;
            if !response.status().is_success() {
                return Err(map_status("put", response.status()));
            }
            Ok(SyncWriteReceipt::new(response_etag(&response)?))
        })
    }
}

fn method_propfind() -> Method {
    Method::from_bytes(b"PROPFIND").expect("constant PROPFIND method")
}

fn method_mkcol() -> Method {
    Method::from_bytes(b"MKCOL").expect("constant MKCOL method")
}

fn validate_options(options: SyncWebDavTransportOptions) -> Result<(), SyncTransportError> {
    if options.connect_timeout.is_zero()
        || options.request_timeout.is_zero()
        || options.max_object_bytes == 0
        || options.max_object_bytes > MAX_SYNC_REMOTE_OBJECT_BYTES
    {
        return Err(invalid_configuration());
    }
    Ok(())
}

fn response_etag(response: &Response) -> Result<Option<SyncEtag>, SyncTransportError> {
    let Some(value) = response.headers().get(ETAG) else {
        return Ok(None);
    };
    let raw = value.to_str().map_err(|_| invalid_response())?;
    SyncEtag::new(raw.to_string())
        .map(Some)
        .map_err(|_| invalid_response())
}

fn enforce_content_length(headers: &HeaderMap, max_bytes: usize) -> Result<(), SyncTransportError> {
    let Some(value) = headers.get(CONTENT_LENGTH) else {
        return Ok(());
    };
    let raw = value.to_str().map_err(|_| invalid_response())?;
    let length = raw.parse::<u64>().map_err(|_| invalid_response())?;
    if length > max_bytes as u64 {
        return Err(limit_exceeded());
    }
    Ok(())
}

fn map_reqwest_error(operation: &'static str, error: &reqwest::Error) -> SyncTransportError {
    let (code, message) = if error.is_timeout() {
        (
            SyncTransportErrorCode::Timeout,
            "sync WebDAV request timed out",
        )
    } else if error.is_connect() {
        (
            SyncTransportErrorCode::ConnectionFailed,
            "sync WebDAV connection failed",
        )
    } else {
        (
            SyncTransportErrorCode::TransportFailed,
            "sync WebDAV transport failed",
        )
    };
    SyncTransportError::new(code, message).with_context("operation", operation)
}

fn map_status(operation: &'static str, status: StatusCode) -> SyncTransportError {
    let (code, message) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => (
            SyncTransportErrorCode::AuthenticationFailed,
            "sync WebDAV authentication failed",
        ),
        StatusCode::PRECONDITION_FAILED => (
            SyncTransportErrorCode::PreconditionFailed,
            "sync WebDAV precondition failed",
        ),
        StatusCode::PAYLOAD_TOO_LARGE | StatusCode::INSUFFICIENT_STORAGE => (
            SyncTransportErrorCode::LimitExceeded,
            "sync WebDAV capacity limit was exceeded",
        ),
        StatusCode::REQUEST_TIMEOUT | StatusCode::GATEWAY_TIMEOUT => (
            SyncTransportErrorCode::Timeout,
            "sync WebDAV request timed out",
        ),
        _ => (
            SyncTransportErrorCode::HttpStatus,
            "sync WebDAV returned an unsuccessful status",
        ),
    };
    SyncTransportError::new(code, message)
        .with_context("operation", operation)
        .with_context("status", status.as_u16().to_string())
}

fn invalid_configuration() -> SyncTransportError {
    SyncTransportError::new(
        SyncTransportErrorCode::InvalidConfiguration,
        "sync WebDAV configuration is invalid",
    )
}

fn invalid_input(message: &'static str) -> SyncTransportError {
    SyncTransportError::new(SyncTransportErrorCode::InvalidInput, message)
}

fn invalid_response() -> SyncTransportError {
    SyncTransportError::new(
        SyncTransportErrorCode::InvalidResponse,
        "sync WebDAV response is invalid",
    )
}

fn limit_exceeded() -> SyncTransportError {
    SyncTransportError::new(
        SyncTransportErrorCode::LimitExceeded,
        "sync WebDAV object exceeds its size limit",
    )
}
