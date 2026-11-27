pub mod access;
pub const UI_CSS: &str = include_str!("ui.css");
pub mod config;
pub mod desktop;
pub mod devices;
pub mod events;
mod folder_picker;
mod idle;
pub mod mcp_setup;
mod meetings;
pub mod network;
pub mod pair_page;
pub mod phone;
pub mod push;
mod server;
pub mod viewing;
pub mod webhooks;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context;
use kumo_core::auth::MemorySessionStore;
use kumo_core::chat::alert::{Alert, AlertKind, message_alert};
use kumo_core::chat::notifications::{MyStatus, NotificationProfile, NotificationsRequest};
use kumo_core::chat::{
    ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore, ConnectionState,
    IncomingMessage, SendbirdTransport,
};
use kumo_core::desktop::{DesktopState, PageError};
use kumo_core::events::{EventsPage, MessageScope, OfficeEvent, WatchRules, Who};
use kumo_core::http::{build_http_client, now_ms};
use kumo_core::{Auth, AuthSession, FileSessionStore, FirebaseAuth, KumoApi, KumoConfig};
use kumo_meetings::{MeetingEntry, Meetings, MeetingsConfig, MeetingsStatus};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub use config::Config;
use devices::Devices;
use phone::PhoneActivity;
use push::{Apns, Ntfy, Outcome};
use viewing::Viewing;

pub struct Relay {
    devices: Devices,
    apns: Option<Apns>,
    ntfy: RwLock<Option<Ntfy>>,
    config_path: Option<PathBuf>,
    mirroring: AtomicBool,
    notifications: RwLock<NotificationProfile>,
    online: AtomicBool,
    state_path: PathBuf,
    agents_path: PathBuf,
    ai_note_saving: std::sync::Mutex<()>,
    config_saving: std::sync::Mutex<()>,
    watch_path: PathBuf,
    chat: std::sync::OnceLock<ChatEngine>,
    events: events::EventLog,
    webhooks: webhooks::Webhooks,
    uid: String,
    desktop: Option<Arc<desktop::Desktop>>,
    viewing: Viewing,
    phone_activity: PhoneActivity,
    meetings: Option<Arc<Meetings>>,
    stopping: watch::Sender<bool>,
    /// Set by the tray app: 0 none (a standalone relay), 1 cannot update itself, 2 can.
    auto_update_support: std::sync::atomic::AtomicU8,
    access: access::NetworkAccess,
    config: std::sync::OnceLock<Config>,
}

pub struct RelayParts {
    pub uid: String,
    pub data_dir: PathBuf,
    pub apns: Option<Apns>,
    pub ntfy: Option<Ntfy>,
    pub config_path: Option<PathBuf>,
    pub agents_path: PathBuf,
    pub watch_path: PathBuf,
    pub desktop: Option<Arc<desktop::Desktop>>,
    pub meetings: Option<Arc<Meetings>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayStatus {
    pub uid: String,
    pub mirroring: bool,
    pub chat_online: bool,
    pub ntfy: bool,
    pub ntfy_topic: Option<String>,
    pub ntfy_server: Option<String>,
    pub apns: bool,
    pub phones: usize,
    pub desktop_connected: bool,
    pub phone_in_call: bool,
    pub notifications: NotificationProfile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct SavedState {
    #[serde(default = "on")]
    enabled: bool,
    #[serde(default)]
    notifications: NotificationProfile,
}

fn on() -> bool {
    true
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            enabled: true,
            notifications: NotificationProfile::default(),
        }
    }
}

impl Relay {
    pub fn new(parts: RelayParts) -> Arc<Self> {
        let state_path = parts.data_dir.join("state.json");
        let saved = std::fs::read_to_string(&state_path)
            .ok()
            .and_then(|t| serde_json::from_str::<SavedState>(&t).ok())
            .unwrap_or_default();
        let events = events::EventLog::default();
        let webhooks =
            webhooks::Webhooks::load(parts.data_dir.join("webhooks.json"), events.boot());
        Arc::new(Self {
            devices: Devices::load(parts.data_dir.join("devices.json")),
            apns: parts.apns,
            ntfy: RwLock::new(parts.ntfy),
            config_path: parts.config_path,
            mirroring: AtomicBool::new(saved.enabled),
            notifications: RwLock::new(saved.notifications),
            online: AtomicBool::new(false),
            state_path,
            agents_path: parts.agents_path,
            ai_note_saving: std::sync::Mutex::new(()),
            config_saving: std::sync::Mutex::new(()),
            watch_path: parts.watch_path,
            chat: std::sync::OnceLock::new(),
            events,
            webhooks,
            uid: parts.uid,
            desktop: parts.desktop,
            viewing: Viewing::default(),
            phone_activity: PhoneActivity::default(),
            meetings: parts.meetings,
            stopping: watch::Sender::new(false),
            auto_update_support: std::sync::atomic::AtomicU8::new(0),
            config: std::sync::OnceLock::new(),
            access: access::NetworkAccess::default(),
        })
    }

