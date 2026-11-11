use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::{CoreError, Result};
use crate::ws::{CLOSE_CODE_BANNED, ClientOp, ConnectParams, ServerOp};

#[derive(Debug, Clone, PartialEq)]
pub enum SpaceEvent {
    Op(ServerOp),
    Malformed(String),
    Closed { code: Option<u16>, banned: bool },
}

pub struct SpaceConnection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl SpaceConnection {
    pub async fn connect(params: &ConnectParams) -> Result<Self> {
        let url = params.url().map_err(|e| CoreError::Socket(e.to_string()))?;
        let (socket, _) = tokio_tungstenite::connect_async(url.as_str()).await?;
        Ok(Self { socket })
    }

    pub async fn send(&mut self, op: &ClientOp) -> Result<()> {
        self.socket.send(Message::text(op.to_frame())).await?;
        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<SpaceEvent> {
        loop {
            let msg = match self.socket.next().await? {
                Ok(m) => m,
                Err(_) => {
                    return Some(SpaceEvent::Closed {
                        code: None,
                        banned: false,
                    });
                }
            };
            match msg {
                Message::Text(text) => {
                    return Some(match ServerOp::parse(text.as_str()) {
                        Ok(op) => SpaceEvent::Op(op),
                        Err(e) => SpaceEvent::Malformed(e.to_string()),
                    });
                }
                Message::Close(frame) => {
                    let code = frame.map(|f| u16::from(f.code));
                    return Some(SpaceEvent::Closed {
                        code,
                        banned: code == Some(CLOSE_CODE_BANNED),
                    });
                }
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
    }

    pub async fn close(mut self) {
        let _ = self
            .socket
            .close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: CloseCode::Normal,
                reason: "".into(),
            }))
            .await;
    }
}
