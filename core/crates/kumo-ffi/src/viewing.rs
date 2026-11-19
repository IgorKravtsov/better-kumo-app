use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::RelayClient;

const REPORT_EVERY: Duration = Duration::from_secs(10);

struct State {
    channel: Option<String>,
    foreground: bool,
    heartbeat: Option<JoinHandle<()>>,
    reported: bool,
}

pub struct ViewingReporter {
    runtime: Handle,
    relay: Arc<RelayClient>,
    state: Mutex<State>,
}

impl ViewingReporter {
    pub fn new(runtime: Handle, relay: Arc<RelayClient>) -> Self {
        Self {
            runtime,
            relay,
            state: Mutex::new(State {
                channel: None,
                foreground: true,
                heartbeat: None,
                reported: false,
            }),
        }
    }

    pub fn open(&self, channel_url: &str) {
        self.update(|s| s.channel = Some(channel_url.to_string()));
    }

    pub fn close(&self, channel_url: &str) {
        self.update(|s| {
            if s.channel.as_deref() == Some(channel_url) {
                s.channel = None;
            }
        });
    }

    pub fn set_foreground(&self, foreground: bool) {
        self.update(|s| s.foreground = foreground);
    }

    fn update(&self, change: impl FnOnce(&mut State)) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let before = state.foreground.then(|| state.channel.clone()).flatten();
        change(&mut state);
        let viewing = state.foreground.then(|| state.channel.clone()).flatten();
        if viewing == before && (viewing.is_none() || state.heartbeat.is_some()) {
            return;
        }
        if let Some(heartbeat) = state.heartbeat.take() {
            heartbeat.abort();
        }
        let relay = self.relay.clone();
        match viewing {
            Some(url) => {
                state.reported = true;
                state.heartbeat = Some(self.runtime.spawn(async move {
                    loop {
                        if let Some(client) = relay.core_client() {
                            let _ = client.set_viewing(Some(url.clone())).await;
                        }
                        tokio::time::sleep(REPORT_EVERY).await;
                    }
                }));
            }
            None if state.reported => {
                state.reported = false;
                self.runtime.spawn(async move {
                    if let Some(client) = relay.core_client() {
                        let _ = client.set_viewing(None).await;
                    }
                });
            }
            None => {}
        }
    }
}
