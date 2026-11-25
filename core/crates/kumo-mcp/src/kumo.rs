use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};
use kumo_core::chat::{
    ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore, ConnectionState,
    SendbirdTransport,
};
use kumo_core::desktop::{AgentAccess, AiNote};
use kumo_core::events::WatchRules;
use kumo_core::http::build_http_client;
use kumo_core::{
    Auth, ChannelUrl, FileSessionStore, FirebaseAuth, KumoApi, KumoConfig, SessionStore,
};
use tokio::sync::watch;

use crate::arrivals::Arrivals;

const READY_TIMEOUT: Duration = Duration::from_secs(30);
const RECONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const ACCESS_POLL: Duration = Duration::from_secs(2);
pub(crate) const ACCESS_OFF: &str = "Access for AI agents is turned off. Turn on \"AI agents can use my chat (MCP)\" in the Better Kumo menu on the computer or in the Better Kumo phone app (You → Notifications relay).";

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub session_path: PathBuf,
    pub space: Option<String>,
    pub db_path: Option<PathBuf>,
    pub devtools_port: u16,
    pub agents_path: PathBuf,
    pub watch_path: PathBuf,
    /// Added to the default cache name. Two chat engines on one cache would count each
    /// live message as unread twice.
    pub cache_tag: &'static str,
}

pub struct Kumo {
    pub engine: ChatEngine,
    pub http: reqwest::Client,
    pub space_name: String,
    pub space_uid: String,
    pub self_uid: String,
    pub self_name: String,
    pub data_dir: PathBuf,
    state: watch::Receiver<ConnectionState>,
    last_error: Arc<Mutex<Option<String>>>,
}

struct Listener {
    state: watch::Sender<ConnectionState>,
    last_error: Arc<Mutex<Option<String>>>,
    arrivals: Arc<Arrivals>,
}

impl ChatListener for Listener {
    fn on_event(&self, event: ChatEvent) {
        match event {
            ChatEvent::Connection(state) => {
                self.state.send_replace(state);
            }
            ChatEvent::Incoming(message) => self.arrivals.push(message),
            ChatEvent::Error(message) => {
                eprintln!("kumo-mcp: {message}");
                if let Ok(mut last) = self.last_error.lock() {
                    *last = Some(message);
                }
            }
            _ => {}
        }
    }
}

pub fn data_dir() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_default()
        .join(".local")
        .join("share")
        .join("kumowatch-mcp")
}

pub struct SignIn {
    pub auth: Arc<Auth>,
    pub api: KumoApi,
    store: Arc<FileSessionStore>,
}

impl SignIn {
    pub fn new(session_path: PathBuf) -> anyhow::Result<Self> {
        let config = KumoConfig::default();
        let store = Arc::new(FileSessionStore::new(session_path));
        let http = build_http_client()?;
        let auth = Arc::new(Auth::new(FirebaseAuth::new(http.clone()), store.clone()));
        let api = KumoApi::new(http, config.api_base_url.clone(), auth.clone());
        Ok(Self { auth, api, store })
    }

    pub fn has_session(&self) -> bool {
        self.store.load().is_some()
    }
}

pub async fn signed_in(options: &ConnectOptions) -> anyhow::Result<SignIn> {
    let sign_in = SignIn::new(options.session_path.clone())?;
    if sign_in.has_session() {
        return Ok(sign_in);
    }
    let from_desktop = import_from_desktop(&sign_in.auth, options.devtools_port).await;
    if from_desktop.is_ok() {
        return Ok(sign_in);
    }
    if let Some(relay_session) = relay_session_path().filter(|p| p.exists()) {
        if let Some(dir) = options.session_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::copy(&relay_session, &options.session_path).with_context(|| {
            format!(
                "could not copy the Better Kumo session from {}",
                relay_session.display()
            )
        })?;
        return SignIn::new(options.session_path.clone());
    }
    from_desktop.context(
        "not signed in to Kumospace; open the Kumospace desktop app (Better Kumo starts it with DevTools) and run `kumo-mcp login`",
    )?;
    Ok(sign_in)
}

