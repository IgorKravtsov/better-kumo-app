use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::desktop::{config_path, save_atomically};
use crate::error::{CoreError, Result};
use crate::space::{Person, SpaceState};

pub const WATCH_FILE: &str = "watch.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Message,
    Nudge,
    Knock,
    EnteredRoom,
    LeftRoom,
    JoinedFloor,
    Online,
    Offline,
    Status,
    NearbyChat,
}

impl EventKind {
    pub const ALL: [EventKind; 10] = [
        EventKind::Message,
        EventKind::Nudge,
        EventKind::Knock,
        EventKind::EnteredRoom,
        EventKind::LeftRoom,
        EventKind::JoinedFloor,
        EventKind::Online,
        EventKind::Offline,
        EventKind::Status,
        EventKind::NearbyChat,
    ];

    pub fn is_office(self) -> bool {
        self != EventKind::Message
    }

    pub fn name(self) -> &'static str {
        match self {
            EventKind::Message => "message",
            EventKind::Nudge => "nudge",
            EventKind::Knock => "knock",
            EventKind::EnteredRoom => "entered_room",
            EventKind::LeftRoom => "left_room",
            EventKind::JoinedFloor => "joined_floor",
            EventKind::Online => "online",
            EventKind::Offline => "offline",
            EventKind::Status => "status",
            EventKind::NearbyChat => "nearby_chat",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.name() == name.trim().to_lowercase())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Who {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OfficeEvent {
    pub kind: EventKind,
    pub at: i64,
    pub person: Who,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room: Option<Place>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<Place>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chat: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
}

impl OfficeEvent {
    pub fn new(kind: EventKind, at: i64, person: Who) -> Self {
        Self {
            kind,
            at,
            person,
            room: None,
            floor: None,
            status: String::new(),
            chat: String::new(),
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub event: OfficeEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsPage {
    pub boot: String,
    pub latest: u64,
    pub events: Vec<LoggedEvent>,
    pub desktop_connected: bool,
}

fn same(a: &str, b: &str) -> bool {
    !a.trim().is_empty() && a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn who_matches(wanted: &Who, user_id: &str, name: &str) -> bool {
    (!wanted.user_id.is_empty() && wanted.user_id == user_id)
        || (wanted.user_id.is_empty() && same(&wanted.name, name))
        || (!wanted.user_id.is_empty() && same(&wanted.user_id, name))
}

fn place_matches(wanted: &Place, place: Option<&Place>) -> bool {
    place.is_some_and(|p| {
        (!wanted.id.is_empty() && (wanted.id == p.id || same(&wanted.id, &p.name)))
            || (wanted.id.is_empty() && same(&wanted.name, &p.name))
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchRule {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default)]
    pub kinds: Vec<EventKind>,
    #[serde(default)]
    pub people: Vec<Who>,
    #[serde(default)]
    pub places: Vec<Place>,
}

impl WatchRule {
    pub fn wants(&self, kind: EventKind) -> bool {
        self.kinds.is_empty() || self.kinds.contains(&kind)
    }

    fn covers_person(&self, user_id: &str, name: &str) -> bool {
        self.people.is_empty() || self.people.iter().any(|w| who_matches(w, user_id, name))
    }

    pub fn matches_message(&self, sender_id: &str, sender_name: &str) -> bool {
        self.wants(EventKind::Message) && self.covers_person(sender_id, sender_name)
    }

    pub fn matches(&self, event: &OfficeEvent) -> bool {
        self.wants(event.kind)
            && self.covers_person(&event.person.user_id, &event.person.name)
            && (self.places.is_empty()
                || self.places.iter().any(|p| {
                    place_matches(p, event.room.as_ref()) || place_matches(p, event.floor.as_ref())
                }))
    }

    pub fn name(&self, index: usize) -> String {
        if self.label.trim().is_empty() {
            format!("rule {}", index + 1)
        } else {
            self.label.trim().to_string()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchRules {
    #[serde(default)]
    pub rules: Vec<WatchRule>,
}

#[derive(Deserialize)]
struct SavedRules {
    #[serde(default)]
    rules: Vec<WatchRule>,
    #[serde(default)]
    people: Vec<Who>,
}

impl WatchRules {
    pub fn default_path() -> PathBuf {
        config_path(WATCH_FILE)
    }

    pub fn load(path: &Path) -> Self {
        let Some(saved) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<SavedRules>(&text).ok())
        else {
            return Self::default();
        };
        let mut rules = saved.rules;
        if !saved.people.is_empty() {
            rules.push(WatchRule {
                label: String::new(),
                kinds: vec![EventKind::Message],
                people: saved.people,
                places: Vec::new(),
            });
        }
        Self { rules }.cleaned()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.cleaned())
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        save_atomically(path, &text)
    }

    pub fn cleaned(&self) -> Self {
        let rules = self
            .rules
            .iter()
            .map(|rule| {
                let mut kinds: Vec<EventKind> = Vec::new();
                for kind in &rule.kinds {
                    if !kinds.contains(kind) {
                        kinds.push(*kind);
                    }
                }
                let mut people: Vec<Who> = Vec::new();
                for who in &rule.people {
                    let who = Who {
                        user_id: who.user_id.trim().to_string(),
                        name: who.name.trim().to_string(),
                    };
                    let empty = who.user_id.is_empty() && who.name.is_empty();
                    let twice = people.iter().any(|p| {
                        if who.user_id.is_empty() {
                            p.user_id.is_empty() && same(&p.name, &who.name)
                        } else {
                            p.user_id == who.user_id
                        }
                    });
                    if !empty && !twice {
                        people.push(who);
                    }
                }
                let mut places: Vec<Place> = Vec::new();
                for place in &rule.places {
                    let place = Place {
                        id: place.id.trim().to_string(),
                        name: place.name.trim().to_string(),
                    };
                    if !(place.id.is_empty() && place.name.is_empty()) && !places.contains(&place) {
                        places.push(place);
                    }
                }
                WatchRule {
                    label: rule.label.trim().to_string(),
                    kinds,
                    people,
                    places,
                }
            })
            .collect();
        Self { rules }
    }

    pub fn wants_messages(&self) -> bool {
        self.rules.iter().any(|r| r.wants(EventKind::Message))
    }

    pub fn wants_office(&self) -> bool {
        self.rules
            .iter()
            .any(|r| r.kinds.is_empty() || r.kinds.iter().any(|k| k.is_office()))
    }

    pub fn matching_message(&self, sender_id: &str, sender_name: &str) -> Vec<String> {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.matches_message(sender_id, sender_name))
            .map(|(i, r)| r.name(i))
            .collect()
    }

    pub fn matching(&self, event: &OfficeEvent) -> Vec<String> {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, r)| r.matches(event))
            .map(|(i, r)| r.name(i))
            .collect()
    }
}

pub struct PlaceNames<'a> {
    pub zones: &'a HashMap<String, String>,
    pub floors: &'a HashMap<String, String>,
}

impl PlaceNames<'_> {
    fn room(&self, id: &str) -> Place {
        Place {
            id: id.to_string(),
            name: self.zones.get(id).cloned().unwrap_or_default(),
        }
    }

    fn floor(&self, id: &str) -> Place {
        Place {
            id: id.to_string(),
            name: self.floors.get(id).cloned().unwrap_or_default(),
        }
    }
}

fn by_uid(state: &SpaceState) -> HashMap<&str, &Person> {
    let me = state.me().map(|p| p.uid.as_str()).filter(|u| !u.is_empty());
    state
        .people
        .values()
        .filter(|p| !p.uid.is_empty() && !p.is_self && Some(p.uid.as_str()) != me)
        .map(|p| (p.uid.as_str(), p))
        .collect()
}

pub fn office_changes(
    before: &SpaceState,
    after: &SpaceState,
    names: &PlaceNames,
    at: i64,
) -> Vec<OfficeEvent> {
    if before.people.is_empty() {
        return Vec::new();
    }
    let old = by_uid(before);
    let new = by_uid(after);
    let who = |p: &Person| Who {
        user_id: p.uid.clone(),
        name: p.name.clone(),
    };
    let located = |kind: EventKind, p: &Person| {
        let mut e = OfficeEvent::new(kind, at, who(p));
        e.room = p.zone_id.as_deref().map(|z| names.room(z));
        e.floor = p.floor_id.as_deref().map(|f| names.floor(f));
        e
    };
    let mut events = Vec::new();
    for (uid, now) in &new {
        let Some(was) = old.get(uid) else {
            events.push(located(EventKind::Online, now));
            continue;
        };
        if now.floor_id != was.floor_id
            && let Some(floor) = &now.floor_id
        {
            let mut e = OfficeEvent::new(EventKind::JoinedFloor, at, who(now));
            e.floor = Some(names.floor(floor));
            events.push(e);
        }
        if now.zone_id != was.zone_id {
            if let Some(zone) = &was.zone_id {
                let mut e = OfficeEvent::new(EventKind::LeftRoom, at, who(now));
                e.room = Some(names.room(zone));
                e.floor = was.floor_id.as_deref().map(|f| names.floor(f));
                events.push(e);
            }
            if now.zone_id.is_some() {
                events.push(located(EventKind::EnteredRoom, now));
            }
        }
        if now.availability != was.availability && !now.availability.is_empty() {
            let mut e = OfficeEvent::new(EventKind::Status, at, who(now));
            e.status = now.availability.clone();
            events.push(e);
        }
    }
    for (uid, was) in &old {
        if !new.contains_key(uid) {
            events.push(OfficeEvent::new(EventKind::Offline, at, who(was)));
        }
    }
    events.sort_by(|a, b| {
        (a.person.name.as_str(), a.kind.name()).cmp(&(b.person.name.as_str(), b.kind.name()))
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::ServerOp;

    fn state(census: &str) -> SpaceState {
        let mut s = SpaceState::default();
        s.apply(ServerOp::parse(&format!(r#"{{"op":"census","data":{census}}}"#)).unwrap());
        s
    }

    fn names() -> (HashMap<String, String>, HashMap<String, String>) {
        (
            HashMap::from([("z1".into(), "Kitchen".into())]),
            HashMap::from([("f1".into(), "Main".into()), ("f2".into(), "Second".into())]),
        )
    }

    fn summary(events: &[OfficeEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| {
                format!(
                    "{} {} {} {} {}",
                    e.kind.name(),
                    e.person.name,
                    e.room.as_ref().map(|r| r.name.as_str()).unwrap_or("-"),
                    e.floor.as_ref().map(|f| f.name.as_str()).unwrap_or("-"),
                    e.status
                )
                .trim()
                .to_string()
            })
            .collect()
    }

    #[test]
    fn the_first_census_is_not_news() {
        let (zones, floors) = names();
        let names = PlaceNames {
            zones: &zones,
            floors: &floors,
        };
        let after = state(r#"{"a":{"firebaseUid":"u1","name":"Amy","roomName":"f1"}}"#);
        assert!(office_changes(&SpaceState::default(), &after, &names, 1).is_empty());
    }

    #[test]
    fn people_moving_arriving_leaving_and_changing_status() {
        let (zones, floors) = names();
        let names = PlaceNames {
            zones: &zones,
            floors: &floors,
        };
        let before = state(
            r#"{"me":{"firebaseUid":"u0","name":"Me","roomName":"f1","isSelf":true},
                "a":{"firebaseUid":"u1","name":"Amy","roomName":"f1","userStatus":{"availability":"available"}},
                "b":{"firebaseUid":"u2","name":"Bo","roomName":"f1","zoneId":"z1"},
                "c":{"firebaseUid":"u3","name":"Cy","roomName":"f1"}}"#,
        );
        let mut after = before.clone();
        after.apply(
            ServerOp::parse(
                r#"{"op":"census","data":{
                    "me":{"firebaseUid":"u0","name":"Me","roomName":"f2","isSelf":true},
                    "a":{"firebaseUid":"u1","name":"Amy","roomName":"f1","zoneId":"z1","userStatus":{"availability":"away"}},
                    "b":{"firebaseUid":"u2","name":"Bo","roomName":"f2"},
                    "c":null,
                    "d":{"firebaseUid":"u4","name":"Di","roomName":"f1"}}}"#,
            )
            .unwrap(),
        );
        assert_eq!(
            summary(&office_changes(&before, &after, &names, 5)),
            [
                "entered_room Amy Kitchen Main",
                "status Amy - - away",
                "joined_floor Bo - Second",
                "left_room Bo Kitchen Main",
                "offline Cy - -",
                "online Di - Main",
            ]
        );
    }

    fn event(kind: EventKind, uid: &str, name: &str) -> OfficeEvent {
        OfficeEvent::new(
            kind,
            0,
            Who {
                user_id: uid.into(),
                name: name.into(),
            },
        )
    }

    #[test]
    fn several_rules_watch_different_things_at_once() {
        let rules = WatchRules {
            rules: vec![
                WatchRule {
                    label: "Amy writes".into(),
                    kinds: vec![EventKind::Message],
                    people: vec![Who {
                        user_id: "u1".into(),
                        name: "Amy".into(),
                    }],
                    places: vec![],
                },
                WatchRule {
                    label: String::new(),
                    kinds: vec![EventKind::Message],
                    people: vec![Who {
                        user_id: String::new(),
                        name: "bo".into(),
                    }],
                    places: vec![],
                },
                WatchRule {
                    label: "Cy arrives".into(),
                    kinds: vec![EventKind::JoinedFloor, EventKind::EnteredRoom],
                    people: vec![Who {
                        user_id: "u3".into(),
                        name: String::new(),
                    }],
                    places: vec![],
                },
                WatchRule {
                    label: "Kitchen".into(),
                    kinds: vec![EventKind::EnteredRoom],
                    people: vec![],
                    places: vec![Place {
                        id: "kitchen".into(),
                        name: String::new(),
                    }],
                },
            ],
        };
        assert_eq!(rules.matching_message("u1", "Amy"), ["Amy writes"]);
        assert_eq!(rules.matching_message("u2", "Bo"), ["rule 2"]);
        assert!(rules.matching_message("u3", "Cy").is_empty());
        assert!(rules.wants_messages() && rules.wants_office());

        let mut cy = event(EventKind::EnteredRoom, "u3", "Cy");
        cy.room = Some(Place {
            id: "z1".into(),
            name: "Kitchen".into(),
        });
        assert_eq!(rules.matching(&cy), ["Cy arrives", "Kitchen"]);
        assert!(
            rules
                .matching(&event(EventKind::Nudge, "u3", "Cy"))
                .is_empty()
        );
        assert!(
            rules
                .matching(&event(EventKind::JoinedFloor, "u1", "Amy"))
                .is_empty()
        );
    }

    #[test]
    fn the_saved_file_keeps_rules_and_reads_the_older_people_list() {
        let dir = std::env::temp_dir().join(format!(
            "kumo-watch-rules-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join(WATCH_FILE);
        assert_eq!(WatchRules::load(&path), WatchRules::default());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"people":[{"user_id":"u1","name":"Amy"}]}"#).unwrap();
        let old = WatchRules::load(&path);
        assert_eq!(old.rules.len(), 1);
        assert_eq!(old.rules[0].kinds, [EventKind::Message]);

        WatchRules {
            rules: vec![WatchRule {
                label: " nudges ".into(),
                kinds: vec![EventKind::Nudge, EventKind::Nudge],
                people: vec![
                    Who::default(),
                    Who {
                        user_id: "u1".into(),
                        name: "Amy".into(),
                    },
                    Who {
                        user_id: "u1".into(),
                        name: "Amy again".into(),
                    },
                ],
                places: vec![],
            }],
        }
        .save(&path)
        .unwrap();
        let saved = WatchRules::load(&path);
        assert_eq!(saved.rules[0].label, "nudges");
        assert_eq!(saved.rules[0].kinds, [EventKind::Nudge]);
        assert_eq!(saved.rules[0].people.len(), 1);
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(WatchRules::load(&path), WatchRules::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
