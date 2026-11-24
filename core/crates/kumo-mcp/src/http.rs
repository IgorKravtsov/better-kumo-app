use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use kumo_core::events::WatchRules;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::server::{KumoServer, RuleParams, rules_from};

pub const DEFAULT_LISTEN: &str = "0.0.0.0:8788";
pub const MCP_PATH: &str = "/mcp";
const TOKEN_FILE: &str = "mcp-token";
const MIN_TOKEN_LEN: usize = 32;

pub fn default_token_path() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("kumowatch")
        .join(TOKEN_FILE)
}

pub fn load_token(path: &Path) -> anyhow::Result<Option<String>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    check_private(path)?;
    let token = text.trim().to_string();
    if token.len() < MIN_TOKEN_LEN {
        bail!(
            "the token in {} is shorter than {MIN_TOKEN_LEN} characters; run `kumo-mcp token --rotate`",
            path.display()
        );
    }
    Ok(Some(token))
}

#[cfg(unix)]
fn check_private(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .with_context(|| format!("cannot read {}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        bail!(
            "{} can be read by other users; run `chmod 600 {}` or `kumo-mcp token --rotate`",
            path.display(),
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

pub fn load_or_create_token(path: &Path) -> anyhow::Result<String> {
    if let Some(token) = load_token(path)? {
        return Ok(token);
    }
    let token = random_hex()?;
    let tmp = write_temp(path, &token)?;
    let published = std::fs::hard_link(&tmp, path);
    let _ = std::fs::remove_file(&tmp);
    match published {
        Ok(()) => Ok(token),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => load_token(path)?
            .with_context(|| format!("{} disappeared while it was being created", path.display())),
        Err(e) => Err(e).with_context(|| {
            format!(
                "cannot create {}; its folder must support hard links, or pass a --token-file on a local disk",
                path.display()
            )
        }),
    }
}

pub fn create_token(path: &Path) -> anyhow::Result<String> {
    let token = random_hex()?;
    let tmp = write_temp(path, &token)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("cannot write {}", path.display()));
    }
    Ok(token)
}

fn random_hex() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_temp(path: &Path, token: &str) -> anyhow::Result<PathBuf> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = dir.join(format!(".{TOKEN_FILE}.{suffix}.tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut file| {
        writeln!(file, "{token}")?;
        file.sync_all()
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("cannot write {}", tmp.display()));
    }
    Ok(tmp)
}

pub fn router(server: KumoServer, token: String, shutdown: CancellationToken) -> Router {
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .enforce_origin_validation()
        .with_cancellation_token(shutdown.clone());
    let events_server = server.clone();
    let service = StreamableHttpService::new(
        move || Ok(server.for_session()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .nest_service(MCP_PATH, service)
        .route("/events", get(events))
        .with_state((events_server, shutdown))
        .layer(middleware::from_fn_with_state(
            Arc::new(token),
            require_token,
        ))
}

#[derive(Debug, Default, Deserialize)]
struct EventsQuery {
    people: Option<String>,
    kinds: Option<String>,
    places: Option<String>,
    label: Option<String>,
}

impl EventsQuery {
    fn rules(self) -> anyhow::Result<Option<WatchRules>> {
        if self.people.is_none()
            && self.kinds.is_none()
            && self.places.is_none()
            && self.label.is_none()
        {
            return Ok(None);
        }
        let split =
            |value: Option<String>| value.map(|v| v.split(',').map(str::to_string).collect());
        rules_from(&[RuleParams {
            people: split(self.people),
            kinds: split(self.kinds),
            places: split(self.places),
            label: self.label,
        }])
        .map(Some)
    }
}

async fn events(
    State((server, shutdown)): State<(KumoServer, CancellationToken)>,
    Query(query): Query<EventsQuery>,
) -> Response {
    let given = match query.rules().and_then(|given| {
        server.event_rules(given.as_ref())?;
        Ok(given)
    }) {
        Ok(rules) => rules,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}\n")).into_response(),
    };
    // Each stream starts now and owns both cursors, independent of every MCP session.
    let server = server.for_session();
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(32);
    let stopping = shutdown.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = stopping.cancelled() => {}
            _ = tx.closed() => {}
            result = server.stream_events(given, &tx) => {
                if let Err(e) = result {
                    let _ = tx.send(json!({ "kind": "error", "text": format!("{e:#}") })).await;
                }
            }
        }
    });
    let heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(60),
    );
    let stream = futures_util::stream::unfold(
        (rx, shutdown, heartbeat),
        |(mut rx, shutdown, mut heartbeat)| async move {
            let value = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return None,
                value = rx.recv() => value?,
                _ = heartbeat.tick() => json!({ "kind": "heartbeat" }),
            };
            let line = format!("{value}\n");
            Some((
                Ok::<_, std::convert::Infallible>(line),
                (rx, shutdown, heartbeat),
            ))
        },
    );
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

