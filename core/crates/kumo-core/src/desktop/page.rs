use serde::Deserialize;
use serde_json::Value;

use super::state::{
    DesktopCall, DesktopFloor, DesktopPerson, DesktopRoomCall, DesktopSelf, DesktopState,
    DesktopZone,
};
use crate::ws::Availability;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PageError {
    #[error("Kumospace desktop is not in a space")]
    NotInSpace,
    #[error("the desktop app is not in a call")]
    NotInCall,
    #[error("the desktop app sent an unexpected {what}: {detail}")]
    Malformed { what: &'static str, detail: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageState {
    floor_id: Option<String>,
    #[serde(default)]
    width: Option<f64>,
    #[serde(default)]
    height: Option<f64>,
    #[serde(default)]
    floors: Vec<PageFloor>,
    #[serde(rename = "self")]
    me: Option<PageSelf>,
    #[serde(default)]
    zones: Vec<PageZone>,
    #[serde(default)]
    people: Vec<PagePerson>,
    #[serde(default)]
    audio: bool,
    #[serde(default)]
    video: bool,
    #[serde(default)]
    earshot: Vec<String>,
    #[serde(default)]
    in_call: bool,
    #[serde(default)]
    call: Option<PageRoomCall>,
}

/// Deliberately has no `url` or `token`: should the page ever send them here, they are dropped.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageRoomCall {
    room_name: Option<String>,
    #[serde(default)]
    peers: Vec<String>,
}

