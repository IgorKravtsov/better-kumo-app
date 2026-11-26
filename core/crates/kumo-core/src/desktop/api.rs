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
    pub const VIEWING: &str = "/v1/viewing";
    pub const DESKTOP: &str = "/v1/desktop";
    pub const MOVE: &str = "/v1/desktop/move";
    pub const ZONE: &str = "/v1/desktop/zone";
    pub const FLOOR: &str = "/v1/desktop/floor";
    pub const FOLLOW: &str = "/v1/desktop/follow";
    pub const AVAILABILITY: &str = "/v1/desktop/status";
    pub const MEDIA: &str = "/v1/desktop/media";
    pub const NUDGE: &str = "/v1/desktop/nudge";
    pub const JOIN_PERSON: &str = "/v1/desktop/join-person";
    pub const CALL: &str = "/v1/desktop/call";
    pub const HANDOFF: &str = "/v1/desktop/handoff";
    pub const RECLAIM: &str = "/v1/desktop/reclaim";
    pub const LEASE: &str = "/v1/desktop/lease";
    pub const HOME: &str = "/v1/desktop/home";
    pub const NEARBY_CHAT: &str = "/v1/desktop/nearby-chat";
    pub const AGENTS: &str = "/v1/agents";
    pub const EVENTS: &str = "/v1/events";
    pub const WEBHOOKS: &str = "/v1/webhooks";
    pub const WEBHOOK: &str = "/v1/webhooks/{id}";

    pub fn device(token: &str) -> String {
        DEVICE.replace("{token}", token)
    }

    pub fn webhook(id: &str) -> String {
        WEBHOOK.replace("{id}", id)
    }
}

pub const RELAY_API: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayCompatibility {
    Compatible,
    RelayOutdated,
    AppOutdated,
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
    #[serde(default = "first_api")]
    pub api: u32,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub agents: Option<bool>,
}

fn first_api() -> u32 {
    1
}

impl RelayStatus {
    pub fn compatibility(&self) -> RelayCompatibility {
        self.compatibility_with(RELAY_API)
    }