async fn require_token(State(token): State<Arc<String>>, request: Request, next: Next) -> Response {
    if authorized(request.headers(), &token) {
        if request.headers().contains_key(header::ORIGIN) {
            return (StatusCode::FORBIDDEN, "browser origins are not allowed\n").into_response();
        }
        return next.run(request).await;
    }
    let mut response =
        (StatusCode::UNAUTHORIZED, "missing or wrong bearer token\n").into_response();
    response
        .headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn authorized(headers: &HeaderMap, token: &str) -> bool {
    let given = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    given.len() == token.len()
        && given
            .bytes()
            .zip(token.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn query_parameters_become_one_rule_or_saved_rules() {
        let empty = Query::<EventsQuery>::try_from_uri(&"/events".parse().unwrap()).unwrap();
        assert!(empty.0.rules().unwrap().is_none());
        let query = Query::<EventsQuery>::try_from_uri(
            &"/events?people=amy,%20Amy%20Smith%20,,&kinds=message,nudge&places=Lobby,%20floor-1&label=Team"
                .parse().unwrap(),
        ).unwrap();
        let rules = query.0.rules().unwrap().unwrap();
        assert_eq!(rules.rules.len(), 1);
        let rule = &rules.rules[0];
        assert_eq!(rule.label, "Team");
        assert_eq!(
            rule.kinds,
            [
                kumo_core::events::EventKind::Message,
                kumo_core::events::EventKind::Nudge
            ]
        );
        assert_eq!(
            rule.people
                .iter()
                .map(|p| p.user_id.as_str())
                .collect::<Vec<_>>(),
            ["amy", "Amy Smith"]
        );
        assert_eq!(
            rule.places
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["Lobby", "floor-1"]
        );
        assert!(
            EventsQuery {
                kinds: Some("invalid".into()),
                ..Default::default()
            }
            .rules()
            .is_err()
        );
    }

    fn incoming(id: i64, sender: &str) -> kumo_core::chat::IncomingMessage {
        kumo_core::chat::IncomingMessage {
            channel_url: "chat".into(),
            channel_title: "Team".into(),
            category: kumo_core::chat::ChannelCategory::Direct,
            message_id: id,
            parent_id: Some(9),
            created_at: 0,
            sender_id: sender.into(),
            sender_name: sender.into(),
            text: "hello\nworld".into(),
            mentions_me: false,
            channel_is_open: false,
        }
    }

    async fn event_line(response: &mut reqwest::Response) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\n") {
                bytes.extend_from_slice(&response.chunk().await.unwrap().expect("stream ended"));
            }
            serde_json::from_slice(&bytes).unwrap()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn streams_matching_messages_with_private_cursors_and_stops_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let connection = Arc::new(crate::kumo::Connection::for_test(
            crate::kumo::ConnectOptions {
                session_path: dir.path().join("session.json"),
                space: None,
                db_path: Some(dir.path().join("chat.db")),
                devtools_port: 1,
                agents_path: dir.path().join("agents.json"),
                watch_path: dir.path().join("watch.json"),
            },
        ));
        let server = KumoServer::new(connection.clone(), crate::server::ServerOptions::default());
        let session = server.for_session();
        connection.arrivals().push(incoming(1, "amy"));
        let shutdown = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/events", listener.local_addr().unwrap());
        let token = "f".repeat(64);
        let app = router(server, token.clone(), shutdown.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await });
        let client = reqwest::Client::new();
        let get = || client.get(format!("{url}?people=amy&kinds=message&label=Team"));
        assert_eq!(
            get().send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get().bearer_auth("wrong").send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            get()
                .bearer_auth(&token)
                .header(header::ORIGIN, "http://evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let no_rules = client.get(&url).bearer_auth(&token).send().await.unwrap();
        assert_eq!(no_rules.status(), StatusCode::BAD_REQUEST);
        assert!(
            no_rules
                .text()
                .await
                .unwrap()
                .contains(crate::server::NO_RULES)
        );
        let invalid = client
            .get(format!("{url}?kinds=invalid"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let mut response = get().bearer_auth(&token).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/x-ndjson"
        );
        let mut second = get().bearer_auth(&token).send().await.unwrap();
        connection.arrivals().push(incoming(2, "bob"));
        connection.arrivals().push(incoming(3, "amy"));
        for response in [&mut response, &mut second] {
            let event = event_line(response).await;
            assert_eq!(event["id"], 3);
            assert_eq!(event["kind"], "message");
            assert_eq!(event["reply_to"], 9);
            assert_eq!(event["text"], "hello\nworld");
            assert_eq!(event["rules"], json!(["Team"]));
        }
        // Reading the stream did not advance the MCP session's cursor.
        let rules = rules_from(&[RuleParams {
            kinds: Some(vec!["message".into()]),
            people: None,
            places: None,
            label: None,
        }])
        .unwrap();
        let kumo = session.kumo().await.unwrap();
        let (events, _, _) = session
            .collect_matching_events(&kumo, &rules)
            .await
            .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
        rules.save(&dir.path().join("watch.json")).unwrap();
        let mut saved = client.get(&url).bearer_auth(&token).send().await.unwrap();
        assert_eq!(saved.status(), StatusCode::OK);
        connection.arrivals().push(incoming(4, "amy"));
        assert_eq!(event_line(&mut saved).await["id"], 4);
        // A changed saved rule set is picked up by the existing stream.
        std::fs::write(dir.path().join("watch.json"), r#"{"rules":[]}"#).unwrap();
        connection.arrivals().push(incoming(5, "bob"));
        let error = event_line(&mut saved).await;
        assert_eq!(error["kind"], "error");
        assert!(
            error["text"]
                .as_str()
                .unwrap()
                .contains(crate::server::NO_RULES)
        );
        assert!(saved.chunk().await.unwrap().is_none());
        // Dropping one client does not stop another watcher.
        drop(second);
        assert_eq!(event_line(&mut response).await["id"], 4);
        shutdown.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), response.chunk())
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        task.abort();
    }

    #[test]
    fn accepts_only_the_exact_bearer_token() {
        let token = "a".repeat(64);
        assert!(authorized(&headers(&format!("Bearer {token}")), &token));
        assert!(!authorized(&headers(&format!("Bearer {token}x")), &token));
        assert!(!authorized(
            &headers(&format!("Bearer {}", "b".repeat(64))),
            &token
        ));
        assert!(!authorized(&headers(&format!("Basic {token}")), &token));
        assert!(!authorized(&HeaderMap::new(), &token));
    }

    #[test]
    fn creates_a_private_token_once_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(TOKEN_FILE);
        assert_eq!(load_token(&path).unwrap(), None);
        let token = load_or_create_token(&path).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(load_or_create_token(&path).unwrap(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let rotated = create_token(&path).unwrap();
        assert_ne!(rotated, token);
        assert_eq!(load_token(&path).unwrap(), Some(rotated));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from(TOKEN_FILE)]);
    }

    #[test]
    fn concurrent_first_runs_agree_on_one_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TOKEN_FILE);
        let tokens: Vec<String> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| load_or_create_token(&path).unwrap()))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let on_disk = load_token(&path).unwrap().unwrap();
        assert!(tokens.iter().all(|t| *t == on_disk));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_token_file_other_users_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TOKEN_FILE);
        std::fs::write(&path, format!("{}\n", "e".repeat(64))).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_token(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load_token(&path).unwrap(), Some("e".repeat(64)));
    }

    #[test]
    fn rejects_a_short_token_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(TOKEN_FILE);
        let tmp = write_temp(&path, "short").unwrap();
        std::fs::rename(tmp, &path).unwrap();
        assert!(load_token(&path).is_err());
    }

    #[tokio::test]
    async fn serves_mcp_only_with_the_token_and_without_a_browser_origin() {
        let dir = tempfile::tempdir().unwrap();
        let connection = Arc::new(crate::kumo::Connection::new(crate::kumo::ConnectOptions {
            session_path: dir.path().join("session.json"),
            space: None,
            db_path: Some(dir.path().join("chat.db")),
            devtools_port: 1,
            agents_path: dir.path().join("agents.json"),
            watch_path: dir.path().join("watch.json"),
        }));
        let server = KumoServer::new(connection, crate::server::ServerOptions::default());
        let token = "c".repeat(64);
        let shutdown = CancellationToken::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{MCP_PATH}", listener.local_addr().unwrap());
        let app = router(server, token.clone(), shutdown.clone());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        });
        let client = reqwest::Client::new();
        let post = || {
            client
                .post(&url)
                .header(header::ACCEPT, "application/json, text/event-stream")
                .json(&initialize)
        };

        let anonymous = post().send().await.unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
        let wrong = post().bearer_auth("d".repeat(64)).send().await.unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let browser = post()
            .bearer_auth(&token)
            .header(header::ORIGIN, "http://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(browser.status(), StatusCode::FORBIDDEN);

        let ok = post()
            .bearer_auth(&token)
            .header(header::HOST, "office-mac.local:8788")
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body = ok.text().await.unwrap();
        assert!(body.contains("kumo-mcp"), "{body}");
        shutdown.cancel();
    }
}
