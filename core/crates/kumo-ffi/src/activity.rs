//! Foreground heartbeats tell the Relay the phone is in use; other clients share its secret, so it can't tell otherwise.
//! There is no report on leaving the foreground: iOS may suspend the app before it goes out and send it on a later
//! background wake, which would count as use. Idle time therefore starts up to `REPORT_EVERY` early.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::RelayClient;

const REPORT_EVERY: Duration = Duration::from_secs(30);
const UNPAIRED_CHECK: Duration = Duration::from_secs(2);

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
            let mut every = tokio::time::interval(REPORT_EVERY);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                every.tick().await;
                // A local check, so a pairing made while on screen reports within seconds, not at the next tick.
                let client = loop {
                    match relay.core_client() {
                        Some(client) => break client,
                        None => tokio::time::sleep(UNPAIRED_CHECK).await,
                    }
                };
                let _ = client.report_phone_activity().await;
            }
        }));
    }
}
