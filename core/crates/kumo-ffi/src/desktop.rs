use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use kumo_core::desktop as core;
use kumo_core::http::HttpClient;
use kumo_core::ws;

use crate::notifications::{MyStatus, NotificationKinds, NotificationProfile};
use crate::{AppEvent, AppListener};

const POLL_EVERY: Duration = Duration::from_secs(3);
const STATUS_MESSAGE_LIMIT: usize = 100;
const COMMON_AREA: &str = "Common area";

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Availability {
    Available,
    Away,
    Focusing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Presence {
    Available,
    Away,
    Focusing,
    Idle,
    Offline,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DesktopRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DesktopFloor {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DesktopZone {
    pub id: String,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub open: bool,
    pub is_mine: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DesktopPerson {
    pub id: String,
    pub uid: String,
    pub name: String,
    pub avatar_url: String,
    pub x: f64,
    pub y: f64,
    pub zone_id: Option<String>,
    pub place: String,
    pub is_self: bool,
    pub presence: Presence,
    pub status_line: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DesktopState {
    pub floor_id: Option<String>,
    pub floor_name: String,
    pub floors: Vec<DesktopFloor>,
    pub place: String,
    pub self_zone_id: Option<String>,
    pub availability: Option<Availability>,
    pub presence: Presence,
    pub status_message: String,
    pub bounds: DesktopRect,
    pub viewport: DesktopRect,
    pub zones: Vec<DesktopZone>,
    pub people: Vec<DesktopPerson>,
    pub zone_mates: Vec<DesktopPerson>,
    pub audio: bool,
    pub video: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DesktopSnapshot {
    pub state: Option<DesktopState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayStatus {
    pub uid: String,
    pub mirroring: bool,
    pub chat_online: bool,
    pub ntfy: bool,
    pub ntfy_topic: Option<String>,
    pub ntfy_server: Option<String>,
    pub ntfy_url: Option<String>,
    pub apns: bool,
    pub phones: u32,
    pub desktop_connected: bool,
    pub notifications: NotificationProfile,
    pub my_status: MyStatus,
    pub relay_version: Option<String>,
    pub compatibility: RelayCompatibility,
    pub agents: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RelayCompatibility {
    Compatible,
    RelayOutdated,
    AppOutdated,
}

impl From<core::RelayCompatibility> for RelayCompatibility {
    fn from(c: core::RelayCompatibility) -> Self {
        match c {
            core::RelayCompatibility::Compatible => Self::Compatible,
            core::RelayCompatibility::RelayOutdated => Self::RelayOutdated,
            core::RelayCompatibility::AppOutdated => Self::AppOutdated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NearbyMessage {
    pub id: String,
    pub sender_name: String,
    pub text: String,
    pub mine: bool,
    pub sent_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NearbyChat {
    pub zone_id: Option<String>,
    pub zone_name: String,
    pub messages: Vec<NearbyMessage>,
}

impl From<core::NearbyChat> for NearbyChat {
    fn from(chat: core::NearbyChat) -> Self {
        let zone_name = if chat.zone_id.is_some() && !chat.zone_name.is_empty() {
            chat.zone_name
        } else {
            COMMON_AREA.to_string()
        };
        NearbyChat {
            zone_id: chat.zone_id,
            zone_name,
            messages: chat
                .messages
                .into_iter()
                .map(|m| NearbyMessage {
                    id: m.id,
                    sender_name: m.sender_name,
                    text: m.text,
                    mine: m.mine,
                    sent_at_ms: m.sent_at_ms,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayPairing {
    pub url: String,
    pub secret: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum RelayError {
    #[error("{0}")]
    NotPaired(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
}

impl From<core::RelayError> for RelayError {
    fn from(e: core::RelayError) -> Self {
        let message = e.to_string();
        match e {
            core::RelayError::NotPaired => RelayError::NotPaired(message),
            core::RelayError::Unauthorized => RelayError::Unauthorized(message),
            core::RelayError::Rejected(_) | core::RelayError::StaleLease => {
                RelayError::Rejected(message)
            }
            core::RelayError::Unavailable(_)
            | core::RelayError::NotFound(_)
            | core::RelayError::NotInCall => RelayError::Unavailable(message),
            core::RelayError::Http { .. }
            | core::RelayError::Network(_)
            | core::RelayError::Protocol(_) => RelayError::Failed(message),
        }
    }
}

#[uniffi::export(with_foreign)]
pub trait RelayStorage: Send + Sync {
    fn load(&self) -> Option<RelayPairing>;
    fn save(&self, pairing: RelayPairing);
    fn clear(&self);
    fn save_mirroring(&self, enabled: bool);
}

#[uniffi::export]
pub fn relay_compatibility_note(compatibility: RelayCompatibility) -> Option<String> {
    match compatibility {
        RelayCompatibility::Compatible => None,
        RelayCompatibility::RelayOutdated => Some(
            "Better Kumo on the computer is older than this app. Update it there, or some settings won't save.".into(),
        ),
        RelayCompatibility::AppOutdated => Some(
            "Better Kumo on the computer is newer than this app. Update the app to use everything it offers.".into(),
        ),
    }
}

/// What the ntfy server field shows for the relay's server: empty for ntfy.sh.
#[uniffi::export]
pub fn ntfy_server_field(server: Option<String>) -> String {
    server
        .filter(|s| s != core::DEFAULT_NTFY_SERVER)
        .unwrap_or_default()
}

#[uniffi::export]
pub fn availability_choices() -> Vec<Availability> {
    ws::Availability::ALL
        .into_iter()
        .map(Availability::from)
        .collect()
}

#[uniffi::export]
pub fn availability_label(availability: Availability) -> String {
    ws::Availability::from(availability).label().to_string()
}

#[uniffi::export]
pub fn presence_label(presence: Presence) -> String {
    core::Presence::from(presence).label().to_string()
}

#[uniffi::export]
pub fn presence_legend() -> Vec<Presence> {
    core::Presence::LEGEND
        .into_iter()
        .map(Presence::from)
        .collect()
}

impl From<ws::Availability> for Availability {
    fn from(a: ws::Availability) -> Self {
        match a {
            ws::Availability::Available => Availability::Available,
            ws::Availability::Away => Availability::Away,
            ws::Availability::Focusing => Availability::Focusing,
        }
    }
}

impl From<Availability> for ws::Availability {
    fn from(a: Availability) -> Self {
        match a {
            Availability::Available => ws::Availability::Available,
            Availability::Away => ws::Availability::Away,
            Availability::Focusing => ws::Availability::Focusing,
        }
    }
}

impl From<core::Presence> for Presence {
    fn from(p: core::Presence) -> Self {
        match p {
            core::Presence::Available => Presence::Available,
            core::Presence::Away => Presence::Away,
            core::Presence::Focusing => Presence::Focusing,
            core::Presence::Idle => Presence::Idle,
            core::Presence::Offline => Presence::Offline,
        }
    }
}

impl From<Presence> for core::Presence {
    fn from(p: Presence) -> Self {
        match p {
            Presence::Available => core::Presence::Available,
            Presence::Away => core::Presence::Away,
            Presence::Focusing => core::Presence::Focusing,
            Presence::Idle => core::Presence::Idle,
            Presence::Offline => core::Presence::Offline,
        }
    }
}

pub fn presence_of(availability: &str) -> Presence {
    core::Presence::of(availability.parse().ok(), false).into()
}

impl From<core::Rect> for DesktopRect {
    fn from(r: core::Rect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }
    }
}

impl From<core::RelayStatus> for RelayStatus {
    fn from(s: core::RelayStatus) -> Self {
        Self {
            ntfy_url: s.ntfy_url(),
            compatibility: s.compatibility().into(),
            relay_version: s.version,
            agents: s.agents,
            uid: s.uid,
            mirroring: s.mirroring,
            chat_online: s.chat_online,
            ntfy: s.ntfy,
            ntfy_topic: s.ntfy_topic,
            ntfy_server: s.ntfy_server,
            apns: s.apns,
            phones: s.phones,
            desktop_connected: s.desktop_connected,
            notifications: s.notifications.into(),
            my_status: s.my_status.into(),
        }
    }
}

fn place(state: &core::DesktopState, zone_id: Option<&str>) -> String {
    zone_id
        .and_then(|id| state.zone(id))
        .map(|z| z.name.clone())
        .unwrap_or_else(|| COMMON_AREA.into())
}

fn person(state: &core::DesktopState, p: &core::DesktopPerson) -> DesktopPerson {
    let presence = p.presence();
    let status_line = if p.message.is_empty() || p.idle {
        presence.label().to_string()
    } else {
        format!("{} · {}", presence.label(), p.message)
    };
    DesktopPerson {
        id: p.id.clone(),
        uid: p.uid.clone(),
        name: p.name.clone(),
        avatar_url: p.avatar.clone(),
        x: p.x,
        y: p.y,
        zone_id: p.zone_id.clone(),
        place: place(state, p.zone_id.as_deref()),
        is_self: p.is_self,
        presence: presence.into(),
        status_line,
    }
}

impl From<&core::DesktopState> for DesktopState {
    fn from(s: &core::DesktopState) -> Self {
        let mine = s.self_zone_id();
        let mut people: Vec<DesktopPerson> = s.people.iter().map(|p| person(s, p)).collect();
        people.sort_by(|a, b| {
            a.is_self
                .cmp(&b.is_self)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.id.cmp(&b.id))
        });
        let mut zone_mates: Vec<DesktopPerson> =
            s.zone_mates().into_iter().map(|p| person(s, p)).collect();
        zone_mates.sort_by(|a, b| a.name.cmp(&b.name));
        DesktopState {
            floor_id: s.floor_id.clone(),
            floor_name: s.floor_name().unwrap_or_default().to_string(),
            floors: s
                .floors
                .iter()
                .map(|f| DesktopFloor {
                    id: f.id.clone(),
                    name: f.name.clone(),
                })
                .collect(),
            place: place(s, mine),
            self_zone_id: mine.map(String::from),
            availability: s.availability().map(Availability::from),
            presence: s.presence().into(),
            status_message: s.me.as_ref().map(|m| m.message.clone()).unwrap_or_default(),
            bounds: s.bounds().into(),
            viewport: s.viewport().into(),
            zones: s
                .zones
                .iter()
                .map(|z| DesktopZone {
                    id: z.id.clone(),
                    name: z.name.clone(),
                    x: z.x,
                    y: z.y,
                    w: z.w,
                    h: z.h,
                    open: z.open,
                    is_mine: Some(z.id.as_str()) == mine,
                })
                .collect(),
            people,
            zone_mates,
            audio: s.audio,
            video: s.video,
        }
    }
}

#[derive(Default)]
struct Watch {
    state: Option<core::DesktopState>,
    error: Option<String>,
    watchers: u32,
    poller: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    runtime: tokio::runtime::Handle,
    http: HttpClient,
    storage: Arc<dyn RelayStorage>,
    listener: Arc<dyn AppListener>,
    pairing: Mutex<Option<core::RelayPairing>>,
    watch: Mutex<Watch>,
}

#[derive(uniffi::Object)]
pub struct RelayClient {
    inner: Arc<Inner>,
}

pub struct RelayClientDeps {
    pub runtime: tokio::runtime::Handle,
    pub http: HttpClient,
    pub storage: Arc<dyn RelayStorage>,
    pub listener: Arc<dyn AppListener>,
}

impl RelayClient {
    pub fn new(deps: RelayClientDeps) -> Arc<Self> {
        let pairing = deps.storage.load().map(|p| core::RelayPairing {
            url: p.url,
            secret: p.secret,
        });
        Arc::new(Self {
            inner: Arc::new(Inner {
                runtime: deps.runtime,
                http: deps.http,
                storage: deps.storage,
                listener: deps.listener,
                pairing: Mutex::new(pairing),
                watch: Mutex::new(Watch::default()),
            }),
        })
    }
}

#[uniffi::export]
impl RelayClient {
    pub fn pair(&self, link: String) -> bool {
        let Some(pairing) = core::RelayPairing::from_link(&link) else {
            return false;
        };
        self.inner.storage.save(RelayPairing {
            url: pairing.url.clone(),
            secret: pairing.secret.clone(),
        });
        if let Ok(mut current) = self.inner.pairing.lock() {
            *current = Some(pairing);
        }
        self.inner.reset_desktop();
        true
    }

    pub fn is_paired(&self) -> bool {
        self.inner.client().is_ok()
    }

    pub fn paired_url(&self) -> Option<String> {
        self.inner.client().ok().map(|c| c.pairing().url.clone())
    }

    pub fn forget(&self) {
        self.inner.storage.clear();
        if let Ok(mut current) = self.inner.pairing.lock() {
            *current = None;
        }
        self.inner.reset_desktop();
    }

    pub async fn status(&self) -> Result<RelayStatus, RelayError> {
        self.mirroring_reply(|c| async move { c.status().await })
            .await
    }

    pub async fn set_mirroring(&self, enabled: bool) -> Result<RelayStatus, RelayError> {
        self.mirroring_reply(move |c| async move { c.set_mirroring(enabled).await })
            .await
    }

    pub async fn set_agents(&self, enabled: bool) -> Result<RelayStatus, RelayError> {
        self.mirroring_reply(move |c| async move { c.set_agents(enabled).await })
            .await
    }

    /// `server` is `None` unless the person edited it, so a topic-only save
    /// keeps whatever server the relay has, even one this app would not accept.
    pub async fn set_ntfy(
        &self,
        topic: Option<String>,
        server: Option<String>,
    ) -> Result<RelayStatus, RelayError> {
        let topic = topic
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        // Turning ntfy off drops the server, so a bad server draft must not block it.
        let server = match (&topic, server) {
            (Some(_), Some(server)) => Some(core::ntfy_server_from_input(&server)?),
            _ => None,
        };
        self.mirroring_reply(move |c| async move { c.set_ntfy(topic, server).await })
            .await
    }

    pub async fn set_notification_kinds(
        &self,
        status: MyStatus,
        kinds: NotificationKinds,
    ) -> Result<RelayStatus, RelayError> {
        let request =
            core::NotificationsRequest::status(status.into(), core::NotificationKinds::from(kinds));
        self.put_notifications(request).await
    }

    pub async fn set_notification_profile(
        &self,
        profile: NotificationProfile,
    ) -> Result<RelayStatus, RelayError> {
        let request = core::NotificationsRequest::from(core::NotificationProfile::from(profile));
        self.put_notifications(request).await
    }

    pub async fn send_test_alert(&self) -> Result<RelayStatus, RelayError> {
        self.mirroring_reply(|c| async move { c.send_test_alert().await })
            .await
    }

    pub async fn register_device(&self, token: String, name: String) -> Result<(), RelayError> {
        self.mirroring_reply(move |c| async move { c.register_device(&token, &name).await })
            .await
            .map(|_| ())
    }

    pub fn watch_desktop(&self) {
        let start = {
            let Ok(mut watch) = self.inner.watch.lock() else {
                return;
            };
            watch.watchers += 1;
            watch.poller.is_none()
        };
        if start {
            let poller = self.inner.runtime.spawn(poll(Arc::downgrade(&self.inner)));
            if let Ok(mut watch) = self.inner.watch.lock() {
                watch.poller = Some(poller);
            }
        }
    }

    pub fn unwatch_desktop(&self) {
        if let Ok(mut watch) = self.inner.watch.lock() {
            watch.watchers = watch.watchers.saturating_sub(1);
            if watch.watchers == 0
                && let Some(poller) = watch.poller.take()
            {
                poller.abort();
            }
        }
    }

    pub fn desktop(&self) -> DesktopSnapshot {
        self.inner
            .watch
            .lock()
            .map(|w| DesktopSnapshot {
                state: w.state.as_ref().map(DesktopState::from),
                error: w.error.clone(),
            })
            .unwrap_or(DesktopSnapshot {
                state: None,
                error: None,
            })
    }

    pub async fn refresh_desktop(&self) -> Result<DesktopState, RelayError> {
        let state = self
            .desktop_action(|c| async move { c.desktop().await })
            .await?;
        Ok(DesktopState::from(&state))
    }

    pub async fn tap(&self, x: f64, y: f64) -> Result<(), RelayError> {
        let state = self.current_or_fetch().await?;
        let Some(target) = state.tap_target(x, y) else {
            return Ok(());
        };
        self.go(target).await
    }

    pub async fn go_to(&self, person_id: String) -> Result<(), RelayError> {
        let state = self.current_or_fetch().await?;
        let Some(target) = state.person_target(&person_id) else {
            return Ok(());
        };
        self.go(target).await
    }

    pub async fn enter_zone(&self, zone_id: String) -> Result<(), RelayError> {
        self.go(core::DesktopTarget::Zone(zone_id)).await
    }

    pub async fn switch_floor(&self, floor_id: String) -> Result<(), RelayError> {
        self.desktop_action(move |c| async move { c.switch_floor(&floor_id).await })
            .await
            .map(|_| ())
    }

    pub async fn follow(&self, user_id: Option<String>) -> Result<(), RelayError> {
        self.desktop_action(move |c| async move { c.follow(user_id).await })
            .await
            .map(|_| ())
    }

    pub async fn set_status(
        &self,
        availability: Availability,
        message: String,
    ) -> Result<(), RelayError> {
        let request = core::StatusRequest {
            availability: availability.into(),
            message: status_message(&message),
        };
        self.desktop_action(move |c| async move { c.set_status(&request).await })
            .await
            .map(|_| ())
    }

    pub async fn set_status_message(&self, message: String) -> Result<(), RelayError> {
        let availability = self
            .current_or_fetch()
            .await?
            .availability()
            .unwrap_or(ws::Availability::Available);
        self.set_status(availability.into(), message).await
    }

    pub async fn set_media(
        &self,
        audio: Option<bool>,
        video: Option<bool>,
    ) -> Result<(), RelayError> {
        let media = core::MediaRequest { audio, video };
        self.desktop_action(move |c| async move { c.set_media(media).await })
            .await
            .map(|_| ())
    }

    pub async fn go_home(&self) -> Result<(), RelayError> {
        self.desktop_action(|c| async move { c.go_home().await })
            .await
            .map(|_| ())
    }

    pub async fn join_person(&self, person_id: String) -> Result<(), RelayError> {
        self.desktop_action(move |c| async move { c.join_person(&person_id).await })
            .await
            .map(|_| ())
    }

    pub async fn nudge(&self, user_id: String) -> Result<(), RelayError> {
        self.call_relay(move |c| async move { c.nudge(&user_id).await })
            .await
    }

    pub async fn nearby_chat(&self) -> Result<NearbyChat, RelayError> {
        self.call_relay(|c| async move { c.nearby_chat().await })
            .await
            .map(NearbyChat::from)
    }

    pub async fn send_nearby(&self, message: String) -> Result<(), RelayError> {
        self.call_relay(move |c| async move { c.send_nearby(&message).await })
            .await
    }
}

impl RelayClient {
    pub(crate) async fn put_notifications(
        &self,
        request: core::NotificationsRequest,
    ) -> Result<RelayStatus, RelayError> {
        self.mirroring_reply(move |c| async move { c.set_notifications(&request).await })
            .await
    }

    pub(crate) fn core_client(&self) -> Option<core::RelayClient> {
        self.inner.client().ok()
    }

    async fn call_relay<T, F, Fut>(&self, f: F) -> Result<T, RelayError>
    where
        T: Send + 'static,
        F: FnOnce(core::RelayClient) -> Fut,
        Fut: std::future::Future<Output = Result<T, core::RelayError>> + Send + 'static,
    {
        let client = self.inner.client()?;
        self.inner
            .runtime
            .spawn(f(client))
            .await
            .map_err(|e| RelayError::Failed(e.to_string()))?
            .map_err(RelayError::from)
    }

    async fn mirroring_reply<F, Fut>(&self, f: F) -> Result<RelayStatus, RelayError>
    where
        F: FnOnce(core::RelayClient) -> Fut,
        Fut: std::future::Future<Output = Result<core::RelayStatus, core::RelayError>>
            + Send
            + 'static,
    {
        let status = self.call_relay(f).await?;
        self.inner.storage.save_mirroring(status.mirroring);
        Ok(status.into())
    }

    async fn desktop_action<F, Fut>(&self, f: F) -> Result<core::DesktopState, RelayError>
    where
        F: FnOnce(core::RelayClient) -> Fut,
        Fut: std::future::Future<Output = Result<core::DesktopState, core::RelayError>>
            + Send
            + 'static,
    {
        let result = self.call_relay(f).await;
        self.inner.record(&result);
        result
    }

    async fn go(&self, target: core::DesktopTarget) -> Result<(), RelayError> {
        self.desktop_action(move |c| async move { c.go(target).await })
            .await
            .map(|_| ())
    }

    async fn current_or_fetch(&self) -> Result<core::DesktopState, RelayError> {
        let cached = self.inner.watch.lock().ok().and_then(|w| w.state.clone());
        match cached {
            Some(state) => Ok(state),
            None => {
                self.desktop_action(|c| async move { c.desktop().await })
                    .await
            }
        }
    }
}

impl Inner {
    fn client(&self) -> Result<core::RelayClient, core::RelayError> {
        self.pairing
            .lock()
            .ok()
            .and_then(|p| p.clone())
            .map(|p| core::RelayClient::new(self.http.clone(), p))
            .ok_or(core::RelayError::NotPaired)
    }

    fn reset_desktop(self: &Arc<Self>) {
        let watching = self
            .watch
            .lock()
            .map(|mut w| {
                w.state = None;
                w.error = None;
                w.watchers > 0
            })
            .unwrap_or_default();
        self.listener.on_event(AppEvent::DesktopChanged);
        if watching {
            let me = self.clone();
            self.runtime.spawn(async move { me.refresh().await });
        }
    }

    async fn refresh(&self) {
        let result = match self.client() {
            Ok(client) => client.desktop().await.map_err(RelayError::from),
            Err(e) => Err(e.into()),
        };
        self.record(&result);
    }

    fn record(&self, result: &Result<core::DesktopState, RelayError>) {
        let changed = {
            let Ok(mut watch) = self.watch.lock() else {
                return;
            };
            let (state, error) = match result {
                Ok(state) => (Some(state.clone()), None),
                Err(e) => (watch.state.clone(), Some(e.to_string())),
            };
            let changed = watch.state != state || watch.error != error;
            watch.state = state;
            watch.error = error;
            changed
        };
        if changed {
            self.listener.on_event(AppEvent::DesktopChanged);
        }
    }
}

async fn poll(inner: Weak<Inner>) {
    loop {
        let Some(me) = inner.upgrade() else {
            return;
        };
        me.refresh().await;
        drop(me);
        tokio::time::sleep(POLL_EVERY).await;
    }
}

fn status_message(message: &str) -> String {
    message.trim().chars().take(STATUS_MESSAGE_LIMIT).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> serde_json::Value {
        serde_json::json!({
            "floorId": "f1",
            "width": 2000,
            "height": 1200,
            "floors": [{ "id": "f1", "name": "Ground" }],
            "self": { "id": "c-me", "x": 410, "y": 220, "zoneId": "z1", "availability": "away", "message": "lunch" },
            "zones": [{ "id": "z1", "name": "Blue Room", "x": 300, "y": 100, "w": 400, "h": 300, "open": true }],
            "people": [
                { "id": "c-me", "name": "Me", "x": 410, "y": 220, "zoneId": "z1", "self": true, "availability": "away", "message": "lunch" },
                { "id": "c-zed", "name": "Zed", "x": 420, "y": 230, "zoneId": "z1", "availability": "available", "message": "hi" },
                { "id": "c-amy", "name": "Amy", "x": 900, "y": 900, "availability": "focusing", "idle": true, "message": "hidden" }
            ],
            "audio": false,
            "video": true
        })
    }

    #[test]
    fn desktop_state_is_rendered_with_places_labels_and_order() {
        let state = DesktopState::from(&core::DesktopState::from_page(page()).unwrap());
        assert_eq!(state.place, "Blue Room");
        assert_eq!(state.floor_name, "Ground");
        assert_eq!(state.availability, Some(Availability::Away));
        assert_eq!(state.status_message, "lunch");
        assert!(state.zones[0].is_mine);
        let names: Vec<&str> = state.people.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Amy", "Zed", "Me"]);
        assert_eq!(state.people[0].place, COMMON_AREA);
        assert_eq!(state.people[0].presence, Presence::Idle);
        assert_eq!(state.people[0].status_line, "Idle");
        assert_eq!(state.people[1].status_line, "Available · hi");
        assert_eq!(state.zone_mates.len(), 1);
        assert_eq!(state.viewport.w, 720.0);
    }

    #[test]
    fn status_messages_are_trimmed_and_capped() {
        assert_eq!(status_message("  hi  "), "hi");
        assert_eq!(status_message(&"x".repeat(150)).len(), STATUS_MESSAGE_LIMIT);
    }

    #[test]
    fn labels_come_from_the_core() {
        assert_eq!(availability_choices().len(), 3);
        assert_eq!(availability_label(Availability::Focusing), "Focusing");
        assert_eq!(presence_label(Presence::Offline), "Offline");
        assert_eq!(presence_legend().last(), Some(&Presence::Idle));
    }
}
