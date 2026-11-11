use std::collections::HashMap;
use std::sync::Arc;

use kumo_core::space::{
    CallCredentials, Person, SpaceListener, SpaceSession, SpaceState, SpaceUpdate,
};

use crate::relay::{RelayState, sound_alert};
use crate::{AppEvent, AppListener};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PersonItem {
    pub key: String,
    pub uid: String,
    pub name: String,
    pub avatar_url: String,
    pub availability: String,
    pub is_self: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RoomItem {
    pub zone_id: String,
    pub floor_id: String,
    pub name: String,
    pub open: bool,
    pub people: Vec<PersonItem>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FloorItem {
    pub id: String,
    pub name: String,
    pub rooms: Vec<RoomItem>,
    pub common_area: Vec<PersonItem>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CallInfo {
    pub daily_url: String,
    pub daily_token: String,
    pub floor_id: String,
    pub zone_id: Option<String>,
    pub room_name: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SpaceView {
    pub connected: bool,
    pub floors: Vec<FloorItem>,
    pub away: Vec<PersonItem>,
    pub self_zone_id: Option<String>,
    pub call: Option<CallInfo>,
}

pub struct SpaceAdapter {
    pub listener: Arc<dyn AppListener>,
    pub relay: Arc<RelayState>,
}

impl SpaceListener for SpaceAdapter {
    fn on_space(&self, update: SpaceUpdate) {
        let event = match update {
            SpaceUpdate::Changed => AppEvent::SpaceChanged,
            SpaceUpdate::Call(_) => AppEvent::CallChanged,
            SpaceUpdate::Sound { data, sender_name } => {
                if !self.relay.is_enabled() {
                    return;
                }
                match sound_alert(&data, sender_name.as_deref()) {
                    Some(alert) => AppEvent::Alert { alert },
                    None => return,
                }
            }
            SpaceUpdate::Notice(message) => AppEvent::Notice { message },
            SpaceUpdate::Error { code, message } => AppEvent::SpaceError { code, message },
        };
        self.listener.on_event(event);
    }
}

#[derive(Default)]
pub struct SpaceHolder {
    pub session: Option<SpaceSession>,
    pub want_ui: bool,
    pub want_call: bool,
    pub floor_names: HashMap<String, String>,
}

impl SpaceHolder {
    pub fn wanted(&self, relay: bool) -> bool {
        self.want_ui || self.want_call || relay
    }
}

fn person(p: &Person) -> PersonItem {
    PersonItem {
        key: p.key.clone(),
        uid: p.uid.clone(),
        name: p.name.clone(),
        avatar_url: p.avatar_url.clone(),
        availability: p.availability.clone(),
        is_self: p.is_self,
    }
}

fn sort_people(people: &mut [PersonItem]) {
    people.sort_by(|a, b| {
        b.is_self
            .cmp(&a.is_self)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
}

pub fn call_info(state: &SpaceState, creds: &CallCredentials) -> CallInfo {
    let zone = state.self_zone_id.as_ref().and_then(|z| state.zones.get(z));
    CallInfo {
        daily_url: creds.daily_url.clone(),
        daily_token: creds.daily_token.clone(),
        floor_id: creds.floor_id.clone(),
        zone_id: state.self_zone_id.clone(),
        room_name: zone.map(|z| z.name.clone()).unwrap_or_default(),
    }
}

pub fn build_view(state: &SpaceState, floor_names: &HashMap<String, String>) -> SpaceView {
    let mut floors: HashMap<String, FloorItem> = HashMap::new();
    let floor_entry = |floors: &mut HashMap<String, FloorItem>, id: &str| {
        floors.entry(id.to_string()).or_insert_with(|| FloorItem {
            id: id.to_string(),
            name: floor_names
                .get(id)
                .cloned()
                .unwrap_or_else(|| "Floor".into()),
            rooms: Vec::new(),
            common_area: Vec::new(),
        });
    };
    for zone in state.zones.values() {
        floor_entry(&mut floors, &zone.floor_id);
        if let Some(f) = floors.get_mut(&zone.floor_id) {
            f.rooms.push(RoomItem {
                zone_id: zone.zone_id.clone(),
                floor_id: zone.floor_id.clone(),
                name: zone.name.clone(),
                open: zone.open,
                people: Vec::new(),
            });
        }
    }
    let mut away = Vec::new();
    for p in state.people.values() {
        let item = person(p);
        match (&p.floor_id, &p.zone_id) {
            (Some(floor), Some(zone)) => {
                floor_entry(&mut floors, floor);
                let f = floors.get_mut(floor).expect("floor exists");
                match f.rooms.iter_mut().find(|r| &r.zone_id == zone) {
                    Some(room) => room.people.push(item),
                    None => f.common_area.push(item),
                }
            }
            (Some(floor), None) => {
                floor_entry(&mut floors, floor);
                if let Some(f) = floors.get_mut(floor) {
                    f.common_area.push(item);
                }
            }
            _ => away.push(item),
        }
    }
    let mut floors: Vec<FloorItem> = floors.into_values().collect();
    for f in &mut floors {
        for r in &mut f.rooms {
            sort_people(&mut r.people);
        }
        sort_people(&mut f.common_area);
        f.rooms.sort_by(|a, b| {
            b.people
                .len()
                .min(1)
                .cmp(&a.people.len().min(1))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.zone_id.cmp(&b.zone_id))
        });
    }
    floors.sort_by(|a, b| {
        let count = |f: &FloorItem| {
            f.rooms.iter().map(|r| r.people.len()).sum::<usize>() + f.common_area.len()
        };
        count(b).cmp(&count(a)).then_with(|| a.name.cmp(&b.name))
    });
    sort_people(&mut away);
    SpaceView {
        connected: state.connected,
        floors,
        away,
        self_zone_id: state.self_zone_id.clone(),
        call: state.call.as_ref().map(|c| call_info(state, c)),
    }
}
