use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api::RelayError;
use super::page::PageError;
use crate::ws::{ClientOp, SessionChatData};

const NEARBY: &str = "nearby";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyChat {
    pub zone_id: Option<String>,
    pub zone_name: String,
    pub messages: Vec<NearbyMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearbyMessage {
    pub id: String,
    pub sender_id: String,
    pub sender_name: String,
    pub text: String,
    pub mine: bool,
    pub sent_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearbyChatRequest {
    pub message: String,
}

impl NearbyChatRequest {
    pub fn text(&self) -> Result<&str, RelayError> {
        let text = self.message.trim();
        if text.is_empty() {
            return Err(RelayError::Rejected("message is empty".into()));
        }
        Ok(text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyRoom {
    self_id: String,
    floor_id: Option<String>,
    zone_id: Option<String>,
    zone_name: String,
    participant_ids: Vec<String>,
    messages: Vec<PageMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageNearby {
    self_id: String,
    floor_id: Option<String>,
    zone_id: Option<String>,
    zone_name: Option<String>,
    #[serde(default)]
    participant_ids: Vec<String>,
    #[serde(default)]
    messages: Vec<PageMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageMessage {
    id: Option<String>,
    sender_id: Option<String>,
    sender_name: Option<String>,
    message: Option<String>,
    zone_id: Option<String>,
    at: Option<i64>,
}

impl NearbyRoom {
    pub fn from_page(value: Value) -> Result<Self, PageError> {
        if value.is_null() {
            return Err(PageError::NotInSpace);
        }
        let page: PageNearby = serde_json::from_value(value).map_err(|e| PageError::Malformed {
            what: "Nearby Chat",
            detail: e.to_string(),
        })?;
        Ok(Self {
            self_id: page.self_id,
            floor_id: page.floor_id.filter(|f| !f.is_empty()),
            zone_id: page.zone_id.filter(|z| !z.is_empty()),
            zone_name: page.zone_name.unwrap_or_default(),
            participant_ids: page.participant_ids,
            messages: page.messages,
        })
    }

    pub fn outgoing(&self, text: &str) -> ClientOp {
        let mut participant_ids = self.participant_ids.clone();
        participant_ids.sort();
        participant_ids.dedup();
        ClientOp::Chat(SessionChatData {
            message: text.to_string(),
            range: NEARBY.into(),
            participant_ids,
            zone_id: self.zone_id.clone(),
        })
    }

    pub fn floor_id(&self) -> Option<&str> {
        self.floor_id.as_deref()
    }

    pub fn chat(&self) -> NearbyChat {
        let mut seen = HashSet::new();
        let messages = self
            .messages
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.zone_id.as_deref().filter(|z| !z.is_empty()) == self.zone_id.as_deref()
            })
            .filter_map(|(index, m)| {
                let text = m.message.clone().filter(|t| !t.is_empty())?;
                let id =
                    m.id.clone()
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| format!("#{index}"));
                if !seen.insert(id.clone()) {
                    return None;
                }
                let sender_id = m.sender_id.clone().unwrap_or_default();
                Some(NearbyMessage {
                    id,
                    mine: sender_id == self.self_id,
                    sender_name: m.sender_name.clone().unwrap_or_default(),
                    sender_id,
                    text,
                    sent_at_ms: m.at.unwrap_or_default(),
                })
            })
            .collect();
        NearbyChat {
            zone_id: self.zone_id.clone(),
            zone_name: self.zone_name.clone(),
            messages,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn room() -> NearbyRoom {
        NearbyRoom::from_page(json!({
            "selfId": "c-me",
            "floorId": "floor-1",
            "zoneId": "zone-a",
            "zoneName": "Blue Room",
            "participantIds": ["uid-me", "uid-amy", "uid-amy"],
            "messages": [
                { "id": "m1", "senderId": "c-amy", "senderName": "Amy", "message": "Coffee?", "zoneId": "zone-a", "at": 10 },
                { "id": "m2", "senderId": "c-bo", "senderName": "Bo", "message": "Other room", "zoneId": "zone-b", "at": 11 },
                { "id": "m3", "senderId": "c-me", "senderName": "Me", "message": "Yes", "zoneId": "zone-a", "at": 12 },
                { "id": "m4", "senderId": "c-sam", "message": "Out in the open", "at": 13 },
                { "id": "m5", "senderId": "c-amy", "message": "", "zoneId": "zone-a" }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn the_chat_keeps_only_messages_of_the_desktop_zone() {
        let chat = room().chat();
        assert_eq!(chat.zone_name, "Blue Room");
        let ids: Vec<_> = chat.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m3"]);
        assert!(!chat.messages[0].mine && chat.messages[1].mine);
        assert_eq!(chat.messages[1].sent_at_ms, 12);
    }

    #[test]
    fn outside_a_zone_the_chat_keeps_zoneless_messages() {
        let mut room = room();
        room.zone_id = None;
        let ids: Vec<_> = room.chat().messages.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["m4"]);
        assert!(!room.outgoing("hi").to_frame().contains("zoneId"));
    }

    #[test]
    fn outgoing_message_is_the_web_apps_nearby_chat_frame() {
        let frame: Value = serde_json::from_str(&room().outgoing("hello").to_frame()).unwrap();
        assert_eq!(
            frame,
            json!({
                "op": "chat",
                "data": {
                    "message": "hello",
                    "range": "nearby",
                    "participantIds": ["uid-amy", "uid-me"],
                    "zoneId": "zone-a"
                }
            })
        );
    }

    #[test]
    fn every_message_gets_a_unique_id() {
        let room = NearbyRoom::from_page(json!({
            "selfId": "c-me",
            "zoneId": "zone-a",
            "messages": [
                { "senderId": "c-amy", "message": "one", "zoneId": "zone-a" },
                { "id": "", "senderId": "c-amy", "message": "two", "zoneId": "zone-a" },
                { "id": "m1", "senderId": "c-amy", "message": "three", "zoneId": "zone-a" },
                { "id": "m1", "senderId": "c-amy", "message": "again", "zoneId": "zone-a" }
            ]
        }))
        .unwrap();
        let ids: Vec<_> = room.chat().messages.into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["#0", "#1", "m1"]);
    }

    #[test]
    fn blank_messages_are_rejected() {
        let request = |m: &str| NearbyChatRequest { message: m.into() };
        assert!(matches!(request("  ").text(), Err(RelayError::Rejected(_))));
        assert_eq!(request(" hi ").text(), Ok("hi"));
        assert_eq!(
            NearbyRoom::from_page(Value::Null),
            Err(PageError::NotInSpace)
        );
    }
}
