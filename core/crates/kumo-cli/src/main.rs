mod desktop;
mod session_file;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use kumo_core::chat::{ChatEngine, ChatEngineDeps, ChatEvent, ChatListener, ChatStore};
use kumo_core::http::{build_http_client, now_ms};
use kumo_core::sendbird::{
    ChannelListQuery, MessageQuery, SbEvent, SendbirdRest, SendbirdSocket, SocketParams,
};
use kumo_core::{
    Auth, ConnectParams, FirebaseAuth, KumoApi, KumoConfig, PasswordCredentials, Platform,
    ServerOp, SessionStore, SpaceConnection, SpaceEvent,
};
use serde_json::Value;
use session_file::FileSessionStore;

#[derive(Parser)]
#[command(
    name = "kumo",
    about = "Kumospace client CLI over the kumowatch Rust core"
)]
struct Cli {
    #[arg(long, global = true)]
    session: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Login {
        #[arg(long, conflicts_with = "email")]
        from_desktop: bool,
        #[arg(long, default_value_t = 9222)]
        port: u16,
        #[arg(long)]
        email: Option<String>,
        #[arg(long, conflicts_with_all = ["from_desktop", "email"])]
        token_stdin: bool,
    },
    TransferToken {
        #[arg(long)]
        link: bool,
    },
    Logout,
    Whoami,
    Spaces,
    DefaultSpace,
    Space {
        space: String,
    },
    ChatConfig {
        space: String,
    },
    Members {
        space: String,
    },
    Get {
        path: String,
    },
    Channels {
        space: String,
        #[arg(long)]
        unread: bool,
    },
    Messages {
        space: String,
        channel_url: String,
        #[arg(long, default_value_t = 30)]
        limit: u32,
        #[arg(long)]
        threads: bool,
    },
    Changelog {
        space: String,
        channel_url: String,
        #[arg(long)]
        since_ms: i64,
    },
    TailChat {
        space: String,
    },
    SendSelf {
        space: String,
        text: String,
    },
    JoinTest {
        space: String,
        floor: String,
        zone: String,
        #[arg(long, default_value_t = 8)]
        seconds: u64,
    },
    DeleteSelf {
        space: String,
        ids: Vec<i64>,
    },
    RawSelf {
        space: String,
    },
    SelfOps {
        space: String,
    },
    ProbeFile {
        space: String,
        url: String,
    },
    ChatSync {
        space: String,
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value_t = 60)]
        seconds: u64,
    },
    TailSpace {
        space: String,
        #[arg(long)]
        raw: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = KumoConfig::default();
    let store = Arc::new(FileSessionStore::new(
        cli.session
            .clone()
            .unwrap_or_else(FileSessionStore::default_path),
    ));
    let http = build_http_client()?;
    let auth = Arc::new(Auth::new(FirebaseAuth::new(http.clone()), store.clone()));
    let api = KumoApi::new(http, config.api_base_url.clone(), auth.clone());

    match cli.command {
        Command::Login {
            from_desktop,
            port,
            email,
            token_stdin,
        } => {
            let session = if token_stdin {
                let mut token = String::new();
                std::io::stdin().read_line(&mut token)?;
                let token = token.trim().trim_start_matches("kumospace://login?token=");
                let token = token.split('&').next().unwrap_or_default();
                auth.sign_in_with_custom_token(&config.firebase_api_key, token)
                    .await?
            } else if from_desktop {
                let session = desktop::import_session(port).await?;
                auth.set_session(session).await?;
                auth.refreshed_id_token()
                    .await
                    .context("imported session could not be refreshed")?;
                auth.session().await.context("session missing")?
            } else {
                let email = match email {
                    Some(e) => e,
                    None => bail!("pass --from-desktop or --email"),
                };
                let password = rpassword::prompt_password("Password: ")?;
                auth.sign_in_with_password(PasswordCredentials {
                    api_key: &config.firebase_api_key,
                    email: &email,
                    password: &password,
                })
                .await?
            };
            println!(
                "signed in as {} ({}), session saved to {}",
                session.email.as_deref().unwrap_or("?"),
                session.uid,
                store.path().display()
            );
        }
        Command::TransferToken { link } => {
            let token = api.mint_login_token().await?;
            if link {
                println!("kumospace://login?token={token}");
            } else {
                println!("{token}");
            }
        }
        Command::Logout => {
            auth.sign_out().await?;
            println!("signed out");
        }
        Command::Whoami => match store.load() {
            Some(s) => {
                let left = s.expires_at_ms.saturating_sub(now_ms()) / 1000;
                println!(
                    "uid={} email={} name={} id_token_valid_for={}s",
                    s.uid,
                    s.email.unwrap_or_default(),
                    s.display_name.unwrap_or_default(),
                    left
                );
            }
            None => println!("not signed in"),
        },
        Command::Spaces => print_json(&Value::Array(api.self_spaces().await?)),
        Command::DefaultSpace => println!("{}", api.default_space().await?.unwrap_or_default()),
        Command::Space { space } => print_json(&api.space(&space).await?),
        Command::ChatConfig { space } => {
            let mut config = api.chat_config(&space).await?;
            config.session_token = format!("<redacted, {} chars>", config.session_token.len());
            print_json(&serde_json::to_value(config)?);
        }
        Command::Members { space } => print_json(&Value::Array(api.user_roles(&space).await?)),
        Command::Get { path } => print_json(&api.get_path(&path).await?),
        Command::Channels { space, unread } => {
            let (rest, socket) = chat_session(&api, &config, &space).await?;
            let started = std::time::Instant::now();
            let channels = rest
                .all_group_channels(&ChannelListQuery {
                    unread_only: unread,
                    include_empty: true,
                    limit: 100,
                    ..Default::default()
                })
                .await?;
            eprintln!("{} channels in {:?}", channels.len(), started.elapsed());
            for ch in &channels {
                let last = ch.last_message.as_ref();
                println!(
                    "{:<10} unread={:<3} mention={:<2} members={:<3} {} | {} | {}",
                    ch.custom_type,
                    ch.unread_message_count,
                    ch.unread_mention_count,
                    ch.member_count,
                    ch.channel_url,
                    display_name(ch, &socket.1),
                    last.map(|m| one_line(&m.message, 60)).unwrap_or_default()
                );
            }
            socket.0.close().await;
        }
        Command::Messages {
            space,
            channel_url,
            limit,
            threads,
        } => {
            let (rest, socket) = chat_session(&api, &config, &space).await?;
            let started = std::time::Instant::now();
            let mut query = MessageQuery::latest(limit);
            query.include_threads = threads;
            let messages = rest.messages(&channel_url, &query).await?;
            eprintln!("{} messages in {:?}", messages.len(), started.elapsed());
            for m in &messages {
                print_message(m);
            }
            socket.0.close().await;
        }
        Command::Changelog {
            space,
            channel_url,
            since_ms,
        } => {
            let (rest, socket) = chat_session(&api, &config, &space).await?;
            let log = rest.message_changelogs(&channel_url, since_ms, "").await?;
            println!(
                "updated={} deleted={} has_more={} next={}",
                log.updated.len(),
                log.deleted.len(),
                log.has_more,
                !log.next.is_empty()
            );
            for m in &log.updated {
                print_message(m);
            }
            socket.0.close().await;
        }
        Command::SendSelf { space, text } => {
            let space_uid = api.space(&space).await?["uid"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let uid = auth.uid().await?;
            let url = kumo_core::ChannelUrl::direct_message(&space_uid, [&uid, &uid]);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let engine = ChatEngine::new(ChatEngineDeps {
                api: api.clone(),
                config: config.clone(),
                http: build_http_client()?,
                store: Arc::new(ChatStore::in_memory()?),
                listener: Arc::new(PrintListener {
                    started: std::time::Instant::now(),
                    tx,
                }),
            });
            engine.start(&space, &space_uid);
            for _ in 0..50 {
                if !engine.self_id().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let started = std::time::Instant::now();
            engine.send_text(&url, &text, None).await?;
            let last = engine.messages(&url, 1)?;
            println!(
                "sent in {:?}: id={} state={:?} text={}",
                started.elapsed(),
                last.last().map(|m| m.id).unwrap_or_default(),
                last.last().map(|m| m.state),
                last.last().map(|m| m.text.clone()).unwrap_or_default()
            );
            engine.stop();
        }
        Command::JoinTest {
            space,
            floor,
            zone,
            seconds,
        } => {
            let params = ConnectParams {
                ws_base_url: config.ws_base_url.clone(),
                space_name: space.clone(),
                id_token: auth.id_token().await?,
                time_zone: local_time_zone(),
                platform: Platform::Android,
                reconnect_as: None,
            };
            let mut conn = SpaceConnection::connect(&params).await?;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            conn.send(&kumo_core::ClientOp::Join(kumo_core::ws::JoinData {
                room_name: Some(floor),
                teleport_target: Some(kumo_core::TeleportTarget::Zone { zone_id: zone }),
                password: None,
            }))
            .await?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);
            while let Ok(Some(event)) = tokio::time::timeout_at(deadline, conn.next_event()).await {
                let line = match &event {
                    SpaceEvent::Op(ServerOp::Room(r)) => format!(
                        "ROOM roomName={:?} dailyUrl={:?} token_len={}",
                        r.room_name,
                        r.daily_url,
                        r.daily_token.as_ref().map(|t| t.len()).unwrap_or(0)
                    ),
                    SpaceEvent::Op(ServerOp::Entities(list)) => {
                        let me: Vec<_> = list.iter().filter(|e| e.is_self).collect();
                        format!("entities {} self={:?}", list.len(), me)
                    }
                    SpaceEvent::Op(ServerOp::Census(_)) => continue,
                    other => one_line(&format!("{other:?}"), 300),
                };
                println!("{line}");
            }
            conn.send(&kumo_core::ClientOp::leave()).await?;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            conn.close().await;
        }
        Command::DeleteSelf { space, ids } => {
            let space_uid = api.space(&space).await?["uid"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let uid = auth.uid().await?;
            let url = kumo_core::ChannelUrl::direct_message(&space_uid, [&uid, &uid]);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let engine = ChatEngine::new(ChatEngineDeps {
                api: api.clone(),
                config: config.clone(),
                http: build_http_client()?,
                store: Arc::new(ChatStore::in_memory()?),
                listener: Arc::new(PrintListener {
                    started: std::time::Instant::now(),
                    tx,
                }),
            });
            engine.start(&space, &space_uid);
            for _ in 0..80 {
                if !engine.self_id().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            engine.open(&url);
            let _ = engine.load_thread(&url, ids[0]).await;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            for id in ids {
                match engine.delete_message(&url, id).await {
                    Ok(()) => println!("deleted {id}"),
                    Err(e) => println!("{id}: {e}"),
                }
            }
            engine.stop();
        }
        Command::RawSelf { space } => {
            let space_uid = api.space(&space).await?["uid"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let uid = auth.uid().await?;
            let url = kumo_core::ChannelUrl::direct_message(&space_uid, [&uid, &uid]);
            let chat = api.chat_config(&space).await?;
            let connected = SendbirdSocket::connect(&SocketParams {
                app_id: config.sendbird_app_id.clone(),
                user_id: chat.user_id,
                access_token: chat.session_token,
            })
            .await?;
            let frame = connected
                .socket
                .request("MESG", serde_json::json!({"channel_url": url, "message": "kumowatch raw ack test", "data": "", "custom_type": ""}))
                .await?;
            let p = &frame.payload;
            let ids: serde_json::Map<String, Value> = p
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter(|(k, v)| {
                            k.contains("id")
                                || k.contains("ts")
                                || k.contains("created")
                                || v.is_number()
                        })
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                })
                .unwrap_or_default();
            println!("{} {}", frame.code, Value::Object(ids));
            connected.socket.close().await;
        }
        Command::SelfOps { space } => {
            let space_uid = api.space(&space).await?["uid"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let uid = auth.uid().await?;
            let url = kumo_core::ChannelUrl::direct_message(&space_uid, [&uid, &uid]);
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let engine = ChatEngine::new(ChatEngineDeps {
                api: api.clone(),
                config: config.clone(),
                http: build_http_client()?,
                store: Arc::new(ChatStore::in_memory()?),
                listener: Arc::new(PrintListener {
                    started: std::time::Instant::now(),
                    tx,
                }),
            });
            engine.start(&space, &space_uid);
            for _ in 0..80 {
                if !engine.self_id().is_empty() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            if std::env::var("THREAD_TEST").is_ok() {
                engine.open(&url);
                engine
                    .send_text(&url, "kumowatch thread parent", None)
                    .await?;
                let parent = engine.messages(&url, 1)?.pop().context("no parent")?;
                engine
                    .send_text(&url, "kumowatch thread reply", Some(parent.id))
                    .await?;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let reply = engine.thread(&url, parent.id)?.pop().context("no reply")?;
                println!("parent={} reply={}", parent.id, reply.id);
                match engine.delete_message(&url, parent.id).await {
                    Ok(()) => println!(
                        "parent soft-deleted: {:?}",
                        engine.messages(&url, 1)?.pop().map(|m| m.text)
                    ),
                    Err(e) => println!("parent delete error: {e}"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                match engine.delete_message(&url, reply.id).await {
                    Ok(()) => println!(
                        "reply deleted; parent now: {:?}",
                        engine.messages(&url, 1)?.pop().map(|m| (m.id, m.text))
                    ),
                    Err(e) => println!("reply delete error: {e}"),
                }
                engine.stop();
                return Ok(());
            }
            engine.send_text(&url, "kumowatch ops test", None).await?;
            let msg = engine.messages(&url, 1)?.pop().context("no message")?;
            println!("sent id={}", msg.id);
            engine.toggle_reaction(&url, msg.id, "👍").await?;
            println!(
                "reaction added: {:?}",
                engine.messages(&url, 1)?.pop().map(|m| m.reactions)
            );
            engine.toggle_reaction(&url, msg.id, "👍").await?;
            println!(
                "reaction removed: {:?}",
                engine.messages(&url, 1)?.pop().map(|m| m.reactions)
            );
            engine
                .edit_message(&url, msg.id, "kumowatch ops test (edited)")
                .await?;
            println!(
                "edited: {:?}",
                engine.messages(&url, 1)?.pop().map(|m| m.text)
            );
            let png: Vec<u8> = vec![
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44,
                0x52, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D,
                0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05,
                0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE,
                0x42, 0x60, 0x82,
            ];
            engine
                .send_file(
                    kumo_core::sendbird::FileUpload {
                        channel_url: url.clone(),
                        name: "kumowatch-test.png".into(),
                        mime_type: "image/png".into(),
                        data: png,
                    },
                    None,
                )
                .await?;
            let file = engine.messages(&url, 1)?.pop().context("no file")?;
            println!(
                "file id={} kind={} type={} url_ok={}",
                file.id,
                file.kind,
                file.file_type,
                !file.file_url.is_empty()
            );
            engine.delete_message(&url, file.id).await?;
            engine.delete_message(&url, msg.id).await?;
            println!(
                "deleted both; remaining last: {:?}",
                engine.messages(&url, 1)?.pop().map(|m| (m.id, m.text))
            );
            engine.stop();
        }
        Command::ProbeFile { space, url } => {
            let chat = api.chat_config(&space).await?;
            let connected = SendbirdSocket::connect(&SocketParams {
                app_id: config.sendbird_app_id.clone(),
                user_id: chat.user_id,
                access_token: chat.session_token,
            })
            .await?;
            let http = build_http_client()?;
            let plain = http.get(&url).send().await?.status();
            let authed = http
                .get(format!("{url}?auth={}", connected.login.ekey))
                .send()
                .await?;
            println!(
                "plain={} with_ekey={} content_type={:?}",
                plain,
                authed.status(),
                authed.headers().get("content-type")
            );
            connected.socket.close().await;
        }
        Command::ChatSync { space, db, seconds } => {
            chat_sync(ChatSyncArgs {
                api: &api,
                config: &config,
                space: &space,
                db: &db,
                seconds,
            })
            .await?
        }
        Command::TailChat { space } => {
            let config_ = api.chat_config(&space).await?;
            let mut connected = SendbirdSocket::connect(&SocketParams {
                app_id: config.sendbird_app_id.clone(),
                user_id: config_.user_id.clone(),
                access_token: config_.session_token.clone(),
            })
            .await?;
            eprintln!(
                "sendbird connected as {} ping={:?}s",
                connected.login.user_id, connected.login.ping_interval
            );
            loop {
                let event = tokio::select! {
                    e = connected.events.recv() => e,
                    _ = tokio::signal::ctrl_c() => {
                        connected.socket.close().await;
                        return Ok(());
                    }
                };
                let Some(event) = event else { return Ok(()) };
                match event {
                    SbEvent::Message(m) => print_message(&m),
                    SbEvent::Disconnected { reason } => {
                        eprintln!("disconnected: {reason}");
                        return Ok(());
                    }
                    SbEvent::Other(f) => {
                        println!("{} {}", f.code, one_line(&f.payload.to_string(), 160))
                    }
                    other => println!("{}", one_line(&format!("{other:?}"), 200)),
                }
            }
        }
        Command::TailSpace { space, raw } => tail_space(&auth, &config, &space, raw).await?,
    }
    Ok(())
}

async fn tail_space(
    auth: &Auth,
    config: &KumoConfig,
    space: &str,
    raw: bool,
) -> anyhow::Result<()> {
    let params = ConnectParams {
        ws_base_url: config.ws_base_url.clone(),
        space_name: space.to_string(),
        id_token: auth.id_token().await?,
        time_zone: local_time_zone(),
        platform: Platform::Ios,
        reconnect_as: None,
    };
    let mut conn = SpaceConnection::connect(&params).await?;
    eprintln!("connected to {space}");
    loop {
        let event = tokio::select! {
            e = conn.next_event() => e,
            _ = tokio::signal::ctrl_c() => {
                conn.close().await;
                return Ok(());
            }
        };
        let Some(event) = event else { return Ok(()) };
        match event {
            SpaceEvent::Op(op) if raw => println!("{op:?}"),
            SpaceEvent::Op(op) => println!("{}", summarize(&op)),
            SpaceEvent::Malformed(e) => eprintln!("malformed frame: {e}"),
            SpaceEvent::Closed { code, banned } => {
                eprintln!("closed code={code:?} banned={banned}");
                return Ok(());
            }
        }
    }
}

fn summarize(op: &ServerOp) -> String {
    match op {
        ServerOp::Config(v) => format!(
            "config space={} uid={}",
            v["name"].as_str().unwrap_or("?"),
            v["uid"].as_str().unwrap_or("?")
        ),
        ServerOp::Census(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(id, u)| match u {
                    Some(u) => format!(
                        "{}[{}]",
                        u.name.as_deref().unwrap_or(id),
                        u.user_status
                            .as_ref()
                            .and_then(|s| s.availability.as_deref())
                            .unwrap_or("-")
                    ),
                    None => format!("-{id}"),
                })
                .collect();
            format!("census {}: {}", map.len(), parts.join(", "))
        }
        ServerOp::Entities(list) => format!("entities {}", list.len()),
        ServerOp::SpaceZones(map) => format!("spaceZones {}", map.len()),
        ServerOp::Room(r) => format!("room {}", r.room_name.as_deref().unwrap_or("-")),
        ServerOp::Error(e) => format!(
            "error {} {}",
            e.code.as_deref().unwrap_or("?"),
            e.message.as_deref().unwrap_or("")
        ),
        ServerOp::Notification(v) => format!("notification {v}"),
        ServerOp::UserSettings(_) => "userSettings".into(),
        ServerOp::HostMute(_) => "hostMute".into(),
        ServerOp::GuestTimeLimitReached(_) => "guestTimeLimitReached".into(),
        ServerOp::PlaySound(v) => format!("playSound {v}"),
        ServerOp::Unknown { op } => format!("unknown op {op}"),
    }
}

struct ChatSyncArgs<'a> {
    api: &'a KumoApi,
    config: &'a KumoConfig,
    space: &'a str,
    db: &'a std::path::Path,
    seconds: u64,
}

struct PrintListener {
    started: std::time::Instant,
    tx: tokio::sync::mpsc::UnboundedSender<ChatEvent>,
}

impl ChatListener for PrintListener {
    fn on_event(&self, event: ChatEvent) {
        if !matches!(
            event,
            ChatEvent::MessagesChanged { .. } | ChatEvent::ChannelsChanged
        ) {
            eprintln!("[{:>6}ms] {event:?}", self.started.elapsed().as_millis());
        }
        let _ = self.tx.send(event);
    }
}

async fn chat_sync(args: ChatSyncArgs<'_>) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    let store = Arc::new(ChatStore::open(args.db)?);
    let space = args.api.space(args.space).await?;
    let space_uid = space["uid"].as_str().unwrap_or_default().to_string();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = ChatEngine::new(ChatEngineDeps {
        api: args.api.clone(),
        config: args.config.clone(),
        http: build_http_client()?,
        store,
        listener: Arc::new(PrintListener { started, tx }),
    });
    engine.start(args.space, &space_uid);
    let t = std::time::Instant::now();
    let cached = engine.channels()?;
    let first = cached.first().map(|c| c.url.clone());
    let msgs = first
        .as_deref()
        .map(|u| engine.messages(u, 50))
        .transpose()?
        .unwrap_or_default();
    eprintln!(
        "cache: {} channels, first channel {} messages, read in {:?}",
        cached.len(),
        msgs.len(),
        t.elapsed()
    );
    let mut changed_channels = std::collections::HashSet::new();
    let deadline = tokio::time::sleep(std::time::Duration::from_secs(args.seconds));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = tokio::signal::ctrl_c() => break,
            e = rx.recv() => match e {
                Some(ChatEvent::MessagesChanged { channel_url }) => { changed_channels.insert(channel_url); }
                Some(ChatEvent::Connection(kumo_core::chat::ConnectionState::Online)) => {
                    eprintln!("[{:>6}ms] online, {} channels got messages", started.elapsed().as_millis(), changed_channels.len());
                }
                Some(_) => {}
                None => break,
            }
        }
    }
    engine.stop();
    for ch in engine.channels()?.iter().take(8) {
        let n = engine.messages(&ch.url, 1000)?.len();
        println!(
            "{:<9} unread={:<3} cached={:<4} {}",
            ch.custom_type,
            ch.unread,
            n,
            one_line(&ch.title, 50)
        );
    }
    Ok(())
}

async fn chat_session(
    api: &KumoApi,
    config: &KumoConfig,
    space: &str,
) -> anyhow::Result<(SendbirdRest, (SendbirdSocket, String))> {
    let started = std::time::Instant::now();
    let chat = api.chat_config(space).await?;
    let connected = SendbirdSocket::connect(&SocketParams {
        app_id: config.sendbird_app_id.clone(),
        user_id: chat.user_id.clone(),
        access_token: chat.session_token.clone(),
    })
    .await?;
    let rest = SendbirdRest::new(build_http_client()?, &config.sendbird_app_id, &chat.user_id);
    rest.set_session_key(&connected.login.key);
    eprintln!("sendbird login in {:?}", started.elapsed());
    Ok((rest, (connected.socket, chat.user_id)))
}

fn display_name(ch: &kumo_core::sendbird::SbChannel, self_id: &str) -> String {
    if !ch.name.is_empty() && ch.custom_type != "user" {
        return ch.name.clone();
    }
    let others: Vec<&str> = ch
        .members
        .iter()
        .filter(|m| m.user_id != self_id)
        .map(|m| m.nickname.as_str())
        .collect();
    if others.is_empty() {
        ch.name.clone()
    } else {
        others.join(", ")
    }
}

fn print_message(m: &kumo_core::sendbird::SbMessage) {
    let who = m.user.as_ref().map(|u| u.nickname.as_str()).unwrap_or("-");
    let thread = m
        .thread_info
        .as_ref()
        .map(|t| format!(" [{} replies]", t.reply_count))
        .unwrap_or_default();
    let parent = if m.is_thread_reply() { " ↳" } else { "" };
    println!(
        "{} {}{} {} {}: {}{}",
        m.created_at,
        m.kind,
        parent,
        m.id(),
        who,
        one_line(&m.message, 120),
        thread
    );
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

fn local_time_zone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        return tz;
    }
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| {
            p.to_string_lossy()
                .split("zoneinfo/")
                .nth(1)
                .map(String::from)
        })
        .unwrap_or_else(|| "UTC".into())
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}
