use std::sync::{Arc, Mutex};
use std::time::Duration;

use kumo_core::chat::alert::{Alert, AlertKind};
use kumo_core::desktop::{
    DesktopFloor, DesktopPerson, DesktopSelf, DesktopState, DesktopTarget, DesktopZone,
    KindsRequest, MediaRequest, MyStatus, NearbyMessage, NotificationKinds, NotificationProfile,
    NotificationsRequest, RelayClient, RelayError, RelayPairing, StatusRequest,
};
use kumo_core::ws::{Availability, ClientOp, Command, Position, SessionChatData, UserStatusChange};
use kumo_relay::desktop::replay::Replay;
use kumo_relay::desktop::{AlertSink, Desktop, DevTools, LeaseTiming};
use kumo_relay::{Config, Relay, RelayParts};
use serde_json::{Value, json};

const SECRET: &str = "test-secret-0123456789abcdef";
const OFFICE_WITH_CALL: &str = include_str!("fixtures/office_with_call.jsonl");
const REFRESHED_ROOM: &str = include_str!("fixtures/refreshed_room.jsonl");
const NUDGE_FROM_STRANGER: &str = include_str!("fixtures/nudge_from_stranger.jsonl");
const KNOCK_FROM_KNOWN_PERSON: &str = include_str!("fixtures/knock_from_known_person.jsonl");
const BROADCASTS_AND_SESSION_CHAT: &str =
    include_str!("fixtures/broadcasts_and_session_chat.jsonl");

#[derive(Default)]
struct Alerts(Mutex<Vec<Alert>>);

impl AlertSink for Alerts {
    fn alert(&self, alert: Alert) {
        self.0.lock().unwrap().push(alert);
    }
}

struct Tee(Arc<Alerts>, Arc<dyn AlertSink>);

impl AlertSink for Tee {
    fn alert(&self, alert: Alert) {
        self.0.alert(alert.clone());
        self.1.alert(alert);
    }
}

#[derive(Clone, Default)]
struct NtfyInbox(Arc<Mutex<Vec<Value>>>);

impl NtfyInbox {
    fn titles(&self) -> Vec<String> {
        let mut titles: Vec<String> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|m| m["title"].as_str().unwrap_or_default().to_string())
            .collect();
        titles.sort();
        titles
    }

    fn by_title(&self, title: &str) -> Value {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|m| m["title"] == title)
            .cloned()
            .unwrap_or(Value::Null)
    }
}

struct Harness {
    dir: std::path::PathBuf,
    base: String,
    http: reqwest::Client,
    client: RelayClient,
    replay: Arc<Replay>,
    desktop: Arc<Desktop>,
    alerts: Arc<Alerts>,
    relay: Arc<Relay>,
}

impl Harness {
    async fn start() -> Self {
        Self::start_with(LeaseTiming::default()).await
    }

