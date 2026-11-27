//! Other clients share the phone's secret, so phone use is only what the phone apps report explicitly.

use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
pub struct PhoneActivity(Mutex<Option<Instant>>);

impl PhoneActivity {
    pub fn seen(&self, now: Instant) {
        if let Ok(mut last) = self.0.lock() {
            *last = Some(now);
        }
    }

    pub fn idle_seconds(&self, now: Instant, handed_off: bool) -> Option<u64> {
        if handed_off {
            return Some(0);
        }
        let last = (*self.0.lock().ok()?)?;
        Some(now.saturating_duration_since(last).as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn unknown_until_a_phone_reports() {
        let activity = PhoneActivity::default();
        assert_eq!(activity.idle_seconds(Instant::now(), false), None);
    }

    #[test]
    fn counts_whole_seconds_since_the_last_report() {
        let activity = PhoneActivity::default();
        let start = Instant::now();
        activity.seen(start);
        assert_eq!(activity.idle_seconds(start, false), Some(0));
        assert_eq!(
            activity.idle_seconds(start + Duration::from_millis(1999), false),
            Some(1)
        );
        assert_eq!(
            activity.idle_seconds(start + Duration::from_secs(90), false),
            Some(90)
        );
        activity.seen(start + Duration::from_secs(60));
        assert_eq!(
            activity.idle_seconds(start + Duration::from_secs(90), false),
            Some(30)
        );
    }

    #[test]
    fn a_handed_off_call_is_phone_use() {
        let activity = PhoneActivity::default();
        let start = Instant::now();
        assert_eq!(activity.idle_seconds(start, true), Some(0));
        activity.seen(start);
        assert_eq!(
            activity.idle_seconds(start + Duration::from_secs(600), true),
            Some(0)
        );
        assert_eq!(
            activity.idle_seconds(start + Duration::from_secs(600), false),
            Some(600)
        );
    }
}
