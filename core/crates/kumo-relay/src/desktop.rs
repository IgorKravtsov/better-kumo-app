use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use kumo_core::chat::alert::{Alert, AlertKind};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::Relay;

const PAGE_JS: &str = include_str!("page.js");
const FIND_SOCKET: &str = "function(){const w=this.filter(w=>w&&w.url&&w.url.startsWith('wss://ws.kumospace.com')&&w.readyState===1).pop();if(!w)return false;window.__kumo.ws=w;return true}";
const ALLOWED_COMMANDS: &[&str] = &[
    "destination",
    "navigateToZone",
    "followUser",
    "stopFollowing",
];

struct Cdp {
    commands: mpsc::UnboundedSender<(Value, oneshot::Sender<Value>)>,
    seq: AtomicU64,
}

impl Cdp {
    async fn call(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.commands
            .send((json!({ "id": id, "method": method, "params": params }), tx))
            .map_err(|_| anyhow!("desktop disconnected"))?;
        let reply = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .context("desktop did not answer")?
            .map_err(|_| anyhow!("desktop disconnected"))?;
        if let Some(err) = reply.get("error") {
            bail!("cdp error: {err}");
        }
        Ok(reply["result"].clone())
    }

    async fn eval(&self, expression: &str) -> anyhow::Result<Value> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(ex) = result.get("exceptionDetails") {
            bail!(
                "page error: {}",
                ex["exception"]["description"].as_str().unwrap_or("unknown")
            );
        }
        Ok(result["result"]["value"].clone())
    }
}

pub struct Desktop {
    cdp_url: String,
    cdp: Mutex<Option<Arc<Cdp>>>,
    connected: AtomicBool,
    room: std::sync::Mutex<Option<(String, String)>>,
    room_ready: tokio::sync::Notify,
    handoff_at: std::sync::Mutex<Option<Instant>>,
    tasks: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

const HANDOFF_TIMEOUT: Duration = Duration::from_secs(45);

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

pub fn local_ws_url(cdp_url: &str, ws_url: &str) -> String {
    let host = cdp_url
        .split("://")
        .nth(1)
        .unwrap_or(cdp_url)
        .split('/')
        .next()
        .unwrap_or_default();
    match ws_url
        .split_once("://")
        .and_then(|(scheme, rest)| rest.split_once('/').map(|(_, path)| (scheme, path)))
    {
        Some((scheme, path)) if !host.is_empty() => format!("{scheme}://{host}/{path}"),
        _ => ws_url.to_string(),
    }
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

impl Desktop {
    pub fn new(cdp_url: String) -> Arc<Self> {
        Arc::new(Self {
            cdp_url,
            cdp: Mutex::new(None),
            connected: AtomicBool::new(false),
            room: std::sync::Mutex::new(None),
            room_ready: tokio::sync::Notify::new(),
            handoff_at: std::sync::Mutex::new(None),
            tasks: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn handed_off(&self) -> bool {
        self.handoff_at.lock().ok().is_some_and(|at| at.is_some())
    }

    pub async fn handoff(&self) -> anyhow::Result<Value> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        if cdp.eval("window.__kumo.handoff()").await? != Value::Bool(true) {
            bail!("the desktop app is not in a call");
        }
        if let Ok(mut at) = self.handoff_at.lock() {
            *at = Some(Instant::now());
        }
        self.call_info().await
    }

    pub async fn reclaim(&self) -> anyhow::Result<()> {
        if let Ok(mut at) = self.handoff_at.lock() {
            *at = None;
        }
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        if cdp.eval("window.__kumo.reclaim()").await? == Value::Bool(true) {
            self.send(&json!({ "op": "refreshDailyToken" })).await?;
        }
        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(mut tasks) = self.tasks.lock() {
            for task in tasks.drain(..) {
                task.abort();
            }
        }
    }

    fn track(&self, task: tokio::task::JoinHandle<()>) {
        if let Ok(mut tasks) = self.tasks.lock() {
            tasks.push(task);
        }
    }

    pub fn spawn_handoff_watchdog(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let Some(desktop) = weak.upgrade() else {
                    return;
                };
                let stale = desktop
                    .handoff_at
                    .lock()
                    .ok()
                    .and_then(|at| *at)
                    .is_some_and(|at| at.elapsed() > HANDOFF_TIMEOUT);
                if stale {
                    eprintln!(
                        "desktop: phone stopped answering, giving the call back to the desktop"
                    );
                    if let Err(e) = desktop.reclaim().await {
                        eprintln!("desktop: reclaim failed: {e:#}");
                    }
                }
            }
        });
        self.track(task);
    }

    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn cdp(&self) -> anyhow::Result<Arc<Cdp>> {
        self.cdp
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("Kumospace desktop is not connected"))
    }

