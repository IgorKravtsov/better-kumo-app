use std::collections::HashMap;
use std::sync::Mutex;

use kumo_core::events::{EventKind, OfficeEvent, Place, PlaceNames, Who, office_changes};
use kumo_core::space::{CallCredentials, SessionChat, SpaceState, SpaceUpdate, sender_ids};
use kumo_core::ws::{ClientOp, ServerOp};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

#[derive(Default)]
struct Mirror {
    office: SpaceState,
    zones: HashMap<String, String>,
    floors: HashMap<String, String>,
}

impl Mirror {
    fn names(&self) -> PlaceNames<'_> {
        PlaceNames {
            zones: &self.zones,
            floors: &self.floors,
        }
    }

    fn remember_zones(&mut self) {
        for zone in self.office.zones.values() {
            if !zone.name.is_empty() {
                self.zones.insert(zone.zone_id.clone(), zone.name.clone());
            }
        }
    }

    fn place(names: &HashMap<String, String>, id: Option<&String>) -> Option<Place> {
        id.map(|id| Place {
            id: id.clone(),
            name: names.get(id).cloned().unwrap_or_default(),
        })
    }

    fn who(&self, id: &str, name: Option<&String>) -> (Who, Option<Place>, Option<Place>) {
        match self.office.person(id) {
            Some(p) => (
                Who {
                    user_id: if p.uid.is_empty() {
                        id.into()
                    } else {
                        p.uid.clone()
                    },
                    name: p.name.clone(),
                },
                Self::place(&self.zones, p.zone_id.as_ref()),
                Self::place(&self.floors, p.floor_id.as_ref()),
            ),
            None => (
                Who {
                    user_id: id.into(),
                    name: name.cloned().unwrap_or_default(),
                },
                None,
                None,
            ),
        }
    }

    fn event(&self, update: &SpaceUpdate, at: i64) -> Option<OfficeEvent> {
        match update {
            SpaceUpdate::Sound { data, sender_name } => {
                let kind = match data["notificationType"].as_str()? {
                    "nudge" => EventKind::Nudge,
                    "doorKnock" => EventKind::Knock,
                    _ => return None,
                };
                let id = sender_ids(data).next().unwrap_or_default();
                let (person, _, _) = self.who(id, sender_name.as_ref());
                let mut event = OfficeEvent::new(kind, at, person);
                if kind == EventKind::Knock {
                    event.room = Self::place(&self.zones, self.office.self_zone_id.as_ref());
                    event.floor = Self::place(&self.floors, self.office.self_floor_id.as_ref());
                }
                Some(event)
            }
            SpaceUpdate::SessionMessage {
                chat,
                sender_id,
                sender_name,
                text,
            } => {
                let (person, room, floor) = self.who(sender_id, sender_name.as_ref());
                let mut event = OfficeEvent::new(EventKind::NearbyChat, at, person);
                event.room = room;
                event.floor = floor;
                event.chat = match chat {
                    SessionChat::Public => "public",
                    SessionChat::Nearby => "nearby",
                    SessionChat::Private => "private",
                }
                .into();
                event.text = text.clone();
                Some(event)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Observed {
    pub updates: Vec<SpaceUpdate>,
    pub events: Vec<OfficeEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Places {
    pub floors: Vec<Place>,
    pub rooms: Vec<Place>,
}

#[derive(Default)]
pub struct OfficeMirror {
    office: Mutex<Mirror>,
    call_ready: Notify,
}

impl OfficeMirror {
    pub fn observe(&self, frame: &str) -> Observed {
        self.observe_at(frame, kumo_core::http::now_ms() as i64)
    }

    pub fn observe_at(&self, frame: &str, at: i64) -> Observed {
        let Ok(op) = ServerOp::parse(frame) else {
            return Observed::default();
        };
        let census = matches!(op, ServerOp::Census(_));
        let Ok(mut mirror) = self.office.lock() else {
            return Observed::default();
        };
        let before = census.then(|| mirror.office.clone());
        let updates = mirror.office.apply(op);
        mirror.remember_zones();
        let mut events = before
            .map(|before| office_changes(&before, &mirror.office, &mirror.names(), at))
            .unwrap_or_default();
        events.extend(updates.iter().filter_map(|u| mirror.event(u, at)));
        drop(mirror);
        if updates
            .iter()
            .any(|u| matches!(u, SpaceUpdate::Call(Some(_))))
        {
            self.call_ready.notify_waiters();
        }
        Observed { updates, events }
    }

    pub fn seed(&self, page: &Value) {
        let Ok(mut mirror) = self.office.lock() else {
            return;
        };
        for floor in page["floors"].as_array().into_iter().flatten() {
            if let (Some(id), Some(name)) = (floor["id"].as_str(), floor["name"].as_str()) {
                mirror.floors.insert(id.into(), name.into());
            }
        }
        for (id, name) in page["zones"].as_object().into_iter().flatten() {
            if let Some(name) = name.as_str().filter(|n| !n.is_empty()) {
                mirror.zones.insert(id.clone(), name.into());
            }
        }
        if page["census"].is_object()
            && let Ok(op) =
                ServerOp::parse(&json!({ "op": "census", "data": page["census"] }).to_string())
        {
            mirror.office.apply(op);
        }
    }

    pub fn places(&self) -> Places {
        let Ok(mirror) = self.office.lock() else {
            return Places::default();
        };
        let sorted = |names: &HashMap<String, String>| {
            let mut places: Vec<Place> = names
                .iter()
                .map(|(id, name)| Place {
                    id: id.clone(),
                    name: name.clone(),
                })
                .collect();
            places.sort_by(|a, b| a.name.cmp(&b.name));
            places
        };
        let mut rooms = sorted(&mirror.zones);
        for i in 0..rooms.len() {
            let shared = rooms
                .iter()
                .filter(|r| !r.name.is_empty() && r.name == rooms[i].name)
                .count()
                > 1;
            if shared {
                rooms[i].id.clear();
            }
        }
        rooms.dedup_by(|a, b| a.id.is_empty() && b.id.is_empty() && a.name == b.name);
        Places {
            floors: sorted(&mirror.floors),
            rooms,
        }
    }

    pub fn reset(&self) {
        if let Ok(mut mirror) = self.office.lock() {
            mirror.office = SpaceState::default();
        }
    }

    pub fn call(&self) -> Option<CallCredentials> {
        self.office.lock().ok().and_then(|m| m.office.call.clone())
    }

    pub fn call_ready(&self) -> Notified<'_> {
        self.call_ready.notified()
    }

    pub fn sender_name(&self, data: &Value) -> Option<String> {
        self.office
            .lock()
            .ok()
            .and_then(|m| m.office.sender_name(data))
    }

    pub fn join_person(&self, id: &str, my_floor: Option<&str>) -> Option<ClientOp> {
        self.office
            .lock()
            .ok()
            .and_then(|m| m.office.join_person_op(id, my_floor))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn room_frame_fills_the_call_credentials() {
        let mirror = OfficeMirror::default();
        let waiter = mirror.call_ready();
        mirror.observe(r#"{"op":"playSound","data":{"notificationType":"nudge"}}"#);
        assert!(mirror.call().is_none());
        mirror.observe(
            r#"{"op":"room","data":{"roomName":"f1","dailyUrl":"https://x.daily.co/r","dailyToken":"fresh"}}"#,
        );
        tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("waiters are woken");
        let call = mirror.call().unwrap();
        assert_eq!(
            (call.daily_url.as_str(), call.daily_token.as_str()),
            ("https://x.daily.co/r", "fresh")
        );
    }

    #[test]
    fn office_frames_become_events_once_the_census_is_seeded() {
        let mirror = OfficeMirror::default();
        mirror.seed(&serde_json::json!({
            "census": {
                "me": {"firebaseUid": "u0", "name": "Me", "roomName": "f1", "zoneId": "z1", "isSelf": true},
                "a": {"firebaseUid": "u1", "name": "Amy", "roomName": "f1"}
            },
            "floors": [{"id": "f1", "name": "Main"}],
            "zones": {"z1": "Kitchen", "z2": "Open space", "z3": "Open space"}
        }));
        let moved = mirror.observe_at(
            r#"{"op":"census","data":{"a":{"firebaseUid":"u1","name":"Amy","roomName":"f1","zoneId":"z1"}}}"#,
            7,
        );
        assert_eq!(moved.events.len(), 1);
        let entered = &moved.events[0];
        assert_eq!(
            (
                entered.kind,
                entered.at,
                entered.person.user_id.as_str(),
                entered.room.as_ref().map(|r| r.name.as_str()),
                entered.floor.as_ref().map(|f| f.name.as_str())
            ),
            (
                EventKind::EnteredRoom,
                7,
                "u1",
                Some("Kitchen"),
                Some("Main")
            )
        );

        let nudge = mirror.observe_at(
            r#"{"op":"notification","data":{"type":"nudge","senderId":"a"}}"#,
            8,
        );
        assert_eq!(nudge.events[0].kind, EventKind::Nudge);
        assert_eq!(nudge.events[0].person.name, "Amy");

        let knock = mirror.observe_at(
            r#"{"op":"playSound","data":{"notificationType":"doorKnock","fromUserId":"a"}}"#,
            9,
        );
        assert_eq!(
            knock.events[0].room.as_ref().map(|r| r.name.as_str()),
            Some("Kitchen")
        );
        assert_eq!(
            mirror.places(),
            Places {
                floors: vec![Place {
                    id: "f1".into(),
                    name: "Main".into()
                }],
                rooms: vec![
                    Place {
                        id: "z1".into(),
                        name: "Kitchen".into()
                    },
                    Place {
                        id: String::new(),
                        name: "Open space".into()
                    },
                ],
            }
        );
    }

    #[test]
    fn names_senders_it_has_seen() {
        let mirror = OfficeMirror::default();
        mirror.observe(r#"{"op":"census","data":{"c1":{"firebaseUid":"u1","name":"Amy"}}}"#);
        assert_eq!(
            mirror.sender_name(&serde_json::json!({"fromUserId":"c1"})),
            Some("Amy".into())
        );
        mirror.reset();
        assert_eq!(
            mirror.sender_name(&serde_json::json!({"fromUserId":"c1"})),
            None
        );
    }
}
