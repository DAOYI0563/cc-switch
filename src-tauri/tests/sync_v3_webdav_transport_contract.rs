use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use wsl_code_switch_lib::adapters::{ReqwestSyncWebDavTransport, SyncWebDavTransportOptions};
use wsl_code_switch_lib::domain::{SyncEtag, SyncRemotePath, SyncWriteCondition};
use wsl_code_switch_lib::ports::{SyncTransportErrorCode, SyncTransportPort};

#[derive(Debug, Clone)]
struct CapturedRequest {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct ScriptedResponse {
    status: &'static str,
    headers: Vec<(&'static str, &'static str)>,
    body: Vec<u8>,
}

impl ScriptedResponse {
    fn new(status: &'static str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.push((name, value));
        self
    }

    fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

struct ScriptedServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: Option<JoinHandle<()>>,
}

impl ScriptedServer {
    fn start(responses: Vec<ScriptedResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind scripted WebDAV server");
        let address = listener.local_addr().expect("read scripted server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let handle = thread::spawn(move || {
            let mut responses = VecDeque::from(responses);
            while let Some(response) = responses.pop_front() {
                let (mut stream, _) = listener.accept().expect("accept scripted request");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("set scripted request timeout");
                let request = read_request(&mut stream);
                captured.lock().expect("capture request").push(request);
                write_response(&mut stream, response);
            }
        });
        Self {
            base_url: format!("http://{address}/dav"),
            requests,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Vec<CapturedRequest> {
        self.handle
            .take()
            .expect("scripted server handle")
            .join()
            .expect("scripted WebDAV server thread");
        Arc::try_unwrap(self.requests)
            .expect("all request references released")
            .into_inner()
            .expect("captured request lock")
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut received = Vec::new();
    let header_end = loop {
        if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).expect("read scripted request");
        assert!(count > 0, "request ended before its headers");
        received.extend_from_slice(&chunk[..count]);
    };

    let head = String::from_utf8(received[..header_end].to_vec()).expect("HTTP header UTF-8");
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("request method").to_string();
    let target = request_line.next().expect("request target").to_string();
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').expect("request header");
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while received.len() - header_end < content_length {
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).expect("read scripted body");
        assert!(count > 0, "request ended before its body");
        received.extend_from_slice(&chunk[..count]);
    }

