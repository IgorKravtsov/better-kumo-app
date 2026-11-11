mod convert;
mod relay;
mod space;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use kumo_core::chat::{ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore};
use kumo_core::http::{HttpClient, build_http_client};
use kumo_core::{
    Auth, AuthSession, ChannelKind, ChannelUrl, CoreError, FirebaseAuth, KumoApi, KumoConfig,
    PasswordCredentials, SessionStore,
};

pub use convert::*;
use kumo_core::space::{SpaceSession, SpaceSessionParams};
pub use space::{CallInfo, FloorItem, PersonItem, RoomItem, SpaceView};

uniffi::setup_scaffolding!();

pub const BROWSER_LOGIN_URL: &str = "https://www.kumospace.com/auth/redirect/desktop";
const LOGIN_LINK_PREFIX: &str = "kumospace://login";

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum KumoError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("{detail}")]
    Auth { detail: String },
    #[error("{detail}")]
    Network { detail: String },
    #[error("{detail}")]
    Other { detail: String },
}

impl From<CoreError> for KumoError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotSignedIn => KumoError::NotSignedIn,
            CoreError::Auth { code } => KumoError::Auth {
                detail: auth_message(&code),
            },
            CoreError::Network(m) | CoreError::Socket(m) => KumoError::Network { detail: m },
            CoreError::Timeout => KumoError::Network {
                detail: "timeout".into(),
            },
            other => KumoError::Other {
                detail: other.to_string(),
            },
        }
    }
}

fn auth_message(code: &str) -> String {
    match code {
        "INVALID_LOGIN_CREDENTIALS" | "INVALID_PASSWORD" | "EMAIL_NOT_FOUND" => {
            "Wrong email or password".into()
        }
        "INVALID_CUSTOM_TOKEN" | "CREDENTIAL_MISMATCH" => "This sign-in link has expired".into(),
        "TOKEN_EXPIRED" | "USER_DISABLED" | "USER_NOT_FOUND" => "Please sign in again".into(),
        other => other.to_string(),
    }
}

#[uniffi::export(with_foreign)]
pub trait SecureStorage: Send + Sync {
    fn load(&self) -> Option<String>;
    fn save(&self, value: String);
    fn clear(&self);
}

#[uniffi::export(with_foreign)]
pub trait AppListener: Send + Sync {
    fn on_event(&self, event: AppEvent);
}

struct StorageAdapter(Arc<dyn SecureStorage>);

impl SessionStore for StorageAdapter {
    fn load(&self) -> Option<AuthSession> {
        serde_json::from_str(&self.0.load()?).ok()
    }

    fn save(&self, session: &AuthSession) -> kumo_core::Result<()> {
        self.0.save(serde_json::to_string(session)?);
        Ok(())
    }

    fn clear(&self) -> kumo_core::Result<()> {
        self.0.clear();
        Ok(())
    }
}

struct ListenerAdapter {
    listener: Arc<dyn AppListener>,
    relay: Arc<relay::RelayState>,
}

impl ChatListener for ListenerAdapter {
    fn on_event(&self, event: ChatEvent) {
        if let ChatEvent::Incoming(incoming) = &event {
            if self.relay.is_enabled()
                && let Some(alert) = message_alert(incoming)
            {
                self.listener.on_event(AppEvent::Alert { alert });
            }
            return;
        }
        self.listener.on_event(AppEvent::from(event));
    }
}

struct ChatState {
    uid: String,
    engine: ChatEngine,
    space: Option<SpaceInfo>,
}

#[derive(uniffi::Object)]
pub struct KumoApp {
    runtime: tokio::runtime::Runtime,
    config: KumoConfig,
    http: HttpClient,
    auth: Arc<Auth>,
    api: KumoApi,
    data_dir: PathBuf,
    listener: Arc<dyn AppListener>,
    chat: Mutex<Option<ChatState>>,
    relay: Arc<relay::RelayState>,
    space: Arc<Mutex<space::SpaceHolder>>,
    space_weak: std::sync::Weak<Mutex<space::SpaceHolder>>,
    device: Mutex<(kumo_core::Platform, String)>,
}

