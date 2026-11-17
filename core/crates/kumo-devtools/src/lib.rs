mod session;

use anyhow::{Context, anyhow};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

pub use session::import_session;

pub const DEFAULT_PORT: u16 = 9222;

pub fn local_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn local_ws_url(devtools_url: &str, ws_url: &str) -> String {
    let host = devtools_url
        .split("://")
        .nth(1)
        .unwrap_or(devtools_url)
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

fn is_desktop_app_page(target: &Value) -> bool {
    target["type"] == "page"
        && target["url"]
            .as_str()
            .is_some_and(|u| u.contains("kumospace.com"))
}

pub async fn find_desktop_app_page(devtools_url: &str) -> anyhow::Result<String> {
    let base = devtools_url.trim_end_matches('/');
    let targets: Vec<Value> = reqwest::get(format!("{base}/json/list"))
        .await
        .with_context(|| format!("Kumospace desktop is not reachable at {base}"))?
        .json()
        .await
        .with_context(|| format!("Kumospace desktop at {base} sent an unreadable page list"))?;
    let ws_url = targets
        .iter()
        .find(|t| is_desktop_app_page(t))
        .and_then(|t| t["webSocketDebuggerUrl"].as_str())
        .ok_or_else(|| anyhow!("Kumospace desktop has no kumospace.com page open yet"))?;
    Ok(local_ws_url(base, ws_url))
}

pub async fn desktop_app_page_open(devtools_url: &str) -> bool {
    find_desktop_app_page(devtools_url).await.is_ok()
}

pub async fn evaluate(ws_url: &str, expression: &str) -> anyhow::Result<Value> {
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url).await?;
    let request = json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": { "expression": expression, "returnByValue": true },
    });
    socket.send(Message::text(request.to_string())).await?;
    let value = loop {
        let msg = socket
            .next()
            .await
            .ok_or_else(|| anyhow!("DevTools closed"))??;
        let Message::Text(text) = msg else { continue };
        let reply: Value = serde_json::from_str(text.as_str())?;
        if reply["id"] == 1 {
            break reply["result"]["result"]["value"].clone();
        }
    };
    let _ = socket.close(None).await;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub(crate) async fn fake_devtools(
        targets: impl Fn(&str) -> Value + Send + 'static,
        evaluated: Value,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut peeked = [0u8; 32];
                let n = stream.peek(&mut peeked).await.unwrap_or(0);
                if peeked[..n].starts_with(b"GET /json/list") {
                    let mut request = [0u8; 4096];
                    let _ = stream.read(&mut request).await;
                    let body = targets(&addr.to_string()).to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                    continue;
                }
                let evaluated = evaluated.clone();
                tokio::spawn(async move {
                    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        let request: Value = serde_json::from_str(text.as_str()).unwrap();
                        let reply = json!({
                            "id": request["id"],
                            "result": { "result": { "value": evaluated } },
                        });
                        let _ = socket.send(Message::text(reply.to_string())).await;
                    }
                });
            }
        });
        format!("http://{addr}")
    }

    pub(crate) fn desktop_app_targets(_: &str) -> Value {
        json!([
            {
                "type": "service_worker",
                "url": "https://app.kumospace.com/sw.js",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/SW",
            },
            {
                "type": "page",
                "url": "file:///splash.html",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/SPLASH",
            },
            {
                "type": "page",
                "url": "https://app.kumospace.com/office",
                "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/OFFICE",
            },
        ])
    }

    #[tokio::test]
    async fn finds_the_desktop_app_page_through_the_devtools_host() {
        let devtools = fake_devtools(desktop_app_targets, Value::Null).await;
        let ws_url = find_desktop_app_page(&devtools).await.unwrap();
        let host = devtools.trim_start_matches("http://");
        assert_eq!(ws_url, format!("ws://{host}/devtools/page/OFFICE"));
        assert!(desktop_app_page_open(&format!("{devtools}/")).await);
    }

    #[tokio::test]
    async fn no_desktop_app_page_yet() {
        let devtools = fake_devtools(
            |_| json!([{ "type": "page", "url": "about:blank" }]),
            Value::Null,
        )
        .await;
        let error = find_desktop_app_page(&devtools).await.unwrap_err();
        assert!(error.to_string().contains("no kumospace.com page"));
        assert!(!desktop_app_page_open(&devtools).await);
    }

    #[tokio::test]
    async fn desktop_app_not_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let devtools = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert!(!desktop_app_page_open(&devtools).await);
    }

    #[tokio::test]
    async fn evaluates_on_the_desktop_app_page() {
        let devtools = fake_devtools(desktop_app_targets, json!({ "answer": 42 })).await;
        let ws_url = find_desktop_app_page(&devtools).await.unwrap();
        assert_eq!(
            evaluate(&ws_url, "1").await.unwrap(),
            json!({ "answer": 42 })
        );
    }

    #[test]
    fn ws_url_follows_devtools_host() {
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

    #[test]
    fn local_url_uses_the_port() {
        assert_eq!(local_url(DEFAULT_PORT), "http://127.0.0.1:9222");
    }
}
