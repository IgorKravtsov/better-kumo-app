use super::engine::IncomingMessage;
use super::model::{ChannelCategory, TextSpan, parse_mentions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    Message,
    Mention,
    Nudge,
    Knock,
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
    if m.channel_is_open || !(personal || m.mentions_me) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming(category: ChannelCategory, mentions_me: bool) -> IncomingMessage {
        IncomingMessage {
            channel_url: "c".into(),
            channel_title: "general".into(),
            category,
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
    }
}
