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

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JoinData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teleport_target: Option<TeleportTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChatData {
    pub message: String,
    pub range: String,
    pub participant_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "op", content = "data", rename_all = "camelCase")]
pub enum ClientOp {
    Join(JoinData),
    Emote { emoji: String },
    Chat(SessionChatData),
    RefreshDailyToken,
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
            space_name: "gobabygo".into(),
            id_token: "tok".into(),
            time_zone: "Europe/Kyiv".into(),
            platform: Platform::Ios,
            reconnect_as: None,
        };
        assert_eq!(
            params.url().unwrap().as_str(),
            "wss://ws.kumospace.com/gobabygo?timeZone=Europe%2FKyiv&idToken=tok&platform=ios"
        );
    }

    #[test]
    fn serializes_client_ops() {
        let join = ClientOp::Join(JoinData {
            room_name: Some("floor-1".into()),
            teleport_target: Some(TeleportTarget::Zone {
                zone_id: "z1".into(),
            }),
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
}
