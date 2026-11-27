use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use kumo_core::events::{EventKind, LoggedEvent, OfficeEvent};
use tokio::sync::watch;

const KEEP: usize = 1000;
pub const MAX_WAIT: Duration = Duration::from_secs(30);

pub struct EventLog {
    boot: String,
    buffer: Mutex<VecDeque<LoggedEvent>>,
    latest: watch::Sender<u64>,
}

impl Default for EventLog {
    fn default() -> Self {
        let mut bytes = [0u8; 8];
        let _ = getrandom::fill(&mut bytes);
        Self {
            boot: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            buffer: Mutex::new(VecDeque::new()),
            latest: watch::Sender::new(0),
        }
    }
}

impl EventLog {
    pub fn boot(&self) -> &str {
        &self.boot
    }

    pub fn latest(&self) -> u64 {
        *self.latest.borrow()
    }

    pub fn push(&self, event: OfficeEvent) -> Option<LoggedEvent> {
        let mut buffer = self.buffer.lock().ok()?;
        let logged = LoggedEvent {
            seq: self.latest() + 1,
            event,
        };
        let chat = logged.event.kind == EventKind::Message;
        buffer.push_back(logged.clone());
        // Office and chat events keep separate budgets so busy chat cannot evict office events.
        if buffer
            .iter()
            .filter(|e| (e.event.kind == EventKind::Message) == chat)
            .count()
            > KEEP
            && let Some(oldest) = buffer
                .iter()
                .position(|e| (e.event.kind == EventKind::Message) == chat)
        {
            buffer.remove(oldest);
        }
        self.latest.send_replace(logged.seq);
        Some(logged)
    }

    pub fn after(&self, boot: Option<&str>, after: Option<u64>) -> Vec<LoggedEvent> {
        let Some(after) = after else {
            return Vec::new();
        };
        let from = if boot == Some(self.boot.as_str()) {
            after
        } else {
            0
        };
        self.buffer
            .lock()
            .map(|b| b.iter().filter(|e| e.seq > from).cloned().collect())
            .unwrap_or_default()
    }

    pub async fn wait(
        &self,
        boot: Option<&str>,
        after: Option<u64>,
        wait: Duration,
    ) -> Vec<LoggedEvent> {
        let mut changes = self.latest.subscribe();
        let after = after.or(Some(self.latest()));
        let deadline = tokio::time::Instant::now() + wait.min(MAX_WAIT);
        loop {
            changes.borrow_and_update();
            let events = self.after(boot.or(Some(self.boot.as_str())), after);
            if !events.is_empty() {
                return events;
            }
            if tokio::time::timeout_at(deadline, changes.changed())
                .await
                .is_err()
            {
                return Vec::new();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::events::Who;

    fn nudge(name: &str) -> OfficeEvent {
        OfficeEvent::new(
            EventKind::Nudge,
            0,
            Who {
                user_id: name.to_lowercase(),
                name: name.into(),
            },
        )
    }

    #[tokio::test]
    async fn a_waiting_reader_gets_what_happens_next() {
        let log = std::sync::Arc::new(EventLog::default());
        log.push(nudge("Old"));
        let boot = log.boot().to_string();
        let start = log.latest();
        let reader = {
            let log = log.clone();
            let boot = boot.clone();
            tokio::spawn(async move {
                log.wait(Some(&boot), Some(start), Duration::from_secs(5))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        log.push(nudge("Amy"));
        let got = reader.await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].event.person.name, "Amy");
        assert_eq!(log.after(Some(&boot), None), []);
        assert_eq!(log.after(Some("restarted"), Some(99)).len(), 2);
        assert!(
            log.wait(Some(&boot), Some(log.latest()), Duration::from_millis(20))
                .await
                .is_empty()
        );
    }

    #[test]
    fn chat_messages_do_not_evict_office_events() {
        let log = EventLog::default();
        log.push(nudge("Amy"));
        for _ in 0..KEEP + 5 {
            log.push(OfficeEvent::new(EventKind::Message, 0, Who::default()));
        }
        let kept = log.after(Some(log.boot()), Some(0));
        assert_eq!(kept.len(), KEEP + 1);
        assert_eq!(kept[0].event.kind, EventKind::Nudge);
        assert_eq!(kept[1].seq, 7);
        for _ in 0..KEEP {
            log.push(nudge("Bo"));
        }
        let kept = log.after(Some(log.boot()), Some(0));
        assert_eq!(kept.len(), 2 * KEEP);
        assert!(kept.iter().all(|e| e.event.person.name != "Amy"));
    }
}
