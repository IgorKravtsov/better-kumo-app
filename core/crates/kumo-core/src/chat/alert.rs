use serde_json::Value;

use super::engine::IncomingMessage;
use super::model::{ChannelCategory, TextSpan, parse_mentions};
use crate::space::{BroadcastKind, SessionChat, SpaceUpdate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

fn who(sender_name: Option<&str>) -> String {
    sender_name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Someone")
        .to_string()
}

fn one_line(text: &str) -> String {
    text.replace('\n', " ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
    pub channel_url: Option<String>,
}

pub fn message_alert(m: &IncomingMessage) -> Option<Alert> {
    let personal = matches!(m.category, ChannelCategory::Direct | ChannelCategory::Group);
    if m.channel_is_open || m.parent_id.is_some() || !(personal || m.mentions_me) {
        return None;
    }
    let body: String = parse_mentions(&m.text, |_| None)
        .into_iter()
        .map(|s| match s {
            TextSpan::Text(t) => t,
            TextSpan::Mention { display, .. } => display,
        })
        .collect::<String>()
        .replace('\n', " ");
    let title = match m.category {
        ChannelCategory::Direct => m.sender_name.clone(),
        _ => format!("{} · {}", m.sender_name, m.channel_title),
    };
    Some(Alert {
        kind: if m.mentions_me && !personal {
            AlertKind::Mention
        } else {
            AlertKind::Message
        },
        title,
        body: if body.trim().is_empty() {
            "Sent a file".into()
        } else {
            body
        },
        channel_url: Some(m.channel_url.clone()),
    })
}

pub fn office_alert(data: &Value, sender_name: Option<&str>) -> Option<Alert> {
    let kind = match data["notificationType"].as_str()? {
        "nudge" => AlertKind::Nudge,
        "doorKnock" => AlertKind::Knock,
        _ => return None,
    };
    let who = sender_name
        .map(String::from)
        .or_else(|| data["senderName"].as_str().map(String::from))
        .unwrap_or_else(|| "Someone".into());
    let (title, body) = match kind {
        AlertKind::Nudge => (format!("{who} nudged you"), "Kumospace".to_string()),
        _ => (
            format!("{who} is knocking"),
            "Someone wants to enter your room".to_string(),
        ),
    };
    Some(Alert {
        kind,
        title,
        body,
        channel_url: None,
    })
}

pub fn broadcast_alert(kind: BroadcastKind, sender_name: Option<&str>) -> Alert {
    Alert {
        kind: AlertKind::Broadcast,
        title: format!("{} started a broadcast", who(sender_name)),
        body: match kind {
            BroadcastKind::Screen => "Screen share",
            BroadcastKind::Camera => "Camera",
        }
        .into(),
        channel_url: None,
    }
}

pub fn session_message_alert(chat: SessionChat, sender_name: Option<&str>, text: &str) -> Alert {
    let who = who(sender_name);
    let (kind, title) = match chat {
        SessionChat::Public => (AlertKind::PublicChat, format!("{who} · Public Chat")),
        SessionChat::Nearby => (AlertKind::NearbyChat, format!("{who} · Nearby Chat")),
        SessionChat::Private => (AlertKind::Message, who),
    };
    Alert {
        kind,
        title,
        body: one_line(text),
        channel_url: None,
    }
}

pub fn floor_entry_alert(message: &str) -> Alert {
    Alert {
        kind: AlertKind::FloorEntry,
        title: one_line(message.trim()),
        body: "Kumospace".into(),
        channel_url: None,
    }
}

pub fn space_alert(update: &SpaceUpdate) -> Option<Alert> {
    match update {
        SpaceUpdate::Sound { data, sender_name } => office_alert(data, sender_name.as_deref()),
        SpaceUpdate::Broadcast {
            kind, sender_name, ..
        } => Some(broadcast_alert(*kind, sender_name.as_deref())),
        SpaceUpdate::SessionMessage {
            chat,
            sender_name,
            text,
            ..
        } => Some(session_message_alert(*chat, sender_name.as_deref(), text)),
        SpaceUpdate::FloorEntry { message } => Some(floor_entry_alert(message)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn incoming(category: ChannelCategory, mentions_me: bool) -> IncomingMessage {
        IncomingMessage {
            channel_url: "c".into(),
            channel_title: "general".into(),
            category,
            message_id: 1,
            parent_id: None,
            created_at: 0,
            sender_id: "amy".into(),
            sender_name: "Amy".into(),
            text: "hi".into(),
            mentions_me,
            channel_is_open: false,
        }
    }

    #[test]
    fn alerts_for_personal_and_mentions_only() {
        assert_eq!(
            message_alert(&incoming(ChannelCategory::Direct, false))
                .unwrap()
                .title,
            "Amy"
        );
        assert!(message_alert(&incoming(ChannelCategory::Channel, false)).is_none());
        let mention = message_alert(&incoming(ChannelCategory::Channel, true)).unwrap();
        assert_eq!(
            (mention.kind, mention.title.as_str()),
            (AlertKind::Mention, "Amy · general")
        );
        let reply = IncomingMessage {
            parent_id: Some(7),
            ..incoming(ChannelCategory::Direct, true)
        };
        assert!(message_alert(&reply).is_none());
    }

    #[test]
    fn maps_nudge_and_knock_and_ignores_other_sounds() {
        let alert = office_alert(&json!({"notificationType":"nudge"}), Some("Amy")).unwrap();
        assert_eq!(
            (alert.kind, alert.title.as_str(), alert.body.as_str()),
            (AlertKind::Nudge, "Amy nudged you", "Kumospace")
        );
        assert!(office_alert(&json!({"notificationType":"emoji"}), None).is_none());
        let knock = office_alert(&json!({"notificationType":"doorKnock"}), None).unwrap();
        assert_eq!(
            (knock.kind, knock.title.as_str(), knock.body.as_str()),
            (
                AlertKind::Knock,
                "Someone is knocking",
                "Someone wants to enter your room"
            )
        );
        let named = office_alert(
            &json!({"notificationType":"doorKnock","senderName":"Bo"}),
            None,
        )
        .unwrap();
        assert_eq!(named.title, "Bo is knocking");
    }

    #[test]
    fn broadcast_and_session_chat_alerts() {
        let screen = broadcast_alert(BroadcastKind::Screen, Some("Amy"));
        assert_eq!(
            (screen.kind, screen.title.as_str(), screen.body.as_str()),
            (
                AlertKind::Broadcast,
                "Amy started a broadcast",
                "Screen share"
            )
        );
        let camera = broadcast_alert(BroadcastKind::Camera, None);
        assert_eq!(
            (camera.title.as_str(), camera.body.as_str()),
            ("Someone started a broadcast", "Camera")
        );
        let public = session_message_alert(SessionChat::Public, Some("Amy"), "hello\nall");
        assert_eq!(
            (public.kind, public.title.as_str(), public.body.as_str()),
            (AlertKind::PublicChat, "Amy · Public Chat", "hello all")
        );
        let nearby = session_message_alert(SessionChat::Nearby, Some(" "), "hey");
        assert_eq!(
            (nearby.kind, nearby.title.as_str()),
            (AlertKind::NearbyChat, "Someone · Nearby Chat")
        );
        let private = session_message_alert(SessionChat::Private, Some("Bo"), "psst");
        assert_eq!(
            (private.kind, private.title.as_str(), private.body.as_str()),
            (AlertKind::Message, "Bo", "psst")
        );
        assert_eq!(
            space_alert(&SpaceUpdate::Broadcast {
                kind: BroadcastKind::Screen,
                host_id: "c1".into(),
                sender_name: Some("Amy".into()),
            }),
            Some(screen)
        );
        assert_eq!(space_alert(&SpaceUpdate::Changed), None);
        let entry = space_alert(&SpaceUpdate::FloorEntry {
            message: "Amy joined the Floor".into(),
        })
        .unwrap();
        assert_eq!(
            (entry.kind, entry.title.as_str(), entry.body.as_str()),
            (AlertKind::FloorEntry, "Amy joined the Floor", "Kumospace")
        );
    }
}
