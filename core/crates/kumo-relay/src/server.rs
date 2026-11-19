use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use kumo_core::desktop::{
    AgentsRequest, DesktopCall, DesktopState, DeviceRequest, FloorRequest, FollowRequest, Handoff,
    LeaseRequest, MediaRequest, MirroringRequest, MoveRequest, NearbyChat, NearbyChatRequest,
    NotificationsRequest, NtfyRequest, NudgeRequest, OkReply, RELAY_API, RelayError, RelayStatus,
    StatusRequest, ViewingRequest, ZoneRequest, routes,
};
use kumo_core::ws::{ClientOp, Command, JoinData};
use serde::de::DeserializeOwned;

use crate::Relay;
use crate::desktop::{NotInCall, StaleLease};
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
        .route(routes::PAIR, get(pair_page))
        .route("/settings", get(settings_page))
        .route(
            "/settings/meetings",
            get(meetings_settings).put(put_meetings_settings),
        )
        .route(
            "/settings/meetings/module",
            post(install_meetings_module).delete(remove_meetings_module),
        )
        .route(
            "/settings/meetings/recording",
            post(start_meeting).delete(stop_meeting),
        )
        .route("/settings/meetings/folder", post(choose_meetings_folder))
        .route("/settings/meetings/list", get(meeting_list))
        .route("/settings/meetings/transcribe", post(transcribe_meetings))
        .route("/settings/meetings/open", post(open_meeting))
        .route("/settings/mcp", get(mcp_setup))
        .route(routes::STATUS, get(status))
        .route(routes::MIRRORING, put(set_mirroring))
        .route(routes::AGENTS, put(set_agents))
        .route(routes::NTFY, put(set_ntfy))
        .route(routes::NOTIFICATIONS, put(set_notifications))
        .route(routes::VIEWING, put(set_viewing))
        .route(routes::DEVICE, put(put_device).delete(delete_device))
        .route(routes::TEST, post(test))
        .route(routes::DESKTOP, get(desktop_state))
        .route(routes::MOVE, post(desktop_move))
        .route(routes::ZONE, post(desktop_zone))
        .route(routes::FLOOR, post(desktop_floor))
        .route(routes::FOLLOW, post(desktop_follow))
        .route(routes::AVAILABILITY, post(desktop_status))
        .route(routes::MEDIA, post(desktop_media))
        .route(routes::NUDGE, post(desktop_nudge))
        .route(routes::CALL, get(desktop_call))
        .route(routes::HANDOFF, post(desktop_handoff))
        .route(routes::RECLAIM, post(desktop_reclaim))
        .route(routes::LEASE, post(desktop_lease))
        .route(routes::HOME, post(desktop_home))
        .route(
            routes::NEARBY_CHAT,
            get(desktop_nearby_chat).post(desktop_send_nearby),
        )
        .with_state(AppState {
            relay,
            secret: Arc::new(secret),
            config: Arc::new(config),
        })
}

type LocalResult<T> = Result<T, (StatusCode, String)>;

fn local_page(peer: &std::net::SocketAddr, headers: &HeaderMap, page: &str) -> LocalResult<()> {
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let host = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let local_host = matches!(host, "127.0.0.1" | "localhost" | "[::1]");
    if !peer.ip().is_loopback() || !local_host {
        return Err((
            StatusCode::FORBIDDEN,
            format!("open this page on the computer itself: http://127.0.0.1:<port>{page}"),
        ));
    }
    Ok(())
}

fn local_call(peer: &std::net::SocketAddr, headers: &HeaderMap, port: u16) -> LocalResult<()> {
    local_page(peer, headers, "/settings")?;
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let from_page = header("x-better-kumo") == Some("settings");
    let origin_ok = header("origin").is_none_or(|origin| {
        origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
    });
    if !from_page || !origin_ok {
        return Err((
            StatusCode::FORBIDDEN,
            "only the settings page may do this".into(),
        ));
    }
    Ok(())
}

