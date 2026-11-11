use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

use crate::{AlertItem, AlertKind};

#[derive(Default)]
pub struct RelayState {
    enabled: AtomicBool,
}

impl RelayState {
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }
}

pub fn sound_alert(data: &Value, sender_name: Option<&str>) -> Option<AlertItem> {
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
    Some(AlertItem {
        kind,
        title,
        body,
        channel_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_nudge_and_ignores_other_sounds() {
        let alert = sound_alert(&json!({"notificationType":"nudge"}), Some("Amy")).unwrap();
        assert_eq!(
            (alert.kind, alert.title.as_str()),
            (AlertKind::Nudge, "Amy nudged you")
        );
        assert!(sound_alert(&json!({"notificationType":"emoji"}), None).is_none());
        let knock = sound_alert(&json!({"notificationType":"doorKnock"}), None).unwrap();
        assert_eq!(knock.title, "Someone is knocking");
    }
}