    pub fn status(&self) -> RelayStatus {
        let ntfy = self.ntfy();
        RelayStatus {
            uid: self.uid.clone(),
            mirroring: self.mirroring.load(Ordering::SeqCst),
            chat_online: self.online.load(Ordering::SeqCst),
            ntfy: ntfy.is_some(),
            ntfy_topic: ntfy.as_ref().map(|n| n.config().topic.clone()),
            ntfy_server: ntfy.as_ref().map(|n| n.config().server.clone()),
            apns: self.apns.is_some(),
            phones: self.devices.all().len(),
            desktop_connected: self.desktop.as_ref().is_some_and(|d| d.connected()),
            phone_in_call: self.desktop.as_ref().is_some_and(|d| d.handed_off()),
            notifications: self.notifications(),
        }
    }

    pub fn notifications(&self) -> NotificationProfile {
        self.notifications.read().map(|n| *n).unwrap_or_default()
    }

    /// The relay.toml this Relay started with: its port and secret are the ones it serves, whatever
    /// the file says now.
    pub fn config(&self) -> Option<&Config> {
        self.config.get()
    }

    /// The Desktop app shows the configured space's office with the user in it. DevTools falls back
    /// to another Kumospace page when that space is not open, so being attached is not enough.
    pub async fn desktop_in_office(&self) -> bool {
        let (Some(desktop), Some(config)) = (&self.desktop, self.config()) else {
            return false;
        };
        if !desktop.connected() {
            return false;
        }
        let path = desktop.evaluate("location.pathname").await.ok();
        let space = path
            .as_ref()
            .and_then(|p| p.as_str())
            .and_then(|p| p.split('/').find(|segment| !segment.is_empty()));
        space == Some(config.space.as_str()) && desktop.state().await.is_ok_and(|s| s.me.is_some())
    }

    pub async fn my_status(&self) -> MyStatus {
        match &self.desktop {
            Some(desktop) => desktop.my_status().await,
            None => MyStatus::Unknown,
        }
    }

    pub fn alert_sink(self: &Arc<Self>) -> Arc<dyn desktop::AlertSink> {
        Arc::new(Listener(self.clone()))
    }

    fn ntfy(&self) -> Option<Ntfy> {
        self.ntfy.read().ok().and_then(|n| n.clone())
    }

    pub fn set_ntfy(&self, target: Option<kumo_core::desktop::NtfyTarget>) -> anyhow::Result<()> {
        let config = target.map(|t| config::NtfyConfig {
            server: t.server,
            topic: t.topic,
            enabled: true,
        });
        self.update_config(|saved| saved.ntfy = config.clone())?;
        let ntfy = match config {
            Some(c) => Some(Ntfy::new(c, build_http_client()?)),
            None => None,
        };
        if let Ok(mut current) = self.ntfy.write() {
            *current = ntfy;
        }
        Ok(())
    }

    /// Changes relay.toml as it is on disk now. Every writer goes through here so that two changes
    /// at once (settings, a phone pairing) cannot drop each other.
    fn update_config(&self, change: impl FnOnce(&mut Config)) -> anyhow::Result<()> {
        self.update_config_if(|config| {
            change(config);
            true
        })
    }

    /// Like `update_config`, but leaves the file untouched when `change` returns false.
    fn update_config_if(&self, change: impl FnOnce(&mut Config) -> bool) -> anyhow::Result<()> {
        let Some(path) = &self.config_path else {
            return Ok(());
        };
        let _saving = self
            .config_saving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut saved = Config::load(path)?;
        if change(&mut saved) {
            saved.save(path)?;
        }
        Ok(())
    }

    pub fn agents_enabled(&self) -> bool {
        kumo_core::desktop::AgentAccess::load(&self.agents_path).enabled
    }

    pub fn set_agents(&self, enabled: bool) -> kumo_core::Result<()> {
        kumo_core::desktop::AgentAccess { enabled }.save(&self.agents_path)
    }

    fn auto_update_path(&self) -> PathBuf {
        kumo_core::desktop::AutoUpdate::path_beside(&self.agents_path)
    }

    pub fn set_auto_update_support(&self, supported: bool) {
        self.auto_update_support
            .store(if supported { 2 } else { 1 }, Ordering::Relaxed);
    }

