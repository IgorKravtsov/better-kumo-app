use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc;

use super::socket::{SpaceConnection, SpaceEvent};
use crate::auth::Auth;
use crate::call::{CallZone, Credentials, Mate};
use crate::chat::notifications::{MyStatus, StatusSignals};
use crate::config::KumoConfig;
use crate::desktop::Presence;
use crate::ws::{
    BroadcastStarted, ClientOp, ConnectParams, JoinData, OfficeNotification, Platform, ServerOp,
    TeleportTarget,
};

const KNOCK_WINDOW: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub zone_id: String,
    pub floor_id: String,
    pub name: String,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub key: String,
    pub uid: String,
    pub name: String,
    pub avatar_url: String,
    pub availability: String,
    pub idle: bool,
    pub zone_id: Option<String>,
    pub floor_id: Option<String>,
    pub is_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallCredentials {
    pub floor_id: String,
    pub daily_url: String,
    pub daily_token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpaceState {
    pub connected: bool,
    pub space_uid: String,
    pub zones: HashMap<String, Zone>,
    pub people: HashMap<String, Person>,
    pub self_key: Option<String>,
    pub self_zone_id: Option<String>,
    pub self_floor_id: Option<String>,
    pub call: Option<CallCredentials>,
    pub recent_knocks: HashMap<String, Instant>,
}

impl SpaceState {
    pub fn me(&self) -> Option<&Person> {
        self.people.values().find(|p| p.is_self)
    }

    pub fn my_status(&self) -> MyStatus {
        let me = self.me();
        MyStatus::of(StatusSignals {
            known: self.connected && me.is_some(),
            availability: me.and_then(|p| p.availability.parse().ok()),
            idle: me.is_some_and(|p| p.idle),
        })
    }

    pub fn person_by_key(&self, key: &str) -> Option<&Person> {
        self.people.get(key)
    }

    pub fn zone_mates(&self) -> Vec<String> {
        let Some(zone) = &self.self_zone_id else {
            return Vec::new();
        };
        self.people
            .values()
            .filter(|p| !p.is_self && p.zone_id.as_ref() == Some(zone))
            .map(|p| p.key.clone())
            .collect()
    }

    pub fn person_by_uid(&self, uid: &str) -> Option<&Person> {
        self.people.values().find(|p| p.uid == uid)
    }

    pub fn call_credentials(&self) -> Option<Credentials> {
        self.call.as_ref().map(|c| Credentials {
            url: c.daily_url.clone(),
            token: c.daily_token.clone(),
        })
    }

    pub fn call_zone(&self) -> CallZone {
        let mate = |p: &Person| Mate {
            key: p.key.clone(),
            name: p.name.clone(),
            avatar: p.avatar_url.clone(),
            presence: Presence::of(p.availability.parse().ok(), false),
        };
        CallZone {
            room_name: self
                .self_zone_id
                .as_ref()
                .and_then(|z| self.zones.get(z))
                .map(|z| z.name.clone())
                .unwrap_or_default(),
            me: self.me().map(mate),
            mates: self
                .zone_mates()
                .iter()
                .filter_map(|key| self.people.get(key))
                .map(mate)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastKind {
    Camera,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChat {
    Public,
    Nearby,
    Private,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpaceUpdate {
    Changed,
    Call(Option<CallCredentials>),
    Sound {
        data: Value,
        sender_name: Option<String>,
    },
    Broadcast {
        kind: BroadcastKind,
        host_id: String,
        sender_name: Option<String>,
    },
    SessionMessage {
        chat: SessionChat,
        sender_id: String,
        sender_name: Option<String>,
        text: String,
    },
    FloorEntry {
        message: String,
    },
    Notice(String),
    Error {
        code: String,
        message: String,
    },
}

impl SpaceUpdate {
    pub fn unnamed_sender(&self) -> Vec<String> {
        match self {
            SpaceUpdate::Sound {
                data,
                sender_name: None,
            } => sender_ids(data).map(String::from).collect(),
            SpaceUpdate::Broadcast {
                host_id,
                sender_name: None,
                ..
            } => vec![host_id.clone()],
            SpaceUpdate::SessionMessage {
                sender_id,
                sender_name: None,
                ..
            } => vec![sender_id.clone()],
            _ => Vec::new(),
        }
    }

    pub fn named(mut self, name: Option<String>) -> Self {
        match &mut self {
            SpaceUpdate::Sound { sender_name, .. }
            | SpaceUpdate::Broadcast { sender_name, .. }
            | SpaceUpdate::SessionMessage { sender_name, .. }
                if sender_name.is_none() =>
            {
                *sender_name = name;
            }
            _ => {}
        }
        self
    }
}

pub const SENDER_KEYS: &[&str] = &[
    "senderId",
    "fromUserId",
    "sourceUserId",
    "userId",
    "firebaseUid",
    "senderFirebaseUid",
    "entityId",
];

pub trait SpaceListener: Send + Sync {
    fn on_space(&self, update: SpaceUpdate);
}

pub struct SpaceSessionParams {
    pub auth: Arc<Auth>,
    pub config: KumoConfig,
    pub space_name: String,
    pub platform: Platform,
    pub time_zone: String,
    pub listener: Arc<dyn SpaceListener>,
}

pub struct SpaceSession {
    state: Arc<Mutex<SpaceState>>,
    commands: mpsc::UnboundedSender<ClientOp>,
    task: tokio::task::JoinHandle<()>,
    space_name: String,
}

impl SpaceSession {
    pub fn start(params: SpaceSessionParams) -> Self {
        let state = Arc::new(Mutex::new(SpaceState::default()));
        let (tx, rx) = mpsc::unbounded_channel();
        let space_name = params.space_name.clone();
        let task = tokio::spawn(run(params, state.clone(), rx));
        Self {
            state,
            commands: tx,
            task,
            space_name,
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn snapshot(&self) -> SpaceState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn join_zone(&self, floor_id: &str, zone_id: &str) {
        let _ = self.commands.send(ClientOp::Join(JoinData {
            room_name: Some(floor_id.to_string()),
            teleport_target: Some(TeleportTarget::Zone {
                zone_id: zone_id.to_string(),
            }),
            destination: None,
            password: None,
        }));
    }

    pub fn join_zone_with_password(&self, floor_id: &str, zone_id: &str, password: &str) {
        let _ = self.commands.send(ClientOp::Join(JoinData {
            room_name: Some(floor_id.to_string()),
            teleport_target: Some(TeleportTarget::Zone {
                zone_id: zone_id.to_string(),
            }),
            destination: None,
            password: Some(password.to_string()),
        }));
    }

    pub fn join_person(&self, uid: &str) -> bool {
        let floor = self
            .snapshot()
            .person_by_uid(uid)
            .and_then(|p| p.floor_id.clone());
        let Some(floor) = floor else { return false };
        let _ = self.commands.send(ClientOp::Join(JoinData {
            room_name: Some(floor),
            teleport_target: Some(TeleportTarget::User {
                user_id: uid.to_string(),
            }),
            destination: None,
            password: None,
        }));
        true
    }

    pub fn leave(&self) {
        let _ = self.commands.send(ClientOp::leave());
    }

    pub fn refresh_daily_token(&self) {
        let _ = self.commands.send(ClientOp::RefreshDailyToken);
    }

    pub fn stop(self) {
        let _ = self.commands.send(ClientOp::leave());
        let task = self.task;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            task.abort();
        });
    }
}

async fn run(
    params: SpaceSessionParams,
    state: Arc<Mutex<SpaceState>>,
    mut commands: mpsc::UnboundedReceiver<ClientOp>,
) {
    let mut attempt: u32 = 0;
    loop {
        if let Ok(id_token) = params.auth.id_token().await {
            let connect = ConnectParams {
                ws_base_url: params.config.ws_base_url.clone(),
                space_name: params.space_name.clone(),
                id_token,
                time_zone: if params.time_zone.is_empty() {
                    "UTC".into()
                } else {
                    params.time_zone.clone()
                },
                platform: params.platform,
                reconnect_as: None,
            };
            if let Ok(mut conn) = SpaceConnection::connect(&connect).await {
                attempt = 0;
                update(&state, |s| {
                    *s = SpaceState {
                        connected: true,
                        ..SpaceState::default()
                    }
                });
                params.listener.on_space(SpaceUpdate::Changed);
                loop {
                    tokio::select! {
                        event = conn.next_event() => {
                            let Some(event) = event else { break };
                            match event {
                                SpaceEvent::Op(op) => apply(&state, op, &params.listener),
                                SpaceEvent::Malformed(_) => {}
                                SpaceEvent::Closed { banned, .. } => {
                                    if banned {
                                        params.listener.on_space(SpaceUpdate::Error {
                                            code: "BANNED".into(),
                                            message: "You cannot join this space".into(),
                                        });
                                        return;
                                    }
                                    break;
                                }
                            }
                        }
                        cmd = commands.recv() => {
                            let Some(cmd) = cmd else {
                                conn.close().await;
                                return;
                            };
                            if conn.send(&cmd).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        update(&state, |s| {
            s.connected = false;
            s.call = None;
        });
        params.listener.on_space(SpaceUpdate::Call(None));
        params.listener.on_space(SpaceUpdate::Changed);
        attempt += 1;
        tokio::time::sleep(Duration::from_secs((1u64 << attempt.min(5)).min(30))).await;
    }
}

fn update(state: &Mutex<SpaceState>, f: impl FnOnce(&mut SpaceState)) {
    if let Ok(mut s) = state.lock() {
        f(&mut s);
    }
}

fn apply(state: &Mutex<SpaceState>, op: ServerOp, listener: &Arc<dyn SpaceListener>) {
    let updates = state.lock().map(|mut s| s.apply(op)).unwrap_or_default();
    for update in updates {
        listener.on_space(update);
    }
}

pub fn sender_ids(data: &Value) -> impl Iterator<Item = &str> {
    SENDER_KEYS.iter().filter_map(|k| data[*k].as_str())
}

impl SpaceState {
    pub fn sender(&self, data: &Value) -> Option<&Person> {
        sender_ids(data).find_map(|id| self.person(id))
    }

    pub fn person(&self, id: &str) -> Option<&Person> {
        self.people.get(id).or_else(|| self.person_by_uid(id))
    }

    pub fn sender_name(&self, data: &Value) -> Option<String> {
        sender_ids(data).find_map(|id| self.name_of(id))
    }

    fn name_of(&self, id: &str) -> Option<String> {
        self.person(id)
            .map(|p| p.name.clone())
            .filter(|n| !n.trim().is_empty())
    }

    pub fn is_me(&self, id: &str) -> bool {
        if self.self_key.as_deref() == Some(id) {
            return true;
        }
        let Some(me) = self.me() else {
            return false;
        };
        me.key == id
            || (!me.uid.is_empty()
                && (me.uid == id || self.person(id).is_some_and(|p| p.uid == me.uid)))
    }

    fn names_me(&self, message: &str) -> bool {
        self.me()
            .map(|me| me.name.trim())
            .is_some_and(|name| !name.is_empty() && message.starts_with(&format!("{name} ")))
    }

    fn first_knock(&mut self, ids: &[&str], now: Instant) -> bool {
        let key = ids
            .iter()
            .find_map(|id| self.person(id).map(|p| p.key.clone()))
            .or_else(|| ids.first().map(|id| id.to_string()))
            .unwrap_or_default();
        self.recent_knocks
            .retain(|_, at| now.saturating_duration_since(*at) < KNOCK_WINDOW);
        if self.recent_knocks.contains_key(&key) {
            return false;
        }
        self.recent_knocks.insert(key, now);
        true
    }

    fn broadcast(&self, b: BroadcastStarted) -> Vec<SpaceUpdate> {
        if self.is_me(&b.host_user_id) || b.presenter_ids.iter().any(|id| self.is_me(id)) {
            return Vec::new();
        }
        vec![SpaceUpdate::Broadcast {
            kind: match b.kind.as_deref() {
                Some("screen") => BroadcastKind::Screen,
                _ => BroadcastKind::Camera,
            },
            sender_name: self.name_of(&b.host_user_id),
            host_id: b.host_user_id,
        }]
    }

    fn session_message(
        &self,
        chat: SessionChat,
        sender_id: String,
        sender_name: Option<String>,
        text: String,
    ) -> Vec<SpaceUpdate> {
        if self.is_me(&sender_id) {
            return Vec::new();
        }
        vec![SpaceUpdate::SessionMessage {
            chat,
            sender_name: self
                .name_of(&sender_id)
                .or(sender_name.filter(|n| !n.trim().is_empty())),
            sender_id,
            text,
        }]
    }

    fn notification(&mut self, data: Value, now: Instant) -> Vec<SpaceUpdate> {
        match serde_json::from_value::<OfficeNotification>(data.clone()) {
            Ok(OfficeNotification::Chat {
                range,
                sender_id,
                sender_name,
                message,
            }) => {
                let chat = match range.as_str() {
                    "space" => SessionChat::Public,
                    "nearby" | "room" => SessionChat::Nearby,
                    _ => return Vec::new(),
                };
                self.session_message(chat, sender_id, sender_name, message)
            }
            Ok(OfficeNotification::Private {
                sender_id,
                recipient_id,
                sender_name,
                message,
            }) => {
                if !recipient_id.is_some_and(|r| self.is_me(&r)) {
                    return Vec::new();
                }
                self.session_message(SessionChat::Private, sender_id, sender_name, message)
            }
            Ok(OfficeNotification::DoorKnock { sender_id, .. }) => {
                if sender_id.as_deref().is_some_and(|id| self.is_me(id)) {
                    return Vec::new();
                }
                let mut data = data;
                data["notificationType"] = Value::from("doorKnock");
                self.sound(data, now)
            }
            _ if data["type"].as_str() == Some("enter") => data["message"]
                .as_str()
                .map(str::trim)
                .filter(|m| !m.is_empty() && !self.names_me(m))
                .map(|m| SpaceUpdate::FloorEntry {
                    message: m.to_string(),
                })
                .into_iter()
                .collect(),
            _ if matches!(
                data["type"].as_str(),
                Some("chat" | "private" | "doorKnock")
            ) =>
            {
                Vec::new()
            }
            _ => data["message"]
                .as_str()
                .filter(|m| !m.contains("nudged you") && !m.contains("knocked on the door"))
                .map(|m| SpaceUpdate::Notice(m.to_string()))
                .into_iter()
                .collect(),
        }
    }

    fn sound(&mut self, data: Value, now: Instant) -> Vec<SpaceUpdate> {
        if data["notificationType"].as_str() == Some("doorKnock") {
            let ids: Vec<String> = sender_ids(&data).map(String::from).collect();
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            if !self.first_knock(&ids, now) {
                return Vec::new();
            }
        }
        let sender_name = self.sender_name(&data);
        vec![SpaceUpdate::Sound { data, sender_name }]
    }

    pub fn apply(&mut self, op: ServerOp) -> Vec<SpaceUpdate> {
        self.apply_at(op, Instant::now())
    }

    pub fn apply_at(&mut self, op: ServerOp, now: Instant) -> Vec<SpaceUpdate> {
        match op {
            ServerOp::Config(v) => {
                self.space_uid = v["uid"].as_str().unwrap_or_default().to_string();
                Vec::new()
            }
            ServerOp::Census(map) => {
                for (key, user) in map {
                    match user {
                        Some(u) => {
                            let existing = self.people.get(&key).cloned();
                            let census_self = u.is_self.unwrap_or(false);
                            let person = Person {
                                key: key.clone(),
                                uid: u
                                    .firebase_uid
                                    .or(existing.as_ref().map(|e| e.uid.clone()))
                                    .unwrap_or_default(),
                                name: u
                                    .name
                                    .or(existing.as_ref().map(|e| e.name.clone()))
                                    .unwrap_or_default(),
                                avatar_url: u
                                    .avatar_url
                                    .or(existing.as_ref().map(|e| e.avatar_url.clone()))
                                    .unwrap_or_default(),
                                idle: match &u.user_status {
                                    Some(st) => st.idle(),
                                    None => existing.as_ref().is_some_and(|e| e.idle),
                                },
                                availability: u
                                    .user_status
                                    .and_then(|st| st.availability)
                                    .or(existing.as_ref().map(|e| e.availability.clone()))
                                    .unwrap_or_default(),
                                zone_id: u.zone_id,
                                floor_id: u.room_name,
                                is_self: match &self.self_key {
                                    Some(k) => k == &key,
                                    None => {
                                        census_self || existing.as_ref().is_some_and(|e| e.is_self)
                                    }
                                },
                            };
                            if person.is_self {
                                self.self_zone_id = person.zone_id.clone();
                                self.self_floor_id = person.floor_id.clone();
                            }
                            self.people.insert(key, person);
                        }
                        None => {
                            self.people.remove(&key);
                        }
                    }
                }
                vec![SpaceUpdate::Changed]
            }
            ServerOp::Entities(list) => {
                let mut changed = false;
                for e in list.iter().filter(|e| e.is_self && !e.is_deleted) {
                    self.self_key = Some(e.id.clone());
                    self.self_zone_id = e.zone_id.clone();
                    self.self_floor_id = e.room_name.clone();
                    for p in self.people.values_mut() {
                        p.is_self = p.key == e.id;
                    }
                    if let Some(p) = self.people.get_mut(&e.id) {
                        p.zone_id = e.zone_id.clone();
                        p.floor_id = e.room_name.clone();
                    }
                    changed = true;
                }
                if changed {
                    vec![SpaceUpdate::Changed]
                } else {
                    Vec::new()
                }
            }
            ServerOp::SpaceZones(map) => {
                for (zone_id, zone) in map {
                    match zone {
                        Some(z) => {
                            self.zones.insert(
                                zone_id.clone(),
                                Zone {
                                    zone_id,
                                    floor_id: z["roomId"].as_str().unwrap_or_default().to_string(),
                                    name: z["displayName"].as_str().unwrap_or_default().to_string(),
                                    open: z["status"].as_str() != Some("closed"),
                                },
                            );
                        }
                        None => {
                            self.zones.remove(&zone_id);
                        }
                    }
                }
                vec![SpaceUpdate::Changed]
            }
            ServerOp::Room(room) => {
                let creds = match (room.room_name, room.daily_url, room.daily_token) {
                    (Some(floor_id), Some(daily_url), Some(daily_token)) => Some(CallCredentials {
                        floor_id,
                        daily_url,
                        daily_token,
                    }),
                    _ => None,
                };
                self.call = creds.clone();
                vec![SpaceUpdate::Call(creds), SpaceUpdate::Changed]
            }
            ServerOp::PlaySound(data) => self.sound(data, now),
            ServerOp::BroadcastStarted(b) => self.broadcast(b),
            ServerOp::Error(e) => vec![SpaceUpdate::Error {
                code: e.code.unwrap_or_default(),
                message: e.message.unwrap_or_default(),
            }],
            ServerOp::Notification(v) => self.notification(v, now),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sink(Mutex<Vec<SpaceUpdate>>);

    impl SpaceListener for Sink {
        fn on_space(&self, update: SpaceUpdate) {
            self.0.lock().unwrap().push(update);
        }
    }

    #[test]
    fn tracks_people_zones_and_call() {
        let state = Mutex::new(SpaceState::default());
        let sink: Arc<dyn SpaceListener> = Arc::new(Sink(Mutex::new(Vec::new())));
        let census = ServerOp::parse(
            r#"{"op":"census","data":{"a":{"firebaseUid":"u1","name":"Amy","zoneId":"z1","roomName":"f1"},"me":{"firebaseUid":"u0","name":"Me","zoneId":"z1","roomName":"f1","isSelf":true}}}"#,
        )
        .unwrap();
        apply(&state, census, &sink);
        let zones = ServerOp::parse(
            r#"{"op":"spaceZones","data":{"z1":{"zoneId":"z1","roomId":"f1","displayName":"Room","status":"open"}}}"#,
        )
        .unwrap();
        apply(&state, zones, &sink);
        let room = ServerOp::parse(
            r#"{"op":"room","data":{"roomName":"f1","dailyUrl":"https://x.daily.co/r","dailyToken":"t"}}"#,
        )
        .unwrap();
        apply(&state, room, &sink);
        let s = state.lock().unwrap();
        assert_eq!(s.zone_mates(), vec!["a".to_string()]);
        assert_eq!(s.zones["z1"].name, "Room");
        assert_eq!(s.call.as_ref().unwrap().daily_url, "https://x.daily.co/r");
    }

    #[test]
    fn my_status_comes_from_my_census_entry_while_connected() {
        let mut state = SpaceState {
            connected: true,
            ..SpaceState::default()
        };
        assert_eq!(state.my_status(), MyStatus::Unknown);
        let census = |status: &str| {
            ServerOp::parse(&format!(
                r#"{{"op":"census","data":{{"me":{{"firebaseUid":"u0","name":"Me","isSelf":true,"userStatus":{status}}},"a":{{"firebaseUid":"u1","name":"Amy","userStatus":{{"availability":"available"}}}}}}}}"#
            ))
            .unwrap()
        };
        state.apply(census(r#"{"availability":"focusing","setBy":"user"}"#));
        assert_eq!(state.my_status(), MyStatus::Focusing);
        state.apply(census(r#"{"availability":"away","setBy":"idle"}"#));
        assert_eq!(state.my_status(), MyStatus::Unknown);
        state.apply(
            ServerOp::parse(r#"{"op":"census","data":{"me":{"name":"Me","zoneId":"z1"}}}"#)
                .unwrap(),
        );
        assert_eq!(state.my_status(), MyStatus::Unknown);
        state.apply(census(r#"{"availability":"away"}"#));
        assert_eq!(state.my_status(), MyStatus::Away);
        state.connected = false;
        assert_eq!(state.my_status(), MyStatus::Unknown);
    }

    #[test]
    fn sound_names_the_sender_from_the_office() {
        let mut state = SpaceState::default();
        state.apply(
            ServerOp::parse(r#"{"op":"census","data":{"c1":{"firebaseUid":"u1","name":"Amy"}}}"#)
                .unwrap(),
        );
        let updates = state.apply(
            ServerOp::parse(
                r#"{"op":"playSound","data":{"notificationType":"nudge","senderFirebaseUid":"u1"}}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            updates,
            vec![SpaceUpdate::Sound {
                data: serde_json::json!({"notificationType":"nudge","senderFirebaseUid":"u1"}),
                sender_name: Some("Amy".into()),
            }]
        );
        assert!(
            state
                .apply(ServerOp::parse(r#"{"op":"config","data":{"uid":"s1"}}"#).unwrap())
                .is_empty()
        );
    }

    fn office() -> SpaceState {
        let mut state = SpaceState::default();
        state.apply(
            ServerOp::parse(
                r#"{"op":"census","data":{"c-amy":{"firebaseUid":"uid-amy","name":"Amy"},"c-me":{"firebaseUid":"uid-me","name":"Me"},"c-me-phone":{"firebaseUid":"uid-me","name":"Me"}}}"#,
            )
            .unwrap(),
        );
        state.apply(
            ServerOp::parse(
                r#"{"op":"entities","data":[{"id":"c-me","type":"user","self":true}]}"#,
            )
            .unwrap(),
        );
        state
    }

    fn frame(state: &mut SpaceState, frame: &str) -> Vec<SpaceUpdate> {
        state.apply(ServerOp::parse(frame).unwrap())
    }

    #[test]
    fn broadcasts_from_others_name_the_host() {
        let mut state = office();
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"broadcastStarted","data":{"id":"b1","type":"screen","hostUserId":"c-amy","presenterIds":["c-amy"],"range":"space"}}"#
            ),
            vec![SpaceUpdate::Broadcast {
                kind: BroadcastKind::Screen,
                host_id: "c-amy".into(),
                sender_name: Some("Amy".into()),
            }]
        );
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"broadcastStarted","data":{"id":"b2","type":"camera","hostUserId":"c-new"}}"#
            ),
            vec![SpaceUpdate::Broadcast {
                kind: BroadcastKind::Camera,
                host_id: "c-new".into(),
                sender_name: None,
            }]
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"broadcastStarted","data":{"id":"b3","type":"camera","hostUserId":"c-me"}}"#
            )
            .is_empty()
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"broadcastStarted","data":{"id":"b4","type":"camera","hostUserId":"c-me-phone"}}"#
            )
            .is_empty()
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"broadcastEnded","data":{"broadcastId":"b1"}}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn session_chat_by_range() {
        let mut state = office();
        let chat = |range: &str, sender: &str| {
            format!(
                r#"{{"op":"notification","data":{{"type":"chat","range":"{range}","senderId":"{sender}","message":"hi there","id":"m1"}}}}"#
            )
        };
        let message = |chat: SessionChat, sender_id: &str, sender_name: Option<&str>| {
            vec![SpaceUpdate::SessionMessage {
                chat,
                sender_id: sender_id.into(),
                sender_name: sender_name.map(String::from),
                text: "hi there".into(),
            }]
        };
        assert_eq!(
            frame(&mut state, &chat("space", "c-amy")),
            message(SessionChat::Public, "c-amy", Some("Amy"))
        );
        assert_eq!(
            frame(&mut state, &chat("nearby", "c-amy")),
            message(SessionChat::Nearby, "c-amy", Some("Amy"))
        );
        assert_eq!(
            frame(&mut state, &chat("room", "c-stranger")),
            message(SessionChat::Nearby, "c-stranger", None)
        );
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"chat","range":"space","senderId":"c-x","senderName":"Xu","message":"hi there"}}"#
            ),
            message(SessionChat::Public, "c-x", Some("Xu"))
        );
        assert!(frame(&mut state, &chat("space", "c-me")).is_empty());
        assert!(frame(&mut state, &chat("space", "c-me-phone")).is_empty());
        assert!(frame(&mut state, &chat("custom", "c-amy")).is_empty());
        assert!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"chat","range":"space"}}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn private_session_messages_only_when_sent_to_me() {
        let mut state = office();
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"private","senderId":"c-amy","recipientId":"c-me","message":"psst"}}"#
            ),
            vec![SpaceUpdate::SessionMessage {
                chat: SessionChat::Private,
                sender_id: "c-amy".into(),
                sender_name: Some("Amy".into()),
                text: "psst".into(),
            }]
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"private","senderId":"c-amy","recipientId":"c-bo","message":"psst"}}"#
            )
            .is_empty()
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"private","senderId":"c-me","recipientId":"c-me","message":"note"}}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn nudge_and_knock_texts_are_not_notices() {
        let mut state = office();
        for frame_text in [
            r#"{"op":"notification","data":{"type":"social","message":"Amy nudged you"}}"#,
            r#"{"op":"notification","data":{"type":"config","message":"Amy knocked on the door"}}"#,
            r#"{"op":"notification","data":{"type":"nudge","senderId":"c-amy"}}"#,
        ] {
            assert!(
                !frame(&mut state, frame_text)
                    .iter()
                    .any(|u| matches!(u, SpaceUpdate::Notice(_))),
                "{frame_text}"
            );
        }
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"social","message":"Amy waved at you"}}"#
            ),
            vec![SpaceUpdate::Notice("Amy waved at you".into())]
        );
    }

    #[test]
    fn someone_else_joining_the_floor_is_a_floor_entry() {
        let mut state = office();
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"enter","message":"Amy joined the Floor"}}"#
            ),
            vec![SpaceUpdate::FloorEntry {
                message: "Amy joined the Floor".into()
            }]
        );
        let me = state.me().map(|p| p.name.clone()).unwrap_or_default();
        assert!(!me.is_empty());
        assert!(
            frame(
                &mut state,
                &format!(
                    r#"{{"op":"notification","data":{{"type":"enter","message":"{me} joined the Floor"}}}}"#
                )
            )
            .is_empty()
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"enter"}}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn other_notifications_stay_notices() {
        let mut state = office();
        assert_eq!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"leave","message":"Amy left the Floor"}}"#
            ),
            vec![SpaceUpdate::Notice("Amy left the Floor".into())]
        );
        assert!(
            frame(
                &mut state,
                r#"{"op":"notification","data":{"type":"somethingNew"}}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn one_knock_alerts_once_from_either_source() {
        let mut state = office();
        let start = Instant::now();
        let sound = ServerOp::parse(
            r#"{"op":"playSound","data":{"notificationType":"doorKnock","fromUserId":"c-amy"}}"#,
        )
        .unwrap();
        let notification = ServerOp::parse(
            r#"{"op":"notification","data":{"type":"doorKnock","senderId":"c-amy"}}"#,
        )
        .unwrap();
        let knocks = |updates: Vec<SpaceUpdate>| {
            updates
                .iter()
                .filter(
                    |u| matches!(u, SpaceUpdate::Sound { sender_name: Some(n), .. } if n == "Amy"),
                )
                .count()
        };
        assert_eq!(knocks(state.apply_at(sound.clone(), start)), 1);
        assert_eq!(
            knocks(state.apply_at(notification.clone(), start + Duration::from_secs(1))),
            0
        );
        assert_eq!(
            knocks(state.apply_at(notification.clone(), start + Duration::from_secs(10))),
            1
        );
        assert_eq!(
            knocks(state.apply_at(sound, start + Duration::from_secs(11))),
            0
        );
        let alert = crate::chat::alert::space_alert(
            &state
                .apply_at(notification, start + Duration::from_secs(30))
                .remove(0),
        )
        .unwrap();
        assert_eq!(alert.title, "Amy is knocking");
        let bo = ServerOp::parse(
            r#"{"op":"playSound","data":{"notificationType":"doorKnock","fromUserId":"c-bo"}}"#,
        )
        .unwrap();
        assert_eq!(state.apply_at(bo, start + Duration::from_secs(30)).len(), 1);
    }

    #[test]
    fn unnamed_updates_take_a_name_from_elsewhere() {
        let update = SpaceUpdate::Broadcast {
            kind: BroadcastKind::Camera,
            host_id: "c-x".into(),
            sender_name: None,
        };
        assert_eq!(update.unnamed_sender(), vec!["c-x".to_string()]);
        let named = update.named(Some("Xu".into()));
        assert!(named.unnamed_sender().is_empty());
        assert_eq!(named.clone().named(Some("Other".into())), named);
    }
}
