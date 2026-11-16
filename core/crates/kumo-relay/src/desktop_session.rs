use anyhow::{Context, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use kumo_core::AuthSession;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const READ_AUTH_USER: &str = r#"(() => {
  const key = Object.keys(localStorage).find((k) => k.startsWith("firebase:authUser:"));
  if (!key) return null;
  return { key, value: localStorage.getItem(key) };
})()"#;

pub async fn import_desktop_session(cdp_url: &str) -> anyhow::Result<AuthSession> {
    let base = cdp_url.trim_end_matches('/');
    let targets: Vec<Value> = reqwest::get(format!("{base}/json/list"))
        .await
        .with_context(|| format!("Kumospace desktop is not reachable at {base}"))?
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
        .ok_or_else(|| anyhow!("Kumospace desktop has no kumospace.com page open yet"))?
        .to_string();
    let ws_url = crate::desktop::local_ws_url(base, &ws_url);
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_url.as_str()).await?;
    let request = json!({
        "id": 1,
        "method": "Runtime.evaluate",
        "params": { "expression": READ_AUTH_USER, "returnByValue": true },
    });
    socket.send(Message::text(request.to_string())).await?;
    let value = loop {
        let msg = socket.next().await.ok_or_else(|| anyhow!("CDP closed"))??;
        let Message::Text(text) = msg else { continue };
        let reply: Value = serde_json::from_str(text.as_str())?;
        if reply["id"] == 1 {
            break reply["result"]["result"]["value"].clone();
        }
    };
    let _ = socket.close(None).await;
    if value.is_null() {
        bail!("Kumospace desktop is not signed in");
    }
    let key = value["key"].as_str().unwrap_or_default();
    let user: Value = serde_json::from_str(value["value"].as_str().unwrap_or("null"))?;
    session_from_firebase_user(key, &user)
}

fn session_from_firebase_user(key: &str, user: &Value) -> anyhow::Result<AuthSession> {
    let api_key = user["apiKey"]
        .as_str()
        .map(String::from)
        .or_else(|| key.split(':').nth(2).map(String::from))
        .ok_or_else(|| anyhow!("no api key in the stored user"))?;
    let tokens = &user["stsTokenManager"];
    Ok(AuthSession {
        uid: user["uid"]
            .as_str()
            .ok_or_else(|| anyhow!("no uid"))?
            .to_string(),
        email: user["email"].as_str().map(String::from),
        display_name: user["displayName"].as_str().map(String::from),
        api_key,
        refresh_token: tokens["refreshToken"]
            .as_str()
            .ok_or_else(|| anyhow!("no refresh token"))?
            .to_string(),
        id_token: tokens["accessToken"].as_str().map(String::from),
        expires_at_ms: tokens["expirationTime"].as_u64().unwrap_or_default(),
    })
}
