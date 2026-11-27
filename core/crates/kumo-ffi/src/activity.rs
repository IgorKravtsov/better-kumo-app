//! Tells the Relay the phone app is in use, for `phone_idle_seconds` in its `GET /v1/status`.
//!
//! The apps call `set_foreground` from their own lifecycle (iOS scene phase, Android `MainActivity` start/stop).
//! While the app is in the foreground, on any screen, it reports at once and then every 30 s; when it leaves the
//! foreground it reports once more, so the Relay counts idle time from that moment. Nothing runs in the background,
//! and a process started in the background (a push, a service) never reports until the app comes to the foreground.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::RelayClient;

const REPORT_EVERY: Duration = Duration::from_secs(30);

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
        if foreground == heartbeat.is_some() {
            return;
        }
        let relay = self.relay.clone();
        if foreground {
            *heartbeat = Some(self.runtime.spawn(async move {
                loop {
                    report(&relay).await;
                    tokio::time::sleep(REPORT_EVERY).await;
                }
            }));
        } else {
            if let Some(task) = heartbeat.take() {
                task.abort();
            }
            self.runtime.spawn(async move { report(&relay).await });
        }
    }
}

async fn report(relay: &RelayClient) {
    if let Some(client) = relay.core_client() {
        let _ = client.report_phone_activity().await;
    }
}