pub async fn import_from_desktop(auth: &Auth, port: u16) -> anyhow::Result<()> {
    let session = kumo_devtools::import_session(&kumo_devtools::local_url(port))
        .await
        .with_context(|| {
            format!("could not read the session from Kumospace desktop on DevTools port {port}")
        })?;
    auth.set_session(session).await?;
    auth.refreshed_id_token()
        .await
        .context("the imported session could not be refreshed")?;
    Ok(())
}

pub(crate) fn relay_setting(key: &str) -> Option<String> {
    let path = std::env::home_dir()?
        .join(".config")
        .join("kumowatch")
        .join("relay.toml");
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let (k, value) = line.split_once('=')?;
        (k.trim() == key)
            .then(|| value.trim().trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}

pub fn relay_space() -> Option<String> {
    relay_setting("space")
}

fn relay_session_path() -> Option<PathBuf> {
    let data_dir = relay_setting("data_dir").map(PathBuf::from).or_else(|| {
        Some(
            std::env::home_dir()?
                .join(".local")
                .join("share")
                .join("kumowatch-relay"),
        )
    })?;
    Some(data_dir.join("session.json"))
}

impl Kumo {
    pub async fn connect(
        options: &ConnectOptions,
        arrivals: Arc<Arrivals>,
    ) -> anyhow::Result<Self> {
        let SignIn { auth, api, .. } = signed_in(options).await?;
        let space_name = match options.space.clone().or_else(relay_space) {
            Some(space) => space,
            None => api
                .default_space()
                .await?
                .ok_or_else(|| anyhow!("no Kumospace space configured; pass --space <name>"))?,
        };
        let space = api.space(&space_name).await?;
        let space_uid = space["uid"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("space {space_name} has no uid"))?
            .to_string();
        let session = auth.session().await.context("session missing")?;
        let data_dir = data_dir();
        let db_path = options.db_path.clone().unwrap_or_else(|| {
            data_dir.join(format!(
                "{space_name}-{}{}.db",
                session.uid, options.cache_tag
            ))
        });
        let store = Arc::new(ChatStore::open(&db_path)?);
        let (state_tx, state) = watch::channel(ConnectionState::Connecting);
        let last_error = Arc::new(Mutex::new(None));
        let config = KumoConfig::default();
        let engine = ChatEngine::new(ChatEngineDeps {
            transport: Arc::new(SendbirdTransport::new(api, &config, build_http_client()?)),
            store,
            listener: Arc::new(Listener {
                state: state_tx,
                last_error: last_error.clone(),
                arrivals,
            }),
        });
        engine.start(&space_name, &space_uid);
        Ok(Self {
            engine,
            http: build_http_client()?,
            space_name,
            space_uid,
            self_uid: session.uid,
            self_name: session.display_name.unwrap_or_default(),
            data_dir,
            state,
            last_error,
        })
    }

    pub async fn ready(&self) -> anyhow::Result<()> {
        let mut state = self.state.clone();
        let was_connected = !self.engine.self_id().is_empty();
        let timeout = if was_connected {
            RECONNECT_TIMEOUT
        } else {
            READY_TIMEOUT
        };
        let online =
            tokio::time::timeout(timeout, state.wait_for(|s| *s == ConnectionState::Online)).await;
        if matches!(online, Ok(Ok(_))) || was_connected {
            return Ok(());
        }
        let last = self.last_error.lock().ok().and_then(|e| e.clone());
        bail!(
            "Kumospace chat is not connected yet{}",
            last.map(|e| format!(": {e}")).unwrap_or_default()
        )
    }

    pub fn connection(&self) -> ConnectionState {
        *self.state.borrow()
    }

    pub fn self_id(&self) -> String {
        let id = self.engine.self_id();
        if id.is_empty() {
            self.self_uid.clone()
        } else {
            id
        }
    }

    pub fn self_channel(&self) -> String {
        let me = self.self_id();
        ChannelUrl::direct_message(&self.space_uid, [&me, &me])
    }

    pub fn channel(&self, name: &str) -> String {
        if name == "self" {
            self.self_channel()
        } else {
            name.to_string()
        }
    }

    pub fn direct_channel(&self, user_id: &str) -> String {
        ChannelUrl::direct_message(&self.space_uid, [&self.self_id(), user_id])
    }

    pub fn url_prefix(&self) -> String {
        format!("{}_", self.space_uid)
    }
}

pub struct Connection {
    options: ConnectOptions,
    slot: tokio::sync::Mutex<Option<Arc<Kumo>>>,
    arrivals: Arc<Arrivals>,
}

impl Connection {
    pub fn new(options: ConnectOptions) -> Self {
        Self {
            options,
            slot: tokio::sync::Mutex::new(None),
            arrivals: Arc::default(),
        }
    }

    #[cfg(test)]
    pub fn for_test(options: ConnectOptions) -> Self {
        let connection = Self::new(options);
        let sign_in = SignIn::new(connection.options.session_path.clone()).unwrap();
        let (state_tx, state) = watch::channel(ConnectionState::Online);
        let last_error = Arc::new(Mutex::new(None));
        let kumo = Kumo {
            engine: ChatEngine::new(ChatEngineDeps {
                transport: Arc::new(SendbirdTransport::new(
                    sign_in.api,
                    &KumoConfig::default(),
                    build_http_client().unwrap(),
                )),
                store: Arc::new(
                    ChatStore::open(connection.options.db_path.as_ref().unwrap()).unwrap(),
                ),
                listener: Arc::new(Listener {
                    state: state_tx,
                    last_error: last_error.clone(),
                    arrivals: connection.arrivals.clone(),
                }),
            }),
            http: build_http_client().unwrap(),
            space_name: "test".into(),
            space_uid: "test".into(),
            self_uid: "me".into(),
            self_name: "Me".into(),
            data_dir: connection
                .options
                .db_path
                .as_ref()
                .unwrap()
                .parent()
                .unwrap()
                .into(),
            state,
            last_error,
        };
        *connection.slot.try_lock().unwrap() = Some(Arc::new(kumo));
        connection
    }

    pub fn arrivals(&self) -> &Arc<Arrivals> {
        &self.arrivals
    }

    pub fn watch_rules(&self) -> WatchRules {
        WatchRules::load(&self.options.watch_path)
    }

    pub fn ai_note(&self) -> AiNote {
        AiNote::load(&AiNote::path_beside(&self.options.agents_path))
    }

    pub fn allowed(&self) -> bool {
        AgentAccess::load(&self.options.agents_path).enabled
    }

    pub async fn close(&self) {
        if let Some(kumo) = self.slot.lock().await.take() {
            kumo.engine.stop();
        }
    }

    pub async fn get(&self) -> anyhow::Result<Arc<Kumo>> {
        let mut slot = self.slot.lock().await;
        if !self.allowed() {
            if let Some(kumo) = slot.take() {
                kumo.engine.stop();
            }
            bail!(ACCESS_OFF);
        }
        if let Some(kumo) = slot.as_ref() {
            return Ok(kumo.clone());
        }
        let kumo = Arc::new(Kumo::connect(&self.options, self.arrivals.clone()).await?);
        if !self.allowed() {
            kumo.engine.stop();
            bail!(ACCESS_OFF);
        }
        *slot = Some(kumo.clone());
        Ok(kumo)
    }

    pub async fn ready(&self) -> anyhow::Result<Arc<Kumo>> {
        let kumo = self.get().await?;
        kumo.ready().await?;
        if !self.allowed() {
            self.close().await;
            bail!(ACCESS_OFF);
        }
        Ok(kumo)
    }

    pub async fn watch_access(&self) {
        loop {
            tokio::time::sleep(ACCESS_POLL).await;
            if !self.allowed() {
                self.close().await;
            }
        }
    }
}