    pub async fn state(&self) -> anyhow::Result<Value> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        let state = cdp.eval("window.__kumo.state()").await?;
        if state.is_null() {
            bail!("Kumospace desktop is not in a space");
        }
        Ok(state)
    }

    pub async fn call_info(&self) -> anyhow::Result<Value> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        let mut info = cdp.eval("window.__kumo.call()").await?;
        if info.is_null() {
            bail!("the desktop app is not in a call");
        }
        let current = info["token"].as_str().unwrap_or_default().to_string();
        if token_seconds_left(&current) > 60 {
            return Ok(info);
        }
        let cached = self.room.lock().ok().and_then(|r| r.clone());
        let fresh = match cached {
            Some((url, token))
                if token_seconds_left(&token) > 60
                    && Some(url.as_str()) == info["url"].as_str() =>
            {
                (url, token)
            }
            _ => {
                let waiter = self.room_ready.notified();
                self.send(&json!({ "op": "refreshDailyToken" })).await?;
                tokio::time::timeout(Duration::from_secs(8), waiter)
                    .await
                    .context("Kumospace did not send a fresh call token")?;
                self.room
                    .lock()
                    .ok()
                    .and_then(|r| r.clone())
                    .context("no call token")?
            }
        };
        info["url"] = json!(fresh.0);
        info["token"] = json!(fresh.1);
        Ok(info)
    }

    pub async fn send(&self, op: &Value) -> anyhow::Result<()> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        let expr = format!("window.__kumo.send({op})");
        if cdp.eval(&expr).await? == Value::Bool(true) {
            return Ok(());
        }
        self.find_socket(&cdp).await?;
        if cdp.eval(&expr).await? == Value::Bool(true) {
            Ok(())
        } else {
            bail!("the desktop app is not connected to the office")
        }
    }

    pub async fn go_home(&self) -> anyhow::Result<()> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        let home = cdp.eval("window.__kumo.home()").await?;
        if home.is_null() {
            bail!("no home is set for you in this space");
        }
        let (x, y) = (home["x"].clone(), home["y"].clone());
        if home["roomId"] == home["currentFloor"] {
            self.command(json!({ "action": "destination", "x": x, "y": y }))
                .await
        } else {
            self.send(&json!({ "op": "join", "data": { "roomName": home["roomId"], "destination": { "x": x, "y": y } } }))
                .await
        }
    }

    pub async fn command(&self, data: Value) -> anyhow::Result<()> {
        let action = data["action"].as_str().unwrap_or_default();
        if !ALLOWED_COMMANDS.contains(&action) {
            bail!("unsupported command {action}");
        }
        self.send(&json!({ "op": "command", "data": data })).await
    }

    pub async fn media(&self, audio: Option<bool>, video: Option<bool>) -> anyhow::Result<()> {
        let cdp = self.cdp().await?;
        cdp.eval(PAGE_JS).await?;
        let js = |v: Option<bool>| v.map(|b| b.to_string()).unwrap_or_else(|| "null".into());
        let ok = cdp
            .eval(&format!(
                "window.__kumo.media({}, {})",
                js(audio),
                js(video)
            ))
            .await?;
        if ok == Value::Bool(true) {
            Ok(())
        } else {
            bail!("the desktop app is not in a call")
        }
    }

    async fn find_socket(&self, cdp: &Cdp) -> anyhow::Result<()> {
        let proto = cdp
            .call(
                "Runtime.evaluate",
                json!({ "expression": "WebSocket.prototype" }),
            )
            .await?;
        let proto_id = proto["result"]["objectId"]
            .as_str()
            .context("no WebSocket prototype")?;
        let objects = cdp
            .call(
                "Runtime.queryObjects",
                json!({ "prototypeObjectId": proto_id }),
            )
            .await?;
        let array_id = objects["objects"]["objectId"]
            .as_str()
            .context("no sockets")?;
        let found = cdp
            .call(
                "Runtime.callFunctionOn",
                json!({ "objectId": array_id, "functionDeclaration": FIND_SOCKET, "returnByValue": true }),
            )
            .await?;
        let _ = cdp
            .call("Runtime.releaseObjectGroup", json!({ "objectGroup": "" }))
            .await;
        if found["result"]["value"] == Value::Bool(true) {
            Ok(())
        } else {
            bail!("the desktop app is not connected to the office")
        }
    }

    pub fn spawn(self: &Arc<Self>, relay: Arc<Relay>) {
        self.spawn_handoff_watchdog();
        let me = self.clone();
        let task = tokio::spawn(async move {
            loop {
                if let Err(e) = me.session(&relay).await
                    && me.connected.swap(false, Ordering::SeqCst)
                {
                    eprintln!("desktop: disconnected ({e})");
                }
                *me.cdp.lock().await = None;
                me.connected.store(false, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        self.track(task);
    }

    async fn session(&self, relay: &Arc<Relay>) -> anyhow::Result<()> {
        let targets: Vec<Value> =
            reqwest::get(format!("{}/json/list", self.cdp_url.trim_end_matches('/')))
                .await?
                .json()
                .await?;
        let ws_url = targets
            .iter()
            .find(|t| {
                t["type"] == "page"
                    && t["url"]
                        .as_str()
                        .is_some_and(|u| u.contains("kumospace.com"))
            })
            .and_then(|t| t["webSocketDebuggerUrl"].as_str())
            .context("no Kumospace page")?;
        let ws_url = local_ws_url(&self.cdp_url, ws_url);
        let (socket, _) = tokio_tungstenite::connect_async(ws_url.as_str()).await?;
        let (mut sink, mut stream) = socket.split();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<(Value, oneshot::Sender<Value>)>();
        let cdp = Arc::new(Cdp {
            commands: cmd_tx,
            seq: AtomicU64::new(1),
        });
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<String>();
        let pump = tokio::spawn(async move {
            let mut pending: HashMap<u64, oneshot::Sender<Value>> = HashMap::new();
            loop {
                tokio::select! {
                    cmd = cmd_rx.recv() => {
                        let Some((msg, tx)) = cmd else { break };
                        if let Some(id) = msg["id"].as_u64() { pending.insert(id, tx); }
                        if sink.send(Message::text(msg.to_string())).await.is_err() { break; }
                    }
                    incoming = stream.next() => {
                        let Some(Ok(Message::Text(text))) = incoming else {
                            if matches!(incoming, Some(Ok(_))) { continue; }
                            break;
                        };
                        let Ok(v) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                        if let Some(id) = v["id"].as_u64() {
                            if let Some(tx) = pending.remove(&id) { let _ = tx.send(v); }
                        } else if v["method"] == "Network.webSocketFrameReceived"
                            && let Some(payload) = v["params"]["response"]["payloadData"].as_str()
                        {
                            let _ = frame_tx.send(payload.to_string());
                        }
                    }
                }
            }
        });
        cdp.call("Network.enable", json!({})).await?;
        cdp.eval(PAGE_JS).await?;
        *self.cdp.lock().await = Some(cdp.clone());
        if !self.connected.swap(true, Ordering::SeqCst) {
            eprintln!("desktop: connected to {ws_url}");
        }
        while let Some(payload) = frame_rx.recv().await {
            let Ok(frame) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            if frame["op"] != "playSound" {
                continue;
            }
            let data = &frame["data"];
            let kind = match data["notificationType"].as_str() {
                Some("nudge") => AlertKind::Nudge,
                Some("doorKnock") => AlertKind::Knock,
                _ => continue,
            };
            let sender = [
                "senderId",
                "fromUserId",
                "sourceUserId",
                "userId",
                "entityId",
            ]
            .iter()
            .find_map(|k| data[*k].as_str())
            .map(String::from);
            let name = match sender {
                Some(id) => cdp
                    .eval(&format!("window.__kumo.nameOf({})", Value::String(id)))
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(String::from)),
                None => None,
            }
            .unwrap_or_else(|| "Someone".into());
            eprintln!("desktop: {kind:?} from {name} ({data})");
            let alert = Alert {
                kind,
                title: match kind {
                    AlertKind::Nudge => format!("{name} nudged you"),
                    _ => format!("{name} is knocking"),
                },
                body: "Kumospace".into(),
                channel_url: None,
            };
            let relay = relay.clone();
            tokio::spawn(async move { relay.dispatch(&alert).await });
        }
        pump.abort();
        bail!("desktop page closed")
    }
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

    #[test]
    fn ws_url_follows_cdp_host() {
        assert_eq!(
            local_ws_url(
                "http://192.168.65.254:9333",
                "ws://127.0.0.1:9222/devtools/page/AB"
            ),
            "ws://192.168.65.254:9333/devtools/page/AB"
        );
        assert_eq!(
            local_ws_url(
                "http://127.0.0.1:9222/",
                "ws://127.0.0.1:9222/devtools/page/AB"
            ),
            "ws://127.0.0.1:9222/devtools/page/AB"
        );
    }
}