    fn compatibility_with(&self, app_api: u32) -> RelayCompatibility {
        match self.api.cmp(&app_api) {
            std::cmp::Ordering::Less => RelayCompatibility::RelayOutdated,
            std::cmp::Ordering::Equal => RelayCompatibility::Compatible,
            std::cmp::Ordering::Greater => RelayCompatibility::AppOutdated,
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentsRequest {
    pub enabled: bool,
}

pub const DEFAULT_NTFY_SERVER: &str = "https://ntfy.sh";

/// Empty input means ntfy.sh and a bare host gets `https://`. ntfy serves
/// topics at the root, so a path (often a pasted topic link) is rejected.
pub fn ntfy_server_from_input(input: &str) -> Result<String, RelayError> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(DEFAULT_NTFY_SERVER.into());
    }
    let invalid = || {
        RelayError::Rejected(
            "ntfy server must be an address like ntfy.example.com or https://ntfy.example.com"
                .into(),
        )
    };
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    // Checked before parsing, which would resolve `/topic/..` to `/`.
    let after_scheme = with_scheme.split_once("://").map_or("", |(_, rest)| rest);
    if after_scheme.contains('\\')
        || after_scheme
            .find('/')
            .is_some_and(|slash| slash + 1 != after_scheme.len())
    {
        return Err(invalid());
    }
    let url = url::Url::parse(&with_scheme).map_err(|_| invalid())?;
    // The URL parser reads `192.168.1.010` as octal, which would silently pick another host.
    let authority = after_scheme.trim_end_matches('/');
    if let Some(url::Host::Ipv4(ip)) = url.host() {
        let typed = authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host);
        if typed != ip.to_string() {
            return Err(invalid());
        }
    }
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || input.chars().any(char::is_whitespace)
    {
        return Err(invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

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
pub struct ViewingRequest {
    #[serde(default)]
    pub channel_url: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinPersonRequest {
    pub user_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MediaRequest {
    #[serde(default)]
    pub audio: Option<bool>,
    #[serde(default)]
    pub video: Option<bool>,
}

pub const MAX_WEBHOOK_URLS: usize = 8;

/// `urls` are tried in order; `secret` keys the HMAC-SHA256 in each delivery's `X-Kumo-Signature`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookRequest {
    pub urls: Vec<String>,
    pub secret: String,
}

impl WebhookRequest {
    pub fn validate(&self, id: &str) -> Result<(), RelayError> {
        let rejected = |m: &str| Err(RelayError::Rejected(m.into()));
        if id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return rejected("webhook id must be 1-64 letters, digits, - or _");
        }
        if self.urls.is_empty() || self.urls.len() > MAX_WEBHOOK_URLS {
            return rejected("webhook needs 1-8 urls");
        }
        for url in &self.urls {
            let parsed = url::Url::parse(url).ok();
            if !parsed.is_some_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
            {
                return rejected("webhook urls must be http:// or https:// addresses");
            }
        }
        if self.secret.len() < 16 || self.secret.len() > 256 {
            return rejected("webhook secret must be 16-256 characters");
        }
        Ok(())
    }
}

/// What `GET /v1/webhooks` lists: never the secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookInfo {
    pub id: String,
    pub urls: Vec<String>,
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
    fn a_relay_without_an_api_number_speaks_the_first_api() {
        let status: RelayStatus = serde_json::from_str(
            r#"{"uid":"u","enabled":true,"online":true,"ntfy":false,"apns":false,"devices":0,"desktop":false}"#,
        )
        .unwrap();
        assert_eq!(status.api, 1);
        assert_eq!(status.version, None);
        assert_eq!(status.compatibility_with(1), RelayCompatibility::Compatible);
        assert_eq!(
            status.compatibility_with(2),
            RelayCompatibility::RelayOutdated
        );
    }

    #[test]
    fn a_newer_relay_asks_for_a_newer_app() {
        let status: RelayStatus = serde_json::from_value(serde_json::json!({
            "uid": "u", "enabled": true, "online": true, "ntfy": false, "apns": false,
            "devices": 0, "desktop": true, "api": 4, "version": "0.4.0"
        }))
        .unwrap();
        assert_eq!(status.version.as_deref(), Some("0.4.0"));
        assert_eq!(
            status.compatibility_with(3),
            RelayCompatibility::AppOutdated
        );
    }

    #[test]
    fn an_older_relay_status_means_every_kind_is_on() {
        let status: RelayStatus = serde_json::from_str(
            r#"{"uid":"u","enabled":true,"online":true,"ntfy":false,"apns":false,"devices":0,"desktop":false}"#,
        )
        .unwrap();
        assert_eq!(status.notifications, NotificationProfile::default());
        assert_eq!(status.my_status, MyStatus::Unknown);
        assert_eq!(status.ntfy_url(), None);
        assert_eq!(status.agents, None);
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
    fn ntfy_server_input_defaults_to_ntfy_sh_and_https() {
        let ok = |input: &str| ntfy_server_from_input(input).unwrap();
        assert_eq!(ok(""), DEFAULT_NTFY_SERVER);
        assert_eq!(ok("  "), DEFAULT_NTFY_SERVER);
        assert_eq!(ok(" ntfy.example.com/ "), "https://ntfy.example.com");
        assert_eq!(ok("http://10.0.0.2:8080"), "http://10.0.0.2:8080");
        assert_eq!(ok("HTTPS://Ntfy.Example.com/"), "https://ntfy.example.com");
        assert_eq!(ok("192.168.1.10:8080"), "https://192.168.1.10:8080");
        for bad in [
            "ntfy.sh/my-topic",
            "https://ntfy.sh/my-topic",
            "ntfy example.com",
            "ftp://ntfy.example.com",
            "https://ntfy.example.com?x=1",
            "https://user@ntfy.example.com",
            "https://:secret@ntfy.example.com",
            "https://ntfy.example.com/my-topic/..",
            "https://ntfy.example.com/my-topic/%2e%2e",
            "https://ntfy.example.com\\my-topic\\..",
            "192.168.1.010",
            "0x7f.1",
            "https://",
        ] {
            assert!(
                matches!(ntfy_server_from_input(bad), Err(RelayError::Rejected(_))),
                "{bad}"
            );
        }
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
        assert_eq!(routes::webhook("entry-1"), "/v1/webhooks/entry-1");
    }

    #[test]
    fn webhook_requests_need_a_plain_id_web_urls_and_a_real_secret() {
        let request = |urls: &[&str], secret: &str| WebhookRequest {
            urls: urls.iter().map(|u| u.to_string()).collect(),
            secret: secret.into(),
        };
        let good = request(
            &[
                "http://homeassistant.local:8123/api/webhook/x",
                "https://ha.example.com/api/webhook/x",
            ],
            &"a".repeat(64),
        );
        assert_eq!(good.validate("01J9ZQ_entry-1"), Ok(()));
        for id in ["", "has space", "a/b", &"x".repeat(65)] {
            assert!(good.validate(id).is_err(), "{id}");
        }
        for bad in [
            request(&[], &"a".repeat(64)),
            request(&["ftp://ha.local/x"], &"a".repeat(64)),
            request(&["not a url"], &"a".repeat(64)),
            request(&["http://ha.local/x"], "short"),
            request(&["http://ha.local/x"; 9], &"a".repeat(64)),
        ] {
            assert!(
                matches!(bad.validate("id"), Err(RelayError::Rejected(_))),
                "{bad:?}"
            );
        }
    }
}