async fn pair_page(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<axum::response::Html<String>> {
    local_page(&peer, &headers, "/pair")?;
    Ok(axum::response::Html(crate::pair_page::render(&s.config)))
}

async fn settings_page(
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<axum::response::Html<&'static str>> {
    local_page(&peer, &headers, "/settings")?;
    Ok(axum::response::Html(include_str!("settings.html")))
}

#[derive(serde::Serialize)]
struct MeetingsSettings {
    available: bool,
    recording: bool,
    summary: String,
    status: Option<kumo_meetings::MeetingsStatus>,
    config: Option<kumo_meetings::MeetingsConfig>,
    folder: String,
    default_folder: String,
}

fn meetings_body(relay: &Relay) -> MeetingsSettings {
    let status = relay.meetings();
    let config = relay.meetings_config();
    MeetingsSettings {
        available: status.is_some(),
        recording: relay.meeting_recording(),
        summary: status
            .as_ref()
            .map(|s| s.summary(kumo_core::http::now_ms() as i64))
            .unwrap_or_default(),
        folder: config
            .as_ref()
            .map(|c| c.folder().display().to_string())
            .unwrap_or_default(),
        default_folder: kumo_meetings::MeetingsConfig::default()
            .folder()
            .display()
            .to_string(),
        status,
        config: config.map(|mut c| {
            c.transcriber = None;
            c
        }),
    }
}

fn rejected(e: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, format!("{e:#}"))
}

async fn meetings_settings(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    Ok(Json(meetings_body(&s.relay)))
}

async fn put_meetings_settings(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    let config: kumo_meetings::MeetingsConfig = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad settings: {e}")))?;
    s.relay.set_meetings_config(config).map_err(rejected)?;
    Ok(Json(meetings_body(&s.relay)))
}

async fn meeting_list(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<Vec<kumo_meetings::MeetingEntry>>> {
    local_call(&peer, &headers, s.config.port())?;
    Ok(Json(listed(&s.relay).await?))
}

async fn listed(relay: &Arc<Relay>) -> LocalResult<Vec<kumo_meetings::MeetingEntry>> {
    let relay = relay.clone();
    tokio::task::spawn_blocking(move || relay.meeting_list())
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))
}

