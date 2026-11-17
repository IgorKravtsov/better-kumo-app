use serde::{Deserialize, Serialize};

use super::state::DesktopCall;
use crate::chat::notifications::{MyStatus, NotificationProfile};

pub use crate::ws::{Position as MoveRequest, UserStatusChange as StatusRequest};

pub mod routes {
    pub const PAIR: &str = "/pair";
    pub const STATUS: &str = "/v1/status";
    pub const MIRRORING: &str = "/v1/enabled";
    pub const DEVICE: &str = "/v1/devices/{token}";
    pub const TEST: &str = "/v1/test";
    pub const NTFY: &str = "/v1/ntfy";
    pub const NOTIFICATIONS: &str = "/v1/notifications";
    pub const DESKTOP: &str = "/v1/desktop";
    pub const MOVE: &str = "/v1/desktop/move";
    pub const ZONE: &str = "/v1/desktop/zone";
    pub const FLOOR: &str = "/v1/desktop/floor";
    pub const FOLLOW: &str = "/v1/desktop/follow";
    pub const AVAILABILITY: &str = "/v1/desktop/status";
    pub const MEDIA: &str = "/v1/desktop/media";
    pub const NUDGE: &str = "/v1/desktop/nudge";
    pub const CALL: &str = "/v1/desktop/call";
    pub const HANDOFF: &str = "/v1/desktop/handoff";
    pub const RECLAIM: &str = "/v1/desktop/reclaim";
    pub const LEASE: &str = "/v1/desktop/lease";
    pub const HOME: &str = "/v1/desktop/home";

