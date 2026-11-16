pub mod config;
pub mod desktop;
mod desktop_session;
pub mod devices;
pub mod network;
mod pair_page;
pub mod push;
mod server;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use kumo_core::chat::alert::{Alert, message_alert};
use kumo_core::chat::{
    ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore, ConnectionState,
};
use kumo_core::http::{build_http_client, now_ms};
use kumo_core::auth::MemorySessionStore;
use kumo_core::{Auth, AuthSession, FileSessionStore, FirebaseAuth, KumoApi, KumoConfig};

pub use config::Config;
pub use desktop_session::import_desktop_session;
use devices::Devices;
use push::{Apns, Ntfy, Outcome};

pub struct Relay {
    pub devices: Devices,
    pub apns: Option<Apns>,
    pub ntfy: Option<Ntfy>,
    pub enabled: AtomicBool,
    pub online: AtomicBool,
    pub state_path: PathBuf,
    pub uid: String,
    pub desktop: Option<Arc<desktop::Desktop>>,
}

impl Relay {
    pub fn mirroring(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn chat_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub fn desktop_connected(&self) -> bool {
        self.desktop.as_ref().is_some_and(|d| d.connected())
    }

    pub fn phone_in_call(&self) -> bool {
        self.desktop.as_ref().is_some_and(|d| d.handed_off())
    }

    pub fn device_count(&self) -> usize {
        self.devices.all().len()
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        let _ = std::fs::write(
            &self.state_path,
            serde_json::json!({ "enabled": enabled }).to_string(),
        );
    }

    pub async fn dispatch(&self, alert: &Alert) {
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        if let Some(ntfy) = self.ntfy.as_ref().filter(|n| n.enabled())
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

impl ChatListener for Listener {
    fn on_event(&self, event: ChatEvent) {
        match event {
            ChatEvent::Incoming(incoming) => {
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
    let mut secret_bytes = [0u8; 24];
    getrandom::fill(&mut secret_bytes).map_err(|e| anyhow::anyhow!("no system randomness: {e}"))?;
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut config = Config::template(&secret, space);
    config.ntfy = ntfy_topic.map(|topic| config::NtfyConfig {
        server: "https://ntfy.sh".into(),
        topic,
        enabled: true,
    });
    config.save(path)?;
    Ok(config)
}

pub fn random_topic() -> String {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);
    format!("kumo-{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
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

pub async fn setup_from_desktop(path: &Path, cdp_url: &str) -> anyhow::Result<Config> {
    let desktop = import_desktop_session(cdp_url).await?;
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
    let mut config = match Config::load(path) {
        Ok(config) => config,
        Err(_) => {
            let space = api
                .default_space()
                .await?
                .context("this Kumospace account has no space")?;
            init(path, &space, Some(random_topic()))?
        }
    };
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

pub async fn run(
    path: &Path,
    shutdown: impl Future<Output = ()> + Send + 'static,
    on_ready: impl FnOnce(Arc<Relay>),
) -> anyhow::Result<()> {

    let config = Config::load(path)?;
    std::fs::create_dir_all(config.data_dir())?;
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
    let space_uid = space["uid"].as_str().unwrap_or_default().to_string();

    let state_path = config.data_dir().join("state.json");
    let enabled = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["enabled"].as_bool())
        .unwrap_or(true);
    let apns = match &config.apns {
        Some(c) => Some(Apns::new(c.clone(), http.clone())?),
        None => None,
    };
    let relay = Arc::new(Relay {
        devices: Devices::load(config.devices_path()),
        apns,
        ntfy: config.ntfy.clone().map(|c| Ntfy::new(c, http.clone())),
        enabled: AtomicBool::new(enabled),
        online: AtomicBool::new(false),
        state_path,
        uid,
        desktop: config.desktop_cdp.clone().map(desktop::Desktop::new),
    });
    if let Some(desktop) = &relay.desktop {
        desktop.spawn(relay.clone());
    }
    let engine = ChatEngine::new(ChatEngineDeps {
        api,
        config: core_config,
        http,
        store: Arc::new(ChatStore::open(&config.store_path())?),
        listener: Arc::new(Listener(relay.clone())),
    });
    engine.start(&config.space, &space_uid);
    eprintln!(
        "kumo-relay: space {} as {}, ntfy={}, apns={}, mirroring={}, listening on {} (started {})",
        config.space,
        relay.uid,
        relay.ntfy.is_some(),
        relay.apns.is_some(),
        enabled,
        config.listen,
        now_ms()
    );
    let app = server::router(relay.clone(), config.clone());
    let listener = tokio::net::TcpListener::bind(&config.listen)
        .await
        .with_context(|| format!("cannot listen on {}; is another kumo-relay running?", config.listen))?;
    on_ready(relay.clone());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await?;
    engine.stop();
    Ok(())
}
