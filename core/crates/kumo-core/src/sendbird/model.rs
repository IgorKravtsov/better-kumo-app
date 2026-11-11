use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(from = "RawUser")]
pub struct SbUser {
    pub user_id: String,
    pub nickname: String,
    pub profile_url: String,
    pub is_online: Option<bool>,
    pub last_seen_at: Option<i64>,
}

#[derive(Deserialize, Default)]
struct RawUser {
    user_id: Option<String>,
    guest_id: Option<String>,
    nickname: Option<String>,
    name: Option<String>,
    profile_url: Option<String>,
    image: Option<String>,
    is_online: Option<bool>,
    last_seen_at: Option<i64>,
}

impl From<RawUser> for SbUser {
    fn from(r: RawUser) -> Self {
        Self {
            user_id: r.user_id.or(r.guest_id).unwrap_or_default(),
            nickname: r.nickname.or(r.name).unwrap_or_default(),
            profile_url: r.profile_url.or(r.image).unwrap_or_default(),
            is_online: r.is_online,
            last_seen_at: r.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbThreadInfo {
    #[serde(default)]
    pub reply_count: u64,
    #[serde(default)]
    pub last_replied_at: i64,
    #[serde(default)]
    pub unread_message_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbReaction {
    pub key: String,
    #[serde(default)]
    pub user_ids: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbFile {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub mime_type: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbMessage {
    #[serde(default)]
    message_id: Option<i64>,
    #[serde(default)]
    msg_id: Option<i64>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub channel_url: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub user: Option<SbUser>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub custom_type: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub mention_type: Option<String>,
    #[serde(default)]
    pub mentioned_users: Vec<SbUser>,
    #[serde(default)]
    pub parent_message_id: Option<i64>,
    #[serde(default)]
    pub thread_info: Option<SbThreadInfo>,
    #[serde(default)]
    pub reactions: Vec<SbReaction>,
    #[serde(default)]
    pub file: Option<SbFile>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub req_id: Option<String>,
    #[serde(default)]
    ts: Option<i64>,
}

impl SbMessage {
    pub fn normalized(mut self) -> Self {
        if self.created_at == 0 {
            self.created_at = self.ts.unwrap_or_default();
        }
        if self.updated_at == 0 {
            self.updated_at = self.created_at;
        }
        self
    }

    pub fn id(&self) -> i64 {
        self.message_id.or(self.msg_id).unwrap_or_default()
    }

    pub fn sender_id(&self) -> Option<&str> {
        self.user.as_ref().map(|u| u.user_id.as_str())
    }

    pub fn is_thread_reply(&self) -> bool {
        self.parent_message_id.is_some_and(|id| id > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbMember {
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub profile_url: String,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SbChannel {
    pub channel_url: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cover_url: String,
    #[serde(default)]
    pub custom_type: String,
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub freeze: bool,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub member_count: u64,
    #[serde(default)]
    pub unread_message_count: u64,
    #[serde(default)]
    pub unread_mention_count: u64,
    #[serde(default)]
    pub last_message: Option<SbMessage>,
    #[serde(default)]
    pub members: Vec<SbMember>,
    #[serde(default)]
    pub metadata: Option<Value>,
}

impl SbChannel {
    pub fn last_activity(&self) -> i64 {
        self.last_message
            .as_ref()
            .map(|m| m.created_at)
            .unwrap_or(self.created_at)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ChannelPage {
    #[serde(default)]
    pub channels: Vec<SbChannel>,
    #[serde(default)]
    pub next: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct MessagePage {
    #[serde(default)]
    pub messages: Vec<SbMessage>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum DeletedItem {
    Id(i64),
    Url(String),
    Object(Value),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct MessageChangelog {
    #[serde(default)]
    pub updated: Vec<SbMessage>,
    #[serde(default)]
    pub deleted: Vec<DeletedItem>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub next: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct ChannelChangelog {
    #[serde(default)]
    pub updated: Vec<SbChannel>,
    #[serde(default)]
    pub deleted: Vec<DeletedItem>,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub next: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct LoginInfo {
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub ekey: String,
    #[serde(default)]
    pub ping_interval: Option<u64>,
    #[serde(default)]
    pub pong_timeout: Option<u64>,
    #[serde(default)]
    pub login_ts: i64,
    #[serde(default)]
    pub reconnect: Option<ReconnectPolicy>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReconnectPolicy {
    #[serde(default)]
    pub interval: f64,
    #[serde(default)]
    pub max_interval: f64,
    #[serde(default)]
    pub retry_cnt: i64,
    #[serde(default)]
    pub mul: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_id_from_rest_or_socket() {
        let rest: SbMessage =
            serde_json::from_str(r#"{"message_id":5,"type":"MESG","message":"hi"}"#).unwrap();
        let ws: SbMessage = serde_json::from_str(r#"{"msg_id":7,"type":"MESG"}"#).unwrap();
        assert_eq!(rest.id(), 5);
        assert_eq!(ws.id(), 7);
    }

    #[test]
    fn ack_time_comes_from_ts() {
        let ack: SbMessage = serde_json::from_str(r#"{"msg_id":3,"ts":1700}"#).unwrap();
        assert_eq!(ack.normalized().created_at, 1700);
    }

    #[test]
    fn socket_user_format() {
        let ws: SbMessage = serde_json::from_str(
            r#"{"msg_id":7,"user":{"guest_id":"u1","name":"Amy","image":"https://a/p.png"},"mentioned_users":[{"guest_id":"u2","name":"Bo"}]}"#,
        )
        .unwrap();
        let user = ws.user.unwrap();
        assert_eq!(
            (user.user_id.as_str(), user.nickname.as_str()),
            ("u1", "Amy")
        );
        assert_eq!(ws.mentioned_users[0].user_id, "u2");
    }

    #[test]
    fn channel_activity_falls_back_to_creation() {
        let ch: SbChannel = serde_json::from_str(r#"{"channel_url":"c","created_at":3}"#).unwrap();
        assert_eq!(ch.last_activity(), 3);
    }
}