    /// `None` without the tray app: nothing would read the switch.
    pub fn auto_update_support(&self) -> Option<bool> {
        match self.auto_update_support.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n == 2),
        }
    }

    pub fn auto_update(&self) -> kumo_core::desktop::AutoUpdate {
        kumo_core::desktop::AutoUpdate::load(&self.auto_update_path())
    }

    pub fn set_auto_update(&self, enabled: bool) -> kumo_core::Result<()> {
        kumo_core::desktop::AutoUpdate { enabled }.save(&self.auto_update_path())
    }

    /// A restart now would cut off a recording, a call on the phone, a transcript or a download.
    pub fn busy_for_update(&self) -> bool {
        let meetings = self.meetings().is_some_and(|m| {
            m.transcribing > 0
                || m.model_download.is_some()
                || matches!(m.module, kumo_meetings::ModuleState::Installing { .. })
        });
        let phone_in_call = self.desktop.as_ref().is_some_and(|d| d.handed_off());
        self.meeting_recording() || phone_in_call || meetings
    }

    fn ai_note_path(&self) -> PathBuf {
        kumo_core::desktop::AiNote::path_beside(&self.agents_path)
    }

    pub fn ai_note(&self) -> kumo_core::desktop::AiNote {
        kumo_core::desktop::AiNote::load(&self.ai_note_path())
    }

    /// Without `exclude` the saved exclusions are kept.
    pub fn set_ai_note(
        &self,
        mut note: kumo_core::desktop::AiNote,
        exclude: Option<Vec<kumo_core::events::Who>>,
    ) -> kumo_core::Result<kumo_core::desktop::AiNote> {
        let _saving = self
            .ai_note_saving
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        note.exclude = match exclude {
            Some(exclude) => exclude,
            None => kumo_core::desktop::AiNote::try_load(&self.ai_note_path())?.exclude,
        };
        note.save(&self.ai_note_path())?;
        Ok(self.ai_note())
    }

    pub fn set_chat(&self, engine: ChatEngine) {
        let _ = self.chat.set(engine);
    }

    pub fn watch_rules(&self) -> WatchRules {
        WatchRules::load(&self.watch_path)
    }

    pub fn set_watch_rules(&self, rules: &WatchRules) -> kumo_core::Result<WatchRules> {
        rules.save(&self.watch_path)?;
        Ok(self.watch_rules())
    }

    pub fn record_event(&self, event: OfficeEvent) {
        if let Some(logged) = self.events.push(event) {
            self.webhooks.publish(&webhooks::Delivery::Event(logged));
        }
    }

    /// Not gated by mirroring: integrations need every message even when pushes are off.
    pub fn record_message(&self, message: &IncomingMessage) {
        let chat = self.chat.get();
        let self_id = chat.map(ChatEngine::self_id).unwrap_or_default();
        let space_uid = chat.map(ChatEngine::space_uid).unwrap_or_default();
        let scope = MessageScope {
            self_id: &self_id,
            space_uid: &space_uid,
        };
        let name_of = |id: &str| chat.and_then(|c| c.user_name(id));
        if let Some(event) = OfficeEvent::from_message(message, &scope, name_of) {
            self.record_event(event);
        }
    }

    pub fn spawn_webhooks(self: &Arc<Self>, timing: webhooks::WebhookTiming) {
        let Some(desktop) = self.desktop.clone() else {
            return;
        };
        let relay = Arc::downgrade(self);
        let mut changes = desktop.changes();
        let mut registered = self.webhooks.registered();
        let mut stopping = self.stopping.subscribe();
        tokio::spawn(async move {
            let mut last: Option<DesktopState> = None;
            // None makes the first round after startup report to webhooks loaded from disk too.
            let mut generation = None;
            let mut failed_reads = 0;
            // Receivers whose full queue (chat events share it) dropped the last state, to send it again.
            let mut retry = std::collections::BTreeSet::new();
            loop {
                tokio::select! {
                    changed = changes.changed() => if changed.is_err() { return },
                    changed = registered.changed() => if changed.is_err() { return },
                    _ = stopping.wait_for(|stop| *stop) => return,
                    () = tokio::time::sleep(timing.poll) => {}
                }
                tokio::time::sleep(timing.debounce).await;
                changes.borrow_and_update();
                let now = Some(*registered.borrow_and_update());
                // A new receiver needs the current state even when nothing else changed.
                let force = now != generation;
                let Some(relay) = relay.upgrade() else {
                    return;
                };
                if relay.webhooks.is_empty() {
                    last = None;
                    failed_reads = 0;
                    retry.clear();
                    continue;
                }
                let state = match desktop.connected() {
                    true => match desktop.state().await {
                        Ok(state) => Some(state),
                        Err(e) if e.downcast_ref::<PageError>().is_some() => None,
                        // A timeout or reload on a still attached page is not a disconnect, unless it persists.
                        Err(_) if failed_reads < webhooks::TRANSIENT_READS => {
                            failed_reads += 1;
                            continue;
                        }
                        Err(_) => None,
                    },
                    false => None,
                };
                failed_reads = 0;
                generation = now;
                let state = state.map(|state| (webhooks::unmoved(state.clone()), state));
                let changed = force || last != state.as_ref().map(|(unmoved, _)| unmoved.clone());
                if changed || !retry.is_empty() {
                    let only = (!changed).then_some(&retry);
                    let delivery = match &state {
                        Some((_, state)) => webhooks::Delivery::Desktop(state.clone()),
                        None => webhooks::Delivery::Disconnected,
                    };
                    retry = relay.webhooks.publish_to(&delivery, only);
                    last = state.map(|(unmoved, _)| unmoved);
                }
            }
        });
    }

    pub async fn events(
        &self,
        boot: Option<&str>,
        after: Option<u64>,
        wait: Duration,
    ) -> EventsPage {
        let events = self.events.wait(boot, after, wait).await;
        EventsPage {
            boot: self.events.boot().to_string(),
            latest: self.events.latest(),
            events,
            desktop_connected: self.desktop.as_ref().is_some_and(|d| d.connected()),
        }
    }

    pub fn places(&self) -> desktop::Places {
        self.desktop
            .as_ref()
            .map(|d| d.places())
            .unwrap_or_default()
    }

    pub fn find_people(&self, query: &str) -> Vec<Who> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let Some(engine) = self.chat.get().filter(|_| !words.is_empty()) else {
            return Vec::new();
        };
        let mut people: std::collections::HashMap<String, String> = engine
            .directory_snapshot()
            .into_values()
            .map(|u| (u.user_id, u.name))
            .collect();
        for member in engine
            .channels()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|c| c.members)
        {
            people.entry(member.user_id).or_insert(member.nickname);
        }
        let me = engine.self_id();
        let mut found: Vec<Who> = people
            .into_iter()
            .filter(|(id, name)| {
                let name = name.to_lowercase();
                *id != me && !name.is_empty() && words.iter().all(|w| name.contains(w.as_str()))
            })
            .map(|(user_id, name)| Who { user_id, name })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found.truncate(20);
        found
    }

    pub fn meetings(&self) -> Option<MeetingsStatus> {
        self.meetings.as_ref().map(|m| m.status())
    }

    pub fn meeting_list(&self) -> anyhow::Result<Vec<MeetingEntry>> {
        match &self.meetings {
            Some(meetings) => meetings.meetings(),
            None => Ok(Vec::new()),
        }
    }

    pub fn transcribe_pending_meetings(&self) -> anyhow::Result<()> {
        self.meetings
            .as_ref()
            .context("meeting recording needs the desktop app")?
            .transcribe_pending()
    }

    pub fn meetings_auto_start(&self) -> bool {
        self.meetings
            .as_ref()
            .is_some_and(|m| m.config().auto_start)
    }

    pub fn meeting_recording(&self) -> bool {
        self.meetings.as_ref().is_some_and(|m| m.recording())
    }

    pub fn start_meeting_now(&self) -> anyhow::Result<()> {
        self.meetings
            .as_ref()
            .context("meeting recording needs the desktop app")?
            .start_now()
    }

    pub fn stop_meeting_now(&self) {
        if let Some(meetings) = &self.meetings {
            meetings.stop_now();
        }
    }

    pub fn meetings_folder(&self) -> Option<PathBuf> {
        self.meetings.as_ref().map(|m| m.folder())
    }

    pub fn meetings_config(&self) -> Option<MeetingsConfig> {
        self.meetings.as_ref().map(|m| m.config())
    }

    pub fn set_meetings_auto_start(&self, auto_start: bool) -> anyhow::Result<()> {
        let mut config = self
            .meetings_config()
            .context("meeting recording needs the desktop app")?;
        config.auto_start = auto_start;
        self.set_meetings_config(config)
    }

    pub fn set_meetings_config(&self, config: MeetingsConfig) -> anyhow::Result<()> {
        let meetings = self
            .meetings
            .as_ref()
            .context("meeting recording needs the desktop app")?;
        let mut config = config.validated()?;
        config.transcriber = meetings.config().transcriber;
        if config.auto_start && !meetings.module_ready() {
            anyhow::bail!("install the meetings module first");
        }
        self.update_config(|saved| saved.meetings = config.clone())?;
        meetings.set_config(config);
        Ok(())
    }

    pub fn install_meetings_module(&self) -> anyhow::Result<()> {
        self.meetings
            .as_ref()
            .context("meeting recording needs the desktop app")?
            .install_module();
        Ok(())
    }

    pub async fn remove_meetings_module(&self) -> anyhow::Result<()> {
        let meetings = self
            .meetings
            .as_ref()
            .context("meeting recording needs the desktop app")?;
        if meetings.config().auto_start {
            self.set_meetings_auto_start(false)?;
        }
        meetings.remove_module().await
    }

    pub fn set_mirroring(&self, mirroring: bool) {
        self.mirroring.store(mirroring, Ordering::SeqCst);
        self.save_state();
    }

    pub fn set_notifications(&self, request: &NotificationsRequest) -> NotificationProfile {
        let merged = match self.notifications.write() {
            Ok(mut current) => {
                *current = current.merged(request);
                *current
            }
            Err(_) => NotificationProfile::default(),
        };
        self.save_state();
        merged
    }

    fn save_state(&self) {
        let state = SavedState {
            enabled: self.mirroring.load(Ordering::SeqCst),
            notifications: self.notifications(),
        };
        if let Ok(text) = serde_json::to_string(&state) {
            let _ = std::fs::write(&self.state_path, text);
        }
    }

    pub fn set_viewing(&self, channel_url: Option<String>) {
        self.viewing.set(channel_url, std::time::Instant::now());
    }

    pub fn note_phone_activity(&self) {
        self.phone_activity.seen(std::time::SystemTime::now);
    }

    pub fn phone_idle_seconds(&self) -> Option<u64> {
        let handed_off = self.desktop.as_ref().is_some_and(|d| d.handed_off());
        self.phone_activity
            .idle_seconds(std::time::SystemTime::now(), handed_off)
    }

    pub fn stop(&self) {
        self.stopping.send_replace(true);
        self.webhooks.stop();
    }

    fn stop_meetings(&self) {
        if let Some(meetings) = &self.meetings {
            meetings.stop();
        }
    }

    async fn reclaim_handed_off_call(&self) {
        if let Some(desktop) = self.desktop.as_ref().filter(|d| d.handed_off()) {
            let _ = tokio::time::timeout(Duration::from_secs(2), desktop.reclaim_held()).await;
        }
    }

    pub async fn send_test_alert(&self) {
        self.deliver(&Alert {
            kind: AlertKind::Message,
            title: "Kumowatch".into(),
            body: "Test notification from your relay".into(),
            channel_url: None,
            person_id: None,
        })
        .await;
    }

    pub(crate) async fn dispatch(&self, alert: &Alert) {
        if !self.mirroring.load(Ordering::SeqCst) {
            return;
        }
        let status = self.my_status().await;
        if self.notifications().allows(status, alert.kind) {
            self.deliver(alert).await;
        }
    }

    async fn deliver(&self, alert: &Alert) {
        let muted = self
            .viewing
            .mutes(alert.channel_url.as_deref(), std::time::Instant::now());
        if let Some(ntfy) = self.ntfy().filter(|n| n.enabled() && !muted)
            && let Err(e) = ntfy.send(alert).await
        {
            eprintln!("ntfy: {e}");
        }
        if let Some(apns) = &self.apns {
            for device in self.devices.enabled() {
                match apns.send(&device.token, alert).await {
                    Ok(Outcome::Delivered) => {}
                    Ok(Outcome::InvalidToken) => {
                        eprintln!(
                            "apns: dropping invalid token {}…",
                            &device.token[..8.min(device.token.len())]
                        );
                        self.devices.remove(&device.token);
                    }
                    Err(e) => eprintln!("apns: {e}"),
                }
            }
        }
    }
}

