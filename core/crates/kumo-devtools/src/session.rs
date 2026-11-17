use anyhow::{anyhow, bail};
use kumo_core::AuthSession;
use serde_json::Value;

use crate::{evaluate, find_desktop_app_page};

const READ_AUTH_USER: &str = r#"(() => {
  const key = Object.keys(localStorage).find((k) => k.startsWith("firebase:authUser:"));
  if (!key) return null;
  return { key, value: localStorage.getItem(key) };
})()"#;

pub async fn import_session(devtools_url: &str) -> anyhow::Result<AuthSession> {
    let ws_url = find_desktop_app_page(devtools_url).await?;
    let value = evaluate(&ws_url, READ_AUTH_USER).await?;
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::tests::{desktop_app_targets, fake_devtools};

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

    #[tokio::test]
    async fn imports_the_desktop_app_session() {
        let user = json!({
            "uid": "u1",
            "stsTokenManager": {"refreshToken": "r", "expirationTime": 7}
        });
        let stored = json!({ "key": "firebase:authUser:KEY:[DEFAULT]", "value": user.to_string() });
        let devtools = fake_devtools(desktop_app_targets, stored).await;
        let s = import_session(&devtools).await.unwrap();
        assert_eq!(s.uid, "u1");
        assert_eq!(s.api_key, "KEY");
        assert_eq!(s.refresh_token, "r");
    }

    #[tokio::test]
    async fn desktop_app_signed_out() {
        let devtools = fake_devtools(desktop_app_targets, Value::Null).await;
        let error = import_session(&devtools).await.unwrap_err();
        assert!(error.to_string().contains("not signed in"));
    }
}
