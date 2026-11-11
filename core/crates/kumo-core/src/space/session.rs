use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::mpsc;

use super::socket::{SpaceConnection, SpaceEvent};
use crate::auth::Auth;
use crate::config::KumoConfig;
use crate::ws::{ClientOp, ConnectParams, JoinData, Platform, ServerOp, TeleportTarget};

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
}

impl SpaceState {
    pub fn me(&self) -> Option<&Person> {
        self.people.values().find(|p| p.is_self)
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpaceUpdate {
    Changed,
    Call(Option<CallCredentials>),
    Sound {
        data: Value,
        sender_name: Option<String>,
    },
    Notice(String),
    Error {
        code: String,
        message: String,
    },
}

const SENDER_KEYS: &[&str] = &[
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
            password: None,
        }));
    }

    pub fn join_zone_with_password(&self, floor_id: &str, zone_id: &str, password: &str) {
        let _ = self.commands.send(ClientOp::Join(JoinData {
            room_name: Some(floor_id.to_string()),
            teleport_target: Some(TeleportTarget::Zone {
                zone_id: zone_id.to_string(),
            }),
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
    match op {
        ServerOp::Config(v) => {
            update(state, |s| {
                s.space_uid = v["uid"].as_str().unwrap_or_default().to_string()
            });
        }
        ServerOp::Census(map) => {
            update(state, |s| {
                for (key, user) in map {
                    match user {
                        Some(u) => {
                            let existing = s.people.get(&key).cloned();
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
                                availability: u
                                    .user_status
                                    .and_then(|st| st.availability)
                                    .or(existing.as_ref().map(|e| e.availability.clone()))
                                    .unwrap_or_default(),
                                zone_id: u.zone_id,
                                floor_id: u.room_name,
                                is_self: match &s.self_key {
                                    Some(k) => k == &key,
                                    None => {
                                        census_self || existing.as_ref().is_some_and(|e| e.is_self)
                                    }
                                },
                            };
                            if person.is_self {
                                s.self_zone_id = person.zone_id.clone();
                                s.self_floor_id = person.floor_id.clone();
                            }
                            s.people.insert(key, person);
                        }
                        None => {
                            s.people.remove(&key);
                        }
                    }
                }
            });
            listener.on_space(SpaceUpdate::Changed);
        }
        ServerOp::Entities(list) => {
            let mut changed = false;
            update(state, |s| {
                for e in list.iter().filter(|e| e.is_self && !e.is_deleted) {
                    s.self_key = Some(e.id.clone());
                    s.self_zone_id = e.zone_id.clone();
                    s.self_floor_id = e.room_name.clone();
                    for p in s.people.values_mut() {
                        p.is_self = p.key == e.id;
                    }
                    if let Some(p) = s.people.get_mut(&e.id) {
                        p.zone_id = e.zone_id.clone();
                        p.floor_id = e.room_name.clone();
                    }
                    changed = true;
                }
            });
            if changed {
                listener.on_space(SpaceUpdate::Changed);
            }
        }
        ServerOp::SpaceZones(map) => {
            update(state, |s| {
                for (zone_id, zone) in map {
                    match zone {
                        Some(z) => {
                            s.zones.insert(
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
                            s.zones.remove(&zone_id);
                        }
                    }
                }
            });
            listener.on_space(SpaceUpdate::Changed);
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
            update(state, |s| s.call = creds.clone());
            listener.on_space(SpaceUpdate::Call(creds));
            listener.on_space(SpaceUpdate::Changed);
        }
        ServerOp::PlaySound(data) => {
            let sender_name = state.lock().ok().and_then(|s| {
                SENDER_KEYS
                    .iter()
                    .filter_map(|k| data[*k].as_str())
                    .find_map(|id| {
                        s.people
                            .get(id)
                            .or_else(|| s.person_by_uid(id))
                            .map(|p| p.name.clone())
                    })
            });
            listener.on_space(SpaceUpdate::Sound { data, sender_name });
        }
        ServerOp::Error(e) => listener.on_space(SpaceUpdate::Error {
            code: e.code.unwrap_or_default(),
            message: e.message.unwrap_or_default(),
        }),
        ServerOp::Notification(v) => {
            if let Some(m) = v["message"].as_str() {
                listener.on_space(SpaceUpdate::Notice(m.to_string()));
            }
        }
        _ => {}
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
}
