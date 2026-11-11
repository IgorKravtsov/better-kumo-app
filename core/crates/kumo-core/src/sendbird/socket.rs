use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;

use super::frame::Frame;
use super::model::{LoginInfo, SbMessage};
use super::{SDK_VERSION, encode_component, sdk_user_agent, user_agent};
use crate::error::{CoreError, Result};
use crate::http::now_ms;

const ACK_TIMEOUT: Duration = Duration::from_secs(10);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct SocketParams {
    pub app_id: String,
    pub user_id: String,
    pub access_token: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SbEvent {
    Message(SbMessage),
    Updated(SbMessage),
    Deleted {
        channel_url: String,
        message_id: i64,
    },
    Read {
        channel_url: String,
        user_id: String,
        ts: i64,
    },
    Typing {
        channel_url: String,
        user_id: String,
        started: bool,
    },
    Reaction {
        channel_url: String,
        message_id: i64,
        payload: Value,
    },
    ThreadInfo {
        channel_url: String,
        payload: Value,
    },
    Channel {
        channel_url: String,
        category: i64,
        payload: Value,
    },
    SessionExpiring {
        expires_in: i64,
    },
    Other(Frame),
    Disconnected {
        reason: String,
    },
}

impl SbEvent {
    pub fn from_frame(frame: Frame) -> Self {
        let p = &frame.payload;
        let channel_url = p["channel_url"].as_str().unwrap_or_default().to_string();
        match frame.code.as_str() {
            "MESG" | "FILE" | "ADMM" | "BRDM" => {
                parse_message(&frame).map_or(SbEvent::Other(frame), SbEvent::Message)
            }
            "MEDI" | "FEDI" | "AEDI" => {
                parse_message(&frame).map_or(SbEvent::Other(frame), SbEvent::Updated)
            }
            "DELM" => SbEvent::Deleted {
                channel_url,
                message_id: p["msg_id"]
                    .as_i64()
                    .or_else(|| p["message_id"].as_i64())
                    .unwrap_or_default(),
            },
            "READ" => SbEvent::Read {
                channel_url,
                user_id: p["user"]["user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                ts: p["ts"].as_i64().unwrap_or_default(),
            },
            "TPST" | "TPEN" => SbEvent::Typing {
                channel_url,
                user_id: p["user"]["user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                started: frame.code == "TPST",
            },
            "MRCT" => SbEvent::Reaction {
                channel_url,
                message_id: p["msg_id"].as_i64().unwrap_or_default(),
                payload: p.clone(),
            },
            "MTHD" => SbEvent::ThreadInfo {
                channel_url,
                payload: p.clone(),
            },
            "SYEV" => SbEvent::Channel {
                channel_url,
                category: p["cat"].as_i64().unwrap_or_default(),
                payload: p.clone(),
            },
            "EXPR" => SbEvent::SessionExpiring {
                expires_in: p["expires_in"].as_i64().unwrap_or_default(),
            },
            _ => SbEvent::Other(frame),
        }
    }
}

fn parse_message(frame: &Frame) -> Option<SbMessage> {
    let mut msg: SbMessage = serde_json::from_value::<SbMessage>(frame.payload.clone())
        .ok()?
        .normalized();
    if msg.kind.is_empty() {
        msg.kind = frame.code.clone();
    }
    Some(msg)
}

enum Command {
    Send {
        frame: Frame,
        ack: Option<oneshot::Sender<Result<Frame>>>,
    },
    Close,
}

#[derive(Clone)]
pub struct SendbirdSocket {
    commands: mpsc::Sender<Command>,
    seq: Arc<AtomicU64>,
}

pub struct FileSend<'a> {
    pub channel_url: &'a str,
    pub file: &'a super::rest::UploadedFile,
    pub name: &'a str,
    pub mime_type: &'a str,
    pub req_id: &'a str,
    pub parent_id: Option<i64>,
}

pub struct Connected {
    pub socket: SendbirdSocket,
    pub login: LoginInfo,
    pub events: mpsc::Receiver<SbEvent>,
}

pub fn connect_url(params: &SocketParams) -> String {
    let extra = "premium_feature_list,file_upload_size_limit,application_attributes,emoji_hash,multiple_file_send_max_size,notifications,message_template,ai_agent";
    format!(
        "wss://ws-{host}.sendbird.com/?p=JS&pv=&sv={SDK_VERSION}&ai={app}&user_id={user}&pmce=0&active=1&device_token_types={dtt}&SB-User-Agent={ua}&SB-SDK-User-Agent={sdkua}&Request-Sent-Timestamp={ts}&include_extra_data={extra}&use_local_cache=0&include_poll_details=1&config_ts=0",
        host = params.app_id.to_lowercase(),
        app = params.app_id,
        user = encode_component(&params.user_id),
        dtt = encode_component("gcm,huawei,apns"),
        ua = encode_component(&user_agent()),
        sdkua = encode_component(&sdk_user_agent()),
        ts = now_ms(),
        extra = encode_component(extra),
    )
}

impl SendbirdSocket {
    pub async fn connect(params: &SocketParams) -> Result<Connected> {
        let mut request = connect_url(params).into_client_request()?;
        let protocol = encode_component(&json!({ "token": params.access_token }).to_string());
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&protocol).map_err(|e| CoreError::Socket(e.to_string()))?,
        );
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await?;

        let login = timeout(LOGIN_TIMEOUT, async {
            while let Some(msg) = ws.next().await {
                let Message::Text(text) = msg? else { continue };
                for frame in Frame::decode_all(text.as_str()) {
                    if frame.code == "LOGI" {
                        if frame.is_error() {
                            return Err(CoreError::Sendbird {
                                code: frame.payload["code"].as_i64().unwrap_or_default(),
                                message: frame.payload["message"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                        return Ok(serde_json::from_value::<LoginInfo>(frame.payload)?);
                    }
                }
            }
            Err(CoreError::Socket("closed before LOGI".into()))
        })
        .await
        .map_err(|_| CoreError::Timeout)??;

        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(1024);
        let ping_every = Duration::from_secs(login.ping_interval.unwrap_or(15).max(5));
        let pong_wait = Duration::from_secs(login.pong_timeout.unwrap_or(5).max(2));
        tokio::spawn(run(ws, cmd_rx, event_tx, ping_every, pong_wait));

        Ok(Connected {
            socket: SendbirdSocket {
                commands: cmd_tx,
                seq: Arc::new(AtomicU64::new(1)),
            },
            login,
            events: event_rx,
        })
    }

    pub fn next_req_id(&self) -> String {
        format!(
            "rq-{}-{}",
            now_ms(),
            self.seq.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub async fn request(&self, code: &str, payload: Value) -> Result<Frame> {
        let req_id = self.next_req_id();
        self.request_with_id(code, payload, &req_id).await
    }

    pub async fn request_with_id(
        &self,
        code: &str,
        mut payload: Value,
        req_id: &str,
    ) -> Result<Frame> {
        payload["req_id"] = Value::String(req_id.to_string());
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Send {
                frame: Frame::new(code, payload),
                ack: Some(tx),
            })
            .await
            .map_err(|_| CoreError::Socket("socket task stopped".into()))?;
        let frame = timeout(ACK_TIMEOUT, rx)
            .await
            .map_err(|_| CoreError::Timeout)?
            .map_err(|_| CoreError::Socket("socket closed".into()))??;
        if frame.is_error() {
            return Err(CoreError::Sendbird {
                code: frame.payload["code"].as_i64().unwrap_or_default(),
                message: frame.payload["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            });
        }
        Ok(frame)
    }

    pub async fn fire(&self, code: &str, payload: Value) -> Result<()> {
        self.commands
            .send(Command::Send {
                frame: Frame::new(code, payload),
                ack: None,
            })
            .await
            .map_err(|_| CoreError::Socket("socket task stopped".into()))
    }

    pub async fn send_text(&self, channel_url: &str, text: &str) -> Result<SbMessage> {
        let req_id = self.next_req_id();
        self.send_message(channel_url, text, &req_id, None).await
    }

    pub async fn send_message(
        &self,
        channel_url: &str,
        text: &str,
        req_id: &str,
        parent_id: Option<i64>,
    ) -> Result<SbMessage> {
        let mut payload =
            json!({ "channel_url": channel_url, "message": text, "data": "", "custom_type": "" });
        if let Some(parent) = parent_id {
            payload["parent_message_id"] = json!(parent);
        }
        let frame = self.request_with_id("MESG", payload, req_id).await?;
        let mut msg: SbMessage = serde_json::from_value::<SbMessage>(frame.payload.clone())
            .map_err(|e| CoreError::Decode(format!("MESG ack: {e}")))?
            .normalized();
        if msg.kind.is_empty() {
            msg.kind = frame.code.clone();
        }
        if msg.req_id.is_none() {
            msg.req_id = Some(req_id.to_string());
        }
        if msg.channel_url.is_empty() {
            msg.channel_url = channel_url.to_string();
        }
        Ok(msg)
    }

    pub async fn edit_message(
        &self,
        channel_url: &str,
        message_id: i64,
        text: &str,
    ) -> Result<SbMessage> {
        self.edit_message_with_data(channel_url, message_id, text, None)
            .await
    }

    pub async fn edit_message_with_data(
        &self,
        channel_url: &str,
        message_id: i64,
        text: &str,
        data: Option<&str>,
    ) -> Result<SbMessage> {
        let mut payload =
            json!({ "channel_url": channel_url, "msg_id": message_id, "message": text });
        if let Some(data) = data {
            payload["data"] = json!(data);
            payload["mentioned_user_ids"] = json!([]);
        }
        let frame = self.request("MEDI", payload).await?;
        let mut msg: SbMessage = serde_json::from_value::<SbMessage>(frame.payload.clone())
            .map_err(|e| CoreError::Decode(format!("MEDI ack: {e}")))?
            .normalized();
        if msg.kind.is_empty() {
            msg.kind = "MESG".into();
        }
        if msg.channel_url.is_empty() {
            msg.channel_url = channel_url.to_string();
        }
        Ok(msg)
    }

    pub async fn send_file(&self, send: &FileSend<'_>) -> Result<SbMessage> {
        let mut payload = json!({
            "channel_url": send.channel_url,
            "url": send.file.url,
            "name": send.name,
            "type": send.mime_type,
            "size": send.file.size,
            "require_auth": send.file.require_auth,
            "custom": "",
            "custom_type": "",
        });
        if !send.file.thumbnails.is_null() {
            payload["thumbnails"] = send.file.thumbnails.clone();
        }
        if let Some(parent) = send.parent_id {
            payload["parent_message_id"] = json!(parent);
        }
        let frame = self.request_with_id("FILE", payload, send.req_id).await?;
        let mut msg: SbMessage = serde_json::from_value::<SbMessage>(frame.payload.clone())
            .map_err(|e| CoreError::Decode(format!("FILE ack: {e}")))?
            .normalized();
        msg.kind = "FILE".into();
        if msg.channel_url.is_empty() {
            msg.channel_url = send.channel_url.to_string();
        }
        if msg.req_id.is_none() {
            msg.req_id = Some(send.req_id.to_string());
        }
        Ok(msg)
    }

    pub async fn mark_read(&self, channel_url: &str) -> Result<()> {
        self.request("READ", json!({ "channel_url": channel_url }))
            .await
            .map(|_| ())
    }

    pub async fn typing(&self, channel_url: &str, started: bool) -> Result<()> {
        let code = if started { "TPST" } else { "TPEN" };
        self.fire(
            code,
            json!({ "channel_url": channel_url, "time": now_ms(), "req_id": "" }),
        )
        .await
    }

    pub async fn close(&self) {
        let _ = self.commands.send(Command::Close).await;
    }
}

async fn run(
    mut ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<SbEvent>,
    ping_every: Duration,
    pong_wait: Duration,
) {
    let mut pending: HashMap<String, oneshot::Sender<Result<Frame>>> = HashMap::new();
    let mut next_ping = Instant::now() + ping_every;
    let mut pong_deadline: Option<Instant> = None;
    let far = || Instant::now() + Duration::from_secs(3600);

    let reason = loop {
        tokio::select! {
            msg = ws.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(f))) => break format!("closed {:?}", f.map(|f| u16::from(f.code))),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => break e.to_string(),
                    None => break "stream ended".into(),
                };
                for frame in Frame::decode_all(text.as_str()) {
                    if frame.code == "PONG" {
                        pong_deadline = None;
                        continue;
                    }
                    if let Some(tx) = frame.req_id().and_then(|id| pending.remove(id)) {
                        let _ = tx.send(Ok(frame));
                        continue;
                    }
                    if frame.code == "NOOP" || frame.code == "LOGI" {
                        continue;
                    }
                    if events.send(SbEvent::from_frame(frame)).await.is_err() {
                        break;
                    }
                }
            }
            cmd = commands.recv() => {
                match cmd {
                    Some(Command::Send { frame, ack }) => {
                        if let (Some(tx), Some(id)) = (ack, frame.req_id()) {
                            pending.insert(id.to_string(), tx);
                        }
                        if let Err(e) = ws.send(Message::text(frame.encode())).await {
                            break e.to_string();
                        }
                        next_ping = Instant::now() + ping_every;
                    }
                    Some(Command::Close) | None => {
                        let _ = ws.close(None).await;
                        break "closed by client".into();
                    }
                }
            }
            _ = sleep_until(next_ping) => {
                let ping = Frame::new("PING", json!({ "id": now_ms(), "active": 1, "req_id": "" }));
                if let Err(e) = ws.send(Message::text(ping.encode())).await {
                    break e.to_string();
                }
                next_ping = Instant::now() + ping_every;
                pong_deadline.get_or_insert(Instant::now() + pong_wait);
            }
            _ = sleep_until(pong_deadline.unwrap_or_else(far)) => {
                break "pong timeout".into();
            }
        }
    };
    for (_, tx) in pending.drain() {
        let _ = tx.send(Err(CoreError::Socket(reason.clone())));
    }
    let _ = events.send(SbEvent::Disconnected { reason }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_carries_user_and_sdk_version() {
        let url = connect_url(&SocketParams {
            app_id: "ABC-1".into(),
            user_id: "u 1".into(),
            access_token: "t".into(),
        });
        assert!(url.starts_with("wss://ws-abc-1.sendbird.com/?p=JS&pv=&sv="));
        assert!(url.contains("&ai=ABC-1&user_id=u%201&"));
        assert!(url.contains("device_token_types=gcm%2Chuawei%2Capns"));
    }

    #[test]
    fn maps_frames_to_events() {
        let mesg = Frame::decode_all(
            r#"MESG{"msg_id":9,"channel_url":"c","message":"hi","user":{"user_id":"u"}}"#,
        );
        let SbEvent::Message(m) = SbEvent::from_frame(mesg[0].clone()) else {
            panic!("expected message")
        };
        assert_eq!(
            (m.id(), m.kind.as_str(), m.sender_id()),
            (9, "MESG", Some("u"))
        );
        let typing = Frame::decode_all(r#"TPEN{"channel_url":"c","user":{"user_id":"u"}}"#);
        assert_eq!(
            SbEvent::from_frame(typing[0].clone()),
            SbEvent::Typing {
                channel_url: "c".into(),
                user_id: "u".into(),
                started: false
            }
        );
    }
}