struct Listener(Arc<Relay>);

impl desktop::AlertSink for Listener {
    fn alert(&self, alert: Alert) {
        let relay = self.0.clone();
        tokio::spawn(async move { relay.dispatch(&alert).await });
    }

    fn event(&self, event: OfficeEvent) {
        self.0.record_event(event);
    }
}

impl ChatListener for Listener {
    fn on_event(&self, event: ChatEvent) {
        match event {
            ChatEvent::Incoming(incoming) => {
                self.0.record_message(&incoming);
                if let Some(alert) = message_alert(&incoming) {
                    let relay = self.0.clone();
                    tokio::spawn(async move { relay.dispatch(&alert).await });
                }
            }
            ChatEvent::Connection(state) => {
                self.0
                    .online
                    .store(state == ConnectionState::Online, Ordering::SeqCst);
                eprintln!("chat: {state:?}");
            }
            ChatEvent::Error(e) => eprintln!("chat error: {e}"),
            _ => {}
        }
    }
}

pub fn init(path: &Path, space: &str, ntfy_topic: Option<String>) -> anyhow::Result<Config> {
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
    }
    let config = new_config(space, ntfy_topic)?;
    config.create(path)?;
    Ok(config)
}

fn new_config(space: &str, ntfy_topic: Option<String>) -> anyhow::Result<Config> {
    let mut secret_bytes = [0u8; 24];
    getrandom::fill(&mut secret_bytes).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut config = Config::template(&secret, space);
    config.ntfy = ntfy_topic.map(|topic| config::NtfyConfig {
        server: "https://ntfy.sh".into(),
        topic,
        enabled: true,
    });
    Ok(config)
}

