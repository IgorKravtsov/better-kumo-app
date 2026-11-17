use std::fmt;
use std::time::Duration;

use tokio::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseTiming {
    pub lapse: Duration,
    pub check_every: Duration,
}

impl Default for LeaseTiming {
    fn default() -> Self {
        Self {
            lapse: Duration::from_secs(45),
            check_every: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleLease;

impl fmt::Display for StaleLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("another phone took over this call")
    }
}

impl std::error::Error for StaleLease {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotInCall;

impl fmt::Display for NotInCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the desktop app is not in a call")
    }
}

impl std::error::Error for NotInCall {}

struct Held {
    id: String,
    renewed_at: Instant,
}

#[derive(Default)]
pub struct Leases {
    held: Option<Held>,
}

impl Leases {
    pub fn grant(&mut self) -> String {
        let id = new_id();
        self.held = Some(Held {
            id: id.clone(),
            renewed_at: Instant::now(),
        });
        id
    }

    pub fn renew(&mut self, id: &str) -> Result<(), StaleLease> {
        match self.held.as_mut().filter(|h| h.id == id) {
            Some(held) => {
                held.renewed_at = Instant::now();
                Ok(())
            }
            None => Err(StaleLease),
        }
    }

    pub fn release(&mut self, id: &str) -> Result<(), StaleLease> {
        if self.held.as_ref().is_some_and(|h| h.id == id) {
            self.held = None;
            Ok(())
        } else {
            Err(StaleLease)
        }
    }

    pub fn take(&mut self) -> Option<String> {
        self.held.take().map(|h| h.id)
    }

    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }

    pub fn lapsed(&self, lapse: Duration) -> Option<String> {
        self.held
            .as_ref()
            .filter(|h| h.renewed_at.elapsed() >= lapse)
            .map(|h| h.id.clone())
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 16];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_grant_wins() {
        let mut leases = Leases::default();
        let first = leases.grant();
        let second = leases.grant();
        assert_ne!(first, second);
        assert_eq!(leases.renew(&first), Err(StaleLease));
        assert_eq!(leases.release(&first), Err(StaleLease));
        assert_eq!(leases.renew(&second), Ok(()));
        assert_eq!(leases.release(&second), Ok(()));
        assert!(!leases.is_held());
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_lapses_after_the_last_renewal() {
        let mut leases = Leases::default();
        let id = leases.grant();
        tokio::time::advance(Duration::from_secs(30)).await;
        leases.renew(&id).unwrap();
        tokio::time::advance(Duration::from_secs(44)).await;
        assert_eq!(leases.lapsed(Duration::from_secs(45)), None);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(leases.lapsed(Duration::from_secs(45)), Some(id));
    }
}
