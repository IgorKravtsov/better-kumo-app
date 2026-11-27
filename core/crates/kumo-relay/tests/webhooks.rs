use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kumo_core::chat::IncomingMessage;
use kumo_core::chat::model::ChannelCategory;
use kumo_core::desktop::DesktopState;
use kumo_core::events::{EventKind, OfficeEvent, Who};
use kumo_relay::desktop::replay::Replay;
use kumo_relay::desktop::{AlertSink, Desktop};
use kumo_relay::webhooks::{WebhookTiming, sign};
use kumo_relay::{Config, Relay, RelayParts};
use serde_json::{Value, json};

const SECRET: &str = "test-secret-0123456789abcdef";
const HOOK_SECRET: &str = "5f2b9c0e7a1d4386b2c9e0f1a7d3b5c86e4f2a1b9c0d7e3f5a2b8c1d6e9f0a3b";

struct Quiet;

impl AlertSink for Quiet {
    fn alert(&self, _alert: kumo_core::chat::alert::Alert) {}
}

#[derive(Clone, Default)]
struct Inbox {
    received: Arc<Mutex<Vec<(String, String)>>>,
    refused: Arc<AtomicUsize>,
}

impl Inbox {
    fn bodies(&self) -> Vec<Value> {
        self.received
            .lock()
            .unwrap()
            .iter()
            .map(|(_, body)| serde_json::from_str(body).unwrap())
            .collect()
    }

