mod attachments;
mod format;
mod kumo;
mod server;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use kumo_core::FileSessionStore;
use kumo_core::desktop::AgentAccess;
use rmcp::ServiceExt;

use crate::kumo::{ConnectOptions, Connection};
use crate::server::{KumoServer, ServerOptions};

#[derive(Parser)]
#[command(
    name = "kumo-mcp",
    version,
    about = "MCP server that gives Claude, Codex and other agents your Kumospace chat"
)]
struct Cli {
    #[arg(long, global = true, help = "Session file (shared with kumo-cli)")]
    session: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Space name; defaults to the Relay's space, then your default space"
    )]
    space: Option<String>,
    #[arg(long, global = true, help = "Chat cache database")]
    db: Option<PathBuf>,
    #[arg(long, global = true, default_value_t = kumo_devtools::DEFAULT_PORT)]
    devtools_port: u16,
    #[arg(
        long,
        global = true,
        help = "The on/off switch for AI agents (shared with Better Kumo)"
    )]
    agents_file: Option<PathBuf>,
    #[arg(long, global = true, help = "Expose only the reading tools")]
    read_only: bool,
    #[arg(
        long,
        global = true,
        help = "Write only to your own notes channel (for testing)"
    )]
    self_only: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Serve MCP over stdio (the default)")]
    Serve,
    #[command(about = "Sign in by importing the session from the running Kumospace desktop app")]
    Login,
    #[command(about = "Connect once and print who you are and how many conversations are synced")]
    Check,
}

impl Cli {
    fn connect_options(&self) -> ConnectOptions {
        ConnectOptions {
            session_path: self
                .session
                .clone()
                .unwrap_or_else(FileSessionStore::default_path),
            space: self.space.clone(),
            db_path: self.db.clone(),
            devtools_port: self.devtools_port,
            agents_path: self
                .agents_file
                .clone()
                .unwrap_or_else(AgentAccess::default_path),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let options = cli.connect_options();
    match cli.command.as_ref().unwrap_or(&Command::Serve) {
        Command::Serve => {
            serve(
                options,
                ServerOptions {
                    read_only: cli.read_only,
                    self_only: cli.self_only,
                },
            )
            .await
        }
        Command::Login => login(&options).await,
        Command::Check => check(options).await,
    }
}

async fn serve(options: ConnectOptions, server_options: ServerOptions) -> anyhow::Result<()> {
    let connection = Arc::new(Connection::new(options));
    let warm = connection.clone();
    tokio::spawn(async move {
        if let Err(e) = warm.get().await {
            eprintln!("kumo-mcp: {e:#}");
        }
    });
    let watcher = connection.clone();
    tokio::spawn(async move { watcher.watch_access().await });
    let service = KumoServer::new(connection, server_options)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

async fn login(options: &ConnectOptions) -> anyhow::Result<()> {
    let sign_in = kumo::SignIn::new(options.session_path.clone())?;
    kumo::import_from_desktop(&sign_in.auth, options.devtools_port).await?;
    let session = sign_in.auth.session().await;
    println!(
        "signed in as {} ({}); session saved to {}",
        session
            .as_ref()
            .and_then(|s| s.display_name.clone())
            .unwrap_or_default(),
        session.map(|s| s.uid).unwrap_or_default(),
        options.session_path.display()
    );
    Ok(())
}

async fn check(options: ConnectOptions) -> anyhow::Result<()> {
    let connection = Connection::new(options);
    let kumo = connection.ready().await?;
    let channels = kumo.engine.channels()?;
    println!(
        "{} ({}) in space {}: chat {:?}, {} conversations, {} unread, {} people in the directory",
        kumo.self_name,
        kumo.self_id(),
        kumo.space_name,
        kumo.connection(),
        channels.len(),
        channels.iter().filter(|c| c.unread > 0).count(),
        kumo.engine.directory_snapshot().len()
    );
    kumo.engine.stop();
    Ok(())
}
