use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    Ios,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::Ios => "ios",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub ws_base_url: String,
    pub space_name: String,
    pub id_token: String,
    pub time_zone: String,
    pub platform: Platform,
    pub reconnect_as: Option<String>,
}

impl ConnectParams {
    pub fn url(&self) -> Result<Url, url::ParseError> {
        let mut url = Url::parse(&self.ws_base_url)?;
        url.path_segments_mut()
            .map_err(|_| url::ParseError::RelativeUrlWithCannotBeABaseBase)?
            .clear()
            .push(&self.space_name);
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("timeZone", &self.time_zone)
                .append_pair("idToken", &self.id_token)
                .append_pair("platform", self.platform.as_str());
            if let Some(entity) = &self.reconnect_as {
                query.append_pair("reconnectAs", entity);
            }
        }
        Ok(url)
    }
}

pub const CLOSE_CODE_BANNED: u16 = 4000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TeleportTarget {
    User {
        #[serde(rename = "userId")]
        user_id: String,
    },
    Zone {
        #[serde(rename = "zoneId")]
        zone_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidOp {
    #[error("bad position")]
    Position,
    #[error("availability must be available, away or focusing")]
    Availability,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PositionData")]
pub struct Position {
    x: f64,
    y: f64,
}

#[derive(Deserialize)]
struct PositionData {
    x: f64,
    y: f64,
}

impl TryFrom<PositionData> for Position {
    type Error = InvalidOp;

    fn try_from(p: PositionData) -> Result<Self, Self::Error> {
        Position::new(p.x, p.y)
    }
}

impl Position {
    pub fn new(x: f64, y: f64) -> Result<Self, InvalidOp> {
        if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 {
            Ok(Self { x, y })
        } else {
            Err(InvalidOp::Position)
        }
    }

    pub fn x(&self) -> f64 {
        self.x
    }

    pub fn y(&self) -> f64 {
        self.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", try_from = "String")]
pub enum Availability {
    Available,
    Away,
    Focusing,
}

impl Availability {
    pub const ALL: [Availability; 3] = [
        Availability::Available,
        Availability::Away,
        Availability::Focusing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Availability::Available => "available",
            Availability::Away => "away",
            Availability::Focusing => "focusing",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Availability::Available => "Available",
            Availability::Away => "Away",
            Availability::Focusing => "Focusing",
        }
    }
}

impl TryFrom<String> for Availability {
    type Error = InvalidOp;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl std::str::FromStr for Availability {
    type Err = InvalidOp;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "available" => Ok(Availability::Available),
            "away" => Ok(Availability::Away),
            "focusing" => Ok(Availability::Focusing),
            _ => Err(InvalidOp::Availability),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Command {
    Destination(Position),
    NavigateToZone {
        #[serde(rename = "zoneId")]
        zone_id: String,
    },
    FollowUser {
        #[serde(rename = "userId")]
        user_id: String,
    },
    StopFollowing {
        #[serde(rename = "userId", default)]
        user_id: String,
    },
}

impl Command {
    pub fn stop_following() -> Self {
        Command::StopFollowing {
            user_id: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JoinData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teleport_target: Option<TeleportTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<Position>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChatData {
    pub message: String,
    pub range: String,
    pub participant_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserStatusChange {
    pub availability: Availability,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", content = "data", rename_all = "camelCase")]
pub enum ClientOp {
    Join(JoinData),
    Emote {
        emoji: String,
    },
    Chat(SessionChatData),
    RefreshDailyToken,
    Command(Command),
    SetUserStatus(UserStatusChange),
    Nudge {
        #[serde(rename = "userId")]
        user_id: String,
    },
}

impl ClientOp {
    pub fn leave() -> Self {
        ClientOp::Join(JoinData::default())
    }

    pub fn to_frame(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UserStatus {
    pub availability: Option<String>,
    pub message: Option<String>,
    pub set_by: Option<String>,
}

impl UserStatus {
    pub fn idle(&self) -> bool {
        self.set_by.as_deref() == Some("idle")
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CensusUser {
    pub firebase_uid: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub user_status: Option<UserStatus>,
    pub zone_id: Option<String>,
    pub room_name: Option<String>,
    pub is_self: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub is_deleted: bool,
    #[serde(default, rename = "self")]
    pub is_self: bool,
    pub name: Option<String>,
    pub firebase_uid: Option<String>,
    pub user_status: Option<UserStatus>,
    pub zone_id: Option<String>,
    pub room_name: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomCredentials {
    pub room_name: Option<String>,
    pub daily_token: Option<String>,
    pub daily_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ServerError {
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastStarted {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub host_user_id: String,
    #[serde(default)]
    pub presenter_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OfficeNotification {
    Chat {
        #[serde(default)]
        range: String,
        sender_id: String,
        #[serde(default)]
        sender_name: Option<String>,
        message: String,
    },
    Private {
        sender_id: String,
        #[serde(default)]
        recipient_id: Option<String>,
        #[serde(default)]
        sender_name: Option<String>,
        message: String,
    },
    DoorKnock {
        #[serde(default)]
        sender_id: Option<String>,
        #[serde(default)]
        sender_name: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "op", content = "data", rename_all = "camelCase")]
pub enum ServerOp {
    Config(Value),
    Census(HashMap<String, Option<CensusUser>>),
    Entities(Vec<Entity>),
    SpaceZones(HashMap<String, Option<Value>>),
    Room(RoomCredentials),
    Error(ServerError),
    Notification(Value),
    UserSettings(Value),
    HostMute(Option<Value>),
    GuestTimeLimitReached(Option<Value>),
    PlaySound(Value),
    BroadcastStarted(BroadcastStarted),
    BroadcastEnded(Option<Value>),
    #[serde(skip)]
    Unknown {
        op: String,
    },
}

const KNOWN_SERVER_OPS: &[&str] = &[
    "config",
    "census",
    "entities",
    "spaceZones",
    "room",
    "error",
    "notification",
    "userSettings",
    "hostMute",
    "guestTimeLimitReached",
    "playSound",
    "broadcastStarted",
    "broadcastEnded",
];

#[derive(Deserialize)]
struct RawFrame {
    op: String,
}

impl ServerOp {
    pub fn parse(frame: &str) -> Result<Self, serde_json::Error> {
        let raw: RawFrame = serde_json::from_str(frame)?;
        if KNOWN_SERVER_OPS.contains(&raw.op.as_str()) {
            serde_json::from_str(frame)
        } else {
            Ok(ServerOp::Unknown { op: raw.op })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_connect_url() {
        let params = ConnectParams {
            ws_base_url: "wss://ws.kumospace.com".into(),
            space_name: "acme-office".into(),
            id_token: "tok".into(),
            time_zone: "Europe/Kyiv".into(),
            platform: Platform::Ios,
            reconnect_as: None,
        };
        assert_eq!(
            params.url().unwrap().as_str(),
            "wss://ws.kumospace.com/acme-office?timeZone=Europe%2FKyiv&idToken=tok&platform=ios"
        );
    }

    #[test]
    fn serializes_client_ops() {
        let join = ClientOp::Join(JoinData {
            room_name: Some("floor-1".into()),
            teleport_target: Some(TeleportTarget::Zone {
                zone_id: "z1".into(),
            }),
            destination: None,
            password: None,
        });
        assert_eq!(
            join.to_frame(),
            r#"{"op":"join","data":{"roomName":"floor-1","teleportTarget":{"type":"zone","zoneId":"z1"}}}"#
        );
        assert_eq!(ClientOp::leave().to_frame(), r#"{"op":"join","data":{}}"#);
        assert_eq!(
            ClientOp::RefreshDailyToken.to_frame(),
            r#"{"op":"refreshDailyToken"}"#
        );
    }

    fn round_trip(op: ClientOp, frame: &str) {
        assert_eq!(op.to_frame(), frame);
        assert_eq!(serde_json::from_str::<ClientOp>(frame).unwrap(), op);
    }

    #[test]
    fn round_trips_office_commands() {
        round_trip(
            ClientOp::Command(Command::Destination(Position::new(120.5, 40.0).unwrap())),
            r#"{"op":"command","data":{"action":"destination","x":120.5,"y":40.0}}"#,
        );
        round_trip(
            ClientOp::Command(Command::NavigateToZone {
                zone_id: "z1".into(),
            }),
            r#"{"op":"command","data":{"action":"navigateToZone","zoneId":"z1"}}"#,
        );
        round_trip(
            ClientOp::Command(Command::FollowUser {
                user_id: "c1".into(),
            }),
            r#"{"op":"command","data":{"action":"followUser","userId":"c1"}}"#,
        );
        round_trip(
            ClientOp::Command(Command::stop_following()),
            r#"{"op":"command","data":{"action":"stopFollowing","userId":""}}"#,
        );
        round_trip(
            ClientOp::SetUserStatus(UserStatusChange {
                availability: Availability::Focusing,
                message: "deep work".into(),
            }),
            r#"{"op":"setUserStatus","data":{"availability":"focusing","message":"deep work"}}"#,
        );
        round_trip(
            ClientOp::Nudge {
                user_id: "c1".into(),
            },
            r#"{"op":"nudge","data":{"userId":"c1"}}"#,
        );
        round_trip(
            ClientOp::Join(JoinData {
                room_name: Some("f2".into()),
                destination: Some(Position::new(10.0, 20.0).unwrap()),
                ..JoinData::default()
            }),
            r#"{"op":"join","data":{"roomName":"f2","destination":{"x":10.0,"y":20.0}}}"#,
        );
        round_trip(ClientOp::RefreshDailyToken, r#"{"op":"refreshDailyToken"}"#);
    }

    #[test]
    fn rejects_invalid_commands() {
        assert_eq!(Position::new(-1.0, 0.0), Err(InvalidOp::Position));
        assert_eq!(Position::new(f64::NAN, 0.0), Err(InvalidOp::Position));
        assert_eq!("busy".parse::<Availability>(), Err(InvalidOp::Availability));
        assert_eq!("away".parse::<Availability>(), Ok(Availability::Away));
        assert!(
            serde_json::from_str::<ClientOp>(
                r#"{"op":"command","data":{"action":"destination","x":-5,"y":1}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<ClientOp>(
                r#"{"op":"command","data":{"action":"removeFollower","userId":"c1"}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn parses_server_ops() {
        let census = ServerOp::parse(
            r#"{"op":"census","data":{"u1":{"name":"Amy","userStatus":{"availability":"away"}},"u2":null}}"#,
        )
        .unwrap();
        let ServerOp::Census(map) = census else {
            panic!("expected census")
        };
        assert_eq!(map["u1"].as_ref().unwrap().name.as_deref(), Some("Amy"));
        assert!(map["u2"].is_none());

        let entities = ServerOp::parse(
            r#"{"op":"entities","data":[{"id":"e1","type":"user","self":true,"zoneId":"z1","x":10.5}]}"#,
        )
        .unwrap();
        let ServerOp::Entities(list) = entities else {
            panic!("expected entities")
        };
        assert!(list[0].is_self);

        assert_eq!(
            ServerOp::parse(r#"{"op":"furniture","data":{}}"#).unwrap(),
            ServerOp::Unknown {
                op: "furniture".into()
            }
        );
        assert_eq!(
            ServerOp::parse(r#"{"op":"hostMute"}"#).unwrap(),
            ServerOp::HostMute(None)
        );
    }

    #[test]
    fn parses_broadcast_ops() {
        assert_eq!(
            ServerOp::parse(
                r#"{"op":"broadcastStarted","data":{"id":"b1","type":"screen","hostUserId":"c-amy","presenterIds":["c-amy"],"range":"space"}}"#
            )
            .unwrap(),
            ServerOp::BroadcastStarted(BroadcastStarted {
                id: Some("b1".into()),
                kind: Some("screen".into()),
                host_user_id: "c-amy".into(),
                presenter_ids: vec!["c-amy".into()],
            })
        );
        assert!(matches!(
            ServerOp::parse(r#"{"op":"broadcastEnded","data":{"broadcastId":"b1"}}"#).unwrap(),
            ServerOp::BroadcastEnded(Some(_))
        ));
    }

    #[test]
    fn parses_office_notifications() {
        let parse = |v: serde_json::Value| serde_json::from_value::<OfficeNotification>(v).unwrap();
        assert_eq!(
            parse(
                serde_json::json!({"type":"chat","range":"space","senderId":"c1","message":"hi","id":"m1"})
            ),
            OfficeNotification::Chat {
                range: "space".into(),
                sender_id: "c1".into(),
                sender_name: None,
                message: "hi".into(),
            }
        );
        assert_eq!(
            parse(
                serde_json::json!({"type":"private","senderId":"c1","recipientId":"c-me","message":"psst"})
            ),
            OfficeNotification::Private {
                sender_id: "c1".into(),
                recipient_id: Some("c-me".into()),
                sender_name: None,
                message: "psst".into(),
            }
        );
        assert_eq!(
            parse(serde_json::json!({"type":"doorKnock","senderId":"c1"})),
            OfficeNotification::DoorKnock {
                sender_id: Some("c1".into()),
                sender_name: None,
            }
        );
        assert_eq!(
            parse(serde_json::json!({"type":"enter","message":"Amy joined the Floor"})),
            OfficeNotification::Other
        );
    }
}
