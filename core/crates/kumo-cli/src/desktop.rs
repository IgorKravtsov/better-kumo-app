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

pub async fn import_session(port: u16) -> anyhow::Result<AuthSession> {
    let targets: Vec<Value> = reqwest::get(format!("http://127.0.0.1:{port}/json/list"))
        .await
        .with_context(|| format!("Kumospace desktop is not listening on port {port}; run `node apps/watcher/launcher.js launch`"))?
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
        .ok_or_else(|| anyhow!("no kumospace.com page in the desktop app"))?
        .to_string();

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
        bail!("the desktop app is not signed in");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_firebase_web_user() {
        let user = json!({
            "uid": "u1",
            "email": "a@b.c",
            "apiKey": "KEY",
            "stsTokenManager": {"refreshToken": "r", "accessToken": "a", "expirationTime": 5}
        });
        let s = session_from_firebase_user("firebase:authUser:KEY:[DEFAULT]", &user).unwrap();
        assert_eq!(s.uid, "u1");
        assert_eq!(s.api_key, "KEY");
        assert_eq!(s.expires_at_ms, 5);
    }
}
