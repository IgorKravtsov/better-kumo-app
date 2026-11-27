use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use super::dev_tools::{DevTools, PageEvent, PageEvents};

const FIND_SOCKET: &str = "function(){const w=this.filter(w=>w&&w.url&&w.url.startsWith('wss://ws.kumospace.com')&&w.readyState===1).pop();if(!w)return false;window.__kumo.ws=w;return true}";

type Pending = (Value, oneshot::Sender<Value>);

struct Session {
    commands: mpsc::UnboundedSender<Pending>,
    seq: AtomicU64,
    pump: tokio::task::JoinHandle<()>,
}

impl Session {
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

    async fn find_socket(&self) -> anyhow::Result<bool> {
        let proto = self
            .call(
                "Runtime.evaluate",
                json!({ "expression": "WebSocket.prototype" }),
            )
            .await?;
        let proto_id = proto["result"]["objectId"]
            .as_str()
            .context("no WebSocket prototype")?;
        let objects = self
            .call(
                "Runtime.queryObjects",
                json!({ "prototypeObjectId": proto_id }),
            )
            .await?;
        let array_id = objects["objects"]["objectId"]
            .as_str()
            .context("no sockets")?;
        let found = self
            .call(
                "Runtime.callFunctionOn",
                json!({ "objectId": array_id, "functionDeclaration": FIND_SOCKET, "returnByValue": true }),
            )
            .await?;
        let _ = self
            .call("Runtime.releaseObjectGroup", json!({ "objectGroup": "" }))
            .await;
        Ok(found["result"]["value"] == Value::Bool(true))
    }
}

pub struct Cdp {
    url: String,
    space: String,
    session: Mutex<Option<Arc<Session>>>,
}

impl Cdp {
    pub fn new(url: String, space: String) -> Self {
        Self {
            url,
            space,
            session: Mutex::new(None),
        }
    }

    fn session(&self) -> anyhow::Result<Arc<Session>> {
        self.session
            .lock()
            .ok()
            .and_then(|s| s.clone())
            .ok_or_else(|| anyhow!("Kumospace desktop is not connected"))
    }

    async fn connect(&self) -> anyhow::Result<PageEvents> {
        self.detach();
        let ws_url = kumo_devtools::find_desktop_app_page_in(&self.url, Some(&self.space)).await?;
        let (socket, _) = tokio_tungstenite::connect_async(ws_url.as_str()).await?;
        let (mut sink, mut stream) = socket.split();
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<Pending>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<PageEvent>();
        let pump = tokio::spawn(async move {
            let mut pending: HashMap<u64, oneshot::Sender<Value>> = HashMap::new();
            let mut cleared = false;
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
                            continue;
                        }
                        let event = match v["method"].as_str() {
                            Some("Network.webSocketFrameReceived") => v["params"]["response"]["payloadData"]
                                .as_str()
                                .map(|p| PageEvent::OfficeFrame(p.to_string())),
                            Some("Runtime.executionContextsCleared") => {
                                cleared = true;
                                None
                            }
                            Some("Runtime.executionContextCreated")
                                if cleared && v["params"]["context"]["auxData"]["isDefault"] == true =>
                            {
                                cleared = false;
                                Some(PageEvent::Reloaded)
                            }
                            _ => None,
                        };
                        if let Some(event) = event && event_tx.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let session = Arc::new(Session {
            commands: cmd_tx,
            seq: AtomicU64::new(1),
            pump,
        });
        session.call("Network.enable", json!({})).await?;
        session.call("Runtime.enable", json!({})).await?;
        if let Ok(mut current) = self.session.lock() {
            *current = Some(session);
        }
        Ok(event_rx)
    }
}

impl DevTools for Cdp {
    fn attach(&self) -> BoxFuture<'_, anyhow::Result<PageEvents>> {
        Box::pin(self.connect())
    }

    fn detach(&self) {
        if let Some(session) = self.session.lock().ok().and_then(|mut s| s.take()) {
            session.pump.abort();
        }
    }

    fn evaluate<'a>(&'a self, expression: &'a str) -> BoxFuture<'a, anyhow::Result<Value>> {
        Box::pin(async move { self.session()?.eval(expression).await })
    }

    fn bind_office_socket(&self) -> BoxFuture<'_, anyhow::Result<bool>> {
        Box::pin(async move { self.session()?.find_socket().await })
    }
}
