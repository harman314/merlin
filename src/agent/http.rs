//! Loopback HTTP transport for the agent API.
//!
//! One `POST` per request, JSON in and JSON out, bound to `127.0.0.1` and
//! behind a bearer token. No cross-origin headers are ever sent, so a page in
//! a browser cannot read an answer even if it guessed the port.

use std::net::Ipv4Addr;
use std::sync::Arc;

use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Bytes, Incoming};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HOST, WWW_AUTHENTICATE};
use hyper::service::service_fn;
use hyper::{Method, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use super::token::Token;
use super::{Call, Failure, Request};

/// Where an MCP client points. Everything else lives under `/v1/`.
pub const MCP_PATH: &str = "/mcp";

/// Largest request body accepted. An agent sends short JSON.
const MAX_BODY: usize = 1 << 20;

/// Everything a connection needs. Shared across accepted sockets.
pub struct Service {
    pub calls: mpsc::UnboundedSender<Call>,
    pub token: Token,
}

impl Service {
    /// Hands a request to the worker and waits for its answer.
    pub async fn ask(&self, request: Request) -> Result<serde_json::Value, Failure> {
        let (reply, receiver) = oneshot::channel();
        self.calls
            .send(Call { request, reply })
            .map_err(|_| Failure::Internal("the backend has stopped".to_owned()))?;
        receiver
            .await
            .map_err(|_| Failure::Internal("the backend dropped the request".to_owned()))?
    }
}

/// Claims the loopback port. Returned separately so the caller can log the
/// real port before the accept loop starts.
pub async fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await
}

/// Claims the port, reads or creates the token, and serves on a task.
///
/// Returns `None` after logging when the port is taken or the token cannot be
/// written. A missing agent API must not stop the window from opening.
pub async fn start(
    port: u16,
    token_file: &std::path::Path,
    calls: mpsc::UnboundedSender<Call>,
) -> Option<tokio::task::JoinHandle<()>> {
    let token = match Token::load_or_create(token_file) {
        Ok(token) => token,
        Err(error) => {
            log::warn!("agent api: no token, so it is not serving: {error:#}");
            return None;
        }
    };
    let listener = match bind(port).await {
        Ok(listener) => listener,
        Err(error) => {
            log::warn!("agent api: port {port} is not available: {error}");
            return None;
        }
    };
    log::info!("agent api: serving on 127.0.0.1:{port}{MCP_PATH}");
    let service = Arc::new(Service { calls, token });
    Some(tokio::spawn(serve(listener, service)))
}

/// Accepts connections until the listener fails or the task is dropped.
pub async fn serve(listener: TcpListener, service: Arc<Service>) {
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _peer)) => stream,
            Err(error) => {
                log::warn!("agent api: stopped accepting connections: {error}");
                return;
            }
        };
        let service = Arc::clone(&service);
        tokio::spawn(async move {
            let connection = hyper::server::conn::http1::Builder::new().serve_connection(
                TokioIo::new(stream),
                service_fn(move |request| {
                    let service = Arc::clone(&service);
                    async move { Ok::<_, std::convert::Infallible>(answer(service, request).await) }
                }),
            );
            if let Err(error) = connection.await {
                log::debug!("agent api: connection ended: {error}");
            }
        });
    }
}

type Reply = hyper::Response<Full<Bytes>>;

fn json_reply(status: StatusCode, body: &serde_json::Value) -> Reply {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    let mut reply = hyper::Response::new(Full::new(Bytes::from(bytes)));
    *reply.status_mut() = status;
    reply
        .headers_mut()
        .insert(CONTENT_TYPE, "application/json".parse().expect("a value"));
    reply
}

fn failed(failure: &Failure) -> Reply {
    let status = StatusCode::from_u16(failure.status()).unwrap_or(StatusCode::BAD_REQUEST);
    json_reply(status, &failure.body())
}

