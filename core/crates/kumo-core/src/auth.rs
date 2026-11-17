use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::{CoreError, Result};
use crate::http::now_ms;

const IDENTITY_TOOLKIT: &str = "https://identitytoolkit.googleapis.com/v1";
const SECURE_TOKEN: &str = "https://securetoken.googleapis.com/v1/token";
const REFRESH_MARGIN_MS: u64 = 120_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthSession {
    pub uid: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub api_key: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub expires_at_ms: u64,
}

impl AuthSession {
    pub fn needs_refresh(&self, now_ms: u64) -> bool {
        self.id_token.is_none() || self.expires_at_ms <= now_ms + REFRESH_MARGIN_MS
    }
}

pub trait SessionStore: Send + Sync {
    fn load(&self) -> Option<AuthSession>;
    fn save(&self, session: &AuthSession) -> Result<()>;
    fn clear(&self) -> Result<()>;
}

#[derive(Default)]
pub struct MemorySessionStore {
    session: std::sync::Mutex<Option<AuthSession>>,
}

impl SessionStore for MemorySessionStore {
    fn load(&self) -> Option<AuthSession> {
        self.session.lock().ok()?.clone()
    }

    fn save(&self, session: &AuthSession) -> Result<()> {
        if let Ok(mut s) = self.session.lock() {
            *s = Some(session.clone());
        }
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        if let Ok(mut s) = self.session.lock() {
            *s = None;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PasswordSignInResponse {
    local_id: String,
    email: Option<String>,
    display_name: Option<String>,
    id_token: String,
    refresh_token: String,
    expires_in: String,
}

#[derive(Deserialize)]
struct RefreshResponse {
    id_token: String,
    refresh_token: String,
    expires_in: String,
    user_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CustomTokenResponse {
    id_token: String,
    refresh_token: String,
    expires_in: String,
}

#[derive(Deserialize)]
struct LookupResponse {
    #[serde(default)]
    users: Vec<LookupUser>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LookupUser {
    local_id: String,
    email: Option<String>,
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct FirebaseErrorBody {
    error: FirebaseErrorDetail,
}

#[derive(Deserialize)]
struct FirebaseErrorDetail {
    message: String,
}

#[derive(Clone)]
pub struct FirebaseAuth {
    http: reqwest::Client,
}

impl FirebaseAuth {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn sign_in_with_password(
        &self,
        credentials: PasswordCredentials<'_>,
    ) -> Result<AuthSession> {
        let url = format!(
            "{IDENTITY_TOOLKIT}/accounts:signInWithPassword?key={}",
            credentials.api_key
        );
        let body = serde_json::json!({
            "email": credentials.email,
            "password": credentials.password,
            "returnSecureToken": true,
        });
        let res = self.http.post(url).json(&body).send().await?;
        let parsed: PasswordSignInResponse = parse_firebase(res).await?;
        Ok(AuthSession {
            uid: parsed.local_id,
            email: parsed.email,
            display_name: parsed.display_name,
            api_key: credentials.api_key.to_string(),
            refresh_token: parsed.refresh_token,
            id_token: Some(parsed.id_token),
            expires_at_ms: now_ms() + expires_in_ms(&parsed.expires_in),
        })
    }

    pub async fn sign_in_with_custom_token(
        &self,
        api_key: &str,
        token: &str,
    ) -> Result<AuthSession> {
        let url = format!("{IDENTITY_TOOLKIT}/accounts:signInWithCustomToken?key={api_key}");
        let body = serde_json::json!({ "token": token, "returnSecureToken": true });
        let res = self.http.post(url).json(&body).send().await?;
        let parsed: CustomTokenResponse = parse_firebase(res).await?;
        let lookup = self.lookup(api_key, &parsed.id_token).await?;
        Ok(AuthSession {
            uid: lookup.local_id,
            email: lookup.email,
            display_name: lookup.display_name,
            api_key: api_key.to_string(),
            refresh_token: parsed.refresh_token,
            id_token: Some(parsed.id_token),
            expires_at_ms: now_ms() + expires_in_ms(&parsed.expires_in),
        })
    }

    async fn lookup(&self, api_key: &str, id_token: &str) -> Result<LookupUser> {
        let url = format!("{IDENTITY_TOOLKIT}/accounts:lookup?key={api_key}");
        let res = self
            .http
            .post(url)
            .json(&serde_json::json!({ "idToken": id_token }))
            .send()
            .await?;
        let parsed: LookupResponse = parse_firebase(res).await?;
        parsed.users.into_iter().next().ok_or(CoreError::Auth {
            code: "USER_NOT_FOUND".into(),
        })
    }

    pub async fn refresh(&self, session: &AuthSession) -> Result<AuthSession> {
        let url = format!("{SECURE_TOKEN}?key={}", session.api_key);
        let body = serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": session.refresh_token,
        });
        let res = self.http.post(url).json(&body).send().await?;
        let parsed: RefreshResponse = parse_firebase(res).await?;
        if parsed.user_id != session.uid {
            return Err(CoreError::Auth {
                code: "USER_MISMATCH".into(),
            });
        }
        Ok(AuthSession {
            refresh_token: parsed.refresh_token,
            id_token: Some(parsed.id_token),
            expires_at_ms: now_ms() + expires_in_ms(&parsed.expires_in),
            ..session.clone()
        })
    }
}

pub struct PasswordCredentials<'a> {
    pub api_key: &'a str,
    pub email: &'a str,
    pub password: &'a str,
}

fn expires_in_ms(raw: &str) -> u64 {
    raw.parse::<u64>().unwrap_or(3600) * 1000
}

async fn parse_firebase<T: serde::de::DeserializeOwned>(res: reqwest::Response) -> Result<T> {
    let status = res.status();
    let text = res.text().await?;
    if status.is_success() {
        return Ok(serde_json::from_str(&text)?);
    }
    let code = serde_json::from_str::<FirebaseErrorBody>(&text)
        .map(|b| b.error.message)
        .unwrap_or_else(|_| format!("HTTP_{}", status.as_u16()));
    Err(CoreError::Auth { code })
}

pub struct Auth {
    firebase: FirebaseAuth,
    store: Arc<dyn SessionStore>,
    current: Mutex<Option<AuthSession>>,
    snapshot: std::sync::Mutex<Option<AuthSession>>,
}

impl Auth {
    pub fn new(firebase: FirebaseAuth, store: Arc<dyn SessionStore>) -> Self {
        let loaded = store.load();
        Self {
            firebase,
            store,
            current: Mutex::new(loaded.clone()),
            snapshot: std::sync::Mutex::new(loaded),
        }
    }

    pub async fn session(&self) -> Option<AuthSession> {
        self.current.lock().await.clone()
    }

    pub fn session_snapshot(&self) -> Option<AuthSession> {
        self.snapshot.lock().ok()?.clone()
    }

    fn publish(&self, session: Option<AuthSession>) {
        if let Ok(mut s) = self.snapshot.lock() {
            *s = session;
        }
    }

    pub async fn set_session(&self, session: AuthSession) -> Result<()> {
        self.store.save(&session)?;
        self.publish(Some(session.clone()));
        *self.current.lock().await = Some(session);
        Ok(())
    }

    pub async fn sign_in_with_password(
        &self,
        credentials: PasswordCredentials<'_>,
    ) -> Result<AuthSession> {
        let session = self.firebase.sign_in_with_password(credentials).await?;
        self.set_session(session.clone()).await?;
        Ok(session)
    }

    pub async fn sign_in_with_custom_token(
        &self,
        api_key: &str,
        token: &str,
    ) -> Result<AuthSession> {
        let session = self
            .firebase
            .sign_in_with_custom_token(api_key, token)
            .await?;
        self.set_session(session.clone()).await?;
        Ok(session)
    }

    pub async fn sign_out(&self) -> Result<()> {
        self.store.clear()?;
        self.publish(None);
        *self.current.lock().await = None;
        Ok(())
    }

    pub async fn uid(&self) -> Result<String> {
        self.session()
            .await
            .map(|s| s.uid)
            .ok_or(CoreError::NotSignedIn)
    }

    pub async fn id_token(&self) -> Result<String> {
        self.token(false).await
    }

    pub async fn refreshed_id_token(&self) -> Result<String> {
        self.token(true).await
    }

    async fn token(&self, force: bool) -> Result<String> {
        let mut guard = self.current.lock().await;
        let session = guard.as_ref().ok_or(CoreError::NotSignedIn)?;
        if !force
            && !session.needs_refresh(now_ms())
            && let Some(token) = &session.id_token
        {
            return Ok(token.clone());
        }
        let refreshed = self.firebase.refresh(session).await?;
        self.store.save(&refreshed)?;
        self.publish(Some(refreshed.clone()));
        let token = refreshed.id_token.clone().unwrap_or_default();
        *guard = Some(refreshed);
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(expires_at_ms: u64, id_token: Option<&str>) -> AuthSession {
        AuthSession {
            uid: "u".into(),
            email: None,
            display_name: None,
            api_key: "k".into(),
            refresh_token: "r".into(),
            id_token: id_token.map(Into::into),
            expires_at_ms,
        }
    }

    #[test]
    fn refreshes_before_expiry() {
        assert!(!session(1_000_000, Some("t")).needs_refresh(800_000));
        assert!(session(1_000_000, Some("t")).needs_refresh(900_000));
        assert!(session(u64::MAX, None).needs_refresh(0));
    }

    #[tokio::test]
    async fn uses_cached_token_without_network() {
        let store = Arc::new(MemorySessionStore::default());
        store.save(&session(u64::MAX / 2, Some("cached"))).unwrap();
        let auth = Auth::new(FirebaseAuth::new(reqwest::Client::new()), store);
        assert_eq!(auth.id_token().await.unwrap(), "cached");
        assert_eq!(auth.uid().await.unwrap(), "u");
    }
}
