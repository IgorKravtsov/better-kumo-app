mod arrivals;
mod attachments;
mod format;
mod http;
mod kumo;
mod relay_events;
mod server;
mod watch;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use kumo_core::FileSessionStore;
use kumo_core::desktop::AgentAccess;
use kumo_core::events::WatchRules;
use rmcp::ServiceExt;
use tokio_util::sync::CancellationToken;

use crate::kumo::{ConnectOptions, Connection};
use crate::server::{KumoServer, ServerOptions};

const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

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
    #[arg(
        long,
        global = true,
        help = "The rules check_events and watch use by default (shared with Better Kumo settings)"
    )]
    watch_file: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Bearer token file for serve-http [default: ~/.config/kumowatch/mcp-token]"
    )]
    token_file: Option<PathBuf>,
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
    #[command(
        about = "Serve MCP over HTTP with a bearer token, for agents on other computers on your network"
    )]
    ServeHttp {
        #[arg(long, default_value = http::DEFAULT_LISTEN, help = "Address and port to listen on")]
        listen: String,
    },
    #[command(about = "Print the bearer token for serve-http, creating it if needed")]
    Token {
        #[arg(
            long,
            help = "Replace it with a new one; restart serve-http afterwards"
        )]
        rotate: bool,
    },
    #[command(
        about = "Print matching messages and office events as they happen, one JSON object per line",
        long_about = "Print matching messages and office events as they happen, one JSON object per line, until stopped.\n\nRun it in the background (e.g. under Claude Code's Monitor or a background shell) to watch for events without blocking the agent's conversation (the check_events tool only answers what already happened). --people/--kinds/--places/--label describe one rule (comma-separated lists, empty = any); --rules takes several, as JSON in the shape of check_events' `rules`. Without either it uses the rules saved under \"Watched events\" in Better Kumo settings. It keeps its own chat cache next to the MCP server's."
    )]
    Watch {
        #[arg(long, value_delimiter = ',', help = "User ids or full names")]
        people: Option<Vec<String>>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "message, nudge, knock, entered_room, left_room, joined_floor, online, offline, status, nearby_chat"
        )]
        kinds: Option<Vec<String>>,
        #[arg(
            long,
            value_delimiter = ',',
            help = "Room or floor ids or names (office events only)"
        )]
        places: Option<Vec<String>>,
        #[arg(long, help = "A name for the rule, shown in each event's `rules`")]
        label: Option<String>,
        #[arg(
            long,
            conflicts_with_all = ["people", "kinds", "places", "label"],
            help = "Several rules as a JSON array of {label, kinds, people, places}"
        )]
        rules: Option<String>,
    },
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
            watch_path: self
                .watch_file
                .clone()
                .unwrap_or_else(WatchRules::default_path),
            cache_tag: "",
        }
    }

    /// `kumo-mcp watch` for the same account, space, switch and saved rules as this process.
    fn watch_command(&self) -> String {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "kumo-mcp".into());
        let mut words = vec![exe];
        let mut flag = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                words.push(name.into());
                words.push(value);
            }
        };
        // The monitor may start in another directory than this server did.
        let path = |p: &Option<PathBuf>| {
            p.as_ref()
                .map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))
                .map(|p| p.display().to_string())
        };
        flag("--session", path(&self.session));
        flag("--space", self.space.clone());
        flag(
            "--devtools-port",
            (self.devtools_port != kumo_devtools::DEFAULT_PORT)
                .then(|| self.devtools_port.to_string()),
        );
        flag("--agents-file", path(&self.agents_file));
        flag("--watch-file", path(&self.watch_file));
        words.push("watch".into());
        words
            .iter()
            .map(|w| server::shell_quote(w))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn token_path(&self) -> PathBuf {
        self.token_file
            .clone()
            .unwrap_or_else(http::default_token_path)
    }

    fn server_options(&self, remote: bool) -> ServerOptions {
        ServerOptions {
            read_only: self.read_only,
            self_only: self.self_only,
            remote,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let options = cli.connect_options();
    match cli.command.as_ref().unwrap_or(&Command::Serve) {
        Command::Serve => serve(options, cli.server_options(false), cli.watch_command()).await,
        Command::ServeHttp { listen } => {
            serve_http(HttpOptions {
                connect: options,
                server: cli.server_options(true),
                listen: listen.clone(),
                token_path: cli.token_path(),
            })
            .await
        }
        Command::Token { rotate } => {
            let path = cli.token_path();
            let token = if *rotate {
                http::create_token(&path)?
            } else {
                http::load_or_create_token(&path)?
            };
            println!("{token}");
            Ok(())
        }
        Command::Watch {
            people,
            kinds,
            places,
            label,
            rules,
        } => {
            let db_path = options.db_path.as_ref().map(|db| {
                let stem = db.file_stem().unwrap_or_default().to_string_lossy();
                db.with_file_name(format!("{stem}-watch.db"))
            });
            let connection = Arc::new(Connection::new(ConnectOptions {
                cache_tag: "-watch",
                db_path,
                ..options
            }));
            let watcher = connection.clone();
            tokio::spawn(async move { watcher.watch_access().await });
            watch::watch(
                connection,
                watch::WatchArgs {
                    people: people.clone(),
                    kinds: kinds.clone(),
                    places: places.clone(),
                    label: label.clone(),
                    rules: rules.clone(),
                },
            )
            .await
        }
        Command::Login => login(&options).await,
        Command::Check => check(options).await,
    }
}

struct HttpOptions {
    connect: ConnectOptions,
    server: ServerOptions,
    listen: String,
    token_path: PathBuf,
}

fn start_connection(options: ConnectOptions) -> Arc<Connection> {
    let connection = Arc::new(Connection::new(options));
    let warm = connection.clone();
    tokio::spawn(async move {
        if let Err(e) = warm.get().await {
            eprintln!("kumo-mcp: {e:#}");
        }
    });
    let watcher = connection.clone();
    tokio::spawn(async move { watcher.watch_access().await });
    connection
}

async fn serve(
    options: ConnectOptions,
    server_options: ServerOptions,
    watch_command: String,
) -> anyhow::Result<()> {
    let connection = start_connection(options);
    let service = KumoServer::new(connection, server_options)
        .with_watch_command(watch_command)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

async fn serve_http(options: HttpOptions) -> anyhow::Result<()> {
    let token = http::load_or_create_token(&options.token_path)?;
    let listener = tokio::net::TcpListener::bind(&options.listen)
        .await
        .with_context(|| format!("cannot listen on {}", options.listen))?;
    let address = listener.local_addr()?;
    let connection = start_connection(options.connect);
    let shutdown = CancellationToken::new();
    let router = http::router(
        KumoServer::new(connection.clone(), options.server),
        token,
        shutdown.clone(),
    );
    eprintln!(
        "kumo-mcp: serving MCP on http://{address}{}; bearer token in {} (print it with `kumo-mcp token`)",
        http::MCP_PATH,
        options.token_path.display()
    );
    let stopping = shutdown.clone();
    let server = axum::serve(listener, router).with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        stopping.cancel();
    });
    tokio::select! {
        result = server => result?,
        _ = async {
            shutdown.cancelled().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => eprintln!("kumo-mcp: closing connections that did not finish in time"),
    }
    if tokio::time::timeout(SHUTDOWN_GRACE, connection.close())
        .await
        .is_err()
    {
        eprintln!("kumo-mcp: Kumospace did not disconnect in time");
    }
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