    CapturedRequest {
        method,
        target,
        headers,
        body: received[header_end..header_end + content_length].to_vec(),
    }
}

fn write_response(stream: &mut TcpStream, response: ScriptedResponse) {
    let mut bytes = format!(
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    )
    .into_bytes();
    for (name, value) in response.headers {
        bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(&response.body);
    stream.write_all(&bytes).expect("write scripted response");
}

fn path(segments: &[&str]) -> SyncRemotePath {
    SyncRemotePath::new(segments.iter().copied()).expect("valid sync remote path")
}

fn transport(base_url: &str, max_object_bytes: usize) -> ReqwestSyncWebDavTransport {
    ReqwestSyncWebDavTransport::with_options(
        base_url,
        "alice",
        "password-secret",
        SyncWebDavTransportOptions {
            connect_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(5),
            max_object_bytes,
        },
    )
    .expect("build sync WebDAV transport")
}

#[tokio::test]
async fn explicit_connection_probe_is_read_only() {
    let server = ScriptedServer::start(vec![ScriptedResponse::new("207 Multi-Status")]);
    let transport = transport(&server.base_url, 1024);

    transport
        .test_connection()
        .await
        .expect("PROPFIND connection probe");

    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "PROPFIND");
    assert_eq!(requests[0].target, "/dav");
    assert_eq!(
        requests[0].headers.get("depth").map(String::as_str),
        Some("0")
    );
    assert!(requests[0].body.is_empty());
}

#[test]
fn remote_paths_etags_and_endpoint_diagnostics_are_strict() {
    let remote = path(&["sync-v3", "records", "provider a.json"]);
    assert_eq!(
        remote.segments(),
        &["sync-v3", "records", "provider a.json"]
    );
    for invalid in [vec![], vec![".."], vec!["a/b"], vec!["a\\b"], vec![""]] {
        assert!(SyncRemotePath::new(invalid).is_err());
    }

    assert_eq!(
        SyncEtag::new("\"manifest-7\"").unwrap().as_str(),
        "\"manifest-7\""
    );
    assert!(SyncEtag::new("manifest-7").is_err());
    assert!(SyncEtag::new("\"bad\r\ntag\"").is_err());

    let error = ReqwestSyncWebDavTransport::new(
        "https://alice:url-secret@example.invalid/dav?token=query-secret",
        "alice",
        "password-secret",
    )
    .unwrap_err();
    let rendered = format!("{error:?} {error}");
    for secret in ["url-secret", "query-secret", "password-secret"] {
        assert!(!rendered.contains(secret));
    }
    assert_eq!(error.code, SyncTransportErrorCode::InvalidConfiguration);
}

#[test]
fn transport_adapter_isolated_from_legacy_sync_and_application_infrastructure() {
    let source = include_str!("../src/adapters/sync_webdav.rs");
    for forbidden in [
        "crate::proxy",
        "crate::services::webdav",
        "crate::services::webdav_sync",
        "crate::database",
        "tauri::",
        "AppError",
        "log::",
    ] {
        assert!(
            !source.contains(forbidden),
            "sync-v3 transport must not depend on {forbidden}"
        );
    }
}

#[tokio::test]
async fn reads_bounded_objects_with_etags_and_basic_auth_without_diagnostic_leaks() {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::new("200 OK")
            .header("ETag", "\"manifest-7\"")
            .body(b"ciphertext".to_vec()),
        ScriptedResponse::new("404 Not Found"),
    ]);
    let endpoint = format!("{}?access_token=query-secret", server.base_url);
    let transport = transport(&endpoint, 1024);
    let debug = format!("{transport:?}");
    for secret in ["alice", "password-secret", "query-secret"] {
        assert!(!debug.contains(secret));
    }

    let object = transport
        .read(&path(&["sync-v3", "manifest.json"]), 1024)
        .await
        .unwrap()
        .expect("remote object");
    assert_eq!(object.bytes(), b"ciphertext");
    assert_eq!(object.etag().unwrap().as_str(), "\"manifest-7\"");
    assert!(transport
        .read(&path(&["sync-v3", "missing.json"]), 1024)
        .await
        .unwrap()
        .is_none());

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].target,
        "/dav/sync-v3/manifest.json?access_token=query-secret"
    );
    assert!(requests[0]
        .headers
        .get("authorization")
        .is_some_and(|value| value.starts_with("Basic ")));
}

#[tokio::test]
async fn conditional_writes_send_exact_cas_headers_and_return_etags() {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::new("204 No Content").header("ETag", "\"next-8\""),
        ScriptedResponse::new("201 Created").header("ETag", "\"created-1\""),
    ]);
    let transport = transport(&server.base_url, 1024);
    let remote = path(&["sync-v3", "manifest.json"]);

    let updated = transport
        .conditional_write(
            &remote,
            b"updated-envelope",
            &SyncWriteCondition::Match(SyncEtag::new("\"old-7\"").unwrap()),
        )
        .await
        .unwrap();
    assert_eq!(updated.etag().unwrap().as_str(), "\"next-8\"");

    let created = transport
        .conditional_write(
            &path(&["sync-v3", "records", "provider-a.json"]),
            b"new-envelope",
            &SyncWriteCondition::CreateOnly,
        )
        .await
        .unwrap();
    assert_eq!(created.etag().unwrap().as_str(), "\"created-1\"");

    let requests = server.finish();
    assert_eq!(requests[0].method, "PUT");
    assert_eq!(requests[0].headers.get("if-match").unwrap(), "\"old-7\"");
    assert_eq!(requests[0].body, b"updated-envelope");
    assert_eq!(requests[1].headers.get("if-none-match").unwrap(), "*");
    assert_eq!(requests[1].body, b"new-envelope");
}

#[tokio::test]
async fn directory_creation_verifies_existing_collections_with_propfind() {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::new("405 Method Not Allowed"),
        ScriptedResponse::new("207 Multi-Status"),
        ScriptedResponse::new("201 Created"),
    ]);
    let transport = transport(&server.base_url, 1024);
    transport
        .ensure_directories(&path(&["sync-v3", "records"]))
        .await
        .unwrap();

    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        vec!["MKCOL", "PROPFIND", "MKCOL"]
    );
    assert_eq!(requests[0].target, "/dav/sync-v3/");
    assert_eq!(requests[1].headers.get("depth").unwrap(), "0");
    assert_eq!(requests[2].target, "/dav/sync-v3/records/");
}

