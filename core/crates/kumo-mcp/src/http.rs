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

use crate::kumo::ACCESS_OFF;
use crate::server::{KumoServer, RuleParams, rules_from};

pub const DEFAULT_LISTEN: &str = "0.0.0.0:8788";
pub const MCP_PATH: &str = "/mcp";
const TOKEN_FILE: &str = "mcp-token";
const MIN_TOKEN_LEN: usize = 32;
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(60);
const OFFICE_PIN: std::time::Duration = std::time::Duration::from_secs(2);

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
#[serde(deny_unknown_fields)]
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
    if !server.access_allowed() {
        return (StatusCode::FORBIDDEN, format!("{ACCESS_OFF}\n")).into_response();
    }
    let (given, wants_office) = match query.rules().and_then(|given| {
        let rules = server.event_rules(given.as_ref())?;
        Ok((given, rules.wants_office()))
    }) {
        Ok(rules) => rules,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}\n")).into_response(),
    };
    // A private session keeps HTTP readers from advancing any MCP session's cursors.
    let server = server.for_session();
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(32);
    if wants_office {
        let pinned = tokio::time::timeout(OFFICE_PIN, server.start_office_cursor()).await;
        let failure = match pinned {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("{e:#}")),
            Err(_) => Some("Better Kumo did not answer in time".to_string()),
        };
        if let Some(why) = failure {
            let text = format!(
                "Office events are counted from the first successful check, not from now: {why}"
            );
            let _ = tx.try_send(json!({ "kind": "note", "text": text }));
        }
    }
    let revoked = CancellationToken::new();
    let stopping = shutdown.clone();
    let revoking = revoked.clone();
    tokio::spawn(async move {
        let run = async {
            let Err(e) = server.stream_events(given, &tx).await else {
                return;
            };
            if server.access_allowed() {
                let _ = tx
                    .send(json!({ "kind": "error", "text": format!("{e:#}") }))
                    .await;
            } else {
                revoking.cancel();
            }
        };
        tokio::select! {
            _ = stopping.cancelled() => {}
            _ = tx.closed() => {}
            // Checked apart from the producer, which can sit in a full channel indefinitely.
            _ = server.access_revoked() => revoking.cancel(),
            _ = run => {}
        }
    });
    let mut heartbeat =
        tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let stream = futures_util::stream::unfold(
        (rx, shutdown, revoked, heartbeat, false),
        |(mut rx, shutdown, revoked, mut heartbeat, ended)| async move {
            if ended {
                return None;
            }
            let (value, ended) = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return None,
                // Queued events were collected under access that has since been withdrawn.
                _ = revoked.cancelled() => (json!({ "kind": "error", "text": ACCESS_OFF }), true),
                value = rx.recv() => {
                    heartbeat.reset();
                    (value?, false)
                }
                _ = heartbeat.tick() => (json!({ "kind": "heartbeat" }), false),
            };
            let line = format!("{value}\n");
            Some((
                Ok::<_, std::convert::Infallible>(line),
                (rx, shutdown, revoked, heartbeat, ended),
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

    struct Fixture {
        dir: tempfile::TempDir,
        connection: Arc<crate::kumo::Connection>,
        session: KumoServer,
        shutdown: CancellationToken,
        url: String,
        token: String,
        client: reqwest::Client,
        task: tokio::task::JoinHandle<std::io::Result<()>>,
    }

    impl Fixture {
        async fn start() -> Self {
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
            let server =
                KumoServer::new(connection.clone(), crate::server::ServerOptions::default());
            let session = server.for_session();
            let shutdown = CancellationToken::new();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/events", listener.local_addr().unwrap());
            let token = "f".repeat(64);
            let app = router(server, token.clone(), shutdown.clone());
            let task = tokio::spawn(async move { axum::serve(listener, app).await });
            Self {
                dir,
                connection,
                session,
                shutdown,
                url,
                token,
                client: reqwest::Client::new(),
                task,
            }
        }

        fn get(&self, query: &str) -> reqwest::RequestBuilder {
            self.client
                .get(format!("{}{query}", self.url))
                .bearer_auth(&self.token)
        }

        async fn open(&self, query: &str) -> reqwest::Response {
            let response = self.get(query).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            response
        }

        fn push(&self, id: i64, sender: &str) {
            self.connection.arrivals().push(incoming(id, sender));
        }

        fn save_rules(&self, json: &str) {
            std::fs::write(self.dir.path().join("watch.json"), json).unwrap();
        }

        fn turn_access_off(&self) {
            std::fs::write(self.dir.path().join("agents.json"), r#"{"enabled":false}"#).unwrap();
        }

        async fn until_connection_count(&self, count: usize) {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while Arc::strong_count(&self.connection) > count {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("stream task was not released");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    const AMY: &str = "?people=amy&kinds=message&label=Team";

    async fn ended(response: &mut reqwest::Response) -> bool {
        tokio::time::timeout(std::time::Duration::from_secs(5), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    }

    #[tokio::test]
    async fn rejects_bad_requests_before_streaming() {
        let f = Fixture::start().await;
        let status = |r: reqwest::RequestBuilder| async { r.send().await.unwrap().status() };
        let bare = f.client.get(format!("{}{AMY}", f.url));
        assert_eq!(status(bare).await, StatusCode::UNAUTHORIZED);
        let wrong = f.client.get(format!("{}{AMY}", f.url)).bearer_auth("wrong");
        assert_eq!(status(wrong).await, StatusCode::UNAUTHORIZED);
        let browser = f.get(AMY).header(header::ORIGIN, "http://evil.example");
        assert_eq!(status(browser).await, StatusCode::FORBIDDEN);
        assert_eq!(
            status(f.get("?kinds=invalid")).await,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(status(f.get("?peple=amy")).await, StatusCode::BAD_REQUEST);
        let no_rules = f.get("").send().await.unwrap();
        assert_eq!(no_rules.status(), StatusCode::BAD_REQUEST);
        assert!(
            no_rules
                .text()
                .await
                .unwrap()
                .contains(crate::server::NO_RULES)
        );
        f.turn_access_off();
        let off = f.get(AMY).send().await.unwrap();
        assert_eq!(off.status(), StatusCode::FORBIDDEN);
        assert!(off.text().await.unwrap().contains(crate::kumo::ACCESS_OFF));
    }

    #[tokio::test]
    async fn streams_start_now_and_leave_session_cursors_alone() {
        let f = Fixture::start().await;
        f.push(1, "amy");
        let mut first = f.open(AMY).await;
        assert_eq!(
            first.headers()[header::CONTENT_TYPE],
            "application/x-ndjson"
        );
        let mut second = f.open(AMY).await;
        f.push(2, "bob");
        f.push(3, "amy");
        for response in [&mut first, &mut second] {
            let event = event_line(response).await;
            assert_eq!(event["id"], 3);
            assert_eq!(event["kind"], "message");
            assert_eq!(event["reply_to"], 9);
            assert_eq!(event["text"], "hello\nworld");
            assert_eq!(event["rules"], json!(["Team"]));
        }
        let rules = rules_from(&[RuleParams {
            kinds: Some(vec!["message".into()]),
            people: None,
            places: None,
            label: None,
        }])
        .unwrap();
        let kumo = f.session.kumo().await.unwrap();
        let events = f
            .session
            .collect_matching_events(&kumo, &rules)
            .await
            .unwrap()
            .events;
        assert_eq!(
            events
                .iter()
                .map(|e| e["id"].as_i64().unwrap())
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[tokio::test]
    async fn saved_rules_are_reloaded_and_explicit_rules_ignore_them() {
        let f = Fixture::start().await;
        f.save_rules(r#"{"rules":[{"kinds":["message"]}]}"#);
        let mut saved = f.open("").await;
        let mut explicit = f.open(AMY).await;
        f.push(4, "amy");
        assert_eq!(event_line(&mut saved).await["id"], 4);
        assert_eq!(event_line(&mut explicit).await["id"], 4);
        f.save_rules(r#"{"rules":[]}"#);
        f.push(5, "bob");
        f.push(6, "amy");
        let error = event_line(&mut saved).await;
        assert_eq!(error["kind"], "error");
        assert!(
            error["text"]
                .as_str()
                .unwrap()
                .contains(crate::server::NO_RULES)
        );
        assert!(ended(&mut saved).await);
        assert_eq!(event_line(&mut explicit).await["id"], 6);
    }

    #[tokio::test]
    async fn a_dropped_client_releases_its_task_and_others_keep_streaming() {
        let f = Fixture::start().await;
        let mut kept = f.open(AMY).await;
        f.push(6, "amy");
        assert_eq!(event_line(&mut kept).await["id"], 6);
        let one_stream = Arc::strong_count(&f.connection);
        let dropped = f.open(AMY).await;
        assert!(Arc::strong_count(&f.connection) > one_stream);
        drop(dropped);
        f.until_connection_count(one_stream).await;
        f.push(7, "amy");
        assert_eq!(event_line(&mut kept).await["id"], 7);
    }

    #[tokio::test]
    async fn turning_access_off_or_shutting_down_ends_streams() {
        let f = Fixture::start().await;
        let mut revoked = f.open(AMY).await;
        f.turn_access_off();
        let error = event_line(&mut revoked).await;
        assert_eq!(error["kind"], "error");
        assert_eq!(error["text"], crate::kumo::ACCESS_OFF);
        assert!(ended(&mut revoked).await);

        let f = Fixture::start().await;
        let mut stopped = f.open(AMY).await;
        f.shutdown.cancel();
        assert!(ended(&mut stopped).await);
    }

    fn any_message() -> WatchRules {
        rules_from(&[RuleParams {
            kinds: Some(vec!["message".into()]),
            people: None,
            places: None,
            label: None,
        }])
        .unwrap()
    }

    #[tokio::test]
    async fn a_lagging_stream_hears_about_the_gap_before_the_events_after_it() {
        let f = Fixture::start().await;
        let session = f.session.for_session();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let producer =
            tokio::spawn(async move { session.stream_events(Some(any_message()), &tx).await });
        for id in 1..=3 {
            f.push(id, "amy");
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        for id in 4..=603 {
            f.push(id, "amy");
        }
        let mut lines = Vec::new();
        while lines.last().is_none_or(|v: &Value| v["id"] != 603) {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            lines.push(line);
        }
        producer.abort();
        let shown = |v: &Value| match v["kind"].as_str() {
            Some("note") => v["text"].as_str().unwrap().to_string(),
            _ => v["id"].to_string(),
        };
        let expected: Vec<String> = ["1", "2", "3"]
            .into_iter()
            .map(String::from)
            .chain(["100 earlier messages were no longer available to check.".to_string()])
            .chain((104..=603).map(|id| id.to_string()))
            .collect();
        assert_eq!(lines.iter().map(shown).collect::<Vec<_>>(), expected);
    }

    #[tokio::test]
    async fn revoking_access_releases_a_stream_whose_reader_stopped_reading() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let f = Fixture::start().await;
        let address = f
            .url
            .trim_start_matches("http://")
            .trim_end_matches("/events");
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!(
            "GET /events{AMY} HTTP/1.1\r\nHost: kumo\r\nAuthorization: Bearer {}\r\n\r\n",
            f.token
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
        }
        assert!(head.starts_with(b"HTTP/1.1 200"));
        let idle = Arc::strong_count(&f.connection);
        let text = "x".repeat(16 * 1024);
        for batch in 0..8 {
            for i in 0..400 {
                let mut message = incoming(batch * 400 + i, "amy");
                message.text = text.clone();
                f.connection.arrivals().push(message);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(Arc::strong_count(&f.connection), idle);
        f.turn_access_off();
        f.until_connection_count(idle - 1).await;
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