#[uniffi::export]
impl KumoApp {
    #[uniffi::constructor]
    pub fn new(
        data_dir: String,
        storage: Arc<dyn SecureStorage>,
        listener: Arc<dyn AppListener>,
    ) -> Result<Arc<Self>, KumoError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("kumo-core")
            .build()
            .map_err(|e| KumoError::Other {
                detail: e.to_string(),
            })?;
        let config = KumoConfig::default();
        let http = build_http_client()?;
        let auth = Arc::new(Auth::new(
            FirebaseAuth::new(http.clone()),
            Arc::new(StorageAdapter(storage)),
        ));
        let api = KumoApi::new(http.clone(), config.api_base_url.clone(), auth.clone());
        let space = Arc::new(Mutex::new(space::SpaceHolder::default()));
        Ok(Arc::new(Self {
            runtime,
            config,
            http,
            auth,
            api,
            data_dir: PathBuf::from(data_dir),
            listener,
            chat: Mutex::new(None),
            relay: Arc::new(relay::RelayState::default()),
            space_weak: Arc::downgrade(&space),
            space,
            device: Mutex::new((kumo_core::Platform::Android, String::new())),
        }))
    }

    pub fn core_version(&self) -> String {
        kumo_core::CORE_VERSION.to_string()
    }

    pub fn browser_login_url(&self) -> String {
        BROWSER_LOGIN_URL.to_string()
    }

    pub fn is_login_link(&self, url: String) -> bool {
        url.starts_with(LOGIN_LINK_PREFIX)
    }

    pub fn current_user(&self) -> Option<UserInfo> {
        let session = self.auth.session_snapshot()?;
        Some(UserInfo::from(&session))
    }

    pub async fn sign_in_with_link(&self, link: String) -> Result<UserInfo, KumoError> {
        let token = login_token_from_link(&link).ok_or(KumoError::Auth {
            detail: "Not a Kumospace sign-in link".into(),
        })?;
        self.sign_in_with_token(token).await
    }

    pub async fn sign_in_with_token(&self, token: String) -> Result<UserInfo, KumoError> {
        let auth = self.auth.clone();
        let key = self.config.firebase_api_key.clone();
        let session = self
            .spawn(async move { auth.sign_in_with_custom_token(&key, token.trim()).await })
            .await?;
        Ok(UserInfo::from(&session))
    }

    pub async fn sign_in_with_password(
        &self,
        email: String,
        password: String,
    ) -> Result<UserInfo, KumoError> {
        let auth = self.auth.clone();
        let key = self.config.firebase_api_key.clone();
        let session = self
            .spawn(async move {
                auth.sign_in_with_password(PasswordCredentials {
                    api_key: &key,
                    email: email.trim(),
                    password: &password,
                })
                .await
            })
            .await?;
        Ok(UserInfo::from(&session))
    }

    pub async fn transfer_link(&self) -> Result<String, KumoError> {
        let api = self.api.clone();
        let token = self
            .spawn(async move { api.mint_login_token().await })
            .await?;
        Ok(format!("{LOGIN_LINK_PREFIX}?token={token}"))
    }

    pub fn sign_out(&self) -> Result<(), KumoError> {
        if let Ok(mut chat) = self.chat.lock()
            && let Some(state) = chat.take()
        {
            let _guard = self.runtime.enter();
            state.engine.stop();
            let _ = state.engine.store().clear();
        }
        let auth = self.auth.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .map(|rt| rt.block_on(auth.sign_out()))
        })
        .join()
        .map_err(|_| KumoError::Other {
            detail: "sign out failed".into(),
        })?
        .map_err(|e| KumoError::Other {
            detail: e.to_string(),
        })??;
        Ok(())
    }

    pub async fn spaces(&self) -> Result<Vec<SpaceInfo>, KumoError> {
        let api = self.api.clone();
        let raw = self.spawn(async move { api.self_spaces().await }).await?;
        Ok(raw.iter().filter_map(SpaceInfo::from_json).collect())
    }

    pub fn selected_space(&self) -> Option<SpaceInfo> {
        let uid = self.auth.session_snapshot()?.uid;
        let store = self.open_store(&uid).ok()?;
        serde_json::from_str::<serde_json::Value>(&store.get_state("selected_space").ok()??)
            .ok()
            .as_ref()
            .and_then(SpaceInfo::from_json)
    }

    pub async fn default_space(&self) -> Result<Option<SpaceInfo>, KumoError> {
        let api = self.api.clone();
        let space = self
            .spawn(async move {
                match api.default_space().await? {
                    Some(name) => api.space(&name).await.map(Some),
                    None => Ok(None),
                }
            })
            .await?;
        Ok(space.as_ref().and_then(SpaceInfo::from_json))
    }

    pub fn select_space(&self, space: SpaceInfo) -> Result<(), KumoError> {
        let state = self.ensure_chat()?;
        state.engine.store().set_state(
            "selected_space",
            &serde_json::json!({
                "name": space.name,
                "uid": space.uid,
                "displayName": space.display_name,
                "memberCount": space.member_count,
            })
            .to_string(),
        )?;
        let _guard = self.runtime.enter();
        state.engine.start(&space.name, &space.uid);
        if let Ok(mut chat) = self.chat.lock()
            && let Some(s) = chat.as_mut()
        {
            s.space = Some(space);
        }
        Ok(())
    }

    pub fn start(&self) -> Result<bool, KumoError> {
        let Some(space) = self.selected_space() else {
            return Ok(false);
        };
        let running = self
            .chat
            .lock()
            .ok()
            .and_then(|c| c.as_ref().and_then(|s| s.space.clone()))
            .is_some_and(|s| s.uid == space.uid);
        if !running {
            self.select_space(space)?;
        }
        Ok(true)
    }

    pub fn relay_enabled(&self) -> bool {
        self.relay.is_enabled()
    }

    pub fn set_device(&self, platform: ClientPlatform, time_zone: String) {
        if let Ok(mut d) = self.device.lock() {
            *d = (
                match platform {
                    ClientPlatform::Android => kumo_core::Platform::Android,
                    ClientPlatform::Ios => kumo_core::Platform::Ios,
                },
                time_zone,
            );
        }
    }

    pub fn set_relay_enabled(&self, enabled: bool) -> Result<(), KumoError> {
        self.relay.set(enabled);
        if enabled {
            self.start()?;
        }
        self.sync_space()
    }

    pub fn space_open(&self) -> Result<(), KumoError> {
        self.with_space(|h| h.want_ui = true);
        self.sync_space()
    }

    pub fn space_close(&self) -> Result<(), KumoError> {
        self.with_space(|h| h.want_ui = false);
        self.sync_space()
    }

    pub fn space_view(&self) -> SpaceView {
        let Ok(holder) = self.space.lock() else {
            return space::build_view(&Default::default(), &Default::default());
        };
        let state = holder
            .session
            .as_ref()
            .map(|s| s.snapshot())
            .unwrap_or_default();
        space::build_view(&state, &holder.floor_names)
    }

    pub fn join_room(&self, floor_id: String, zone_id: String) -> Result<(), KumoError> {
        self.with_space(|h| h.want_call = true);
        self.sync_space()?;
        self.with_space(|h| {
            if let Some(s) = &h.session {
                s.join_zone(&floor_id, &zone_id);
            }
        });
        Ok(())
    }

    pub fn join_room_with_password(
        &self,
        floor_id: String,
        zone_id: String,
        password: String,
    ) -> Result<(), KumoError> {
        self.with_space(|h| h.want_call = true);
        self.sync_space()?;
        self.with_space(|h| {
            if let Some(s) = &h.session {
                s.join_zone_with_password(&floor_id, &zone_id, &password);
            }
        });
        Ok(())
    }

    pub fn join_person(&self, uid: String) -> Result<bool, KumoError> {
        self.with_space(|h| h.want_call = true);
        self.sync_space()?;
        let mut ok = false;
        self.with_space(|h| {
            if let Some(s) = &h.session {
                ok = s.join_person(&uid);
            }
        });
        Ok(ok)
    }

    pub fn leave_call(&self) -> Result<(), KumoError> {
        self.with_space(|h| {
            if let Some(s) = &h.session {
                s.leave();
            }
            h.want_call = false;
        });
        self.sync_space()
    }

    pub fn refresh_call_token(&self) {
        self.with_space(|h| {
            if let Some(s) = &h.session {
                s.refresh_daily_token();
            }
        });
    }

    pub fn call_info(&self) -> Option<CallInfo> {
        let holder = self.space.lock().ok()?;
        let state = holder.session.as_ref()?.snapshot();
        state.call.as_ref().map(|c| space::call_info(&state, c))
    }

    pub fn call_peers(&self) -> Vec<String> {
        self.space
            .lock()
            .ok()
            .and_then(|h| h.session.as_ref().map(|s| s.snapshot().zone_mates()))
            .unwrap_or_default()
    }

    pub fn person_name(&self, uid: String) -> Option<String> {
        let holder = self.space.lock().ok()?;
        let state = holder.session.as_ref()?.snapshot();
        state.person_by_uid(&uid).map(|p| p.name.clone())
    }

    pub fn stop(&self) {
        if let Ok(mut chat) = self.chat.lock()
            && let Some(state) = chat.as_mut()
        {
            let _guard = self.runtime.enter();
            state.engine.stop();
            state.space = None;
        }
    }

    pub fn channels(&self) -> Vec<ChannelItem> {
        let Ok(state) = self.ensure_chat() else {
            return Vec::new();
        };
        let self_id = state.uid.clone();
        state
            .engine
            .channels()
            .unwrap_or_default()
            .iter()
            .map(|c| ChannelItem::from_row(c, &self_id))
            .collect()
    }

    pub fn messages(&self, channel_url: String, limit: u32) -> Vec<MessageItem> {
        self.message_rows(|engine| engine.messages(&channel_url, limit), &channel_url)
    }

    pub fn messages_before(
        &self,
        channel_url: String,
        before_ts: i64,
        limit: u32,
    ) -> Vec<MessageItem> {
        self.message_rows(
            |engine| engine.messages_before(&channel_url, before_ts, limit),
            &channel_url,
        )
    }

    pub fn thread(&self, channel_url: String, parent_id: i64) -> Vec<MessageItem> {
        self.message_rows(
            |engine| engine.thread(&channel_url, parent_id),
            &channel_url,
        )
    }

    pub fn open_channel(&self, channel_url: String) {
        if let Ok(state) = self.ensure_chat() {
            let _guard = self.runtime.enter();
            state.engine.open(&channel_url);
        }
    }

    pub fn close_channel(&self, channel_url: String) {
        if let Ok(state) = self.ensure_chat() {
            state.engine.close(&channel_url);
        }
    }

    pub async fn load_older(&self, channel_url: String, before_ts: i64) -> Result<u32, KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.load_older(&channel_url, before_ts).await })
            .await
    }

    pub async fn load_thread(&self, channel_url: String, parent_id: i64) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.load_thread(&channel_url, parent_id).await })
            .await
    }

    pub async fn send_text(
        &self,
        channel_url: String,
        text: String,
        parent_id: Option<i64>,
    ) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.send_text(&channel_url, &text, parent_id).await })
            .await
    }

    pub async fn retry_message(&self, channel_url: String, local_id: i64) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.retry(&channel_url, local_id).await })
            .await
    }

    pub async fn open_direct(&self, user_id: String) -> Result<String, KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.open_direct(&user_id).await })
            .await
    }

    pub fn space_people(&self) -> Vec<MemberItem> {
        let Ok(state) = self.ensure_chat() else {
            return Vec::new();
        };
        let mut people: Vec<MemberItem> = state
            .engine
            .directory_snapshot()
            .into_values()
            .filter(|u| u.user_id != state.uid)
            .map(|u| MemberItem {
                uid: u.user_id,
                name: u.name,
                avatar_url: u.avatar_url,
                is_online: false,
            })
            .collect();
        people.sort_by_key(|p| p.name.to_lowercase());
        people
    }

    pub async fn toggle_reaction(
        &self,
        channel_url: String,
        message_id: i64,
        key: String,
    ) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.toggle_reaction(&channel_url, message_id, &key).await })
            .await
    }

    pub async fn edit_message(
        &self,
        channel_url: String,
        message_id: i64,
        text: String,
    ) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.edit_message(&channel_url, message_id, &text).await })
            .await
    }

    pub async fn delete_message(
        &self,
        channel_url: String,
        message_id: i64,
    ) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move { engine.delete_message(&channel_url, message_id).await })
            .await
    }

    pub async fn send_file(&self, file: OutgoingFile) -> Result<(), KumoError> {
        let engine = self.ensure_chat()?.engine;
        self.spawn(async move {
            engine
                .send_file(
                    kumo_core::sendbird::FileUpload {
                        channel_url: file.channel_url,
                        name: file.name,
                        mime_type: file.mime_type,
                        data: file.data,
                    },
                    file.parent_id,
                )
                .await
        })
        .await
    }

    pub fn channel_members(&self, channel_url: String) -> Vec<MemberItem> {
        let Ok(state) = self.ensure_chat() else {
            return Vec::new();
        };
        let directory = state.engine.directory_snapshot();
        state
            .engine
            .store()
            .channel(&channel_url)
            .ok()
            .flatten()
            .map(|c| c.members)
            .unwrap_or_default()
            .into_iter()
            .filter(|m| m.user_id != state.uid)
            .map(|m| MemberItem {
                avatar_url: directory
                    .get(&m.user_id)
                    .map(|u| u.avatar_url.clone())
                    .filter(|a| !a.is_empty())
                    .unwrap_or(m.profile_url),
                uid: m.user_id,
                name: m.nickname,
                is_online: m.is_online,
            })
            .collect()
    }

    pub async fn set_typing(&self, channel_url: String, typing: bool) {
        if let Ok(state) = self.ensure_chat() {
            let engine = state.engine;
            let _ = self
                .spawn(async move {
                    engine.set_typing(&channel_url, typing).await;
                    Ok::<(), CoreError>(())
                })
                .await;
        }
    }
}

