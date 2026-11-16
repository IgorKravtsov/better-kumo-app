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
    config: Arc<crate::Config>,
}

pub fn router(relay: Arc<Relay>, config: crate::Config) -> Router {
    let secret = config.secret.clone();
    Router::new()
        .route("/pair", get(pair_page))
        .route("/v1/status", get(status))
        .route("/v1/enabled", put(set_enabled))
        .route("/v1/devices/{token}", put(put_device).delete(delete_device))
        .route("/v1/test", post(test))
        .route("/v1/desktop", get(desktop_state))
        .route("/v1/desktop/move", post(desktop_move))
        .route("/v1/desktop/zone", post(desktop_zone))
        .route("/v1/desktop/floor", post(desktop_floor))
        .route("/v1/desktop/follow", post(desktop_follow))
        .route("/v1/desktop/status", post(desktop_status))
        .route("/v1/desktop/media", post(desktop_media))
        .route("/v1/desktop/nudge", post(desktop_nudge))
        .route("/v1/desktop/call", get(desktop_call))
        .route("/v1/desktop/handoff", post(desktop_handoff))
        .route("/v1/desktop/reclaim", post(desktop_reclaim))
        .route("/v1/desktop/home", post(desktop_home))
        .with_state(AppState {
            relay,
            secret: Arc::new(secret),
            config: Arc::new(config),
        })
}

async fn pair_page(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> Result<axum::response::Html<String>, (StatusCode, String)> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let local_host = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    if !peer.ip().is_loopback() || !local_host {
        return Err((
            StatusCode::FORBIDDEN,
            "open this page on the computer itself: http://127.0.0.1:<port>/pair".into(),
        ));
    }
    Ok(axum::response::Html(crate::pair_page::render(&s.config)))
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
        "desktop": relay.desktop.as_ref().is_some_and(|d| d.connected()),
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

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn desktop(
    s: &AppState,
    headers: &HeaderMap,
) -> Result<Arc<crate::desktop::Desktop>, (StatusCode, String)> {
    authorized(headers, &s.secret).map_err(|c| (c, "unauthorized".into()))?;
    s.relay
        .desktop
        .clone()
        .ok_or((StatusCode::NOT_FOUND, "desktop bridge is disabled".into()))
}

fn fail(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::SERVICE_UNAVAILABLE, e.to_string())
}

async fn after(d: &crate::desktop::Desktop) -> ApiResult {
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    d.state().await.map(Json).map_err(fail)
}

async fn desktop_state(State(s): State<AppState>, headers: HeaderMap) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.state().await.map(Json).map_err(fail)
}

#[derive(Deserialize)]
struct MoveBody {
    x: f64,
    y: f64,
}

async fn desktop_move(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<MoveBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    if !(b.x.is_finite() && b.y.is_finite()) || b.x < 0.0 || b.y < 0.0 {
        return Err((StatusCode::BAD_REQUEST, "bad position".into()));
    }
    d.command(json!({ "action": "destination", "x": b.x, "y": b.y }))
        .await
        .map_err(fail)?;
    after(&d).await
}

#[derive(Deserialize)]
struct ZoneBody {
    zone_id: String,
}

async fn desktop_zone(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<ZoneBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.command(json!({ "action": "navigateToZone", "zoneId": b.zone_id }))
        .await
        .map_err(fail)?;
    after(&d).await
}

#[derive(Deserialize)]
struct FloorBody {
    floor_id: String,
}

async fn desktop_floor(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<FloorBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.send(&json!({ "op": "join", "data": { "roomName": b.floor_id } }))
        .await
        .map_err(fail)?;
    after(&d).await
}

#[derive(Deserialize)]
struct UserBody {
    user_id: Option<String>,
}

async fn desktop_follow(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<UserBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    let data = match b.user_id {
        Some(id) => json!({ "action": "followUser", "userId": id }),
        None => json!({ "action": "stopFollowing", "userId": "" }),
    };
    d.command(data).await.map_err(fail)?;
    after(&d).await
}

#[derive(Deserialize)]
struct StatusBody {
    availability: String,
    #[serde(default)]
    message: String,
}

async fn desktop_status(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<StatusBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    if !["available", "away", "focusing"].contains(&b.availability.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            "availability must be available, away or focusing".into(),
        ));
    }
    d.send(&json!({ "op": "setUserStatus", "data": { "availability": b.availability, "message": b.message } }))
        .await
        .map_err(fail)?;
    after(&d).await
}

#[derive(Deserialize)]
struct MediaBody {
    audio: Option<bool>,
    video: Option<bool>,
}

async fn desktop_media(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<MediaBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.media(b.audio, b.video).await.map_err(fail)?;
    after(&d).await
}

async fn desktop_nudge(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<UserBody>,
) -> ApiResult {
    let d = desktop(&s, &headers)?;
    let id = b
        .user_id
        .ok_or((StatusCode::BAD_REQUEST, "user_id required".into()))?;
    d.send(&json!({ "op": "nudge", "data": { "userId": id } }))
        .await
        .map_err(fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn desktop_call(State(s): State<AppState>, headers: HeaderMap) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.call_info().await.map(Json).map_err(fail)
}

async fn desktop_handoff(State(s): State<AppState>, headers: HeaderMap) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.handoff().await.map(Json).map_err(fail)
}

async fn desktop_reclaim(State(s): State<AppState>, headers: HeaderMap) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.reclaim().await.map_err(fail)?;
    Ok(Json(json!({ "ok": true })))
}

async fn desktop_home(State(s): State<AppState>, headers: HeaderMap) -> ApiResult {
    let d = desktop(&s, &headers)?;
    d.go_home().await.map_err(fail)?;
    after(&d).await
}