#[tokio::test]
async fn auth_cas_capacity_timeout_and_http_statuses_map_to_stable_codes() {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::new("401 Unauthorized"),
        ScriptedResponse::new("412 Precondition Failed"),
        ScriptedResponse::new("507 Insufficient Storage"),
        ScriptedResponse::new("504 Gateway Timeout"),
        ScriptedResponse::new("500 Internal Server Error").body(b"server-secret".to_vec()),
    ]);
    let endpoint = format!("{}?access_token=query-secret", server.base_url);
    let transport = transport(&endpoint, 1024);
    let remote = path(&["sync-v3", "manifest.json"]);

    let auth = transport.read(&remote, 1024).await.unwrap_err();
    let cas = transport
        .conditional_write(
            &remote,
            b"body",
            &SyncWriteCondition::Match(SyncEtag::new("\"old\"").unwrap()),
        )
        .await
        .unwrap_err();
    let capacity = transport
        .conditional_write(&remote, b"body", &SyncWriteCondition::CreateOnly)
        .await
        .unwrap_err();
    let timeout = transport.read(&remote, 1024).await.unwrap_err();
    let http = transport.read(&remote, 1024).await.unwrap_err();

    assert_eq!(auth.code, SyncTransportErrorCode::AuthenticationFailed);
    assert_eq!(cas.code, SyncTransportErrorCode::PreconditionFailed);
    assert_eq!(capacity.code, SyncTransportErrorCode::LimitExceeded);
    assert_eq!(timeout.code, SyncTransportErrorCode::Timeout);
    assert_eq!(http.code, SyncTransportErrorCode::HttpStatus);
    assert_eq!(http.context.get("status").map(String::as_str), Some("500"));

    let rendered = format!("{auth:?} {cas:?} {capacity:?} {timeout:?} {http:?}");
    for secret in ["alice", "password-secret", "query-secret", "server-secret"] {
        assert!(!rendered.contains(secret));
    }
    assert_eq!(server.finish().len(), 5);
}

#[tokio::test]
async fn local_and_remote_size_limits_and_invalid_etags_fail_closed() {
    let server = ScriptedServer::start(vec![
        ScriptedResponse::new("200 OK")
            .header("Content-Length", "9")
            .body(b"123456789".to_vec()),
        ScriptedResponse::new("200 OK")
            .header("ETag", "not-quoted")
            .body(b"1234".to_vec()),
    ]);
    let transport = transport(&server.base_url, 8);
    let remote = path(&["sync-v3", "manifest.json"]);

    let local = transport
        .conditional_write(&remote, b"123456789", &SyncWriteCondition::CreateOnly)
        .await
        .unwrap_err();
    assert_eq!(local.code, SyncTransportErrorCode::LimitExceeded);

    let remote_limit = transport.read(&remote, 8).await.unwrap_err();
    assert_eq!(remote_limit.code, SyncTransportErrorCode::LimitExceeded);

    let invalid_etag = transport.read(&remote, 8).await.unwrap_err();
    assert_eq!(invalid_etag.code, SyncTransportErrorCode::InvalidResponse);
    assert_eq!(
        server.finish().len(),
        2,
        "local rejection must perform zero I/O"
    );
}

#[tokio::test]
async fn actual_request_deadline_maps_to_timeout_without_endpoint_details() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind timeout server");
    let address = listener.local_addr().expect("read timeout server address");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept timeout request");
        let _request = read_request(&mut stream);
        thread::sleep(Duration::from_millis(250));
    });
    let endpoint = format!("http://{address}/dav?access_token=query-secret");
    let transport = ReqwestSyncWebDavTransport::with_options(
        &endpoint,
        "alice",
        "password-secret",
        SyncWebDavTransportOptions {
            connect_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_millis(50),
            max_object_bytes: 1024,
        },
    )
    .unwrap();

    let error = transport
        .read(&path(&["sync-v3", "manifest.json"]), 1024)
        .await
        .unwrap_err();
    assert_eq!(error.code, SyncTransportErrorCode::Timeout);
    let rendered = format!("{error:?} {error}");
    let address_text = address.to_string();
    for secret in [
        "alice",
        "password-secret",
        "query-secret",
        address_text.as_str(),
    ] {
        assert!(!rendered.contains(secret));
    }
    handle.join().expect("timeout server thread");
}
