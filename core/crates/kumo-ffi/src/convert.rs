use kumo_core::AuthSession;
use kumo_core::chat::markdown::{self, Inline};
use kumo_core::chat::{
    ChannelCategory, ChannelRow, ChatEvent, ConnectionState, MemberRow, MessageRow, MessageState,
    TextSpan, parse_mentions,
};
use serde_json::Value;

#[derive(Debug, Clone, uniffi::Record)]
pub struct UserInfo {
    pub uid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

impl From<&AuthSession> for UserInfo {
    fn from(s: &AuthSession) -> Self {
        Self {
            uid: s.uid.clone(),
            email: s.email.clone(),
            display_name: s.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SpaceInfo {
    pub name: String,
    pub uid: String,
    pub display_name: String,
    pub member_count: u64,
}

impl SpaceInfo {
    pub fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            name: v["name"].as_str()?.to_string(),
            uid: v["uid"].as_str()?.to_string(),
            display_name: v["displayName"]
                .as_str()
                .or_else(|| v["name"].as_str())
                .unwrap_or_default()
                .to_string(),
            member_count: v["memberCount"].as_u64().unwrap_or_default(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChannelType {
    Channel,
    Group,
    Direct,
    Other,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ChannelItem {
    pub url: String,
    pub title: String,
    pub channel_type: ChannelType,
    pub avatar_url: String,
    pub is_online: bool,
    pub unread: u64,
    pub mentions: u64,
    pub member_count: u64,
    pub frozen: bool,
    pub last_message_at: i64,
    pub last_message_text: String,
    pub last_sender_name: String,
}

impl ChannelItem {
    pub fn from_row(row: &ChannelRow, self_id: &str) -> Self {
        let channel_type = match row.category() {
            ChannelCategory::Channel => ChannelType::Channel,
            ChannelCategory::Group => ChannelType::Group,
            ChannelCategory::Direct => ChannelType::Direct,
            ChannelCategory::Other => ChannelType::Other,
        };
        let other = row.other_members(self_id).next();
        let (avatar_url, is_online) = match channel_type {
            ChannelType::Direct => other
                .map(|m| (m.profile_url.clone(), m.is_online))
                .unwrap_or_default(),
            _ => (row.cover_url.clone(), false),
        };
        Self {
            url: row.url.clone(),
            title: if row.title.is_empty() {
                row.compute_title(self_id)
            } else {
                row.title.clone()
            },
            channel_type,
            avatar_url,
            is_online,
            unread: row.unread,
            mentions: row.mentions,
            member_count: row.member_count,
            frozen: row.frozen,
            last_message_at: row.last_message_at,
            last_message_text: plain_text(&row.last_message_text, &row.members),
            last_sender_name: row.last_sender_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SendState {
    Sent,
    Pending,
    Failed,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub highlight: bool,
    pub link: Option<String>,
    pub mention_user_id: Option<String>,
}

pub struct MessageContext<'a> {
    pub self_id: &'a str,
    pub members: &'a [MemberRow],
    pub file_url: &'a dyn Fn(&str) -> String,
}

fn spans_of(parsed: &markdown::Parsed, members: &[MemberRow]) -> Vec<Span> {
    parsed
        .inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { text, style } => Span {
                text: text.clone(),
                bold: style.bold,
                italic: style.italic,
                strike: style.strike,
                code: style.code,
                highlight: style.highlight,
                link: None,
                mention_user_id: None,
            },
            Inline::Link { text, url, style } => Span {
                text: text.clone(),
                bold: style.bold,
                italic: style.italic,
                strike: style.strike,
                code: style.code,
                highlight: style.highlight,
                link: Some(url.clone()),
                mention_user_id: None,
            },
            Inline::Mention { user_id, kind } => Span {
                text: if kind == "user" {
                    format!(
                        "@{}",
                        name_of(members, user_id).unwrap_or_else(|| "unknown".into())
                    )
                } else {
                    format!("@{kind}")
                },
                bold: true,
                italic: false,
                strike: false,
                code: false,
                highlight: false,
                link: None,
                mention_user_id: Some(user_id.clone()),
            },
        })
        .collect()
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReactionItem {
    pub key: String,
    pub count: u32,
    pub mine: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MessageItem {
    pub id: i64,
    pub channel_url: String,
    pub state: SendState,
    pub kind: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub sender_id: String,
    pub sender_name: String,
    pub sender_avatar: String,
    pub is_mine: bool,
    pub text: String,
    pub spans: Vec<Span>,
    pub images: Vec<String>,
    pub parent_id: i64,
    pub reply_count: u64,
    pub reactions: Vec<ReactionItem>,
    pub file_url: String,
    pub file_name: String,
    pub file_type: String,
}

impl MessageItem {
    pub fn from_row(row: &MessageRow, ctx: &MessageContext<'_>) -> Self {
        let self_id = ctx.self_id;
        let is_file = row.kind == "FILE";
        let parsed = if is_file && row.text == row.file_name {
            markdown::Parsed::default()
        } else {
            markdown::parse(&row.text)
        };
        let spans = spans_of(&parsed, ctx.members);
        Self {
            id: row.id,
            channel_url: row.channel_url.clone(),
            state: match row.state {
                MessageState::Sent => SendState::Sent,
                MessageState::Pending => SendState::Pending,
                MessageState::Failed => SendState::Failed,
            },
            kind: row.kind.clone(),
            created_at: row.created_at,
            updated_at: row.updated_at,
            sender_id: row.sender_id.clone(),
            sender_name: row.sender_name.clone(),
            sender_avatar: row.sender_avatar.clone(),
            is_mine: row.sender_id == self_id,
            text: spans.iter().map(|s| s.text.as_str()).collect(),
            spans,
            parent_id: row.parent_id,
            reply_count: row.reply_count,
            reactions: row
                .reactions
                .iter()
                .filter(|r| !r.user_ids.is_empty())
                .map(|r| ReactionItem {
                    key: r.key.clone(),
                    count: r.user_ids.len() as u32,
                    mine: r.user_ids.iter().any(|u| u == self_id),
                })
                .collect(),
            images: parsed.images.clone(),
            file_url: (ctx.file_url)(&row.file_url),
            file_name: row.file_name.clone(),
            file_type: row.file_type.clone(),
        }
    }
}

fn name_of(members: &[MemberRow], id: &str) -> Option<String> {
    members
        .iter()
        .find(|m| m.user_id == id)
        .map(|m| m.nickname.clone())
}

fn plain_text(text: &str, members: &[MemberRow]) -> String {
    parse_mentions(&kumo_core::chat::markdown::summarize_images(text), |id| {
        name_of(members, id)
    })
    .into_iter()
    .map(|s| match s {
        TextSpan::Text(t) => t,
        TextSpan::Mention { display, .. } => display,
    })
    .collect::<String>()
    .replace('\n', " ")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectionStatus {
    Offline,
    Connecting,
    Syncing,
    Online,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum AppEvent {
    ChannelsChanged,
    MessagesChanged {
        channel_url: String,
    },
    Typing {
        channel_url: String,
        user_names: Vec<String>,
    },
    Connection {
        status: ConnectionStatus,
    },
    Error {
        message: String,
    },
    Alert {
        alert: AlertItem,
    },
    SpaceChanged,
    CallChanged,
    Notice {
        message: String,
    },
    SpaceError {
        code: String,
        message: String,
    },
    DesktopChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AlertKind {
    Message,
    Mention,
    Nudge,
    Knock,
    Broadcast,
    PublicChat,
    NearbyChat,
    FloorEntry,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AlertItem {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    pub channel_url: Option<String>,
    pub person_id: Option<String>,
}

pub fn alert_item(a: kumo_core::chat::alert::Alert) -> AlertItem {
    AlertItem {
        kind: match a.kind {
            kumo_core::chat::alert::AlertKind::Message => AlertKind::Message,
            kumo_core::chat::alert::AlertKind::Mention => AlertKind::Mention,
            kumo_core::chat::alert::AlertKind::Nudge => AlertKind::Nudge,
            kumo_core::chat::alert::AlertKind::Knock => AlertKind::Knock,
            kumo_core::chat::alert::AlertKind::Broadcast => AlertKind::Broadcast,
            kumo_core::chat::alert::AlertKind::PublicChat => AlertKind::PublicChat,
            kumo_core::chat::alert::AlertKind::NearbyChat => AlertKind::NearbyChat,
            kumo_core::chat::alert::AlertKind::FloorEntry => AlertKind::FloorEntry,
        },
        title: a.title,
        body: a.body,
        channel_url: a.channel_url,
        person_id: a.person_id,
    }
}

impl From<ChatEvent> for AppEvent {
    fn from(e: ChatEvent) -> Self {
        match e {
            ChatEvent::ChannelsChanged => AppEvent::ChannelsChanged,
            ChatEvent::MessagesChanged { channel_url } => AppEvent::MessagesChanged { channel_url },
            ChatEvent::Typing {
                channel_url,
                user_names,
            } => AppEvent::Typing {
                channel_url,
                user_names,
            },
            ChatEvent::Connection(state) => AppEvent::Connection {
                status: match state {
                    ConnectionState::Offline => ConnectionStatus::Offline,
                    ConnectionState::Connecting => ConnectionStatus::Connecting,
                    ConnectionState::Syncing => ConnectionStatus::Syncing,
                    ConnectionState::Online => ConnectionStatus::Online,
                },
            },
            ChatEvent::Error(message) => AppEvent::Error { message },
            ChatEvent::Incoming(_) => AppEvent::ChannelsChanged,
        }
    }
}
