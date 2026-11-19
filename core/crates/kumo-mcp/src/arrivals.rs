use std::collections::VecDeque;
use std::sync::Mutex;

use kumo_core::chat::IncomingMessage;
use tokio::sync::watch;

const KEEP: usize = 500;

pub struct Arrivals {
    buffer: Mutex<VecDeque<(u64, IncomingMessage)>>,
    latest: watch::Sender<u64>,
}

impl Default for Arrivals {
    fn default() -> Self {
        Self {
            buffer: Mutex::new(VecDeque::new()),
            latest: watch::Sender::new(0),
        }
    }
}

impl Arrivals {
    pub fn push(&self, message: IncomingMessage) {
        let Ok(mut buffer) = self.buffer.lock() else {
            return;
        };
        let seq = *self.latest.borrow() + 1;
        buffer.push_back((seq, message));
        while buffer.len() > KEEP {
            buffer.pop_front();
        }
        self.latest.send_replace(seq);
    }

    pub fn latest(&self) -> u64 {
        *self.latest.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.latest.subscribe()
    }

    pub fn after(&self, cursor: u64) -> (u64, Vec<IncomingMessage>) {
        let Ok(buffer) = self.buffer.lock() else {
            return (cursor, Vec::new());
        };
        let messages = buffer
            .iter()
            .filter(|(seq, _)| *seq > cursor)
            .map(|(_, m)| m.clone())
            .collect();
        (buffer.back().map_or(cursor, |(seq, _)| *seq), messages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::chat::ChannelCategory;

    fn message(id: i64) -> IncomingMessage {
        IncomingMessage {
            channel_url: "c".into(),
            channel_title: "Amy".into(),
            category: ChannelCategory::Direct,
            message_id: id,
            parent_id: None,
            created_at: 0,
            sender_id: "amy".into(),
            sender_name: "Amy".into(),
            text: "hi".into(),
            mentions_me: false,
            channel_is_open: false,
        }
    }

    #[test]
    fn each_cursor_sees_a_message_once() {
        let arrivals = Arrivals::default();
        let start = arrivals.latest();
        arrivals.push(message(1));
        arrivals.push(message(2));
        let (cursor, first) = arrivals.after(start);
        assert_eq!(
            first.iter().map(|m| m.message_id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert!(arrivals.after(cursor).1.is_empty());
        arrivals.push(message(3));
        let (_, next) = arrivals.after(cursor);
        assert_eq!(next.iter().map(|m| m.message_id).collect::<Vec<_>>(), [3]);
        assert_eq!(arrivals.after(start).1.len(), 3);
    }

    #[test]
    fn keeps_only_the_newest() {
        let arrivals = Arrivals::default();
        for id in 0..(KEEP as i64 + 10) {
            arrivals.push(message(id));
        }
        let (_, all) = arrivals.after(0);
        assert_eq!(all.len(), KEEP);
        assert_eq!(all[0].message_id, 10);
    }
}
