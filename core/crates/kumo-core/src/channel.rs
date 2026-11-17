#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelKind {
    DirectMessage { user_ids: Vec<String> },
    GroupDm { id: String },
    Custom { id: String },
    Nearby { owner_uid: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelUrl {
    pub space_uid: String,
    pub kind: ChannelKind,
}

const GROUP_DMS: &str = "group_dms_";
const CUSTOM: &str = "custom_";
const NEARBY: &str = "nearby_";

impl ChannelUrl {
    pub fn direct_message(space_uid: &str, user_ids: [&str; 2]) -> String {
        let mut ids = user_ids;
        ids.sort_unstable();
        format!("{space_uid}_{}_{}", ids[0], ids[1])
    }

    pub fn nearby_prefix(space_uid: &str) -> String {
        format!("{space_uid}_{NEARBY}")
    }

    pub fn parse(url: &str, space_uid: &str) -> Self {
        let kind = url
            .strip_prefix(space_uid)
            .and_then(|rest| rest.strip_prefix('_'))
            .map(classify)
            .unwrap_or(ChannelKind::Unknown);
        Self {
            space_uid: space_uid.to_string(),
            kind,
        }
    }
}

fn classify(rest: &str) -> ChannelKind {
    if let Some(id) = rest.strip_prefix(GROUP_DMS) {
        return ChannelKind::GroupDm { id: id.to_string() };
    }
    if let Some(id) = rest.strip_prefix(CUSTOM) {
        return ChannelKind::Custom { id: id.to_string() };
    }
    if let Some(owner) = rest.strip_prefix(NEARBY) {
        return ChannelKind::Nearby {
            owner_uid: owner.to_string(),
        };
    }
    match rest.split_once('_') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() && !b.contains('_') => {
            ChannelKind::DirectMessage {
                user_ids: vec![a.to_string(), b.to_string()],
            }
        }
        _ => ChannelKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_message_url_is_order_independent() {
        assert_eq!(
            ChannelUrl::direct_message("sp", ["zed", "amy"]),
            "sp_amy_zed"
        );
        assert_eq!(
            ChannelUrl::direct_message("sp", ["amy", "zed"]),
            "sp_amy_zed"
        );
    }

    #[test]
    fn classifies_every_channel_type() {
        assert_eq!(
            ChannelUrl::parse("sp_amy_zed", "sp").kind,
            ChannelKind::DirectMessage {
                user_ids: vec!["amy".into(), "zed".into()]
            }
        );
        assert_eq!(
            ChannelUrl::parse("sp_group_dms_0a1b2c3d4e5f", "sp").kind,
            ChannelKind::GroupDm {
                id: "0a1b2c3d4e5f".into()
            }
        );
        assert_eq!(
            ChannelUrl::parse("sp_custom_0a1b2c3d4e5f", "sp").kind,
            ChannelKind::Custom {
                id: "0a1b2c3d4e5f".into()
            }
        );
        assert_eq!(
            ChannelUrl::parse("sp_nearby_amy", "sp").kind,
            ChannelKind::Nearby {
                owner_uid: "amy".into()
            }
        );
        assert_eq!(
            ChannelUrl::parse("other_amy_zed", "sp").kind,
            ChannelKind::Unknown
        );
    }
}
