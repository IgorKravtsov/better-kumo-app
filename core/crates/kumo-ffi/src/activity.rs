//! Foreground heartbeats tell the Relay the phone is in use; other clients share its secret, so it can't tell otherwise.
//! There is no report on leaving the foreground: iOS may suspend the app before it goes out and send it on a later
//! background wake, which would count as use. Idle time therefore starts up to `REPORT_EVERY` early.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use kumo_core::desktop::RelayError;

use crate::RelayClient;

const REPORT_EVERY: Duration = Duration::from_secs(30);
// A network blip (Wi-Fi to cellular) shouldn't cost a whole interval; other errors won't heal in seconds.
const RETRY_AFTER: Duration = Duration::from_secs(5);
const RETRIES: usize = 3;

pub struct ActivityReporter {
    runtime: Handle,
    relay: Arc<RelayClient>,
    heartbeat: Mutex<Option<JoinHandle<()>>>,
}

impl ActivityReporter {
    pub fn new(runtime: Handle, relay: Arc<RelayClient>) -> Self {
        Self {
            runtime,
            relay,
            heartbeat: Mutex::new(None),
        }
    }

    pub fn set_foreground(&self, foreground: bool) {
        let Ok(mut heartbeat) = self.heartbeat.lock() else {
            return;
        };
        if !foreground {
            if let Some(task) = heartbeat.take() {
                task.abort();
            }
            return;
        }
        if heartbeat.is_some() {
            return;
        }
        let relay = self.relay.clone();
        *heartbeat = Some(self.runtime.spawn(async move {
            let mut every =
                tokio::time::interval_at(tokio::time::Instant::now() + REPORT_EVERY, REPORT_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                // Enabled before reading the pairing, so a pair() during the report still wakes us.
                let changed = relay.pairing_changed();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let client = relay.core_client();
                let work = async {
                    if let Some(client) = client {
                        report(&client).await;
                    }
                    every.tick().await;
                };
                // A new or forgotten pairing cancels a report (or its retries) still aimed at the old Relay.
                let repaired = tokio::select! {
                    _ = work => false,
                    _ = &mut changed => true,
                };
                if repaired {
                    every.reset();
                }
            }
        }));
    }
}

async fn report(client: &kumo_core::desktop::RelayClient) {
    for attempt in 0..=RETRIES {
        match client.report_phone_activity().await {
            Err(RelayError::Network(_)) if attempt < RETRIES => {
                tokio::time::sleep(RETRY_AFTER).await;
            }
            _ => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::{RelayClientDeps, RelayPairing, RelayStorage};
    use crate::{AppEvent, AppListener};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct NoStorage;
    impl RelayStorage for NoStorage {
        fn load(&self) -> Option<RelayPairing> {
            None
        }
        fn save(&self, _: RelayPairing) {}
        fn clear(&self) {}
        fn save_mirroring(&self, _: bool) {}
    }

    struct NoListener;
    impl AppListener for NoListener {
        fn on_event(&self, _: AppEvent) {}
    }

    async fn fake_relay(hang: bool) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut chunk = [0; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                if head.starts_with(b"POST /v1/phone/activity ") {
                    counted.fetch_add(1, Ordering::SeqCst);
                }
                if hang {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        drop(socket);
                    });
                    continue;
                }
                let body = r#"{"ok":true}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            }
        });
        (url, reports)
    }

    async fn until(reports: &AtomicUsize, count: usize) {
        for _ in 0..200 {
            if reports.load(Ordering::SeqCst) >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "expected {count} reports, got {}",
            reports.load(Ordering::SeqCst)
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reports_on_foreground_and_on_every_pairing_but_not_in_the_background() {
        let relay = RelayClient::new(RelayClientDeps {
            runtime: Handle::current(),
            http: kumo_core::http::build_http_client().unwrap(),
            storage: Arc::new(NoStorage),
            listener: Arc::new(NoListener),
        });
        let reporter = ActivityReporter::new(Handle::current(), relay.clone());
        let (a, to_a) = fake_relay(false).await;
        let (b, to_b) = fake_relay(false).await;
        let link = |url: &str| format!("kumowatch://relay?url={url}&secret=s3cret-s3cret-s3cret");

        reporter.set_foreground(true);
        assert!(relay.pair(link(&a)));
        until(&to_a, 1).await;
        assert!(relay.pair(link(&b)));
        until(&to_b, 1).await;
        reporter.set_foreground(true);
        reporter.set_foreground(false);
        assert!(relay.pair(link(&a)));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(to_a.load(Ordering::SeqCst), 1);
        assert_eq!(to_b.load(Ordering::SeqCst), 1);

        reporter.set_foreground(true);
        until(&to_a, 2).await;
        reporter.set_foreground(false);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_new_pairing_does_not_wait_for_a_hung_report_to_the_old_relay() {
        let relay = RelayClient::new(RelayClientDeps {
            runtime: Handle::current(),
            http: kumo_core::http::build_http_client().unwrap(),
            storage: Arc::new(NoStorage),
            listener: Arc::new(NoListener),
        });
        let reporter = ActivityReporter::new(Handle::current(), relay.clone());
        let (a, to_a) = fake_relay(true).await;
        let (b, to_b) = fake_relay(false).await;
        let link = |url: &str| format!("kumowatch://relay?url={url}&secret=s3cret-s3cret-s3cret");

        assert!(relay.pair(link(&a)));
        reporter.set_foreground(true);
        until(&to_a, 1).await;
        assert!(relay.pair(link(&b)));
        until(&to_b, 1).await;
        reporter.set_foreground(false);
    }
}
