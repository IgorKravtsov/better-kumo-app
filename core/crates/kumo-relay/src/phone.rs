//! Other clients share the phone's secret, so phone use is only what the phone apps report explicitly.
//! Wall-clock time, because `Instant` stops while the computer sleeps (macOS and Linux) and would undercount.

use std::sync::Mutex;
use std::time::{Duration, SystemTime};

#[derive(Default)]
pub struct PhoneActivity(Mutex<Option<SystemTime>>);

impl PhoneActivity {
    pub fn seen(&self, now: SystemTime) {
        if let Ok(mut last) = self.0.lock() {
            *last = Some(last.map_or(now, |last| last.max(now)));
        }
    }

    pub fn idle_seconds(&self, now: SystemTime, handed_off: bool) -> Option<u64> {
        if handed_off {
            return Some(0);
        }
        let last = (*self.0.lock().ok()?)?;
        Some(now.duration_since(last).unwrap_or(Duration::ZERO).as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_until_a_phone_reports() {
        let activity = PhoneActivity::default();
        assert_eq!(activity.idle_seconds(SystemTime::now(), false), None);
    }

    #[test]
    fn counts_whole_seconds_since_the_last_report() {
        let activity = PhoneActivity::default();
        let start = SystemTime::now();
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
    fn a_late_report_keeps_the_newest_and_a_clock_set_back_reads_as_zero() {
        let activity = PhoneActivity::default();
        let start = SystemTime::now();
        activity.seen(start + Duration::from_secs(10));
        activity.seen(start);
        assert_eq!(
            activity.idle_seconds(start + Duration::from_secs(15), false),
            Some(5)
        );
        assert_eq!(activity.idle_seconds(start, false), Some(0));
    }

    #[test]
    fn a_handed_off_call_is_phone_use() {
        let activity = PhoneActivity::default();
        let start = SystemTime::now();
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