async fn transcribe_meetings(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<Vec<kumo_meetings::MeetingEntry>>> {
    local_call(&peer, &headers, s.config.port())?;
    s.relay.transcribe_pending_meetings().map_err(rejected)?;
    Ok(Json(listed(&s.relay).await?))
}

#[derive(serde::Deserialize)]
struct OpenMeeting {
    name: String,
}

async fn open_meeting(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> LocalResult<StatusCode> {
    local_call(&peer, &headers, s.config.port())?;
    let OpenMeeting { name } = serde_json::from_slice(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad request: {e}")))?;
    let meeting = listed(&s.relay)
        .await?
        .into_iter()
        .find(|m| m.name == name)
        .ok_or((
            StatusCode::BAD_REQUEST,
            format!("no meeting named \"{name}\""),
        ))?;
    let path = meeting.transcript.unwrap_or(meeting.dir);
    open::that_detached(&path)
        .map_err(|e| rejected(anyhow::anyhow!("cannot open {}: {e}", path.display())))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mcp_setup(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<crate::mcp_setup::McpSetup>> {
    local_call(&peer, &headers, s.config.port())?;
    Ok(Json(crate::mcp_setup::current()))
}

#[derive(serde::Serialize)]
struct ChosenFolder {
    folder: Option<String>,
}

async fn choose_meetings_folder(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<ChosenFolder>> {
    local_call(&peer, &headers, s.config.port())?;
    let start = s
        .relay
        .meetings_config()
        .map(|c| c.folder())
        .unwrap_or_else(|| kumo_meetings::MeetingsConfig::default().folder());
    let chosen = crate::folder_picker::choose_folder(&start)
        .await
        .map_err(rejected)?;
    Ok(Json(ChosenFolder {
        folder: chosen.map(|p| p.display().to_string()),
    }))
}

async fn install_meetings_module(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    s.relay.install_meetings_module().map_err(rejected)?;
    Ok(Json(meetings_body(&s.relay)))
}

async fn start_meeting(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    s.relay.start_meeting_now().map_err(rejected)?;
    Ok(Json(meetings_body(&s.relay)))
}

async fn stop_meeting(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    s.relay.stop_meeting_now();
    Ok(Json(meetings_body(&s.relay)))
}

async fn remove_meetings_module(
    State(s): State<AppState>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> LocalResult<Json<MeetingsSettings>> {
    local_call(&peer, &headers, s.config.port())?;
    s.relay.remove_meetings_module().await.map_err(rejected)?;
    Ok(Json(meetings_body(&s.relay)))
}

struct ApiError(RelayError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, self.0.to_string()).into_response()
    }
}

impl From<RelayError> for ApiError {
    fn from(e: RelayError) -> Self {
        ApiError(e)
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn parse<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|e| ApiError(RelayError::rejected_body(&e)))
}

fn authorized(headers: &HeaderMap, secret: &str) -> Result<(), ApiError> {
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
        Err(ApiError(RelayError::Unauthorized))
    }
}

async fn status_body(relay: &Relay) -> Json<RelayStatus> {
    let my_status = relay.my_status().await;
    let status = relay.status();
    Json(RelayStatus {
        uid: status.uid,
        mirroring: status.mirroring,
        chat_online: status.chat_online,
        ntfy: status.ntfy,
        apns: status.apns,
        phones: u32::try_from(status.phones).unwrap_or(u32::MAX),
        desktop_connected: status.desktop_connected,
        ntfy_topic: status.ntfy_topic,
        ntfy_server: status.ntfy_server,
        notifications: status.notifications,
        my_status,
        api: RELAY_API,
        version: Some(env!("CARGO_PKG_VERSION").into()),
        agents: Some(relay.agents_enabled()),
    })
}

async fn status(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    Ok(status_body(&s.relay).await)
}

async fn set_ntfy(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    let request: NtfyRequest = parse(&body)?;
    let current = s.relay.status().ntfy_server;
    let target = request.target(current.as_deref())?;
    s.relay.set_ntfy(target).map_err(|e| {
        ApiError(RelayError::Unavailable(format!(
            "cannot save ntfy settings: {e}"
        )))
    })?;
    Ok(status_body(&s.relay).await)
}

async fn set_mirroring(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    let request: MirroringRequest = parse(&body)?;
    s.relay.set_mirroring(request.enabled);
    Ok(status_body(&s.relay).await)
}

async fn set_agents(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    let request: AgentsRequest = parse(&body)?;
    s.relay.set_agents(request.enabled).map_err(|e| {
        ApiError(RelayError::Unavailable(format!(
            "cannot save the AI agents setting: {e}"
        )))
    })?;
    Ok(status_body(&s.relay).await)
}

async fn set_notifications(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    let request: NotificationsRequest = parse(&body)?;
    s.relay.set_notifications(&request);
    Ok(status_body(&s.relay).await)
}

async fn set_viewing(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<OkReply> {
    authorized(&headers, &s.secret)?;
    let request: ViewingRequest = parse(&body)?;
    s.relay.set_viewing(request.channel_url);
    Ok(Json(OkReply::OK))
}

async fn put_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    body: Bytes,
) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    if token.len() < 32 || !token.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError(RelayError::Rejected("bad device token".into())));
    }
    let request: DeviceRequest = parse(&body)?;
    s.relay.devices.upsert(Device {
        token,
        name: request.name,
        enabled: request.enabled,
        registered_at: kumo_core::http::now_ms(),
    });
    Ok(status_body(&s.relay).await)
}

async fn delete_device(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<StatusCode, ApiError> {
    authorized(&headers, &s.secret)?;
    if s.relay.devices.remove(&token) {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError(RelayError::NotFound("unknown device".into())))
    }
}

async fn test(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<RelayStatus> {
    authorized(&headers, &s.secret)?;
    s.relay.send_test_alert().await;
    Ok(status_body(&s.relay).await)
}

fn desktop(s: &AppState, headers: &HeaderMap) -> Result<Arc<crate::desktop::Desktop>, ApiError> {
    authorized(headers, &s.secret)?;
    s.relay.desktop.clone().ok_or(ApiError(RelayError::NotFound(
        "desktop bridge is disabled".into(),
    )))
}

fn fail(e: anyhow::Error) -> ApiError {
    if e.downcast_ref::<StaleLease>().is_some() {
        return ApiError(RelayError::StaleLease);
    }
    if e.downcast_ref::<NotInCall>().is_some() {
        return ApiError(RelayError::NotInCall);
    }
    ApiError(RelayError::Unavailable(e.to_string()))
}

async fn send_then_state(d: &crate::desktop::Desktop, op: ClientOp) -> ApiResult<DesktopState> {
    d.send(&op).await.map_err(fail)?;
    after(d).await
}

async fn after(d: &crate::desktop::Desktop) -> ApiResult<DesktopState> {
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    d.state().await.map(Json).map_err(fail)
}

async fn desktop_state(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    d.state().await.map(Json).map_err(fail)
}

async fn desktop_move(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let spot: MoveRequest = parse(&body)?;
    send_then_state(&d, ClientOp::Command(Command::Destination(spot))).await
}

async fn desktop_zone(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let request: ZoneRequest = parse(&body)?;
    send_then_state(
        &d,
        ClientOp::Command(Command::NavigateToZone {
            zone_id: request.zone_id,
        }),
    )
    .await
}

async fn desktop_floor(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let request: FloorRequest = parse(&body)?;
    send_then_state(
        &d,
        ClientOp::Join(JoinData {
            room_name: Some(request.floor_id),
            ..JoinData::default()
        }),
    )
    .await
}

async fn desktop_follow(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let request: FollowRequest = parse(&body)?;
    let command = match request.user_id {
        Some(user_id) => Command::FollowUser { user_id },
        None => Command::stop_following(),
    };
    send_then_state(&d, ClientOp::Command(command)).await
}

async fn desktop_status(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let request: StatusRequest = parse(&body)?;
    send_then_state(&d, ClientOp::SetUserStatus(request)).await
}

async fn desktop_media(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    let request: MediaRequest = parse(&body)?;
    d.media(request).await.map_err(fail)?;
    after(&d).await
}

async fn desktop_nudge(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<OkReply> {
    let d = desktop(&s, &headers)?;
    let request: NudgeRequest = parse(&body)?;
    d.send(&ClientOp::Nudge {
        user_id: request.user_id,
    })
    .await
    .map_err(fail)?;
    Ok(Json(OkReply::OK))
}

async fn desktop_call(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<DesktopCall> {
    let d = desktop(&s, &headers)?;
    d.call_info().await.map(Json).map_err(fail)
}

async fn desktop_handoff(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Handoff> {
    let d = desktop(&s, &headers)?;
    d.handoff().await.map(Json).map_err(fail)
}

async fn desktop_lease(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<OkReply> {
    let d = desktop(&s, &headers)?;
    let request: LeaseRequest = parse(&body)?;
    d.renew(&request.lease_id)
        .map_err(|_| ApiError(RelayError::StaleLease))?;
    Ok(Json(OkReply::OK))
}

async fn desktop_reclaim(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<OkReply> {
    let d = desktop(&s, &headers)?;
    let request: LeaseRequest = parse(&body)?;
    d.reclaim(&request.lease_id).await.map_err(fail)?;
    Ok(Json(OkReply::OK))
}

async fn desktop_home(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<DesktopState> {
    let d = desktop(&s, &headers)?;
    d.go_home().await.map_err(fail)?;
    after(&d).await
}

async fn desktop_nearby_chat(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<NearbyChat> {
    let d = desktop(&s, &headers)?;
    d.nearby_chat().await.map(Json).map_err(fail)
}

async fn desktop_send_nearby(
    State(s): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<OkReply> {
    let d = desktop(&s, &headers)?;
    let request: NearbyChatRequest = parse(&body)?;
    d.send_nearby(request.text()?).await.map_err(fail)?;
    Ok(Json(OkReply::OK))
}
