use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use kumo_core::chat::alert::{Alert, AlertKind};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Relay;
use crate::devices::Device;

#[derive(Clone)]
struct AppState {
    relay: Arc<Relay>,
    secret: Arc<String>,
}

pub fn router(relay: Arc<Relay>, secret: String) -> Router {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/enabled", put(set_enabled))
        .route("/v1/devices/{token}", put(put_device).delete(delete_device))
        .route("/v1/test", post(test))
        .with_state(AppState {
            relay,
            secret: Arc::new(secret),
        })
}

fn authorized(headers: &HeaderMap, secret: &str) -> Result<(), StatusCode> {
    let given = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    let matches = given.len() == secret.len()
        && given
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if matches {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

fn status_body(relay: &Relay) -> Value {
    json!({
        "uid": relay.uid,
        "enabled": relay.enabled.load(Ordering::SeqCst),
        "online": relay.online.load(Ordering::SeqCst),
        "ntfy": relay.ntfy.is_some(),
        "apns": relay.apns.is_some(),
        "devices": relay.devices.all().len(),
    })
}

async fn status(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    authorized(&headers, &s.secret)?;
    Ok(Json(status_body(&s.relay)))
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

async fn set_enabled(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<EnabledBody>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers, &s.secret)?;
    s.relay.set_enabled(body.enabled);
    Ok(Json(status_body(&s.relay)))
}

#[derive(Deserialize)]
struct DeviceBody {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    name: String,
}

fn default_true() -> bool {
    true
}

async fn put_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(body): Json<DeviceBody>,
) -> Result<Json<Value>, StatusCode> {
    authorized(&headers, &s.secret)?;
    if token.len() < 32 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    s.relay.devices.upsert(Device {
        token,
        name: body.name,
        enabled: body.enabled,
        registered_at: kumo_core::http::now_ms(),
    });
    Ok(Json(status_body(&s.relay)))
}

async fn delete_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<StatusCode, StatusCode> {
    authorized(&headers, &s.secret)?;
    Ok(if s.relay.devices.remove(&token) {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}

async fn test(State(s): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    authorized(&headers, &s.secret)?;
    let was = s.relay.enabled.swap(true, Ordering::SeqCst);
    s.relay
        .dispatch(&Alert {
            kind: AlertKind::Message,
            title: "Kumowatch".into(),
            body: "Test notification from your relay".into(),
            channel_url: None,
        })
        .await;
    s.relay.enabled.store(was, Ordering::SeqCst);
    Ok(Json(status_body(&s.relay)))
}
