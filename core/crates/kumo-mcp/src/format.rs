use std::collections::HashMap;

use kumo_core::chat::{
    ChannelCategory, ChannelRow, DirectoryUser, MessageRow, MessageState, TextSpan, parse_mentions,
};
use serde_json::{Value, json};

pub struct Renderer<'a> {
    pub directory: &'a HashMap<String, DirectoryUser>,
    pub self_id: &'a str,
}

impl Renderer<'_> {
    pub fn name_of(&self, user_id: &str) -> Option<String> {
        self.directory
            .get(user_id)
            .map(|u| u.name.clone())
            .filter(|n| !n.is_empty())
    }

    pub fn text(&self, text: &str) -> String {
        parse_mentions(text, |id| self.name_of(id))
            .into_iter()
            .map(|span| match span {
                TextSpan::Text(t) => t,
                TextSpan::Mention { display, .. } => display,
            })
            .collect()
    }

    pub fn message(&self, m: &MessageRow) -> Value {
        let mut out = json!({
            "id": m.id,
            "time": iso_time(m.created_at),
            "from": if m.sender_name.is_empty() {
                self.name_of(&m.sender_id).unwrap_or_default()
            } else {
                m.sender_name.clone()
            },
            "from_id": m.sender_id,
        });
        let fields = out.as_object_mut().expect("object");
        if m.sender_id == self.self_id {
            fields.insert("mine".into(), json!(true));
        }
        if m.kind == "FILE" {
            fields.insert(
                "file".into(),
                json!({ "name": m.file_name, "type": m.file_type }),
            );
            if !m.text.is_empty() && m.text != m.file_name {
                fields.insert("text".into(), json!(self.text(&m.text)));
            }
        } else {
            fields.insert("text".into(), json!(self.text(&m.text)));
        }
        if m.kind == "ADMM" {
            fields.insert("system".into(), json!(true));
        }
        if m.parent_id > 0 {
            fields.insert("reply_to".into(), json!(m.parent_id));
        }
        if m.reply_count > 0 {
            fields.insert("thread_replies".into(), json!(m.reply_count));
        }
        if !m.reactions.is_empty() {
            let reactions: Vec<Value> = m
                .reactions
                .iter()
                .filter(|r| !r.user_ids.is_empty())
                .map(|r| {
                    json!({
                        "emoji": r.key,
                        "by": r.user_ids.iter().map(|id| self.name_of(id).unwrap_or_else(|| id.clone())).collect::<Vec<_>>(),
                    })
                })
                .collect();
            if !reactions.is_empty() {
                fields.insert("reactions".into(), json!(reactions));
            }
        }
        match m.state {
            MessageState::Sent => {}
            MessageState::Pending => {
                fields.insert("state".into(), json!("pending"));
            }
            MessageState::Failed => {
                fields.insert("state".into(), json!("failed"));
            }
        }
        out
    }

    pub fn channel(&self, c: &ChannelRow) -> Value {
        let mut out = json!({
            "channel": c.url,
            "name": c.title,
            "kind": kind_name(c.category()),
            "unread": c.unread,
        });
        let fields = out.as_object_mut().expect("object");
        if c.mentions > 0 {
            fields.insert("unread_mentions".into(), json!(c.mentions));
        }
        if c.last_message_at > 0 {
            fields.insert("last_activity".into(), json!(iso_time(c.last_message_at)));
            fields.insert(
                "last_message".into(),
                json!(format!(
                    "{}: {}",
                    c.last_sender_name,
                    one_line(&self.text(&c.last_message_text), 140)
                )),
            );
        }
        match c.category() {
            ChannelCategory::Direct => {
                if let Some(other) = c.other_members(self.self_id).next() {
                    fields.insert("user_id".into(), json!(other.user_id));
                } else {
                    fields.insert("is_self".into(), json!(true));
                }
            }
            ChannelCategory::Group => {
                fields.insert(
                    "members".into(),
                    json!(
                        c.other_members(self.self_id)
                            .map(|m| json!({"name": m.nickname, "user_id": m.user_id}))
                            .collect::<Vec<_>>()
                    ),
                );
            }
            ChannelCategory::Channel | ChannelCategory::Other => {
                fields.insert("member_count".into(), json!(c.member_count));
            }
        }
        out
    }
}

pub fn kind_name(category: ChannelCategory) -> &'static str {
    match category {
        ChannelCategory::Direct => "direct",
        ChannelCategory::Group => "group",
        ChannelCategory::Channel => "channel",
        ChannelCategory::Other => "other",
    }
}

pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

pub fn encode_mentions(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        let Some(len) = rest[start + 2..].find('>') else {
            break;
        };
        let inner = &rest[start + 2..start + 2 + len];
        let id = inner.split('|').next().unwrap_or_default();
        out.push_str(&rest[..start]);
        if id.is_empty() || id.contains(char::is_whitespace) {
            out.push_str(&rest[start..start + 3 + len]);
        } else {
            out.push_str(&format!("@mention(type=user&id={id})"));
        }
        rest = &rest[start + 3 + len..];
    }
    out.push_str(rest);
    out
}

pub fn iso_time(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_times() {
        assert_eq!(iso_time(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_time(1_795_000_000_123), "2026-11-18T11:06:40Z");
        assert_eq!(iso_time(951_782_400_000), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn encodes_slack_style_mentions() {
        assert_eq!(
            encode_mentions("hi <@u1> and <@u2|Amy>!"),
            "hi @mention(type=user&id=u1) and @mention(type=user&id=u2)!"
        );
        assert_eq!(encode_mentions("a <@ b> c"), "a <@ b> c");
        assert_eq!(encode_mentions("x <@open"), "x <@open");
    }

    #[test]
    fn renders_mentions_with_names() {
        let directory = HashMap::from([(
            "u1".to_string(),
            DirectoryUser {
                user_id: "u1".into(),
                name: "Amy".into(),
                avatar_url: String::new(),
            },
        )]);
        let renderer = Renderer {
            directory: &directory,
            self_id: "me",
        };
        assert_eq!(renderer.text("ping @mention(type=user&id=u1)"), "ping @Amy");
    }
}
