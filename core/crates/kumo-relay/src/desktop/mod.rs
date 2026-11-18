mod cdp;
mod dev_tools;
mod lease;
mod office_mirror;
pub mod replay;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use kumo_core::chat::alert::{Alert, space_alert};
use kumo_core::chat::notifications::MyStatus;
use kumo_core::desktop::{
    DesktopCall, DesktopState, Handoff, MediaRequest, NearbyChat, NearbyRoom,
};
use kumo_core::space::SpaceUpdate;
use kumo_core::ws::{ClientOp, Command, JoinData, Position};
use serde_json::Value;

pub use cdp::Cdp;
pub use dev_tools::{DevTools, PageEvent, PageEvents};
pub use lease::{LeaseTiming, NotInCall, StaleLease};
pub use office_mirror::OfficeMirror;

use lease::Leases;

const PAGE_JS: &str = include_str!("page.js");
pub const BROADCAST_WATCH: Duration = Duration::from_secs(4);
const BROADCAST_POLL: Duration = Duration::from_millis(400);
const NEARBY_SEND_ATTEMPTS: usize = 3;

pub trait AlertSink: Send + Sync {
    fn alert(&self, alert: Alert);
}

pub struct Desktop {
    dev_tools: Arc<dyn DevTools>,
    office: OfficeMirror,
    connected: AtomicBool,
    leases: Mutex<Leases>,
    lease_timing: LeaseTiming,
    broadcast_watch: Duration,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl Desktop {
    pub fn new(cdp_url: String) -> Arc<Self> {
        Self::with_dev_tools(Arc::new(Cdp::new(cdp_url)))
    }

    pub fn with_dev_tools(dev_tools: Arc<dyn DevTools>) -> Arc<Self> {
        Self::with_lease_timing(dev_tools, LeaseTiming::default())
    }

    pub fn with_lease_timing(dev_tools: Arc<dyn DevTools>, lease_timing: LeaseTiming) -> Arc<Self> {
        Self::with_timing(dev_tools, lease_timing, BROADCAST_WATCH)
    }

    pub fn with_timing(
        dev_tools: Arc<dyn DevTools>,
        lease_timing: LeaseTiming,
        broadcast_watch: Duration,
    ) -> Arc<Self> {
        Arc::new(Self {
            dev_tools,
            office: OfficeMirror::default(),
            connected: AtomicBool::new(false),
            leases: Mutex::new(Leases::default()),
            lease_timing,
            broadcast_watch,
            tasks: Mutex::new(Vec::new()),
        })
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn handed_off(&self) -> bool {
        self.leases.lock().ok().is_some_and(|l| l.is_held())
    }

    pub async fn my_status(&self) -> MyStatus {
        if !self.connected() {
            return MyStatus::Unknown;
        }
        self.state()
            .await
            .map(|s| s.my_status())
            .unwrap_or(MyStatus::Unknown)
    }

    pub async fn state(&self) -> anyhow::Result<DesktopState> {
        let state = self.dev_tools.evaluate("window.__kumo.state()").await?;
        Ok(DesktopState::from_page(state)?)
    }

    pub async fn nearby_chat(&self) -> anyhow::Result<NearbyChat> {
        Ok(self.nearby_room().await?.chat())
    }

    pub async fn send_nearby(&self, text: &str) -> anyhow::Result<()> {
        for _ in 0..NEARBY_SEND_ATTEMPTS {
            let room = self.nearby_room().await?;
            let expression = format!(
                "window.__kumo.sendNearby({}, {})",
                Value::String(room.outgoing(text).to_frame()),
                Value::from(room.floor_id())
            );
            if self.deliver(&expression).await? != "moved" {
                return Ok(());
            }
        }
        bail!("the people near the desktop keep changing, try again")
    }

    async fn nearby_room(&self) -> anyhow::Result<NearbyRoom> {
        let page = self.dev_tools.evaluate("window.__kumo.nearby()").await?;
        Ok(NearbyRoom::from_page(page)?)
    }

    pub async fn call_info(&self) -> anyhow::Result<DesktopCall> {
        let page = self.dev_tools.evaluate("window.__kumo.call()").await?;
        let mut info = DesktopCall::from_page(page)?;
        if token_seconds_left(&info.token) > 60 {
            return Ok(info);
        }
        let fresh = match self.office.call() {
            Some(call)
                if token_seconds_left(&call.daily_token) > 60 && call.daily_url == info.url =>
            {
                call
            }
            _ => {
                let waiter = self.office.call_ready();
                self.send(&ClientOp::RefreshDailyToken).await?;
                tokio::time::timeout(Duration::from_secs(8), waiter)
                    .await
                    .context("Kumospace did not send a fresh call token")?;
                self.office.call().context("no call token")?
            }
        };
        info.url = fresh.daily_url;
        info.token = fresh.daily_token;
        Ok(info)
    }

    pub async fn handoff(&self) -> anyhow::Result<Handoff> {
        if self.dev_tools.evaluate("window.__kumo.handoff()").await? != Value::Bool(true) {
            return Err(NotInCall.into());
        }
        let lease_id = self
            .leases
            .lock()
            .map_err(|_| anyhow::anyhow!("lease table is poisoned"))?
            .grant();
        let call = self.call_info().await?;
        Ok(Handoff { lease_id, call })
    }

    pub fn renew(&self, lease_id: &str) -> Result<(), StaleLease> {
        self.leases.lock().map_err(|_| StaleLease)?.renew(lease_id)
    }

    pub async fn reclaim(&self, lease_id: &str) -> anyhow::Result<()> {
        self.leases
            .lock()
            .map_err(|_| StaleLease)?
            .release(lease_id)?;
        self.give_back().await
    }

    pub async fn reclaim_held(&self) -> anyhow::Result<()> {
        let held = self.leases.lock().ok().and_then(|mut l| l.take());
        if held.is_some() {
            self.give_back().await?;
        }
        Ok(())
    }

    async fn give_back(&self) -> anyhow::Result<()> {
        if self.dev_tools.evaluate("window.__kumo.reclaim()").await? == Value::Bool(true) {
            self.send(&ClientOp::RefreshDailyToken).await?;
        }
        Ok(())
    }

    pub async fn send(&self, op: &ClientOp) -> anyhow::Result<()> {
        let expression = format!("window.__kumo.send({})", Value::String(op.to_frame()));
        self.deliver(&expression).await.map(|_| ())
    }

    async fn deliver(&self, expression: &str) -> anyhow::Result<Value> {
        let reply = self.dev_tools.evaluate(expression).await?;
        if reply != Value::Bool(false) && !reply.is_null() {
            return Ok(reply);
        }
        if self.dev_tools.bind_office_socket().await? {
            let reply = self.dev_tools.evaluate(expression).await?;
            if reply != Value::Bool(false) && !reply.is_null() {
                return Ok(reply);
            }
        }
        bail!("the desktop app is not connected to the office")
    }

    pub async fn go_home(&self) -> anyhow::Result<()> {
        let home = self.dev_tools.evaluate("window.__kumo.home()").await?;
        if home.is_null() {
            bail!("no home is set for you in this space");
        }
        let position = Position::new(
            home["x"].as_f64().unwrap_or(-1.0),
            home["y"].as_f64().unwrap_or(-1.0),
        )?;
        let op = if home["roomId"] == home["currentFloor"] {
            ClientOp::Command(Command::Destination(position))
        } else {
            ClientOp::Join(JoinData {
                room_name: home["roomId"].as_str().map(String::from),
                destination: Some(position),
                ..JoinData::default()
            })
        };
        self.send(&op).await
    }

    pub async fn media(&self, media: MediaRequest) -> anyhow::Result<()> {
        let js = |v: Option<bool>| v.map(|b| b.to_string()).unwrap_or_else(|| "null".into());
        let ok = self
            .dev_tools
            .evaluate(&format!(
                "window.__kumo.media({}, {})",
                js(media.audio),
                js(media.video)
            ))
            .await?;
        if ok == Value::Bool(true) {
            Ok(())
        } else {
            bail!("the desktop app is not in a call")
        }
    }

    pub fn spawn(self: &Arc<Self>, alerts: Arc<dyn AlertSink>) {
        self.spawn_lease_watch();
        let me = self.clone();
        let task = tokio::spawn(async move {
            loop {
                let result = match me.dev_tools.attach().await {
                    Ok(events) => me.attached(events, alerts.as_ref()).await,
                    Err(e) => Err(e),
                };
                me.dev_tools.detach();
                if let Err(e) = result
                    && me.connected.swap(false, Ordering::SeqCst)
                {
                    eprintln!("desktop: disconnected ({e})");
                }
                me.connected.store(false, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        self.track(task);
    }

    pub fn stop(&self) {
        if let Ok(mut tasks) = self.tasks.lock() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
        self.dev_tools.detach();
    }

    fn track(&self, task: tokio::task::JoinHandle<()>) {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.push(task);
        }
    }

    fn spawn_lease_watch(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let timing = self.lease_timing;
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(timing.check_every).await;
                let Some(desktop) = weak.upgrade() else {
                    return;
                };
                let lapsed = desktop
                    .leases
                    .lock()
                    .ok()
                    .and_then(|l| l.lapsed(timing.lapse));
                if let Some(lease_id) = lapsed {
                    eprintln!(
                        "desktop: the phone's lease lapsed, giving the call back to the desktop"
                    );
                    if let Err(e) = desktop.reclaim(&lease_id).await {
                        eprintln!("desktop: reclaim failed: {e:#}");
                    }
                }
            }
        });
        self.track(task);
    }

    async fn inject(&self) -> anyhow::Result<()> {
        self.office.reset();
        self.dev_tools.evaluate(PAGE_JS).await.map(|_| ())
    }

    async fn attached(&self, mut events: PageEvents, alerts: &dyn AlertSink) -> anyhow::Result<()> {
        self.inject().await?;
        if !self.connected.swap(true, Ordering::SeqCst) {
            eprintln!("desktop: connected");
        }
        while let Some(event) = events.recv().await {
            match event {
                PageEvent::OfficeFrame(frame) => self.observe(&frame, alerts).await,
                PageEvent::Reloaded => {
                    if let Err(e) = self.inject().await {
                        eprintln!("desktop: page reloaded, injecting failed: {e:#}");
                    }
                }
            }
        }
        bail!("desktop page closed")
    }

    async fn observe(&self, frame: &str, alerts: &dyn AlertSink) {
        for update in self.office.observe(frame) {
            if space_alert(&update).is_none() {
                continue;
            }
            if matches!(update, SpaceUpdate::Broadcast { .. }) && self.watching_broadcast().await {
                eprintln!("desktop: the broadcast is already on screen, no alert");
                continue;
            }
            let update = self.named(update).await;
            if let Some(alert) = space_alert(&update) {
                eprintln!("desktop: {:?} ({})", alert.kind, alert.title);
                alerts.alert(alert);
            }
        }
    }

    async fn watching_broadcast(&self) -> bool {
        if !matches!(
            self.my_status().await,
            MyStatus::Available | MyStatus::Focusing
        ) {
            return false;
        }
        let deadline = tokio::time::Instant::now() + self.broadcast_watch;
        loop {
            let shown = self
                .dev_tools
                .evaluate("window.__kumo.broadcastShown()")
                .await;
            if matches!(shown, Ok(Value::Bool(true))) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(BROADCAST_POLL).await;
        }
    }

    async fn named(&self, update: SpaceUpdate) -> SpaceUpdate {
        let ids = update.unnamed_sender();
        if ids.is_empty() {
            return update;
        }
        let name = self.page_name(&ids).await;
        update.named(name)
    }

    async fn page_name(&self, ids: &[String]) -> Option<String> {
        for id in ids {
            let expression = format!("window.__kumo.nameOf({})", Value::String(id.clone()));
            if let Ok(Value::String(name)) = self.dev_tools.evaluate(&expression).await {
                return Some(name);
            }
        }
        None
    }
}

fn token_seconds_left(token: &str) -> i64 {
    let Some(payload) = token.split('.').nth(1) else {
        return 0;
    };
    let Some(bytes) = base64_url_decode(payload) else {
        return 0;
    };
    let exp = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["exp"].as_i64())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    exp - now
}

fn base64_url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in input.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_jwt_expiry() {
        let payload = base64_url_encode(br#"{"exp":4102444800}"#);
        assert!(token_seconds_left(&format!("h.{payload}.s")) > 1_000_000);
        assert!(token_seconds_left("garbage") <= 0);
    }

    fn base64_url_encode(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for i in 0..=chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }
}