    pub fn device(token: &str) -> String {
        DEVICE.replace("{token}", token)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStatus {
    pub uid: String,
    #[serde(rename = "enabled")]
    pub mirroring: bool,
    #[serde(rename = "online")]
    pub chat_online: bool,
    pub ntfy: bool,
    pub apns: bool,
    #[serde(rename = "devices")]
    pub phones: u32,
    #[serde(rename = "desktop")]
    pub desktop_connected: bool,
    #[serde(default)]
    pub ntfy_topic: Option<String>,
    #[serde(default)]
    pub ntfy_server: Option<String>,
    #[serde(default)]
    pub notifications: NotificationProfile,
    #[serde(default)]
    pub my_status: MyStatus,
}

impl RelayStatus {
    pub fn ntfy_url(&self) -> Option<String> {
        let topic = self.ntfy_topic.as_deref()?;
        let server = self
            .ntfy_server
            .as_deref()
            .unwrap_or(DEFAULT_NTFY_SERVER)
            .trim_end_matches('/');
        Some(format!("{server}/{topic}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirroringRequest {
    pub enabled: bool,
}

pub const DEFAULT_NTFY_SERVER: &str = "https://ntfy.sh";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfyRequest {
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtfyTarget {
    pub server: String,
    pub topic: String,
}

impl NtfyRequest {
    pub fn target(&self, current_server: Option<&str>) -> Result<Option<NtfyTarget>, RelayError> {
        let topic = self.topic.as_deref().map(str::trim).unwrap_or_default();
        if topic.is_empty() {
            return Ok(None);
        }
        if topic.len() > 64
            || !topic
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(RelayError::Rejected(
                "ntfy topic must be 1-64 letters, digits, - or _".into(),
            ));
        }
        let server = self
            .server
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or(current_server)
            .unwrap_or(DEFAULT_NTFY_SERVER)
            .trim_end_matches('/')
            .to_string();
        if !(server.starts_with("https://") || server.starts_with("http://")) {
            return Err(RelayError::Rejected(
                "ntfy server must start with http:// or https://".into(),
            ));
        }
        Ok(Some(NtfyTarget {
            server,
            topic: topic.to_string(),
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRequest {
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneRequest {
    pub zone_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloorRequest {
    pub floor_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowRequest {
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NudgeRequest {
    pub user_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaRequest {
    #[serde(default)]
    pub audio: Option<bool>,
    #[serde(default)]
    pub video: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRequest {
    pub lease_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub lease_id: String,
    #[serde(flatten)]
    pub call: DesktopCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkReply {
    pub ok: bool,
}

impl OkReply {
    pub const OK: OkReply = OkReply { ok: true };
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    #[error("Relay is not set up")]
    NotPaired,
    #[error("Wrong relay secret")]
    Unauthorized,
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("Another phone took over this call")]
    StaleLease,
    #[error("The desktop app is not in a call")]
    NotInCall,
    #[error("{message}")]
    Http { status: u16, message: String },
    #[error("Relay not reachable: {0}")]
    Network(String),
    #[error("Unexpected reply from the relay: {0}")]
    Protocol(String),
}

impl RelayError {
    pub fn from_reply(status: u16, body: &str) -> Self {
        let message = body.trim().to_string();
        match status {
            401 => RelayError::Unauthorized,
            400 | 422 => RelayError::Rejected(message),
            404 => RelayError::NotFound(if message.is_empty() {
                "Not found".into()
            } else {
                message
            }),
            503 => RelayError::Unavailable(message),
            409 => RelayError::StaleLease,
            412 => RelayError::NotInCall,
            _ => RelayError::Http {
                status,
                message: if message.is_empty() {
                    format!("Relay returned {status}")
                } else {
                    message
                },
            },
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            RelayError::Unauthorized => 401,
            RelayError::Rejected(_) => 400,
            RelayError::NotFound(_) => 404,
            RelayError::Unavailable(_) => 503,
            RelayError::StaleLease => 409,
            RelayError::NotInCall => 412,
            RelayError::Http { status, .. } => *status,
            RelayError::NotPaired | RelayError::Network(_) | RelayError::Protocol(_) => 500,
        }
    }

    pub fn rejected_body(e: &serde_json::Error) -> Self {
        let text = e.to_string();
        let message = text
            .rsplit_once(" at line ")
            .map_or(text.as_str(), |(message, _)| message);
        RelayError::Rejected(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_older_relay_status_means_every_kind_is_on() {
        let status: RelayStatus = serde_json::from_str(
            r#"{"uid":"u","enabled":true,"online":true,"ntfy":false,"apns":false,"devices":0,"desktop":false}"#,
        )
        .unwrap();
        assert_eq!(status.notifications, NotificationProfile::default());
        assert_eq!(status.my_status, MyStatus::Unknown);
        assert_eq!(status.ntfy_url(), None);
    }

    #[test]
    fn flat_kinds_from_an_older_relay_apply_to_every_status() {
        let status: RelayStatus = serde_json::from_str(
            r#"{"uid":"u","enabled":true,"online":true,"ntfy":false,"apns":false,"devices":0,"desktop":true,"notifications":{"broadcasts":false}}"#,
        )
        .unwrap();
        assert_eq!(
            status.notifications,
            NotificationProfile::uniform(crate::chat::notifications::NotificationKinds {
                broadcasts: false,
                ..Default::default()
            })
        );
        let current: RelayStatus = serde_json::from_value(serde_json::json!({
            "uid": "u", "enabled": true, "online": true, "ntfy": false, "apns": false,
            "devices": 0, "desktop": true, "my_status": "focusing",
            "notifications": {"focusing": {"knocks": false}}
        }))
        .unwrap();
        assert_eq!(current.my_status, MyStatus::Focusing);
        assert!(!current.notifications.focusing.knocks);
        assert!(current.notifications.away.knocks);
    }

    #[test]
    fn ntfy_url_uses_the_relay_server_or_ntfy_sh() {
        let mut status: RelayStatus = serde_json::from_str(
            r#"{"uid":"u","enabled":true,"online":true,"ntfy":true,"apns":false,"devices":0,"desktop":false,"ntfy_topic":"kumo-1"}"#,
        )
        .unwrap();
        assert_eq!(status.ntfy_url().as_deref(), Some("https://ntfy.sh/kumo-1"));
        status.ntfy_server = Some("https://ntfy.example.com/".into());
        assert_eq!(
            status.ntfy_url().as_deref(),
            Some("https://ntfy.example.com/kumo-1")
        );
    }

    #[test]
    fn reply_statuses_round_trip_through_relay_errors() {
        for (status, body) in [
            (400, "bad position"),
            (404, "gone"),
            (503, "asleep"),
            (418, "teapot"),
        ] {
            let error = RelayError::from_reply(status, body);
            assert_eq!(error.status(), status);
            assert_eq!(error.to_string(), body);
        }
        assert_eq!(RelayError::from_reply(401, ""), RelayError::Unauthorized);
        for error in [RelayError::StaleLease, RelayError::NotInCall] {
            assert_eq!(RelayError::from_reply(error.status(), "ignored"), error);
        }
        assert_eq!(
            RelayError::from_reply(500, "").to_string(),
            "Relay returned 500"
        );
    }

    #[test]
    fn ntfy_requests_validate_the_topic_and_keep_the_server() {
        let request = |topic: Option<&str>, server: Option<&str>| NtfyRequest {
            topic: topic.map(Into::into),
            server: server.map(Into::into),
        };
        assert_eq!(request(None, None).target(None), Ok(None));
        assert_eq!(request(Some("  "), None).target(None), Ok(None));
        assert_eq!(
            request(Some(" my-kumo-alerts "), None).target(Some("https://ntfy.example.com/")),
            Ok(Some(NtfyTarget {
                server: "https://ntfy.example.com".into(),
                topic: "my-kumo-alerts".into(),
            }))
        );
        assert_eq!(
            request(Some("a-b_9"), None)
                .target(None)
                .unwrap()
                .unwrap()
                .server,
            DEFAULT_NTFY_SERVER
        );
        assert!(matches!(
            request(Some("has space"), None).target(None),
            Err(RelayError::Rejected(_))
        ));
        assert!(matches!(
            request(Some(&"x".repeat(65)), None).target(None),
            Err(RelayError::Rejected(_))
        ));
        assert!(matches!(
            request(Some("ok"), Some("ftp://x")).target(None),
            Err(RelayError::Rejected(_))
        ));
    }

    #[test]
    fn rejected_bodies_drop_the_json_position() {
        let e = serde_json::from_str::<StatusRequest>(r#"{"availability":"busy"}"#).unwrap_err();
        assert_eq!(
            RelayError::rejected_body(&e),
            RelayError::Rejected("availability must be available, away or focusing".into())
        );
        let e = serde_json::from_str::<MoveRequest>(r#"{"x":-1,"y":2}"#).unwrap_err();
        assert_eq!(RelayError::rejected_body(&e).to_string(), "bad position");
    }

    #[test]
    fn handoff_carries_the_lease_next_to_the_call() {
        let handoff = Handoff {
            lease_id: "l1".into(),
            call: DesktopCall {
                url: "https://x.daily.co/r".into(),
                token: "t".into(),
                self_id: "c-me".into(),
                peers: vec!["c-amy".into()],
                room_name: "Blue".into(),
            },
        };
        let wire = serde_json::to_value(&handoff).unwrap();
        assert_eq!(wire["lease_id"], "l1");
        assert_eq!(wire["roomName"], "Blue");
        assert_eq!(serde_json::from_value::<Handoff>(wire).unwrap(), handoff);
    }

    #[test]
    fn device_route_fills_in_the_token() {
        assert_eq!(routes::device("abc"), "/v1/devices/abc");
    }
}