pub async fn login_with_token(path: &Path, token: &str) -> anyhow::Result<AuthSession> {
    let config = Config::load(path)?;
    let token = token.trim().trim_start_matches("kumospace://login?token=");
    let token = token.split('&').next().unwrap_or_default();
    let http = build_http_client()?;
    let auth = Auth::new(
        FirebaseAuth::new(http),
        Arc::new(FileSessionStore::new(config.session_path())),
    );
    Ok(auth
        .sign_in_with_custom_token(&KumoConfig::default().firebase_api_key, token)
        .await?)
}

pub fn is_signed_in(config: &Config) -> bool {
    kumo_core::SessionStore::load(&FileSessionStore::new(config.session_path())).is_some()
}

/// Only a missing file is initialized; an unusable one is reported, never replaced.
async fn load_or_init(
    path: &Path,
    space: impl AsyncFnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<Config> {
    let exists = std::fs::exists(path)
        .map_err(|e| anyhow::anyhow!("cannot check relay.toml: {e} ({})", path.display()))?;
    if exists {
        return Config::load(path);
    }
    let mut config = new_config(&space().await?, None)?;
    config.local_until_paired = true;
    // `kumo-relay init` may have written one while the space was looked up: keep that one.
    match config.create(path) {
        Ok(()) => Ok(config),
        Err(e)
            if e.downcast_ref::<std::io::Error>().map(std::io::Error::kind)
                == Some(std::io::ErrorKind::AlreadyExists) =>
        {
            Config::load(path)
        }
        Err(e) => Err(e),
    }
}

pub async fn setup_from_desktop(path: &Path, cdp_url: &str) -> anyhow::Result<Config> {
    let desktop = kumo_devtools::import_session(cdp_url).await?;
    let http = build_http_client()?;
    let core_config = KumoConfig::default();
    let auth = Arc::new(Auth::new(
        FirebaseAuth::new(http.clone()),
        Arc::new(MemorySessionStore::default()),
    ));
    auth.set_session(desktop).await?;
    auth.refreshed_id_token()
        .await
        .context("the desktop session could not be refreshed")?;
    let api = KumoApi::new(http, core_config.api_base_url.clone(), auth);
    let mut config = load_or_init(path, async || {
        first_space(&api, cdp_url)
            .await?
            .context("this Kumospace account has no space")
    })
    .await?;
    if config.desktop_cdp.is_none() {
        config.desktop_cdp = Some(cdp_url.to_string());
        config.save(path)?;
    }
    if !is_signed_in(&config) {
        let token = api.mint_login_token().await?;
        login_with_token(path, &token).await?;
    }
    Ok(config)
}

async fn first_space(api: &KumoApi, cdp_url: &str) -> anyhow::Result<Option<String>> {
    let open = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            usable_space(api, kumo_devtools::desktop_app_spaces(cdp_url).await?).await
        })
        .await
        .context("the space open in Kumospace desktop could not be checked in time")?
    };
    // Kumospace may leave defaultSpace empty, as it did for a member who never picked one.
    let default = async { usable_space(api, api.default_space().await?).await };
    let member = async { usable_space(api, names(&api.self_spaces().await?)).await };
    let recent = async { usable_space(api, names(&api.recent_spaces().await?)).await };
    first_found([
        Box::pin(open) as futures_util::future::BoxFuture<'_, anyhow::Result<Option<String>>>,
        Box::pin(default),
        Box::pin(member),
        Box::pin(recent),
    ])
    .await
}

