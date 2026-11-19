use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const VIEWING_TTL: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct Viewing(Mutex<Option<(String, Instant)>>);

impl Viewing {
    pub fn set(&self, channel_url: Option<String>, now: Instant) {
        if let Ok(mut viewing) = self.0.lock() {
            *viewing = channel_url.map(|url| (url, now + VIEWING_TTL));
        }
    }

    pub fn mutes(&self, channel_url: Option<&str>, now: Instant) -> bool {
        let Some(channel_url) = channel_url else {
            return false;
        };
        self.0
            .lock()
            .map(|v| {
                v.as_ref()
                    .is_some_and(|(url, until)| url == channel_url && now < *until)
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutes_only_the_viewed_chat_until_the_phone_stops_reporting_it() {
        let viewing = Viewing::default();
        let now = Instant::now();
        assert!(!viewing.mutes(Some("c1"), now));
        viewing.set(Some("c1".into()), now);
        assert!(viewing.mutes(Some("c1"), now));
        assert!(!viewing.mutes(Some("c2"), now));
        assert!(!viewing.mutes(None, now));
        assert!(!viewing.mutes(Some("c1"), now + VIEWING_TTL));
        viewing.set(None, now);
        assert!(!viewing.mutes(Some("c1"), now));
    }
}