    async fn start_with(lease_timing: LeaseTiming) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kumo-relay-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let replay = Arc::new(Replay::new());
        replay.answer("window.__kumo.send(", json!(true));
        replay.answer("window.__kumo.state()", desktop_state());
        let desktop = Desktop::with_timing(replay.clone(), lease_timing, Duration::ZERO);
        let relay = Relay::new(RelayParts {
            uid: "uid-me".into(),
            data_dir: dir.clone(),
            apns: None,
            ntfy: None,
            config_path: Some(dir.join("relay.toml")),
            desktop: Some(desktop.clone()),
        });
        let alerts = Arc::new(Alerts::default());
        desktop.spawn(Arc::new(Tee(alerts.clone(), relay.alert_sink())));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut config = Config::template(SECRET, "test-office");
        config.data_dir = Some(dir.clone());
        config.save(&dir.join("relay.toml")).unwrap();
        let pairing = RelayPairing::from_link(&kumo_relay::pair_link(&config, &base)).unwrap();
        tokio::spawn(kumo_relay::serve(
            listener,
            relay.clone(),
            config,
            std::future::pending(),
        ));
        let harness = Self {
            dir,
            base,
            http: reqwest::Client::new(),
            client: RelayClient::new(reqwest::Client::new(), pairing),
            replay,
            desktop,
            alerts,
            relay,
        };
        harness.until(|h| h.desktop.connected()).await;
        harness
    }

    async fn until(&self, done: impl Fn(&Self) -> bool) {
        for _ in 0..200 {
            if done(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached");
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(SECRET)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    fn sent(&self) -> Vec<ClientOp> {
        self.replay
            .evaluated()
            .iter()
            .filter_map(|e| e.strip_prefix("window.__kumo.send(")?.strip_suffix(')'))
            .map(|arg| {
                let frame: String = serde_json::from_str(arg).unwrap();
                serde_json::from_str(&frame).unwrap()
            })
            .collect()
    }

    fn sent_nearby(&self) -> Vec<(ClientOp, Value)> {
        self.replay
            .evaluated()
            .iter()
            .filter_map(|e| {
                e.strip_prefix("window.__kumo.sendNearby(")?
                    .strip_suffix(')')
            })
            .map(|args| {
                let args: Vec<Value> = serde_json::from_str(&format!("[{args}]")).unwrap();
                let frame = serde_json::from_str(args[0].as_str().unwrap()).unwrap();
                (frame, args[1].clone())
            })
            .collect()
    }

    fn injections(&self) -> usize {
        self.replay
            .evaluated()
            .iter()
            .filter(|e| e.contains("window.__kumo = {"))
            .count()
    }

    async fn settle(&self) {
        let seen = self.alerts().len();
        self.replay.frame(
            r#"{"op":"playSound","data":{"notificationType":"nudge","senderName":"Settle"}}"#,
        );
        self.until(|h| h.alerts().len() > seen).await;
    }

    fn alerts(&self) -> Vec<Alert> {
        self.alerts.0.lock().unwrap().clone()
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self
            .http
            .put(format!("{}{path}", self.base))
            .bearer_auth(SECRET)
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn ntfy_inbox(&self) -> NtfyInbox {
        let inbox = NtfyInbox::default();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post({
                let inbox = inbox.clone();
                move |axum::Json(body): axum::Json<Value>| async move {
                    inbox.0.lock().unwrap().push(body);
                    "ok"
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });
        let (status, _) = self
            .put(
                "/v1/ntfy",
                json!({ "topic": "kumo-test", "server": server }),
            )
            .await;
        assert_eq!(status, 200);
        inbox
    }

    async fn replay_office_events(&self) {
        let seen = self.alerts().len();
        self.replay
            .answer(r#"window.__kumo.nameOf("c-sam")"#, json!("Sam Placeholder"));
        self.replay.recording(BROADCASTS_AND_SESSION_CHAT);
        self.until(|h| h.alerts().len() >= seen + 7).await;
    }

    async fn until_delivered(&self, inbox: &NtfyInbox, title: &str) {
        for _ in 0..200 {
            if inbox.titles().iter().any(|t| t == title) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{title} was not delivered");
    }
}

fn alert(kind: AlertKind, title: &str, body: &str) -> Alert {
    Alert {
        kind,
        title: title.into(),
        body: body.into(),
        channel_url: None,
    }
}

fn sorted(titles: &[&str]) -> Vec<String> {
    let mut titles: Vec<String> = titles.iter().map(|t| t.to_string()).collect();
    titles.sort();
    titles
}

fn desktop_state() -> Value {
    desktop_state_as("available", false)
}

fn desktop_state_as(availability: &str, idle: bool) -> Value {
    json!({
        "floorId": "floor-1",
        "width": 2000,
        "height": 1200,
        "floors": [{ "id": "floor-1", "name": "Ground" }, { "id": "floor-2", "name": "Roof" }],
        "self": { "id": "c-me", "x": 410.5, "y": 220.0, "zoneId": "zone-a", "availability": availability, "message": "" },
        "zones": [
            { "id": "zone-a", "name": "Blue Room", "x": 300, "y": 100, "w": 400, "h": 300, "open": true },
            { "id": "zone-b", "name": "Quiet", "x": 1200, "y": 600, "w": 300, "h": 200, "open": false }
        ],
        "people": [
            { "id": "c-me", "uid": "uid-me", "name": "Me", "avatar": "", "x": 410.5, "y": 220.0,
              "zoneId": "zone-a", "self": true, "availability": availability, "idle": idle, "message": "" },
            { "id": "c-amy", "uid": "uid-amy", "name": "Amy", "avatar": "https://img/amy.png", "x": 900, "y": 1000,
              "zoneId": null, "self": false, "availability": "", "idle": true, "message": "brb" }
        ],
        "audio": true,
        "video": false
    })
}

fn typed_desktop_state() -> DesktopState {
    DesktopState {
        floor_id: Some("floor-1".into()),
        width: 2000.0,
        height: 1200.0,
        floors: vec![
            DesktopFloor {
                id: "floor-1".into(),
                name: "Ground".into(),
            },
            DesktopFloor {
                id: "floor-2".into(),
                name: "Roof".into(),
            },
        ],
        me: Some(DesktopSelf {
            id: "c-me".into(),
            x: 410.5,
            y: 220.0,
            zone_id: Some("zone-a".into()),
            availability: Some(Availability::Available),
            message: String::new(),
        }),
        zones: vec![
            DesktopZone {
                id: "zone-a".into(),
                name: "Blue Room".into(),
                x: 300.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
                open: true,
            },
            DesktopZone {
                id: "zone-b".into(),
                name: "Quiet".into(),
                x: 1200.0,
                y: 600.0,
                w: 300.0,
                h: 200.0,
                open: false,
            },
        ],
        people: vec![
            DesktopPerson {
                id: "c-me".into(),
                uid: "uid-me".into(),
                name: "Me".into(),
                avatar: String::new(),
                x: 410.5,
                y: 220.0,
                zone_id: Some("zone-a".into()),
                is_self: true,
                availability: Some(Availability::Available),
                idle: false,
                message: String::new(),
            },
            DesktopPerson {
                id: "c-amy".into(),
                uid: "uid-amy".into(),
                name: "Amy".into(),
                avatar: "https://img/amy.png".into(),
                x: 900.0,
                y: 1000.0,
                zone_id: None,
                is_self: false,
                availability: None,
                idle: true,
                message: "brb".into(),
            },
        ],
        audio: true,
        video: false,
    }
}

fn token_expiring_in(seconds: i64) -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        + seconds;
    format!(
        "eyJhbGciOiJIUzI1NiJ9.{}.page-signature",
        base64_url(format!(r#"{{"exp":{exp}}}"#).as_bytes())
    )
}

fn base64_url(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..=chunk.len() {
            out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

fn page_call(token: &str) -> Value {
    json!({
        "url": "https://kumo-test.daily.co/zone-a",
        "token": token,
        "selfId": "c-me",
        "peers": ["c-amy"],
        "roomName": "Blue Room"
    })
}

fn last_frame_token(recording: &str) -> String {
    let line = recording.lines().rfind(|l| !l.trim().is_empty()).unwrap();
    let frame: Value = serde_json::from_str(line).unwrap();
    frame["data"]["dailyToken"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn call_uses_the_room_frame_token_when_the_page_token_is_near_expiry() {
    let h = Harness::start().await;
    h.replay
        .answer("window.__kumo.call()", page_call(&token_expiring_in(30)));
    h.replay.recording(OFFICE_WITH_CALL);
    h.settle().await;
    let fresh = last_frame_token(OFFICE_WITH_CALL);
    let call = h.client.call().await.unwrap();
    assert_eq!(call.token, fresh);
    assert_eq!(call.url, "https://kumo-test.daily.co/zone-a");
    assert_eq!(call.peers, vec!["c-amy".to_string()]);
    assert_eq!(call.room_name, "Blue Room");
    assert!(h.sent().is_empty());
}

#[tokio::test]
async fn call_asks_for_a_fresh_token_and_waits_for_the_room_frame() {
    let h = Harness::start().await;
    h.replay
        .answer("window.__kumo.call()", page_call(&token_expiring_in(20)));
    let refreshed = REFRESHED_ROOM.trim();
    h.replay.answer_with_frames(
        r#"window.__kumo.send("{\"op\":\"refreshDailyToken\"}")"#,
        json!(true),
        &[refreshed],
    );
    let call = h.client.call().await.unwrap();
    assert_eq!(call.token, last_frame_token(REFRESHED_ROOM));
    assert_eq!(h.sent(), vec![ClientOp::RefreshDailyToken]);
}

#[tokio::test]
async fn call_keeps_a_page_token_that_is_still_fresh() {
    let h = Harness::start().await;
    let token = token_expiring_in(3600);
    h.replay.answer("window.__kumo.call()", page_call(&token));
    assert_eq!(h.client.call().await.unwrap().token, token);
    assert!(h.sent().is_empty());
}

#[tokio::test]
async fn nudge_from_someone_the_mirror_has_not_seen_is_named_by_the_page() {
    let h = Harness::start().await;
    h.replay
        .answer(r#"window.__kumo.nameOf("c-sam")"#, json!("Sam Placeholder"));
    h.replay.recording(NUDGE_FROM_STRANGER);
    h.until(|h| !h.alerts().is_empty()).await;
    assert_eq!(
        h.alerts(),
        vec![Alert {
            kind: AlertKind::Nudge,
            title: "Sam Placeholder nudged you".into(),
            body: "Kumospace".into(),
            channel_url: None,
        }]
    );
}

#[tokio::test]
async fn knock_from_someone_in_the_mirror_needs_no_page_lookup() {
    let h = Harness::start().await;
    h.replay.recording(KNOCK_FROM_KNOWN_PERSON);
    h.until(|h| !h.alerts().is_empty()).await;
    assert_eq!(
        h.alerts(),
        vec![Alert {
            kind: AlertKind::Knock,
            title: "Bo Sample is knocking".into(),
            body: "Someone wants to enter your room".into(),
            channel_url: None,
        }]
    );
    assert!(
        !h.replay
            .evaluated()
            .iter()
            .any(|e| e.starts_with("window.__kumo.nameOf("))
    );
}

#[tokio::test]
async fn client_reads_the_page_state_as_typed_desktop_state() {
    let h = Harness::start().await;
    let state = h.client.desktop().await.unwrap();
    assert_eq!(state, typed_desktop_state());
    assert_eq!(state, DesktopState::from_page(desktop_state()).unwrap());
    assert_eq!(state.floor_name(), Some("Ground"));
    assert_eq!(
        state.self_zone().map(|z| z.name.as_str()),
        Some("Blue Room")
    );
}

#[tokio::test]
async fn desktop_state_survives_the_wire_unchanged() {
    let state = typed_desktop_state();
    let wire = serde_json::to_string(&state).unwrap();
    assert_eq!(serde_json::from_str::<DesktopState>(&wire).unwrap(), state);
    let h = Harness::start().await;
    let response = h
        .http
        .get(format!("{}/v1/desktop", h.base))
        .bearer_auth(SECRET)
        .send()
        .await
        .unwrap();
    let served: Value = response.json().await.unwrap();
    assert_eq!(served, serde_json::to_value(&state).unwrap());
}

#[tokio::test]
async fn status_change_sends_a_typed_frame_and_returns_desktop_state() {
    let h = Harness::start().await;
    let state = h
        .client
        .set_status(&StatusRequest {
            availability: Availability::Focusing,
            message: "heads down".into(),
        })
        .await
        .unwrap();
    assert_eq!(state, typed_desktop_state());
    assert_eq!(
        h.sent(),
        vec![ClientOp::SetUserStatus(UserStatusChange {
            availability: Availability::Focusing,
            message: "heads down".into(),
        })]
    );
}

#[tokio::test]
async fn navigation_follow_and_nudge_send_commands() {
    let h = Harness::start().await;
    let state = h.client.desktop().await.unwrap();
    let tap = state.tap_target(1300.0, 700.0).unwrap();
    assert_eq!(tap, DesktopTarget::Zone("zone-b".into()));
    h.client.go(tap).await.unwrap();
    h.client
        .go(state.person_target("c-amy").unwrap())
        .await
        .unwrap();
    h.client.switch_floor("floor-2").await.unwrap();
    h.client.follow(Some("c-amy".into())).await.unwrap();
    h.client.follow(None).await.unwrap();
    h.client.nudge("c-amy").await.unwrap();
    assert_eq!(
        h.sent(),
        vec![
            ClientOp::Command(Command::NavigateToZone {
                zone_id: "zone-b".into()
            }),
            ClientOp::Command(Command::Destination(Position::new(900.0, 1000.0).unwrap())),
            ClientOp::Join(kumo_core::ws::JoinData {
                room_name: Some("floor-2".into()),
                ..Default::default()
            }),
            ClientOp::Command(Command::FollowUser {
                user_id: "c-amy".into()
            }),
            ClientOp::Command(Command::stop_following()),
            ClientOp::Nudge {
                user_id: "c-amy".into()
            },
        ]
    );
}

fn nearby_page() -> Value {
    json!({
        "selfId": "c-me",
        "floorId": "floor-1",
        "zoneId": "zone-a",
        "zoneName": "Blue Room",
        "participantIds": ["uid-me", "uid-amy"],
        "messages": [
            { "id": "m1", "senderId": "c-amy", "senderName": "Amy", "message": "Coffee?", "zoneId": "zone-a", "at": 10 },
            { "id": "m2", "senderId": "c-bo", "senderName": "Bo", "message": "Elsewhere", "zoneId": "zone-b", "at": 11 }
        ]
    })
}

#[tokio::test]
async fn nearby_chat_reads_and_sends_in_the_desktop_zone() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.nearby()", nearby_page());
    let chat = h.client.nearby_chat().await.unwrap();
    assert_eq!(chat.zone_name, "Blue Room");
    assert_eq!(
        chat.messages,
        vec![NearbyMessage {
            id: "m1".into(),
            sender_id: "c-amy".into(),
            sender_name: "Amy".into(),
            text: "Coffee?".into(),
            mine: false,
            sent_at_ms: 10,
        }]
    );
    h.replay.answer("window.__kumo.sendNearby(", json!(true));
    h.client.send_nearby("  on my way ").await.unwrap();
    assert_eq!(
        h.sent_nearby(),
        vec![(
            ClientOp::Chat(SessionChatData {
                message: "on my way".into(),
                range: "nearby".into(),
                participant_ids: vec!["uid-amy".into(), "uid-me".into()],
                zone_id: Some("zone-a".into()),
            }),
            json!("floor-1")
        )]
    );
    assert!(h.sent().is_empty());
}

#[tokio::test]
async fn a_desktop_whose_nearby_group_keeps_changing_gets_no_nearby_message() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.nearby()", nearby_page());
    h.replay.answer("window.__kumo.sendNearby(", json!("moved"));
    assert!(matches!(
        h.client.send_nearby("hi").await,
        Err(RelayError::Unavailable(_))
    ));
    assert_eq!(h.sent_nearby().len(), 3);
}

#[tokio::test]
async fn a_blank_nearby_message_never_reaches_the_desktop() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.nearby()", nearby_page());
    assert_eq!(
        h.client.send_nearby("   ").await,
        Err(RelayError::Rejected("message is empty".into()))
    );
    assert!(h.sent_nearby().is_empty());
}

#[tokio::test]
async fn media_changes_reach_the_page() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.media(", json!(true));
    h.client
        .set_media(MediaRequest {
            audio: Some(false),
            video: None,
        })
        .await
        .unwrap();
    assert!(
        h.replay
            .evaluated()
            .iter()
            .any(|e| e == "window.__kumo.media(false, null)")
    );
}

fn reclaims(h: &Harness) -> usize {
    h.replay
        .evaluated()
        .iter()
        .filter(|e| *e == "window.__kumo.reclaim()")
        .count()
}

async fn handed_off_harness(lease_timing: LeaseTiming) -> Harness {
    let h = Harness::start_with(lease_timing).await;
    h.replay.answer("window.__kumo.handoff()", json!(true));
    h.replay
        .answer("window.__kumo.call()", page_call(&token_expiring_in(3600)));
    h.replay.answer("window.__kumo.reclaim()", json!(false));
    h
}

#[tokio::test]
async fn handoff_returns_a_lease_that_renews_and_reclaims() {
    let h = handed_off_harness(LeaseTiming::default()).await;
    let handoff = h.client.handoff().await.unwrap();
    assert_eq!(handoff.call.self_id, "c-me");
    assert!(!handoff.lease_id.is_empty());
    assert!(h.desktop.handed_off());
    h.client.renew(&handoff.lease_id).await.unwrap();
    h.client.reclaim(&handoff.lease_id).await.unwrap();
    assert!(!h.desktop.handed_off());
    assert_eq!(reclaims(&h), 1);
}

#[tokio::test]
async fn a_newer_handoff_makes_the_older_lease_stale() {
    let h = handed_off_harness(LeaseTiming::default()).await;
    let first = h.client.handoff().await.unwrap();
    let second = h.client.handoff().await.unwrap();
    assert_ne!(first.lease_id, second.lease_id);
    assert_eq!(
        h.client.renew(&first.lease_id).await,
        Err(RelayError::StaleLease)
    );
    let (status, _) = h
        .post("/v1/desktop/lease", json!({ "lease_id": first.lease_id }))
        .await;
    assert_eq!(status, 409);
    h.client.renew(&second.lease_id).await.unwrap();
}

#[tokio::test]
async fn a_stale_reclaim_does_not_take_the_call_back_from_the_newer_phone() {
    let h = handed_off_harness(LeaseTiming::default()).await;
    let first = h.client.handoff().await.unwrap();
    let second = h.client.handoff().await.unwrap();
    assert_eq!(
        h.client.reclaim(&first.lease_id).await,
        Err(RelayError::StaleLease)
    );
    assert!(h.desktop.handed_off());
    assert_eq!(reclaims(&h), 0);
    h.client.renew(&second.lease_id).await.unwrap();
}

#[tokio::test]
async fn a_handoff_without_a_desktop_call_is_reported_as_not_in_call() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.handoff()", json!(false));
    assert_eq!(h.client.handoff().await, Err(RelayError::NotInCall));
    assert!(!h.desktop.handed_off());
}

#[tokio::test]
async fn the_relay_reclaims_when_the_lease_lapses() {
    let timing = LeaseTiming {
        lapse: Duration::from_millis(500),
        check_every: Duration::from_millis(20),
    };
    let h = handed_off_harness(timing).await;
    let handoff = h.client.handoff().await.unwrap();
    for _ in 0..6 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        h.client.renew(&handoff.lease_id).await.unwrap();
    }
    assert!(h.desktop.handed_off());
    assert_eq!(reclaims(&h), 0);
    h.until(|h| !h.desktop.handed_off()).await;
    assert_eq!(reclaims(&h), 1);
    assert_eq!(
        h.client.renew(&handoff.lease_id).await,
        Err(RelayError::StaleLease)
    );
}

#[tokio::test]
async fn relay_status_mirroring_and_devices_through_the_client() {
    let h = Harness::start().await;
    let status = h.client.status().await.unwrap();
    assert_eq!(
        (status.uid.as_str(), status.mirroring, status.phones),
        ("uid-me", true, 0)
    );
    assert!(status.desktop_connected);
    assert!(!h.client.set_mirroring(false).await.unwrap().mirroring);
    let token = "ab".repeat(32);
    assert_eq!(
        h.client
            .register_device(&token, "Test phone")
            .await
            .unwrap()
            .phones,
        1
    );
    assert_eq!(h.client.send_test_alert().await.unwrap().phones, 1);
    h.client.remove_device(&token).await.unwrap();
    assert!(matches!(
        h.client.remove_device(&token).await,
        Err(RelayError::NotFound(_))
    ));
    assert!(matches!(
        h.client.register_device("short", "x").await,
        Err(RelayError::Rejected(_))
    ));
}

#[tokio::test]
async fn a_wrong_secret_is_reported_as_unauthorized() {
    let h = Harness::start().await;
    let stranger = RelayClient::new(
        reqwest::Client::new(),
        RelayPairing {
            url: h.base.clone(),
            secret: "not-the-secret".into(),
        },
    );
    assert_eq!(stranger.desktop().await, Err(RelayError::Unauthorized));
    assert_eq!(stranger.status().await, Err(RelayError::Unauthorized));
}

#[tokio::test]
async fn a_desktop_outside_a_space_is_reported_as_unavailable() {
    let h = Harness::start().await;
    h.replay.answer("window.__kumo.state()", Value::Null);
    assert_eq!(
        h.client.desktop().await,
        Err(RelayError::Unavailable(
            "Kumospace desktop is not in a space".into()
        ))
    );
}

#[tokio::test]
async fn invalid_commands_are_rejected_before_reaching_the_desktop() {
    let h = Harness::start().await;
    assert_eq!(
        h.post(
            "/v1/desktop/status",
            json!({ "availability": "busy", "message": "" })
        )
        .await,
        (
            400,
            json!("availability must be available, away or focusing")
        )
    );
    assert_eq!(
        h.post("/v1/desktop/move", json!({ "x": -4.0, "y": 10.0 }))
            .await,
        (400, json!("bad position"))
    );
    assert!(h.sent().is_empty());
}

#[tokio::test]
async fn page_script_is_injected_once_per_attach_and_again_after_a_reload() {
    let h = Harness::start().await;
    h.client.desktop().await.unwrap();
    h.client.enter_zone("zone-a").await.unwrap();
    assert_eq!(h.injections(), 1);
    h.replay.reload();
    h.until(|h| h.injections() == 2).await;
}

#[tokio::test]
async fn ntfy_topic_is_set_from_the_phone_and_saved() {
    let h = Harness::start().await;
    let status = h.client.status().await.unwrap();
    assert_eq!((status.ntfy, status.ntfy_topic), (false, None));
    let status = h
        .client
        .set_ntfy_topic(Some(" my-kumo-alerts ".into()))
        .await
        .unwrap();
    assert!(status.ntfy);
    assert_eq!(status.ntfy_topic.as_deref(), Some("my-kumo-alerts"));
    assert_eq!(status.ntfy_server.as_deref(), Some("https://ntfy.sh"));
    let saved = Config::load(&h.dir.join("relay.toml")).unwrap();
    assert_eq!(
        saved.ntfy.map(|n| (n.server, n.topic, n.enabled)),
        Some(("https://ntfy.sh".into(), "my-kumo-alerts".into(), true))
    );
    assert!(matches!(
        h.client.set_ntfy_topic(Some("bad topic!".into())).await,
        Err(RelayError::Rejected(_))
    ));
    assert_eq!(
        h.client.status().await.unwrap().ntfy_topic.as_deref(),
        Some("my-kumo-alerts")
    );
    let status = h.client.set_ntfy_topic(None).await.unwrap();
    assert_eq!((status.ntfy, status.ntfy_topic), (false, None));
    assert!(
        Config::load(&h.dir.join("relay.toml"))
            .unwrap()
            .ntfy
            .is_none()
    );
}

#[tokio::test]
async fn broadcasts_and_session_chat_become_alerts() {
    let h = Harness::start().await;
    h.replay_office_events().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.alerts(),
        vec![
            alert(
                AlertKind::Broadcast,
                "Amy started a broadcast",
                "Screen share"
            ),
            alert(
                AlertKind::PublicChat,
                "Bo Sample · Public Chat",
                "Standup in 5"
            ),
            alert(AlertKind::NearbyChat, "Amy · Nearby Chat", "Coffee?"),
            alert(
                AlertKind::NearbyChat,
                "Sam Placeholder · Nearby Chat",
                "Can you hear me?"
            ),
            alert(AlertKind::Message, "Bo Sample", "Quick question"),
            alert(AlertKind::FloorEntry, "Amy joined the Floor", "Kumospace"),
            alert(
                AlertKind::Knock,
                "Bo Sample is knocking",
                "Someone wants to enter your room"
            ),
        ]
    );
}

const AMY_BROADCASTS: &str = r#"{"op":"broadcastStarted","data":{"id":"bc-9","type":"camera","hostUserId":"c-amy","presenterIds":["c-amy"],"range":"space"}}"#;

#[tokio::test]
async fn a_broadcast_already_on_screen_raises_no_alert() {
    let h = Harness::start().await;
    h.replay
        .answer("window.__kumo.broadcastShown()", json!(true));
    h.replay.frame(AMY_BROADCASTS);
    h.settle().await;
    assert_eq!(
        h.alerts(),
        vec![alert(AlertKind::Nudge, "Settle nudged you", "Kumospace")]
    );
}

#[tokio::test]
async fn a_broadcast_on_screen_still_alerts_when_away() {
    let h = Harness::start().await;
    h.replay
        .answer("window.__kumo.state()", desktop_state_as("away", false));
    h.replay
        .answer("window.__kumo.broadcastShown()", json!(true));
    h.replay.frame(AMY_BROADCASTS);
    h.until(|h| !h.alerts().is_empty()).await;
    assert_eq!(h.alerts()[0].kind, AlertKind::Broadcast);
}

#[tokio::test]
async fn a_kind_turned_off_stops_only_that_kind() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    let status = h
        .client
        .set_notifications(&NotificationsRequest::All(KindsRequest {
            public_chat: Some(false),
            broadcasts: Some(false),
            ..KindsRequest::default()
        }))
        .await
        .unwrap();
    assert_eq!(
        status.notifications,
        NotificationProfile::uniform(NotificationKinds {
            public_chat: false,
            broadcasts: false,
            ..NotificationKinds::default()
        })
    );
    h.replay_office_events().await;
    h.settle().await;
    h.until_delivered(&inbox, "Settle nudged you").await;
    assert_eq!(
        inbox.titles(),
        sorted(&[
            "Amy · Nearby Chat",
            "Bo Sample",
            "Bo Sample is knocking",
            "Sam Placeholder · Nearby Chat",
            "Settle nudged you",
        ])
    );
    let nearby = inbox.by_title("Amy · Nearby Chat");
    assert_eq!(
        (&nearby["message"], &nearby["priority"], &nearby["tags"]),
        (
            &json!("Coffee?"),
            &json!(3),
            &json!(["busts_in_silhouette"])
        )
    );
}

#[tokio::test]
async fn floor_entries_reach_ntfy_only_when_turned_on_for_my_status() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    h.client
        .set_notifications(&NotificationsRequest::status(
            MyStatus::Away,
            KindsRequest {
                floor_entries: Some(true),
                ..KindsRequest::default()
            },
        ))
        .await
        .unwrap();
    h.replay_office_events().await;
    h.until_delivered(&inbox, "Bo Sample is knocking").await;
    assert!(!inbox.titles().iter().any(|t| t == "Amy joined the Floor"));

    h.client
        .set_notifications(&NotificationsRequest::status(
            MyStatus::Available,
            KindsRequest {
                floor_entries: Some(true),
                ..KindsRequest::default()
            },
        ))
        .await
        .unwrap();
    h.replay
        .frame(r#"{"op":"notification","data":{"type":"enter","message":"Amy joined the Floor"}}"#);
    h.until_delivered(&inbox, "Amy joined the Floor").await;
    let entry = inbox.by_title("Amy joined the Floor");
    assert_eq!(
        (&entry["message"], &entry["priority"], &entry["tags"]),
        (&json!("Kumospace"), &json!(3), &json!(["walking"]))
    );
}

#[tokio::test]
async fn broadcasts_reach_ntfy_with_a_high_priority() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    h.replay_office_events().await;
    h.until_delivered(&inbox, "Bo Sample is knocking").await;
    h.until_delivered(&inbox, "Amy started a broadcast").await;
    let broadcast = inbox.by_title("Amy started a broadcast");
    assert_eq!(
        (&broadcast["priority"], &broadcast["tags"]),
        (&json!(4), &json!(["movie_camera"]))
    );
    assert_eq!(
        inbox.by_title("Bo Sample · Public Chat")["tags"],
        json!(["loudspeaker"])
    );
    assert_eq!(inbox.titles().len(), 6);
}

#[tokio::test]
async fn the_main_switch_off_stops_every_kind_but_not_the_test_alert() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    assert!(!h.client.set_mirroring(false).await.unwrap().mirroring);
    h.replay_office_events().await;
    h.settle().await;
    h.client.send_test_alert().await.unwrap();
    h.until_delivered(&inbox, "Kumowatch").await;
    assert_eq!(inbox.titles(), sorted(&["Kumowatch"]));
}

#[tokio::test]
async fn notification_kinds_survive_a_relay_restart() {
    let h = Harness::start().await;
    h.client.set_mirroring(false).await.unwrap();
    h.client
        .set_notifications(&NotificationsRequest::status(
            MyStatus::Focusing,
            KindsRequest {
                nudges: Some(false),
                ..KindsRequest::default()
            },
        ))
        .await
        .unwrap();
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(h.dir.join("state.json")).unwrap()).unwrap();
    assert_eq!(saved["enabled"], json!(false));
    assert_eq!(saved["notifications"]["focusing"]["nudges"], json!(false));
    assert_eq!(saved["notifications"]["focusing"]["knocks"], json!(true));
    assert_eq!(saved["notifications"]["away"]["nudges"], json!(true));
    let restarted = Relay::new(RelayParts {
        uid: "uid-me".into(),
        data_dir: h.dir.clone(),
        apns: None,
        ntfy: None,
        config_path: None,
        desktop: None,
    });
    assert_eq!(restarted.notifications(), h.relay.notifications());
    assert!(!restarted.notifications().focusing.nudges);
    assert!(restarted.notifications().unknown.nudges);
    assert!(!restarted.status().mirroring);
    assert_eq!(restarted.my_status().await, MyStatus::Unknown);
}

fn old_state_dir(state: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kumo-relay-old-state-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("state.json"), state).unwrap();
    dir
}

fn relay_in(dir: std::path::PathBuf) -> Arc<Relay> {
    Relay::new(RelayParts {
        uid: "uid-me".into(),
        data_dir: dir,
        apns: None,
        ntfy: None,
        config_path: None,
        desktop: None,
    })
}

#[tokio::test]
async fn an_older_state_file_means_every_kind_is_on() {
    let relay = relay_in(old_state_dir(r#"{"enabled":false}"#));
    assert!(!relay.status().mirroring);
    assert_eq!(relay.notifications(), NotificationProfile::default());
}

#[tokio::test]
async fn flat_kinds_in_an_older_state_file_apply_to_every_status() {
    let relay = relay_in(old_state_dir(
        r#"{"enabled":true,"notifications":{"broadcasts":false,"knocks":false}}"#,
    ));
    assert_eq!(
        relay.notifications(),
        NotificationProfile::uniform(NotificationKinds {
            broadcasts: false,
            knocks: false,
            ..NotificationKinds::default()
        })
    );
}

fn kinds_json(nearby_chat: bool, messages: bool) -> Value {
    json!({
        "messages": messages,
        "mentions": true,
        "nudges": true,
        "knocks": true,
        "broadcasts": true,
        "public_chat": true,
        "nearby_chat": nearby_chat,
        "floor_entries": false
    })
}

#[tokio::test]
async fn an_old_flat_body_applies_to_every_status() {
    let h = Harness::start().await;
    let (status, body) = h
        .put("/v1/notifications", json!({ "nearby_chat": false }))
        .await;
    assert_eq!(status, 200);
    for key in ["available", "away", "focusing", "unknown"] {
        assert_eq!(body["notifications"][key], kinds_json(false, true));
    }
    assert_eq!(body["my_status"], json!("available"));
    let (_, body) = h
        .put(
            "/v1/notifications",
            json!({ "messages": false, "nearby_chat": true }),
        )
        .await;
    assert_eq!(body["notifications"]["away"], kinds_json(true, false));
    assert_eq!(body["notifications"]["unknown"], kinds_json(true, false));
    assert_eq!(body["enabled"], json!(true));
    let (status, _) = h
        .put("/v1/notifications", json!({ "broadcast": false }))
        .await;
    assert_eq!(status, 400);
    assert!(
        h.client
            .status()
            .await
            .unwrap()
            .notifications
            .unknown
            .broadcasts
    );
}

#[tokio::test]
async fn a_status_body_changes_only_that_status() {
    let h = Harness::start().await;
    let (status, body) = h
        .put(
            "/v1/notifications",
            json!({ "status": "focusing", "kinds": { "nearby_chat": false } }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["notifications"]["focusing"], kinds_json(false, true));
    assert_eq!(body["notifications"]["available"], kinds_json(true, true));
    let (status, body) = h
        .put(
            "/v1/notifications",
            json!({ "away": { "messages": false }, "unknown": { "messages": false } }),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["notifications"]["away"], kinds_json(true, false));
    assert_eq!(body["notifications"]["unknown"], kinds_json(true, false));
    assert_eq!(body["notifications"]["focusing"], kinds_json(false, true));
    for bad in [
        json!({ "status": "busy", "kinds": {} }),
        json!({ "status": "away", "kinds": { "broadcast": false } }),
    ] {
        let (status, _) = h.put("/v1/notifications", bad).await;
        assert_eq!(status, 400);
    }
    let profile = h.client.status().await.unwrap().notifications;
    assert_eq!(
        profile,
        NotificationProfile::default()
            .with_kinds(
                MyStatus::Focusing,
                NotificationKinds {
                    nearby_chat: false,
                    ..NotificationKinds::default()
                }
            )
            .with_kinds(
                MyStatus::Away,
                NotificationKinds {
                    messages: false,
                    ..NotificationKinds::default()
                }
            )
            .with_kinds(
                MyStatus::Unknown,
                NotificationKinds {
                    messages: false,
                    ..NotificationKinds::default()
                }
            )
    );
}

const ANOTHER_BROADCAST: &str = r#"{"op":"broadcastStarted","data":{"id":"bc-9","type":"camera","hostUserId":"c-amy","presenterIds":["c-amy"],"range":"space"}}"#;

#[tokio::test]
async fn kinds_follow_my_status_on_the_desktop() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    h.replay
        .answer("window.__kumo.state()", desktop_state_as("focusing", false));
    let status = h
        .client
        .set_notifications(&NotificationsRequest::status(
            MyStatus::Focusing,
            KindsRequest {
                broadcasts: Some(false),
                ..KindsRequest::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(status.my_status, MyStatus::Focusing);
    h.replay_office_events().await;
    h.settle().await;
    h.until_delivered(&inbox, "Settle nudged you").await;
    let titles = inbox.titles();
    assert!(titles.contains(&"Bo Sample is knocking".to_string()));
    assert!(!titles.contains(&"Amy started a broadcast".to_string()));

    h.replay
        .answer("window.__kumo.state()", desktop_state_as("away", false));
    assert_eq!(h.client.status().await.unwrap().my_status, MyStatus::Away);
    h.replay.frame(ANOTHER_BROADCAST);
    h.until_delivered(&inbox, "Amy started a broadcast").await;
    assert_eq!(
        inbox.by_title("Amy started a broadcast")["message"],
        json!("Camera")
    );
}

#[tokio::test]
async fn idle_on_the_desktop_means_unknown() {
    let h = Harness::start().await;
    h.replay
        .answer("window.__kumo.state()", desktop_state_as("focusing", true));
    assert_eq!(
        h.client.status().await.unwrap().my_status,
        MyStatus::Unknown
    );
    h.replay
        .answer("window.__kumo.state()", desktop_state_as("", false));
    assert_eq!(
        h.client.status().await.unwrap().my_status,
        MyStatus::Unknown
    );
}

#[tokio::test]
async fn a_disconnected_desktop_app_means_unknown() {
    let h = Harness::start().await;
    let inbox = h.ntfy_inbox().await;
    h.client
        .set_notifications(&NotificationsRequest::status(
            MyStatus::Unknown,
            KindsRequest {
                broadcasts: Some(false),
                ..KindsRequest::default()
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        h.client.status().await.unwrap().my_status,
        MyStatus::Available
    );
    h.replay.detach();
    h.until(|h| !h.desktop.connected()).await;
    let status = h.client.status().await.unwrap();
    assert_eq!(
        (status.desktop_connected, status.my_status),
        (false, MyStatus::Unknown)
    );
    let sink = h.relay.alert_sink();
    sink.alert(alert(
        AlertKind::Broadcast,
        "Amy started a broadcast",
        "Camera",
    ));
    sink.alert(alert(AlertKind::Knock, "Bo is knocking", "Let me in"));
    h.until_delivered(&inbox, "Bo is knocking").await;
    assert_eq!(inbox.titles(), sorted(&["Bo is knocking"]));
}