/// The first candidate `run` can start with. The saved space is reused on every start, so
/// a stale one (or a non-space screen like /app) must not be saved.
async fn usable_space(
    api: &KumoApi,
    candidates: impl IntoIterator<Item = String>,
) -> anyhow::Result<Option<String>> {
    let mut error = None;
    for candidate in candidates.into_iter().filter(|c| !c.is_empty()) {
        match api.space(&candidate).await {
            Ok(space) if space["uid"].as_str().is_some_and(|uid| !uid.is_empty()) => {
                return Ok(Some(candidate));
            }
            Ok(_) => {}
            Err(kumo_core::CoreError::Api(e)) if matches!(e.status, 400 | 403 | 404) => {}
            Err(e) => {
                error.get_or_insert(e);
            }
        }
    }
    error.map_or(Ok(None), |e| Err(e.into()))
}

/// A failing source does not stop the search; all errors are reported only if no source finds a space.
async fn first_found<'a>(
    sources: impl IntoIterator<
        Item = futures_util::future::BoxFuture<'a, anyhow::Result<Option<String>>>,
    >,
) -> anyhow::Result<Option<String>> {
    let mut errors = Vec::new();
    for source in sources {
        match source.await {
            Ok(Some(space)) => return Ok(Some(space)),
            Ok(None) => {}
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    if errors.is_empty() {
        Ok(None)
    } else {
        Err(anyhow::anyhow!(errors.join("; ")))
    }
}

// The recentSpaces item shape is undocumented, so a few likely fields are tried.
fn names(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|v| {
            [&v["name"], &v["space"]["name"], &v["spaceName"]]
                .into_iter()
                .find_map(|f| f.as_str().filter(|s| !s.is_empty()))
                .map(String::from)
        })
        .collect()
}

pub fn pair_link(config: &Config, url: &str) -> String {
    let enc = |s: &str| -> String {
        s.bytes()
            .map(|b| match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    };
    format!(
        "kumowatch://relay?url={}&secret={}",
        enc(url),
        enc(&config.secret)
    )
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    relay: Arc<Relay>,
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    if !config::has_secret(&config.secret) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the relay secret is empty",
        ));
    }
    axum::serve(
        listener,
        server::router(relay.clone(), config)
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown({
        let mut stopping = relay.stopping.subscribe();
        async move {
            tokio::select! {
                _ = shutdown => {}
                _ = stopping.wait_for(|stop| *stop) => {}
            }
        }
    })
    .await
}

