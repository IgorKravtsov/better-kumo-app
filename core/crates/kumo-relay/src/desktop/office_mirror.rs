use std::sync::Mutex;

use kumo_core::space::{CallCredentials, SpaceState, SpaceUpdate};
use kumo_core::ws::ServerOp;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::sync::futures::Notified;

#[derive(Default)]
pub struct OfficeMirror {
    office: Mutex<SpaceState>,
    call_ready: Notify,
}

impl OfficeMirror {
    pub fn observe(&self, frame: &str) -> Vec<SpaceUpdate> {
        let Ok(op) = ServerOp::parse(frame) else {
            return Vec::new();
        };
        let updates = self
            .office
            .lock()
            .map(|mut office| office.apply(op))
            .unwrap_or_default();
        if updates
            .iter()
            .any(|u| matches!(u, SpaceUpdate::Call(Some(_))))
        {
            self.call_ready.notify_waiters();
        }
        updates
    }

    pub fn reset(&self) {
        if let Ok(mut office) = self.office.lock() {
            *office = SpaceState::default();
        }
    }

    pub fn call(&self) -> Option<CallCredentials> {
        self.office.lock().ok().and_then(|o| o.call.clone())
    }

    pub fn call_ready(&self) -> Notified<'_> {
        self.call_ready.notified()
    }

    pub fn sender_name(&self, data: &Value) -> Option<String> {
        self.office.lock().ok().and_then(|o| o.sender_name(data))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn room_frame_fills_the_call_credentials() {
        let mirror = OfficeMirror::default();
        let waiter = mirror.call_ready();
        mirror.observe(r#"{"op":"playSound","data":{"notificationType":"nudge"}}"#);
        assert!(mirror.call().is_none());
        mirror.observe(
            r#"{"op":"room","data":{"roomName":"f1","dailyUrl":"https://x.daily.co/r","dailyToken":"fresh"}}"#,
        );
        tokio::time::timeout(Duration::from_millis(100), waiter)
            .await
            .expect("waiters are woken");
        let call = mirror.call().unwrap();
        assert_eq!(
            (call.daily_url.as_str(), call.daily_token.as_str()),
            ("https://x.daily.co/r", "fresh")
        );
    }

    #[test]
    fn names_senders_it_has_seen() {
        let mirror = OfficeMirror::default();
        mirror.observe(r#"{"op":"census","data":{"c1":{"firebaseUid":"u1","name":"Amy"}}}"#);
        assert_eq!(
            mirror.sender_name(&serde_json::json!({"fromUserId":"c1"})),
            Some("Amy".into())
        );
        mirror.reset();
        assert_eq!(
            mirror.sender_name(&serde_json::json!({"fromUserId":"c1"})),
            None
        );
    }
}
