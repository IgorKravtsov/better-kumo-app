mod config;
mod devices;
mod push;
mod server;

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use clap::{Parser, Subcommand};
use kumo_core::chat::alert::{Alert, message_alert};
use kumo_core::chat::{
    ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore, ConnectionState,
};
use kumo_core::http::{build_http_client, now_ms};
use kumo_core::{Auth, FileSessionStore, FirebaseAuth, KumoApi, KumoConfig};

use config::Config;
use devices::Devices;
use push::{Apns, Ntfy, Outcome};

#[derive(Parser)]
#[command(
    name = "kumo-relay",
    about = "Mirrors Kumospace chat notifications to phones (ntfy and APNs)"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Init {
        #[arg(long)]
        space: String,
        #[arg(long)]
        ntfy_topic: Option<String>,
    },
    Login {
        #[arg(long)]
        token_stdin: bool,
    },
    Run,
    Status,
    Pair {
        #[arg(long)]
        url: String,
    },
}

pub struct Relay {
    pub devices: Devices,
    pub apns: Option<Apns>,
    pub ntfy: Option<Ntfy>,
    pub enabled: AtomicBool,
    pub online: AtomicBool,
    pub state_path: PathBuf,
    pub uid: String,
}

impl Relay {
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);
    match cli.command {
        Command::Init { space, ntfy_topic } => init(&config_path, &space, ntfy_topic),
        Command::Login { token_stdin } => login(&config_path, token_stdin).await,
        Command::Run => run(&config_path).await,
        Command::Pair { url } => {
            let config = Config::load(&config_path)?;
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
            println!(
                "Open this link on the phone (keep it private, it contains the relay secret):\nkumowatch://relay?url={}&secret={}",
                enc(&url),
                enc(&config.secret)
            );
            Ok(())
        }
        Command::Status => {
            let config = Config::load(&config_path)?;
            let store = FileSessionStore::new(config.session_path());
            match kumo_core::SessionStore::load(&store) {
                Some(s) => println!("signed in as {} ({})", s.email.unwrap_or_default(), s.uid),
                None => println!("not signed in"),
            }
            println!(
                "devices: {}",
                Devices::load(config.devices_path()).all().len()
            );
            Ok(())
        }
    }
}

fn init(path: &std::path::Path, space: &str, ntfy_topic: Option<String>) -> anyhow::Result<()> {
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
    }
    let mut secret_bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut secret_bytes)?;
    let secret: String = secret_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut config = Config::template(&secret, space);
    config.ntfy = ntfy_topic.map(|topic| config::NtfyConfig {
        server: "https://ntfy.sh".into(),
        topic,
        enabled: true,
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, toml::to_string_pretty(&config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    println!("wrote {}", path.display());
    Ok(())
}

async fn login(path: &std::path::Path, token_stdin: bool) -> anyhow::Result<()> {
    anyhow::ensure!(
        token_stdin,
        "pipe a sign-in token: kumo-cli transfer-token | kumo-relay login --token-stdin"
    );
    let config = Config::load(path)?;
    let mut token = String::new();
    std::io::stdin().read_line(&mut token)?;
    let token = token.trim().trim_start_matches("kumospace://login?token=");
    let token = token.split('&').next().unwrap_or_default();
    let http = build_http_client()?;
    let auth = Auth::new(
        FirebaseAuth::new(http),
        Arc::new(FileSessionStore::new(config.session_path())),
    );
    let session = auth
        .sign_in_with_custom_token(&KumoConfig::default().firebase_api_key, token)
        .await?;
    println!(
        "relay signed in as {} ({})",
        session.email.unwrap_or_default(),
        session.uid
    );
    Ok(())
}

async fn run(path: &std::path::Path) -> anyhow::Result<()> {
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
    });
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
    let app = server::router(relay.clone(), config.secret.clone());
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    engine.stop();
    Ok(())
}
