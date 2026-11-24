use std::collections::HashMap;
use std::sync::Arc;

use kumo_core::app_session::AppSession;
use kumo_core::chat::alert::space_alert;
use kumo_core::space::{Person, SpaceListener, SpaceSession, SpaceState, SpaceUpdate};

use crate::desktop::{Presence, presence_of};
use crate::mirroring::PhoneMirroring;
use crate::{AppEvent, AppListener};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PersonItem {
    pub key: String,
    pub uid: String,
    pub name: String,
    pub avatar_url: String,
    pub presence: Presence,
    /// Any device of the signed-in account, so the user's desktop session also reads "(you)".
    pub is_self: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PersonJoin {
    pub floor_id: String,
    pub zone_id: String,
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
pub struct SpaceView {
    pub connected: bool,
    pub floors: Vec<FloorItem>,
    pub away: Vec<PersonItem>,
    pub self_zone_id: Option<String>,
}

pub struct SpaceAdapter {
    pub listener: Arc<dyn AppListener>,
    pub mirroring: Arc<PhoneMirroring>,
    pub calls: Arc<AppSession>,
}

impl SpaceListener for SpaceAdapter {
    fn on_space(&self, update: SpaceUpdate) {
        let event = match update {
            SpaceUpdate::Changed => {
                self.calls.office_changed();
                AppEvent::SpaceChanged
            }
            SpaceUpdate::Call(_) => {
                self.calls.office_changed();
                return;
            }
            SpaceUpdate::Notice(message) => AppEvent::Notice { message },
            SpaceUpdate::Error { code, message } => {
                self.calls.office_rejected();
                AppEvent::SpaceError { code, message }
            }
            update => match self.mirroring.local_alert(space_alert(&update)) {
                Some(alert) => AppEvent::Alert { alert },
                None => return,
            },
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
    pub fn wanted(&self, mirroring: bool) -> bool {
        self.want_ui || self.want_call || mirroring
    }
}

fn person(state: &SpaceState, p: &Person) -> PersonItem {
    PersonItem {
        key: p.key.clone(),
        uid: p.uid.clone(),
        name: p.name.clone(),
        avatar_url: p.avatar_url.clone(),
        presence: presence_of(&p.availability),
        is_self: state.is_mine(p),
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
        let item = person(state, p);
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
    }
}
