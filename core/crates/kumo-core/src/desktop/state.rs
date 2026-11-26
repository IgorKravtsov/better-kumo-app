use serde::{Deserialize, Serialize};

use crate::call::{CallZone, Mate};
use crate::chat::notifications::{MyStatus, StatusSignals};
use crate::ws::{Availability, Position};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopState {
    pub floor_id: Option<String>,
    pub width: f64,
    pub height: f64,
    pub floors: Vec<DesktopFloor>,
    #[serde(rename = "self")]
    pub me: Option<DesktopSelf>,
    pub zones: Vec<DesktopZone>,
    pub people: Vec<DesktopPerson>,
    pub audio: bool,
    pub video: bool,
    /// `people[].id` of everyone whose audio the Desktop app receives.
    #[serde(default)]
    pub earshot: Vec<String>,
    #[serde(default)]
    pub in_call: bool,
    #[serde(default)]
    pub call: Option<DesktopRoomCall>,
}

/// The Desktop app's Daily call as anyone paired may see it: never its URL or token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRoomCall {
    pub room_name: String,
    pub peers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopFloor {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSelf {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub zone_id: Option<String>,
    pub availability: Option<Availability>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopZone {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPerson {
    pub id: String,
    pub uid: String,
    pub name: String,
    pub avatar: String,
    pub x: f64,
    pub y: f64,
    pub zone_id: Option<String>,
    #[serde(rename = "self")]
    pub is_self: bool,
    pub availability: Option<Availability>,
    pub idle: bool,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCall {
    pub url: String,
    pub token: String,
    pub self_id: String,
    pub peers: Vec<String>,
    pub room_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presence {
    Available,
    Away,
    Focusing,
    Idle,
    Offline,
}

impl Presence {
    pub const LEGEND: [Presence; 4] = [
        Presence::Available,
        Presence::Away,
        Presence::Focusing,
        Presence::Idle,
    ];

    pub fn of(availability: Option<Availability>, idle: bool) -> Self {
        match (idle, availability) {
            (true, _) => Presence::Idle,
            (false, Some(Availability::Available)) => Presence::Available,
            (false, Some(Availability::Away)) => Presence::Away,
            (false, Some(Availability::Focusing)) => Presence::Focusing,
            (false, None) => Presence::Offline,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Presence::Available => Availability::Available.label(),
            Presence::Away => Availability::Away.label(),
            Presence::Focusing => Availability::Focusing.label(),
            Presence::Idle => "Idle",
            Presence::Offline => "Offline",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DesktopTarget {
    Zone(String),
    Spot(Position),
}

impl DesktopPerson {
    pub fn presence(&self) -> Presence {
        Presence::of(self.availability, self.idle)
    }
}

impl DesktopState {
    pub fn self_zone_id(&self) -> Option<&str> {
        self.me.as_ref()?.zone_id.as_deref()
    }

    pub fn zone(&self, id: &str) -> Option<&DesktopZone> {
        self.zones.iter().find(|z| z.id == id)
    }

    pub fn self_zone(&self) -> Option<&DesktopZone> {
        self.zone(self.self_zone_id()?)
    }

    pub fn floor_name(&self) -> Option<&str> {
        let id = self.floor_id.as_deref()?;
        self.floors
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.name.as_str())
    }

    pub fn self_person(&self) -> Option<&DesktopPerson> {
        self.people.iter().find(|p| p.is_self)
    }

    pub fn person(&self, id: &str) -> Option<&DesktopPerson> {
        self.people.iter().find(|p| p.id == id)
    }

    pub fn availability(&self) -> Option<Availability> {
        self.me.as_ref()?.availability
    }

    pub fn presence(&self) -> Presence {
        Presence::of(self.availability(), false)
    }

    pub fn my_status(&self) -> MyStatus {
        MyStatus::of(StatusSignals {
            known: self.me.is_some(),
            availability: self.availability(),
            idle: self.self_person().is_some_and(|p| p.idle),
        })
    }

    pub fn call_zone(&self) -> CallZone {
        let mate = |p: &DesktopPerson| Mate {
            key: p.id.clone(),
            name: p.name.clone(),
            avatar: p.avatar.clone(),
            presence: p.presence(),
        };
        CallZone {
            room_name: self.self_zone().map(|z| z.name.clone()).unwrap_or_default(),
            me: self.self_person().map(mate),
            mates: self.zone_mates().into_iter().map(mate).collect(),
        }
    }

    pub fn zone_mates(&self) -> Vec<&DesktopPerson> {
        let Some(zone) = self.self_zone_id() else {
            return Vec::new();
        };
        self.people
            .iter()
            .filter(|p| !p.is_self && p.zone_id.as_deref() == Some(zone))
            .collect()
    }

    pub fn zone_at(&self, x: f64, y: f64) -> Option<&DesktopZone> {
        self.zones
            .iter()
            .find(|z| x >= z.x && x <= z.x + z.w && y >= z.y && y <= z.y + z.h)
    }

    pub fn tap_target(&self, x: f64, y: f64) -> Option<DesktopTarget> {
        match self.zone_at(x, y) {
            Some(zone) if Some(zone.id.as_str()) != self.self_zone_id() => {
                Some(DesktopTarget::Zone(zone.id.clone()))
            }
            _ => Position::new(x, y).ok().map(DesktopTarget::Spot),
        }
    }

    pub fn person_target(&self, person_id: &str) -> Option<DesktopTarget> {
        let person = self.person(person_id)?;
        match &person.zone_id {
            Some(zone) => Some(DesktopTarget::Zone(zone.clone())),
            None => Position::new(person.x, person.y)
                .ok()
                .map(DesktopTarget::Spot),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(id: &str, x: f64, y: f64) -> DesktopZone {
        DesktopZone {
            id: id.into(),
            name: id.into(),
            x,
            y,
            w: 100.0,
            h: 100.0,
            open: true,
        }
    }

    fn state() -> DesktopState {
        DesktopState {
            floor_id: Some("f1".into()),
            width: 1000.0,
            height: 800.0,
            floors: vec![DesktopFloor {
                id: "f1".into(),
                name: "Ground".into(),
            }],
            me: Some(DesktopSelf {
                id: "me".into(),
                x: 50.0,
                y: 50.0,
                zone_id: Some("a".into()),
                availability: Some(Availability::Away),
                message: String::new(),
            }),
            zones: vec![zone("a", 0.0, 0.0), zone("b", 300.0, 300.0)],
            people: vec![DesktopPerson {
                id: "amy".into(),
                uid: "u-amy".into(),
                name: "Amy".into(),
                avatar: String::new(),
                x: 600.0,
                y: 650.0,
                zone_id: None,
                is_self: false,
                availability: None,
                idle: false,
                message: String::new(),
            }],
            audio: false,
            video: false,
            earshot: Vec::new(),
            in_call: false,
            call: None,
        }
    }

    #[test]
    fn tapping_another_zone_enters_it_and_tapping_your_own_moves_inside_it() {
        let s = state();
        assert_eq!(
            s.tap_target(350.0, 350.0),
            Some(DesktopTarget::Zone("b".into()))
        );
        assert_eq!(
            s.tap_target(20.0, 30.0),
            Some(DesktopTarget::Spot(Position::new(20.0, 30.0).unwrap()))
        );
        assert_eq!(s.tap_target(-5.0, 30.0), None);
    }

    #[test]
    fn going_to_someone_outside_a_zone_walks_to_their_spot() {
        assert_eq!(
            state().person_target("amy"),
            Some(DesktopTarget::Spot(Position::new(600.0, 650.0).unwrap()))
        );
        assert_eq!(state().person_target("nobody"), None);
    }

    #[test]
    fn presence_prefers_idle_and_falls_back_to_offline() {
        assert_eq!(
            Presence::of(Some(Availability::Available), true),
            Presence::Idle
        );
        assert_eq!(Presence::of(None, false), Presence::Offline);
        assert_eq!(
            Presence::of(Some(Availability::Focusing), false).label(),
            "Focusing"
        );
        assert_eq!(state().presence(), Presence::Away);
    }

    #[test]
    fn my_status_follows_my_availability_unless_idle_or_absent() {
        let mut s = state();
        assert_eq!(s.my_status(), MyStatus::Away);
        s.people.push(DesktopPerson {
            id: "me".into(),
            uid: "u-me".into(),
            name: "Me".into(),
            avatar: String::new(),
            x: 50.0,
            y: 50.0,
            zone_id: Some("a".into()),
            is_self: true,
            availability: Some(Availability::Away),
            idle: true,
            message: String::new(),
        });
        assert_eq!(s.my_status(), MyStatus::Unknown);
        s.people.pop();
        s.me = None;
        assert_eq!(s.my_status(), MyStatus::Unknown);
    }

    #[test]
    fn earshot_and_the_call_go_out_in_camel_case_and_older_relays_mean_none() {
        let mut s = state();
        s.earshot = vec!["amy".into()];
        s.in_call = true;
        s.call = Some(DesktopRoomCall {
            room_name: "Blue".into(),
            peers: vec!["amy".into()],
        });
        let wire = serde_json::to_value(&s).unwrap();
        assert_eq!(wire["earshot"], serde_json::json!(["amy"]));
        assert_eq!(wire["inCall"], true);
        assert_eq!(
            wire["call"],
            serde_json::json!({ "roomName": "Blue", "peers": ["amy"] })
        );
        assert_eq!(serde_json::from_value::<DesktopState>(wire).unwrap(), s);

        let mut older = serde_json::to_value(state()).unwrap();
        for field in ["earshot", "inCall", "call"] {
            older.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(
            serde_json::from_value::<DesktopState>(older).unwrap(),
            state()
        );
    }
}