/// Rejects a `Host` that is not this machine, which is what a browser sends
/// when a page tries to reach the port by a rebound name.
fn host_is_local<B>(request: &hyper::Request<B>) -> bool {
    let Some(host) = request
        .headers()
        .get(HOST)
        .and_then(|host| host.to_str().ok())
    else {
        // HTTP/1.1 requires one. Anything without it is not a client we serve.
        return false;
    };
    let host = host.rsplit_once(':').map_or(host, |(name, _port)| name);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

async fn answer(service: Arc<Service>, request: hyper::Request<Incoming>) -> Reply {
    if !host_is_local(&request) {
        return failed(&Failure::Refused(
            "the agent API answers requests addressed to this machine only".to_owned(),
        ));
    }
    let presented = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(Token::from_header);
    if !presented.is_some_and(|token| service.token.matches(token)) {
        let mut reply = json_reply(
            StatusCode::UNAUTHORIZED,
            &serde_json::json!({ "error": {
                "code": "unauthorized",
                "message": "send the agent token as `Authorization: Bearer <token>`",
            }}),
        );
        reply
            .headers_mut()
            .insert(WWW_AUTHENTICATE, "Bearer".parse().expect("a value"));
        return reply;
    }
    if request.method() != Method::POST {
        // 405 rather than 400: an MCP client probes `GET /mcp` for a stream to
        // subscribe to, and reads this as "this server does not stream".
        let mut reply = json_reply(
            StatusCode::METHOD_NOT_ALLOWED,
            &serde_json::json!({ "error": {
                "code": "method_not_allowed",
                "message": "every agent API route takes POST",
            }}),
        );
        reply
            .headers_mut()
            .insert(hyper::header::ALLOW, "POST".parse().expect("a value"));
        return reply;
    }
    let path = request.uri().path().to_owned();
    let body = match read_body(request).await {
        Ok(body) => body,
        Err(failure) => return failed(&failure),
    };
    if path == MCP_PATH {
        return match super::mcp::handle(&service, &body).await {
            Some(answer) => json_reply(StatusCode::OK, &answer),
            // A notification is acknowledged with no body.
            None => {
                let mut reply = hyper::Response::new(Full::new(Bytes::new()));
                *reply.status_mut() = StatusCode::ACCEPTED;
                reply
            }
        };
    }
    match route(&path) {
        Some(Route::Rpc) => match parse_rpc(&body) {
            Ok(request) => reply_to(&service, request).await,
            Err(failure) => failed(&failure),
        },
        Some(Route::Method(method)) => {
            match parse_params(&body).and_then(|params| Request::from_call(&method, params)) {
                Ok(request) => reply_to(&service, request).await,
                Err(failure) => failed(&failure),
            }
        }
        None => failed(&Failure::Invalid(format!("no route at {path}"))),
    }
}

async fn reply_to(service: &Service, request: Request) -> Reply {
    match service.ask(request).await {
        Ok(value) => json_reply(StatusCode::OK, &value),
        Err(failure) => failed(&failure),
    }
}

enum Route {
    /// `POST /v1/rpc` with `{"method": ..., "params": {...}}`.
    Rpc,
    /// `POST /v1/<method>` with the parameters as the body.
    Method(String),
}

fn route(path: &str) -> Option<Route> {
    let rest = path.strip_prefix("/v1/")?;
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    if rest == "rpc" {
        return Some(Route::Rpc);
    }
    Some(Route::Method(rest.to_owned()))
}

async fn read_body(request: hyper::Request<Incoming>) -> Result<Bytes, Failure> {
    let body = request.into_body();
    // Refuse an oversized body before collecting it rather than after.
    if body
        .size_hint()
        .upper()
        .is_some_and(|upper| upper as usize > MAX_BODY)
    {
        return Err(Failure::Invalid("the request body is too large".to_owned()));
    }
    let collected = body
        .collect()
        .await
        .map_err(|error| Failure::Invalid(format!("could not read the request body: {error}")))?
        .to_bytes();
    if collected.len() > MAX_BODY {
        return Err(Failure::Invalid("the request body is too large".to_owned()));
    }
    Ok(collected)
}

/// An absent or empty body means no parameters.
fn parse_params(body: &[u8]) -> Result<serde_json::Value, Failure> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(serde_json::Value::Object(Default::default()));
    }
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|error| Failure::Invalid(format!("the body is not JSON: {error}")))?;
    match value {
        serde_json::Value::Object(_) | serde_json::Value::Null => Ok(value),
        _ => Err(Failure::Invalid(
            "the body must be a JSON object of parameters".to_owned(),
        )),
    }
}

