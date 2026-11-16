use std::path::PathBuf;

use clap::{Parser, Subcommand};
use kumo_core::FileSessionStore;
use kumo_relay::Config;
use kumo_relay::config;
use kumo_relay::devices::Devices;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(config::default_config_path);
    match cli.command {
        Command::Init { space, ntfy_topic } => {
            kumo_relay::init(&config_path, &space, ntfy_topic)?;
            println!("wrote {}", config_path.display());
            Ok(())
        }
        Command::Login { token_stdin } => {
            anyhow::ensure!(
                token_stdin,
                "pipe a sign-in token: kumo-cli transfer-token | kumo-relay login --token-stdin"
            );
            let mut token = String::new();
            std::io::stdin().read_line(&mut token)?;
            let session = kumo_relay::login_with_token(&config_path, &token).await?;
            println!(
                "relay signed in as {} ({})",
                session.email.unwrap_or_default(),
                session.uid
            );
            Ok(())
        }
        Command::Run => {
            kumo_relay::run(
                &config_path,
                async {
                    let _ = tokio::signal::ctrl_c().await;
                },
                |_| {},
            )
            .await
        }
        Command::Pair { url } => {
            let config = Config::load(&config_path)?;
            println!(
                "Open this link on the phone (keep it private, it contains the relay secret):\n{}",
                kumo_relay::pair_link(&config, &url)
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