impl KumoApp {
    fn with_space(&self, f: impl FnOnce(&mut space::SpaceHolder)) {
        if let Ok(mut h) = self.space.lock() {
            f(&mut h);
        }
    }

    fn sync_space(&self) -> Result<(), KumoError> {
        let space_info = self.selected_space();
        let mut holder = self.space.lock().map_err(|_| KumoError::Other {
            detail: "poisoned".into(),
        })?;
        let wanted = holder.wanted(self.relay.is_enabled());
        let current = holder.session.as_ref().map(|s| s.space_name().to_string());
        let target = space_info.as_ref().map(|s| s.name.clone());
        if (!wanted || current != target)
            && let Some(old) = holder.session.take()
        {
            let _guard = self.runtime.enter();
            old.stop();
        }
        if !wanted || holder.session.is_some() {
            return Ok(());
        }
        let Some(space_info) = space_info else {
            return Err(KumoError::Other {
                detail: "Choose a space first".into(),
            });
        };
        let (platform, time_zone) = self
            .device
            .lock()
            .map(|d| d.clone())
            .unwrap_or((kumo_core::Platform::Android, String::new()));
        let _guard = self.runtime.enter();
        holder.session = Some(SpaceSession::start(SpaceSessionParams {
            auth: self.auth.clone(),
            config: self.config.clone(),
            space_name: space_info.name.clone(),
            platform,
            time_zone,
            listener: Arc::new(space::SpaceAdapter {
                listener: self.listener.clone(),
                relay: self.relay.clone(),
            }),
        }));
        if holder.floor_names.is_empty() {
            let api = self.api.clone();
            let name = space_info.name.clone();
            let listener = self.listener.clone();
            let names = self.floor_names_sink();
            self.runtime.spawn(async move {
                if let Ok(space) = api.space(&name).await {
                    let map: std::collections::HashMap<String, String> = space["rooms"]
                        .as_array()
                        .map(|rooms| {
                            rooms
                                .iter()
                                .filter_map(|r| {
                                    Some((
                                        r["id"].as_str()?.to_string(),
                                        r["displayName"].as_str().unwrap_or_default().to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    names(map);
                    listener.on_event(AppEvent::SpaceChanged);
                }
            });
        }
        Ok(())
    }

    fn floor_names_sink(
        &self,
    ) -> impl Fn(std::collections::HashMap<String, String>) + Send + 'static {
        let space = self.space_weak.clone();
        move |map| {
            if let Some(space) = space.upgrade()
                && let Ok(mut h) = space.lock()
            {
                h.floor_names = map;
            }
        }
    }

    async fn spawn<T, F>(&self, fut: F) -> Result<T, KumoError>
    where
        T: Send + 'static,
        F: std::future::Future<Output = kumo_core::Result<T>> + Send + 'static,
    {
        self.runtime
            .spawn(fut)
            .await
            .map_err(|e| KumoError::Other {
                detail: e.to_string(),
            })?
            .map_err(KumoError::from)
    }

    fn open_store(&self, uid: &str) -> kumo_core::Result<Arc<ChatStore>> {
        if let Ok(chat) = self.chat.lock()
            && let Some(state) = chat.as_ref()
            && state.uid == uid
        {
            return Ok(state.engine.store().clone());
        }
        Ok(Arc::new(ChatStore::open(
            &self.data_dir.join(format!("chat-{uid}.db")),
        )?))
    }

    fn ensure_chat(&self) -> Result<EngineHandle, KumoError> {
        let session = self.auth.session_snapshot().ok_or(KumoError::NotSignedIn)?;
        let mut chat = self.chat.lock().map_err(|_| KumoError::Other {
            detail: "poisoned".into(),
        })?;
        if chat.as_ref().is_none_or(|s| s.uid != session.uid) {
            if let Some(old) = chat.take() {
                let _guard = self.runtime.enter();
                old.engine.stop();
            }
            let store = Arc::new(ChatStore::open(
                &self.data_dir.join(format!("chat-{}.db", session.uid)),
            )?);
            let engine = ChatEngine::new(ChatEngineDeps {
                api: self.api.clone(),
                config: self.config.clone(),
                http: self.http.clone(),
                store,
                listener: Arc::new(ListenerAdapter {
                    listener: self.listener.clone(),
                    relay: self.relay.clone(),
                }),
            });
            *chat = Some(ChatState {
                uid: session.uid.clone(),
                engine,
                space: None,
            });
        }
        let state = chat.as_ref().ok_or(KumoError::NotSignedIn)?;
        Ok(EngineHandle {
            uid: state.uid.clone(),
            engine: state.engine.clone(),
        })
    }

    fn message_rows(
        &self,
        query: impl FnOnce(&ChatEngine) -> kumo_core::Result<Vec<kumo_core::chat::MessageRow>>,
        channel_url: &str,
    ) -> Vec<MessageItem> {
        let Ok(state) = self.ensure_chat() else {
            return Vec::new();
        };
        let rows = query(&state.engine).unwrap_or_default();
        let members = state
            .engine
            .store()
            .channel(channel_url)
            .ok()
            .flatten()
            .map(|c| c.members)
            .unwrap_or_default();
        let engine = state.engine.clone();
        let file_url = move |url: &str| engine.authorized_file_url(url);
        let ctx = MessageContext {
            self_id: &state.uid,
            members: &members,
            file_url: &file_url,
        };
        rows.iter()
            .map(|r| MessageItem::from_row(r, &ctx))
            .collect()
    }
}

struct EngineHandle {
    uid: String,
    engine: ChatEngine,
}

fn login_token_from_link(link: &str) -> Option<String> {
    let trimmed = link.trim();
    if !trimmed.starts_with(LOGIN_LINK_PREFIX) {
        return (!trimmed.is_empty() && !trimmed.contains("://")).then(|| trimmed.to_string());
    }
    let parsed = url::Url::parse(trimmed).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct OutgoingFile {
    pub channel_url: String,
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct MemberItem {
    pub uid: String,
    pub name: String,
    pub avatar_url: String,
    pub is_online: bool,
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ClientPlatform {
    Android,
    Ios,
}

#[uniffi::export]
pub fn direct_message_channel_url(space_uid: String, user_a: String, user_b: String) -> String {
    ChannelUrl::direct_message(&space_uid, [&user_a, &user_b])
}

#[uniffi::export]
pub fn is_direct_message_url(url: String, space_uid: String) -> bool {
    matches!(
        ChannelUrl::parse(&url, &space_uid).kind,
        ChannelKind::DirectMessage { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_token_from_links() {
        assert_eq!(
            login_token_from_link("kumospace://login?token=abc.def&linkedPath=/x").as_deref(),
            Some("abc.def")
        );
        assert_eq!(
            login_token_from_link(" raw.token ").as_deref(),
            Some("raw.token")
        );
        assert_eq!(login_token_from_link("https://evil/x"), None);
    }
}
