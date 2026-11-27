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
    target["type"] == "page" && target["url"].as_str().is_some_and(is_kumospace_url)
}

fn is_kumospace_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();
    matches!(scheme, "http" | "https")
        && (host == "kumospace.com" || host.ends_with(".kumospace.com"))
}

async fn desktop_app_targets(devtools_url: &str) -> anyhow::Result<Vec<Value>> {
    let base = devtools_url.trim_end_matches('/');
    let targets: Vec<Value> = reqwest::get(format!("{base}/json/list"))
        .await
        .with_context(|| format!("Kumospace desktop is not reachable at {base}"))?
        .json()
        .await
        .with_context(|| format!("Kumospace desktop at {base} sent an unreadable page list"))?;
    Ok(targets.into_iter().filter(is_desktop_app_page).collect())
}

pub async fn find_desktop_app_page(devtools_url: &str) -> anyhow::Result<String> {
    find_desktop_app_page_in(devtools_url, None).await
}

/// Prefers the page showing `space`, so the relay drives the office it was set up for;
/// without one open it takes the first desktop app page, as before.
pub async fn find_desktop_app_page_in(
    devtools_url: &str,
    space: Option<&str>,
) -> anyhow::Result<String> {
    let targets = desktop_app_targets(devtools_url).await?;
    let in_space = |t: &&Value| {
        space
            .is_some_and(|space| t["url"].as_str().and_then(space_in_url).as_deref() == Some(space))
    };
    let target = targets
        .iter()
        .find(in_space)
        .or_else(|| targets.first())
        .ok_or_else(|| anyhow!("Kumospace desktop has no kumospace.com page open yet"))?;
    let ws_url = target["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| anyhow!("Kumospace desktop has no kumospace.com page open yet"))?;
    Ok(local_ws_url(devtools_url.trim_end_matches('/'), ws_url))
}

/// Non-space routes also yield candidates, so callers must check each is a real space.
pub async fn desktop_app_spaces(devtools_url: &str) -> anyhow::Result<Vec<String>> {
    let mut spaces: Vec<String> = Vec::new();
    for target in desktop_app_targets(devtools_url).await? {
        if let Some(space) = target["url"].as_str().and_then(space_in_url)
            && !spaces.contains(&space)
        {
            spaces.push(space);
        }
    }
    Ok(spaces)
}

fn space_in_url(url: &str) -> Option<String> {
    let path = url.split_once("://")?.1.split_once('/')?.1;
    let segment = path.split(['/', '?', '#']).next()?;
    (!segment.is_empty()).then(|| segment.to_string())
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
    async fn prefers_the_page_of_the_given_space() {
        let devtools = fake_devtools(
            |_| {
                json!([
                    {
                        "type": "page",
                        "url": "https://www.kumospace.com/app",
                        "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/APP",
                    },
                    {
                        "type": "page",
                        "url": "https://www.kumospace.com/acme?x=1",
                        "webSocketDebuggerUrl": "ws://127.0.0.1:9222/devtools/page/ACME",
                    },
                ])
            },
            Value::Null,
        )
        .await;
        let page = |space| {
            let devtools = devtools.clone();
            async move { find_desktop_app_page_in(&devtools, space).await.unwrap() }
        };
        assert!(page(Some("acme")).await.ends_with("/devtools/page/ACME"));
        assert!(page(Some("other")).await.ends_with("/devtools/page/APP"));
        assert!(page(None).await.ends_with("/devtools/page/APP"));
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
    fn only_kumospace_hosts_are_desktop_app_pages() {
        assert!(is_kumospace_url("https://www.kumospace.com/acme"));
        assert!(is_kumospace_url("https://kumospace.com"));
        assert!(is_kumospace_url("https://app.kumospace.com:443/office"));
        assert!(!is_kumospace_url(
            "https://example.org/acme?next=kumospace.com"
        ));
        assert!(!is_kumospace_url("https://kumospace.com.evil.org/acme"));
        assert!(!is_kumospace_url("https://notkumospace.com/acme"));
        assert!(!is_kumospace_url("https://kumospace.com@evil.org/acme"));
        assert!(!is_kumospace_url("file:///kumospace.com/acme"));
    }

    #[test]
    fn space_is_the_first_path_segment() {
        assert_eq!(
            space_in_url("https://www.kumospace.com/acme?x=1").as_deref(),
            Some("acme")
        );
        assert_eq!(
            space_in_url("https://www.kumospace.com/acme/room#a").as_deref(),
            Some("acme")
        );
        assert_eq!(space_in_url("https://www.kumospace.com/"), None);
        assert_eq!(space_in_url("https://www.kumospace.com"), None);
    }

    #[tokio::test]
    async fn reads_the_space_of_the_open_page() {
        let devtools = fake_devtools(
            |_| {
                json!([
                    { "type": "page", "url": "https://www.kumospace.com/acme" },
                    { "type": "page", "url": "https://www.kumospace.com/app" },
                    { "type": "page", "url": "https://www.kumospace.com/acme/room" },
                ])
            },
            Value::Null,
        )
        .await;
        assert_eq!(
            desktop_app_spaces(&devtools).await.unwrap(),
            ["acme", "app"]
        );
    }

    #[test]
    fn local_url_uses_the_port() {
        assert_eq!(local_url(DEFAULT_PORT), "http://127.0.0.1:9222");
    }
}