    async fn serve(&self) -> String {
        let app = axum::Router::new()
            .route(
                "/ok",
                axum::routing::post({
                    let received = self.received.clone();
                    move |headers: axum::http::HeaderMap, body: String| async move {
                        let signature = headers["x-kumo-signature"].to_str().unwrap().to_string();
                        received.lock().unwrap().push((signature, body));
                        "ok"
                    }
                }),
            )
            .route(
                "/slow",
                axum::routing::post({
                    let refused = self.refused.clone();
                    move || async move {
                        refused.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        "ok"
                    }
                }),
            )
            .route(
                "/down",
                axum::routing::post({
                    let refused = self.refused.clone();
                    move || async move {
                        refused.fetch_add(1, Ordering::SeqCst);
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        base
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    base: String,
    http: reqwest::Client,
    replay: Arc<Replay>,
    relay: Arc<Relay>,
}

fn office(earshot: &[&str]) -> Value {
    json!({
        "floorId": "floor-1",
        "width": 2000,
        "height": 1200,
        "floors": [{ "id": "floor-1", "name": "Ground" }],
        "self": { "id": "c-me", "x": 410.5, "y": 220.0, "zoneId": "zone-a", "availability": "available", "message": "" },
        "zones": [{ "id": "zone-a", "name": "Blue Room", "x": 300, "y": 100, "w": 400, "h": 300, "open": true }],
        "people": [],
        "audio": true,
        "video": false,
        "earshot": earshot,
        "inCall": !earshot.is_empty(),
        "call": if earshot.is_empty() { Value::Null } else { json!({ "roomName": "Blue Room", "peers": earshot }) }
    })
}

impl Harness {
    async fn start(dir: tempfile::TempDir) -> Self {
        Self::start_with(dir, office(&["c-amy"])).await
    }

    async fn start_with(dir: tempfile::TempDir, state: Value) -> Self {
        let replay = Arc::new(Replay::new());
        replay.answer("window.__kumo.state()", state);
        let desktop = Desktop::with_dev_tools(replay.clone());
        let relay = Relay::new(RelayParts {
            uid: "uid-me".into(),
            data_dir: dir.path().to_path_buf(),
            apns: None,
            ntfy: None,
            config_path: None,
            agents_path: dir.path().join("agents.json"),
            watch_path: dir.path().join("watch.json"),
            desktop: Some(desktop.clone()),
            meetings: None,
        });
        desktop.spawn(Arc::new(Quiet));
        relay.spawn_webhooks(WebhookTiming {
            debounce: Duration::from_millis(250),
            poll: Duration::from_secs(60),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut config = Config::template(SECRET, "test-office");
        config.data_dir = Some(dir.path().to_path_buf());
        tokio::spawn(kumo_relay::serve(
            listener,
            relay.clone(),
            config,
            std::future::pending(),
        ));
        let harness = Self {
            _dir: dir,
            base,
            http: reqwest::Client::new(),
            replay,
            relay,
        };
        for _ in 0..200 {
            if desktop.connected() {
                return harness;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("desktop never connected");
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(SECRET);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    fn burst(&self, frames: usize) {
        for _ in 0..frames {
            self.replay.frame(r#"{"op":"noop"}"#);
        }
    }
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..300 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} never happened");
}

#[tokio::test]
async fn webhooks_get_signed_numbered_debounced_deliveries_and_fall_back_through_urls() {
    let harness = Harness::start(tempfile::tempdir().unwrap()).await;
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    let (status, registered) = harness
        .call(
            reqwest::Method::PUT,
            "/v1/webhooks/entry-1",
            Some(json!({ "urls": [format!("{receiver}/down"), format!("{receiver}/ok")], "secret": HOOK_SECRET })),
        )
        .await;
    assert_eq!(status, 200, "{registered}");
    assert_eq!(registered["id"], "entry-1");

    until("the first Desktop state", || inbox.bodies().len() == 1).await;
    let (signature, raw) = inbox.received.lock().unwrap()[0].clone();
    assert_eq!(signature, sign(HOOK_SECRET, raw.as_bytes()));
    let first = &inbox.bodies()[0];
    let (_, events) = harness.call(reqwest::Method::GET, "/v1/events", None).await;
    assert_eq!(first["boot"], events["boot"]);
    assert_eq!(first["seq"], 1);
    assert_eq!(first["kind"], "desktop");
    assert_eq!(first["desktop"]["earshot"], json!(["c-amy"]));
    assert_eq!(first["desktop"]["inCall"], true);
    assert_eq!(
        first["desktop"]["call"],
        json!({ "roomName": "Blue Room", "peers": ["c-amy"] })
    );
    assert_eq!(inbox.refused.load(Ordering::SeqCst), 1);

    harness.burst(5);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        inbox.bodies().len(),
        1,
        "an unchanged state is not sent again"
    );

    harness
        .replay
        .answer("window.__kumo.state()", office(&["c-amy", "c-bo"]));
    harness.burst(5);
    until("the changed Desktop state", || inbox.bodies().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let bodies = inbox.bodies();
    assert_eq!(bodies.len(), 2, "a burst of changes is sent once");
    assert_eq!(bodies[1]["seq"], 2);
    assert_eq!(bodies[1]["desktop"]["earshot"], json!(["c-amy", "c-bo"]));
    assert_eq!(
        inbox.refused.load(Ordering::SeqCst),
        1,
        "the URL that worked is tried first"
    );

    harness.relay.record_event(OfficeEvent::new(
        EventKind::Nudge,
        0,
        Who {
            user_id: "uid-amy".into(),
            name: "Amy".into(),
        },
    ));
    until("the office event", || inbox.bodies().len() == 3).await;
    let event = &inbox.bodies()[2];
    assert_eq!(event["kind"], "event");
    assert_eq!(event["seq"], 3);
    assert_eq!(event["event"]["seq"], 1);
    assert_eq!(event["event"]["person"]["name"], "Amy");
    assert!(event.get("desktop").is_none());

    harness.replay.answer("window.__kumo.state()", Value::Null);
    harness.burst(1);
    until("the disconnect", || inbox.bodies().len() == 4).await;
    let gone = &inbox.bodies()[3];
    assert_eq!(gone["kind"], "disconnected");
    assert_eq!(gone["seq"], 4);
    assert!(gone.get("desktop").is_none() && gone.get("event").is_none());

    for (signature, raw) in inbox.received.lock().unwrap().iter() {
        assert_eq!(*signature, sign(HOOK_SECRET, raw.as_bytes()));
        assert!(!raw.contains("token") && !raw.contains("daily.co"));
    }
}

#[tokio::test]
async fn webhooks_are_listed_without_secrets_kept_across_restarts_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let harness = Harness::start(dir).await;
    let unauthorized = harness
        .http
        .get(format!("{}/v1/webhooks", harness.base))
        .send()
        .await
        .unwrap();
    assert_eq!(unauthorized.status().as_u16(), 401);

    let urls = json!(["http://127.0.0.1:9/a", "http://127.0.0.1:9/b"]);
    let (status, _) = harness
        .call(
            reqwest::Method::PUT,
            "/v1/webhooks/entry-1",
            Some(json!({ "urls": urls, "secret": HOOK_SECRET })),
        )
        .await;
    assert_eq!(status, 200);
    let (status, rejected) = harness
        .call(
            reqwest::Method::PUT,
            "/v1/webhooks/entry-2",
            Some(json!({ "urls": ["file:///etc/passwd"], "secret": HOOK_SECRET })),
        )
        .await;
    assert_eq!(status, 400, "{rejected}");

    let (status, listed) = harness
        .call(reqwest::Method::GET, "/v1/webhooks", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(listed, json!([{ "id": "entry-1", "urls": urls }]));
    assert!(!listed.to_string().contains(HOOK_SECRET));

    let saved = kumo_relay::webhooks::Webhooks::load(path.join("webhooks.json"), "next-boot");
    assert_eq!(serde_json::to_value(saved.list()).unwrap(), listed);

    let (status, _) = harness
        .call(reqwest::Method::DELETE, "/v1/webhooks/entry-1", None)
        .await;
    assert_eq!(status, 204);
    let (_, listed) = harness
        .call(reqwest::Method::GET, "/v1/webhooks", None)
        .await;
    assert_eq!(listed, json!([]));
    let saved = kumo_relay::webhooks::Webhooks::load(path.join("webhooks.json"), "next-boot");
    assert!(saved.is_empty());
}

async fn register(harness: &Harness, id: &str, url: String) {
    let (status, body) = harness
        .call(
            reqwest::Method::PUT,
            &format!("/v1/webhooks/{id}"),
            Some(json!({ "urls": [url], "secret": HOOK_SECRET })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn a_new_registration_does_not_swallow_a_disconnect_others_must_hear() {
    let harness = Harness::start(tempfile::tempdir().unwrap()).await;
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    register(&harness, "first", format!("{receiver}/ok")).await;
    until("the first Desktop state", || inbox.bodies().len() == 1).await;

    harness.replay.answer("window.__kumo.state()", Value::Null);
    harness.burst(1);
    register(&harness, "second", "http://127.0.0.1:9/nobody".into()).await;
    until("the disconnect", || inbox.bodies().len() == 2).await;
    assert_eq!(inbox.bodies()[1]["kind"], "disconnected");
}

#[tokio::test]
async fn chat_messages_reach_the_event_log_and_webhooks() {
    let harness = Harness::start(tempfile::tempdir().unwrap()).await;
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    register(&harness, "chat", format!("{receiver}/ok")).await;
    until("the first Desktop state", || inbox.bodies().len() == 1).await;
    let (_, start) = harness.call(reqwest::Method::GET, "/v1/events", None).await;

    harness.relay.record_message(&IncomingMessage {
        channel_url: "sendbird_group_channel_9".into(),
        channel_title: "General".into(),
        category: ChannelCategory::Channel,
        message_id: 42,
        parent_id: Some(40),
        created_at: 1_700_000_000_000,
        sender_id: "uid-amy".into(),
        sender_name: "Amy".into(),
        text: "lunch?".into(),
        mentions_me: false,
        channel_is_open: false,
    });
    until("the message event", || inbox.bodies().len() == 2).await;
    let posted = &inbox.bodies()[1];
    assert_eq!(posted["kind"], "event");
    assert_eq!(
        posted["event"],
        json!({
            "seq": 1,
            "kind": "message",
            "at": 1_700_000_000_000_i64,
            "person": { "user_id": "uid-amy", "name": "Amy" },
            "chat": "General",
            "text": "lunch?",
            "channel": "sendbird_group_channel_9",
            "channel_type": "channel",
            "thread": true,
        })
    );
    let (_, page) = harness
        .call(
            reqwest::Method::GET,
            &format!(
                "/v1/events?boot={}&after={}",
                start["boot"].as_str().unwrap(),
                start["latest"]
            ),
            None,
        )
        .await;
    assert_eq!(page["events"], json!([posted["event"]]));
}

#[tokio::test]
async fn a_removed_webhook_stops_posting_what_was_queued() {
    let harness = Harness::start(tempfile::tempdir().unwrap()).await;
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    register(&harness, "slow", format!("{receiver}/slow")).await;
    for _ in 0..5 {
        harness.relay.record_event(OfficeEvent::new(
            EventKind::Nudge,
            0,
            Who {
                user_id: "uid-amy".into(),
                name: "Amy".into(),
            },
        ));
    }
    until("the first slow post", || {
        inbox.refused.load(Ordering::SeqCst) >= 1
    })
    .await;
    let (status, _) = harness
        .call(reqwest::Method::DELETE, "/v1/webhooks/slow", None)
        .await;
    assert_eq!(status, 204);
    let at_removal = inbox.refused.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(inbox.refused.load(Ordering::SeqCst), at_removal);
}

#[tokio::test]
async fn webhooks_saved_before_a_start_without_kumospace_hear_it_is_gone() {
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    let dir = tempfile::tempdir().unwrap();
    kumo_relay::webhooks::Webhooks::load(dir.path().join("webhooks.json"), "earlier-boot")
        .register(
            "saved",
            kumo_core::desktop::WebhookRequest {
                urls: vec![format!("{receiver}/ok")],
                secret: HOOK_SECRET.into(),
            },
        )
        .unwrap();
    let _harness = Harness::start_with(dir, Value::Null).await;
    until("the startup disconnect", || inbox.bodies().len() == 1).await;
    assert_eq!(inbox.bodies()[0]["kind"], "disconnected");
    assert_eq!(inbox.bodies()[0]["seq"], 1);
}

fn amy(x: f64, y: f64, availability: &str) -> Value {
    json!({ "id": "c-amy", "uid": "uid-amy", "name": "Amy", "avatar": "", "x": x, "y": y, "zoneId": "zone-a",
            "self": false, "availability": availability, "idle": false, "message": "" })
}

#[tokio::test]
async fn people_walking_alone_send_nothing_but_a_change_while_walking_sends_the_whole_state() {
    let mut state = office(&["c-amy"]);
    state["people"] = json!([amy(500.0, 200.0, "available")]);
    let harness = Harness::start_with(tempfile::tempdir().unwrap(), state.clone()).await;
    let inbox = Inbox::default();
    let receiver = inbox.serve().await;
    register(&harness, "walk", format!("{receiver}/ok")).await;
    until("the first Desktop state", || inbox.bodies().len() == 1).await;

    let mut step = 0.0;
    let mut walk = |state: &mut Value| {
        step += 10.0;
        state["self"]["x"] = json!(410.5 + step);
        state["self"]["y"] = json!(220.0 - step);
        state["people"][0]["x"] = json!(500.0 + step);
        state["people"][0]["y"] = json!(200.0 + step);
    };
    for _ in 0..3 {
        walk(&mut state);
        harness
            .replay
            .answer("window.__kumo.state()", state.clone());
        harness.burst(1);
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    assert_eq!(inbox.bodies().len(), 1, "movement alone is not delivered");

    type Change = (&'static str, fn(&mut Value));
    let changes: [Change; 7] = [
        ("status", |s| s["self"]["availability"] = json!("away")),
        ("someone's status", |s| {
            s["people"][0] = amy(0.0, 0.0, "focusing")
        }),
        ("earshot", |s| s["earshot"] = json!([])),
        ("room", |s| s["self"]["zoneId"] = Value::Null),
        ("media", |s| s["video"] = json!(true)),
        ("call", |s| {
            s["inCall"] = json!(false);
            s["call"] = Value::Null;
        }),
        ("someone joining", |s| {
            s["people"].as_array_mut().unwrap().push(json!({
                "id": "c-bo", "uid": "uid-bo", "name": "Bo", "avatar": "", "x": 900.0, "y": 900.0, "zoneId": null,
                "self": false, "availability": "available", "idle": false, "message": "" }));
        }),
    ];
    for (n, (what, change)) in changes.into_iter().enumerate() {
        change(&mut state);
        walk(&mut state);
        harness
            .replay
            .answer("window.__kumo.state()", state.clone());
        harness.burst(1);
        until(what, || inbox.bodies().len() == n + 2).await;
        let sent: DesktopState =
            serde_json::from_value(inbox.bodies()[n + 1]["desktop"].clone()).unwrap();
        assert_eq!(
            sent,
            DesktopState::from_page(state.clone()).unwrap(),
            "{what} is delivered with the current positions"
        );
    }
}