fn parse_rpc(body: &[u8]) -> Result<Request, Failure> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rpc {
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    }
    let rpc: Rpc = serde_json::from_slice(body)
        .map_err(|error| Failure::Invalid(format!("the body is not a call: {error}")))?;
    Request::from_call(&rpc.method, rpc.params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_host(host: &str) -> hyper::Request<()> {
        hyper::Request::builder()
            .header(HOST, host)
            .body(())
            .expect("a request")
    }

    #[test]
    fn only_this_machines_own_names_are_served() {
        for host in ["localhost", "127.0.0.1:47145", "[::1]:47145", "localhost:1"] {
            assert!(host_is_local(&with_host(host)), "{host}");
        }
        // A page reached through a rebound name must not be answered.
        for host in ["example.com", "merlin.attacker.test:47145", "0.0.0.0"] {
            assert!(!host_is_local(&with_host(host)), "{host}");
        }
    }

    #[test]
    fn a_request_without_a_host_header_is_not_served() {
        let request = hyper::Request::builder().body(()).expect("a request");
        assert!(!host_is_local(&request));
    }

    #[test]
    fn a_path_maps_to_one_method_or_to_nothing() {
        assert!(matches!(route("/v1/rpc"), Some(Route::Rpc)));
        match route("/v1/list_chats") {
            Some(Route::Method(method)) => assert_eq!(method, "list_chats"),
            other => panic!("expected a method route, got {}", other.is_some()),
        }
        assert!(route("/v1/").is_none());
        assert!(route("/v1/a/b").is_none());
        assert!(route("/").is_none());
        assert!(route("/mcp").is_none());
    }

    #[test]
    fn an_empty_body_counts_as_no_parameters() {
        assert_eq!(
            parse_params(b"").expect("no parameters"),
            serde_json::json!({})
        );
        assert_eq!(
            parse_params(b"  \n").expect("no parameters"),
            serde_json::json!({})
        );
        assert_eq!(
            parse_params(b"{\"limit\":5}").expect("parameters")["limit"],
            5
        );
    }

    #[test]
    fn a_body_that_is_not_an_object_is_refused() {
        for body in [&b"[1,2]"[..], &b"\"hello\""[..], &b"7"[..]] {
            let failure = parse_params(body).expect_err("a rejection");
            assert_eq!(failure.code(), "invalid_request");
        }
        let failure = parse_params(b"{nope}").expect_err("a rejection");
        assert!(failure.message().contains("not JSON"), "{failure}");
    }

    #[test]
    fn an_rpc_body_names_its_method_and_parameters() {
        let request =
            parse_rpc(br#"{"method":"list_chats","params":{"limit":3}}"#).expect("a request");
        assert_eq!(
            request,
            Request::ListChats {
                limit: 3,
                query: None,
                unread_only: false,
                include_archived: false,
            }
        );
        // A call with no params at all is still a call.
        assert_eq!(
            parse_rpc(br#"{"method":"status"}"#).expect("a request"),
            Request::Status
        );
        assert!(parse_rpc(br#"{"verb":"status"}"#).is_err());
    }

    /// Stands in for the worker: answers every request with a marker, so the
    /// test is about the transport and nothing else.
    fn fake_worker() -> (Arc<Service>, Token, tokio::task::JoinHandle<()>) {
        let (calls, mut receiver) = mpsc::unbounded_channel::<Call>();
        let token = Token::generate().expect("a token");
        let service = Arc::new(Service {
            calls,
            token: token.clone(),
        });
        let worker = tokio::spawn(async move {
            while let Some(call) = receiver.recv().await {
                let method = call.request.method();
                let _ = call
                    .reply
                    .send(Ok(serde_json::json!({ "answered": method })));
            }
        });
        (service, token, worker)
    }

    async fn raw(port: u16, request: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .expect("a connection");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the request");
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.expect("the response");
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn post(path: &str, token: Option<&str>, body: &str) -> String {
        let auth = match token {
            Some(token) => format!("Authorization: Bearer {token}\r\n"),
            None => String::new(),
        };
        format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n{auth}\
             Content-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn the_socket_serves_reads_only_to_a_caller_with_the_token() {
        let (service, token, worker) = fake_worker();
        let listener = bind(0).await.expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        let server = tokio::spawn(serve(listener, service));

        // The right token gets the worker's answer.
        let answer = raw(port, &post("/v1/list_chats", Some(token.as_str()), "{}")).await;
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        assert!(answer.contains("\"answered\":\"list_chats\""), "{answer}");

        // No token, and a wrong token, are both turned away.
        for presented in [None, Some("deadbeef")] {
            let answer = raw(port, &post("/v1/list_chats", presented, "{}")).await;
            assert!(answer.starts_with("HTTP/1.1 401 "), "{answer}");
            // Header names arrive lowercase on the wire.
            assert!(answer.contains("www-authenticate: Bearer"), "{answer}");
            assert!(!answer.contains("answered"), "{answer}");
        }

        // A browser reaching the port under another name is refused.
        let rebound = format!(
            "POST /v1/status HTTP/1.1\r\nHost: evil.test\r\n\
             Authorization: Bearer {}\r\nContent-Length: 0\r\n\
             Connection: close\r\n\r\n",
            token.as_str()
        );
        let answer = raw(port, &rebound).await;
        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");

        // Nothing is served without a cross-origin header, so no page can read it.
        let answer = raw(port, &post("/v1/status", Some(token.as_str()), "{}")).await;
        assert!(
            !answer.to_lowercase().contains("access-control-allow"),
            "{answer}"
        );

        // An unknown route and a GET both come back as plain refusals.
        let answer = raw(port, &post("/v1/nonsense", Some(token.as_str()), "{}")).await;
        assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
        let get = format!(
            "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1\r\n\
             Authorization: Bearer {}\r\nConnection: close\r\n\r\n",
            token.as_str()
        );
        let answer = raw(port, &get).await;
        assert!(answer.starts_with("HTTP/1.1 405 "), "{answer}");
        assert!(answer.contains("allow: POST"), "{answer}");

        server.abort();
        worker.abort();
    }

    #[tokio::test]
    async fn an_mcp_client_lists_the_tools_over_the_same_socket() {
        let (service, token, worker) = fake_worker();
        let listener = bind(0).await.expect("a loopback port");
        let port = listener.local_addr().expect("an address").port();
        let server = tokio::spawn(serve(listener, service));

        let answer = raw(
            port,
            &post(
                MCP_PATH,
                Some(token.as_str()),
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
            ),
        )
        .await;
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        assert!(answer.contains("send_text"), "{answer}");

        // A notification is acknowledged with no body.
        let answer = raw(
            port,
            &post(
                MCP_PATH,
                Some(token.as_str()),
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            ),
        )
        .await;
        assert!(answer.starts_with("HTTP/1.1 202 "), "{answer}");

        // The token guards MCP exactly as it guards the plain routes.
        let answer = raw(port, &post(MCP_PATH, None, "{}")).await;
        assert!(answer.starts_with("HTTP/1.1 401 "), "{answer}");

        server.abort();
        worker.abort();
    }

    #[tokio::test]
    async fn the_token_file_is_created_when_the_server_starts() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("agent-token");
        let (calls, _receiver) = mpsc::unbounded_channel();
        let server = start(0, &path, calls).await.expect("a server");
        assert!(path.exists());
        server.abort();
    }

    #[test]
    fn a_failure_keeps_its_code_and_status_on_the_wire() {
        let reply = failed(&Failure::Offline);
        assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            reply.headers().get(CONTENT_TYPE).expect("a type"),
            "application/json"
        );
    }
}