struct PairingListenersGuard(Arc<Relay>);

impl Drop for PairingListenersGuard {
    fn drop(&mut self) {
        self.0.stop_pairing_listeners();
    }
}

pub async fn run(
    path: &Path,
    shutdown: impl Future<Output = ()> + Send + 'static,
    on_ready: impl FnOnce(Arc<Relay>),
) -> anyhow::Result<()> {
    let config = Config::load(path)?;
    std::fs::create_dir_all(config.data_dir())?;
    let bind = config.bind_address();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("cannot listen on {bind}; is another kumo-relay running?"))?;
    let core_config = KumoConfig::default();
    let http = build_http_client()?;
    let auth = Arc::new(Auth::new(
        FirebaseAuth::new(http.clone()),
        Arc::new(FileSessionStore::new(config.session_path())),
    ));
    let uid = auth
        .uid()
        .await
        .context("relay is not signed in; run `kumo-relay login`")?;
    let api = KumoApi::new(http.clone(), core_config.api_base_url.clone(), auth.clone());
    let space = api.space(&config.space).await?;
    // Chat events are scoped to this space; without its uid they would export every space's chats.
    let space_uid = space["uid"]
        .as_str()
        .filter(|uid| !uid.is_empty())
        .context("Kumospace returned the space without a uid")?
        .to_string();

    let desktop = config
        .desktop_cdp
        .clone()
        .map(|cdp| desktop::Desktop::new(cdp, config.space.clone()));
    let relay = Relay::new(RelayParts {
        uid,
        data_dir: config.data_dir(),
        apns: match &config.apns {
            Some(c) => Some(Apns::new(c.clone(), http.clone())?),
            None => None,
        },
        ntfy: config.ntfy.clone().map(|c| Ntfy::new(c, http.clone())),
        config_path: Some(path.to_path_buf()),
        agents_path: kumo_core::desktop::AgentAccess::default_path(),
        watch_path: WatchRules::default_path(),
        meetings: desktop.as_ref().map(|d| meetings::for_desktop(&config, d)),
        desktop,
    });
    relay.access.wait_for_phone(config.waiting_for_phone());
    let _ = relay.config.set(config.clone());
    if let Ok(bound) = listener.local_addr() {
        relay.access.listening_on(bound);
    }
    if let Some(desktop) = &relay.desktop {
        desktop.spawn(relay.alert_sink());
    }
    relay.spawn_webhooks(webhooks::WebhookTiming::default());
    if let Some(meetings) = &relay.meetings {
        meetings.spawn();
    }
    let engine = ChatEngine::new(ChatEngineDeps {
        transport: Arc::new(SendbirdTransport::new(api, &core_config, http)),
        store: Arc::new(ChatStore::open(&config.store_path())?),
        listener: Arc::new(Listener(relay.clone())),
    });
    // Installed first, so the first message already finds the engine and its scope.
    relay.set_chat(engine.clone());
    engine.start(&config.space, &space_uid);
    eprintln!(
        "kumo-relay: space {} as {}, ntfy={}, apns={}, mirroring={}, listening on {} (started {})",
        config.space,
        relay.uid,
        relay.status().ntfy,
        relay.apns.is_some(),
        relay.status().mirroring,
        if config.waiting_for_phone() {
            format!("{bind} until a phone pairs")
        } else {
            bind.clone()
        },
        now_ms()
    );
    on_ready(relay.clone());
    // Dropping `run` (or a Ctrl-C) must not leave the pairing listeners serving the network.
    let _pairing = PairingListenersGuard(relay.clone());
    let shutdown = {
        let relay = relay.clone();
        async move {
            shutdown.await;
            relay.stop_pairing_listeners();
        }
    };
    let served = serve(listener, relay.clone(), config.clone(), shutdown).await;
    relay.close_pairing_listeners().await;
    relay.reclaim_handed_off_call().await;
    relay.stop_meetings();
    engine.stop();
    if let Some(desktop) = &relay.desktop {
        desktop.stop();
    }
    relay.stop();
    served?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::chat::model::ChannelCategory;
    use kumo_core::chat::transport::ChatCall;
    use kumo_core::chat::{ChatConnection, ChatTransport};

    fn source<'a>(
        result: anyhow::Result<Option<&'static str>>,
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move { result.map(|s| s.map(String::from)) })
    }

    #[tokio::test]
    async fn first_found_takes_the_earliest_space() {
        let space = first_found([source(Ok(Some("open"))), source(Ok(Some("default")))])
            .await
            .unwrap();
        assert_eq!(space.as_deref(), Some("open"));
        let space = first_found([source(Ok(None)), source(Ok(Some("default")))])
            .await
            .unwrap();
        assert_eq!(space.as_deref(), Some("default"));
    }

    #[tokio::test]
    async fn first_found_skips_a_failing_source() {
        let space = first_found([
            source(Ok(None)),
            source(Err(anyhow::anyhow!("defaultSpace down"))),
            source(Ok(Some("member"))),
        ])
        .await
        .unwrap();
        assert_eq!(space.as_deref(), Some("member"));
    }

    #[tokio::test]
    async fn first_found_reports_an_error_only_when_nothing_is_found() {
        let error = first_found([
            source(Err(anyhow::anyhow!("desktop timed out"))),
            source(Ok(None)),
            source(Err(anyhow::anyhow!("spaces down"))),
        ])
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "desktop timed out; spaces down");
        assert_eq!(first_found([source(Ok(None))]).await.unwrap(), None);
    }

    #[test]
    fn names_reads_every_known_item_shape() {
        use serde_json::json;
        assert_eq!(
            names(&[
                json!({ "name": "a" }),
                json!({}),
                json!({ "space": { "name": "b" } }),
                json!({ "name": "", "spaceName": "c" }),
                json!({ "name": "" }),
            ]),
            ["a", "b", "c"]
        );
        assert!(names(&[]).is_empty());
    }

    struct Offline;

    impl ChatTransport for Offline {
        fn connect<'a>(&'a self, _space_name: &'a str) -> ChatCall<'a, ChatConnection> {
            Box::pin(async { Err(kumo_core::CoreError::Storage("offline".into())) })
        }
    }

    #[tokio::test]
    async fn setup_initializes_only_a_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.toml");
        let created = load_or_init(&path, async || Ok("office".to_string()))
            .await
            .unwrap();
        assert_eq!(created.space, "office");
        assert!(config::has_secret(&created.secret));
        assert!(created.local_until_paired);
        assert_eq!(
            Config::load(&path).unwrap().bind_address(),
            "127.0.0.1:8787"
        );

        for text in [
            "listen = '0.0.0.0:8787'\nsecret = ' '\nspace = 'office'\n",
            "secret = \"",
        ] {
            std::fs::write(&path, text).unwrap();
            let error = load_or_init(&path, async || panic!("must not initialize"))
                .await
                .unwrap_err();
            assert!(format!("{error:#}").starts_with("relay.toml "), "{error:#}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
    }

    #[tokio::test]
    async fn incoming_chat_messages_are_logged_with_mirroring_off() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(RelayParts {
            uid: "uid-me".into(),
            data_dir: dir.path().to_path_buf(),
            apns: None,
            ntfy: None,
            config_path: None,
            agents_path: dir.path().join("agents.json"),
            watch_path: dir.path().join("watch.json"),
            desktop: None,
            meetings: None,
        });
        relay.set_mirroring(false);
        let engine = ChatEngine::new(ChatEngineDeps {
            transport: Arc::new(Offline),
            store: Arc::new(ChatStore::in_memory().unwrap()),
            listener: Arc::new(Listener(relay.clone())),
        });
        engine.start("acme", "sp");
        relay.set_chat(engine);
        Listener(relay.clone()).on_event(ChatEvent::Incoming(IncomingMessage {
            channel_url: "sp_amy_me".into(),
            channel_title: "Amy".into(),
            category: ChannelCategory::Direct,
            message_id: 1,
            parent_id: None,
            created_at: 5,
            sender_id: "amy".into(),
            sender_name: "Amy".into(),
            text: "hi".into(),
            mentions_me: false,
            channel_is_open: false,
        }));
        let logged = relay.events.after(Some(relay.events.boot()), Some(0));
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].event.kind, kumo_core::events::EventKind::Message);
        assert_eq!(logged[0].event.channel_type, "direct");
    }
}