#[derive(Deserialize)]
struct PageFloor {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageSelf {
    id: String,
    x: Option<f64>,
    y: Option<f64>,
    zone_id: Option<String>,
    availability: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct PageZone {
    id: String,
    name: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
    w: Option<f64>,
    h: Option<f64>,
    open: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PagePerson {
    id: String,
    uid: Option<String>,
    name: Option<String>,
    avatar: Option<String>,
    x: Option<f64>,
    y: Option<f64>,
    zone_id: Option<String>,
    #[serde(rename = "self")]
    is_self: Option<bool>,
    availability: Option<String>,
    idle: Option<bool>,
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageCall {
    url: String,
    token: String,
    self_id: String,
    #[serde(default)]
    peers: Vec<String>,
    room_name: Option<String>,
}

fn availability(value: Option<String>) -> Option<Availability> {
    value?.parse().ok()
}

fn number(value: Option<f64>) -> f64 {
    value.filter(|v| v.is_finite()).unwrap_or_default()
}

fn present(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

impl DesktopState {
    pub fn from_page(value: Value) -> Result<Self, PageError> {
        if value.is_null() {
            return Err(PageError::NotInSpace);
        }
        let page: PageState = serde_json::from_value(value).map_err(|e| PageError::Malformed {
            what: "Desktop state",
            detail: e.to_string(),
        })?;
        Ok(DesktopState {
            floor_id: present(page.floor_id),
            width: number(page.width).max(0.0),
            height: number(page.height).max(0.0),
            floors: page
                .floors
                .into_iter()
                .map(|f| DesktopFloor {
                    id: f.id,
                    name: f.name.unwrap_or_default(),
                })
                .collect(),
            me: page.me.map(|me| DesktopSelf {
                id: me.id,
                x: number(me.x),
                y: number(me.y),
                zone_id: present(me.zone_id),
                availability: availability(me.availability),
                message: me.message.unwrap_or_default(),
            }),
            zones: page
                .zones
                .into_iter()
                .filter_map(|z| {
                    let (w, h) = (number(z.w), number(z.h));
                    (w > 0.0 && h > 0.0).then(|| DesktopZone {
                        id: z.id,
                        name: z.name.unwrap_or_default(),
                        x: number(z.x),
                        y: number(z.y),
                        w,
                        h,
                        open: z.open.unwrap_or(true),
                    })
                })
                .collect(),
            people: page
                .people
                .into_iter()
                .map(|p| DesktopPerson {
                    id: p.id,
                    uid: p.uid.unwrap_or_default(),
                    name: p.name.unwrap_or_default(),
                    avatar: p.avatar.unwrap_or_default(),
                    x: number(p.x),
                    y: number(p.y),
                    zone_id: present(p.zone_id),
                    is_self: p.is_self.unwrap_or_default(),
                    availability: availability(p.availability),
                    idle: p.idle.unwrap_or_default(),
                    message: p.message.unwrap_or_default(),
                })
                .collect(),
            audio: page.audio,
            video: page.video,
            earshot: page.earshot,
            in_call: page.in_call,
            call: page.call.map(|c| DesktopRoomCall {
                room_name: c.room_name.unwrap_or_default(),
                peers: c.peers,
            }),
        })
    }
}

impl DesktopCall {
    pub fn from_page(value: Value) -> Result<Self, PageError> {
        if value.is_null() {
            return Err(PageError::NotInCall);
        }
        let page: PageCall = serde_json::from_value(value).map_err(|e| PageError::Malformed {
            what: "call",
            detail: e.to_string(),
        })?;
        Ok(DesktopCall {
            url: page.url,
            token: page.token,
            self_id: page.self_id,
            peers: page.peers,
            room_name: page.room_name.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn page_state_is_validated_into_typed_desktop_state() {
        let state = DesktopState::from_page(json!({
            "floorId": "floor-1",
            "width": 2000,
            "height": 1200,
            "floors": [{ "id": "floor-1", "name": "Ground" }, { "id": "floor-2", "name": "" }],
            "self": { "id": "c-me", "zoneId": "zone-a", "availability": "focusing", "message": "deep work" },
            "zones": [
                { "id": "zone-a", "name": "Blue Room", "x": 100, "y": 50, "w": 300, "h": 200, "open": false },
                { "id": "zone-flat", "name": "Line", "x": 0, "y": 0, "w": 0, "h": 10, "open": true }
            ],
            "people": [
                { "id": "c-me", "uid": "uid-me", "name": "Me", "avatar": "", "x": 410.5, "y": 220,
                  "zoneId": "zone-a", "self": true, "availability": "focusing", "idle": false, "message": "deep work" },
                { "id": "c-amy", "uid": "uid-amy", "name": "Amy", "avatar": "https://a/amy.png", "x": 0, "y": 0,
                  "zoneId": null, "self": false, "availability": "", "idle": true, "message": "" },
                { "id": "c-bo", "name": "Bo", "availability": "busy" }
            ],
            "audio": true,
            "video": false
        }))
        .unwrap();
        assert_eq!(state.floor_name(), Some("Ground"));
        assert_eq!(
            state.self_zone().map(|z| z.name.as_str()),
            Some("Blue Room")
        );
        assert_eq!(state.zones.len(), 1);
        assert!(!state.zones[0].open);
        let me = state.me.as_ref().unwrap();
        assert_eq!((me.x, me.y), (0.0, 0.0));
        assert_eq!(me.availability, Some(Availability::Focusing));
        assert_eq!(state.people[1].zone_id, None);
        assert_eq!(state.people[1].availability, None);
        assert_eq!(state.people[2].availability, None);
        assert_eq!(state.people[2].uid, "");
        assert!(state.audio && !state.video);
        assert!(state.earshot.is_empty() && !state.in_call && state.call.is_none());
    }

    #[test]
    fn earshot_and_the_call_are_read_without_credentials() {
        let state = DesktopState::from_page(json!({
            "floorId": "floor-1",
            "self": null,
            "earshot": ["c-amy", "c-bo"],
            "inCall": true,
            "call": { "roomName": "Blue Room", "peers": ["c-amy"], "url": "https://x.daily.co/r", "token": "t" }
        }))
        .unwrap();
        assert_eq!(state.earshot, vec!["c-amy", "c-bo"]);
        assert!(state.in_call);
        let wire = serde_json::to_string(&state).unwrap();
        assert!(wire.contains(r#""call":{"roomName":"Blue Room","peers":["c-amy"]}"#));
        assert!(!wire.contains("daily.co") && !wire.contains("token"));
    }

    #[test]
    fn null_page_state_means_the_desktop_is_not_in_a_space() {
        assert_eq!(
            DesktopState::from_page(Value::Null),
            Err(PageError::NotInSpace)
        );
        assert!(matches!(
            DesktopState::from_page(json!({ "floors": 3 })),
            Err(PageError::Malformed { .. })
        ));
    }

    #[test]
    fn page_call_defaults_a_missing_room_name() {
        let call = DesktopCall::from_page(json!({
            "url": "https://x.daily.co/r", "token": "t", "selfId": "c-me", "peers": ["c-amy"]
        }))
        .unwrap();
        assert_eq!(call.room_name, "");
        assert_eq!(call.peers, vec!["c-amy".to_string()]);
        assert_eq!(
            DesktopCall::from_page(Value::Null),
            Err(PageError::NotInCall)
        );
    }
}
