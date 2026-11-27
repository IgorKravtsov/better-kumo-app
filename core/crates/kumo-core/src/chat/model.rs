use serde::{Deserialize, Serialize};

use super::markdown;
use crate::sendbird::SbMember;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MemberRow {
    pub user_id: String,
    pub nickname: String,
    pub profile_url: String,
    pub is_online: bool,
}

impl From<&SbMember> for MemberRow {
    fn from(m: &SbMember) -> Self {
        Self {
            user_id: m.user_id.clone(),
            nickname: m.nickname.clone(),
            profile_url: m.profile_url.clone(),
            is_online: m.is_online,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelCategory {
    Channel,
    Group,
    Direct,
    Other,
}

impl ChannelCategory {
    pub fn name(self) -> &'static str {
        match self {
            ChannelCategory::Channel => "channel",
            ChannelCategory::Group => "group",
            ChannelCategory::Direct => "direct",
            ChannelCategory::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRow {
    pub url: String,
    pub custom_type: String,
    pub raw_name: String,
    pub title: String,
    pub cover_url: String,
    pub member_count: u64,
    pub unread: u64,
    pub mentions: u64,
    pub frozen: bool,
    pub last_message_id: i64,
    pub last_message_at: i64,
    pub last_message_text: String,
    pub last_sender_name: String,
    pub members: Vec<MemberRow>,
}

impl ChannelRow {
    pub fn category(&self) -> ChannelCategory {
        match self.custom_type.as_str() {
            "custom" => ChannelCategory::Channel,
            "group_dms" => ChannelCategory::Group,
            "user" => ChannelCategory::Direct,
            _ => ChannelCategory::Other,
        }
    }

    pub fn other_members<'a>(
        &'a self,
        self_id: &'a str,
    ) -> impl Iterator<Item = &'a MemberRow> + 'a {
        self.members.iter().filter(move |m| m.user_id != self_id)
    }

    pub fn compute_title(&self, self_id: &str) -> String {
        match self.category() {
            ChannelCategory::Channel => self
                .raw_name
                .rsplit_once(" | ")
                .map(|(name, _)| name.to_string())
                .unwrap_or_else(|| self.raw_name.clone()),
            ChannelCategory::Direct | ChannelCategory::Group => {
                let names: Vec<&str> = self
                    .other_members(self_id)
                    .map(|m| m.nickname.as_str())
                    .collect();
                if names.is_empty() {
                    self.members
                        .first()
                        .map(|m| format!("{} (you)", m.nickname))
                        .unwrap_or_else(|| self.raw_name.clone())
                } else {
                    names.join(", ")
                }
            }
            ChannelCategory::Other => self.raw_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageState {
    Sent,
    Pending,
    Failed,
}

impl MessageState {
    pub fn from_code(code: i64) -> Self {
        match code {
            1 => Self::Pending,
            2 => Self::Failed,
            _ => Self::Sent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReactionRow {
    pub key: String,
    pub user_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub channel_url: String,
    pub id: i64,
    pub state: MessageState,
    pub kind: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub sender_id: String,
    pub sender_name: String,
    pub sender_avatar: String,
    pub text: String,
    pub parent_id: i64,
    pub reply_count: u64,
    pub reactions: Vec<ReactionRow>,
    pub file_url: String,
    pub file_name: String,
    pub file_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

pub const MAX_ATTACHMENTS: usize = 10;
pub const IMAGE_MAX_SIDE: u32 = 1600;
pub const IMAGE_JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextSpan {
    Text(String),
    Mention { user_id: String, display: String },
}

/// Splits text into plain text and the mentions the chat would render, so a mention token inside code, a link or a URL
/// stays as written. A user mention shows `name_of`, then the token's own `name=`, then the id.
pub fn parse_mentions(text: &str, name_of: impl Fn(&str) -> Option<String>) -> Vec<TextSpan> {
    let mut spans = Vec::new();
    let mut at = 0;
    for (range, m) in markdown::mentions(text) {
        if range.start > at {
            spans.push(TextSpan::Text(text[at..range.start].to_string()));
        }
        let display = match m.kind.as_str() {
            "user" => format!(
                "@{}",
                name_of(&m.user_id)
                    .or_else(|| (!m.name.is_empty()).then(|| m.name.clone()))
                    .unwrap_or_else(|| m.user_id.clone())
            ),
            other => format!("@{other}"),
        };
        spans.push(TextSpan::Mention {
            user_id: m.user_id,
            display,
        });
        at = range.end;
    }
    if at < text.len() {
        spans.push(TextSpan::Text(text[at..].to_string()));
    }
    spans
}

pub use super::markdown::mentions_user;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_by_category() {
        let mut row = ChannelRow {
            url: "u".into(),
            custom_type: "custom".into(),
            raw_name: "general | acme-office".into(),
            title: String::new(),
            cover_url: String::new(),
            member_count: 2,
            unread: 0,
            mentions: 0,
            frozen: false,
            last_message_id: 0,
            last_message_at: 0,
            last_message_text: String::new(),
            last_sender_name: String::new(),
            members: vec![
                MemberRow {
                    user_id: "me".into(),
                    nickname: "Me".into(),
                    ..Default::default()
                },
                MemberRow {
                    user_id: "a".into(),
                    nickname: "Amy".into(),
                    ..Default::default()
                },
            ],
        };
        assert_eq!(row.compute_title("me"), "general");
        row.custom_type = "group_dms".into();
        assert_eq!(row.compute_title("me"), "Amy");
        row.members.truncate(1);
        assert_eq!(row.compute_title("me"), "Me (you)");
    }

    #[test]
    fn parses_mentions() {
        let spans = parse_mentions(
            "hi @mention(type=user&id=u1) and @mention(type=channel)!",
            |id| (id == "u1").then(|| "Amy".to_string()),
        );
        assert_eq!(
            spans,
            vec![
                TextSpan::Text("hi ".into()),
                TextSpan::Mention {
                    user_id: "u1".into(),
                    display: "@Amy".into()
                },
                TextSpan::Text(" and ".into()),
                TextSpan::Mention {
                    user_id: String::new(),
                    display: "@channel".into()
                },
                TextSpan::Text("!".into()),
            ]
        );
        assert!(mentions_user("x @mention(type=user&id=u1)", "u1"));
        assert!(!mentions_user("x @mention(type=user&id=u2)", "u1"));
    }
}
