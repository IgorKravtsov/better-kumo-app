#![cfg_attr(windows, windows_subsystem = "windows")]

mod icon;
mod kumospace;
mod supervisor;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::watch;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use kumospace::Kumospace;
use supervisor::{Shared, Status};

const APP_NAME: &str = "Better Kumo";

struct Items {
    kumospace: MenuItem,
    relay: MenuItem,
    pair: MenuItem,
    open_kumospace: MenuItem,
    restart: MenuItem,
    mirror: CheckMenuItem,
    login: CheckMenuItem,
    log: MenuItem,
    quit: MenuItem,
}

fn log_path() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Logs/kumowatch/better-kumo.log")
    } else {
        home.join(".local/share/kumowatch-relay/better-kumo.log")
    }
}

#[cfg(unix)]
fn redirect_logs() {
    use std::os::fd::AsRawFd;
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
            libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        }
        std::mem::forget(file);
    }
}

#[cfg(not(unix))]
fn redirect_logs() {}

fn autostart() -> Option<AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    AutoLaunchBuilder::new()
        .set_app_name("better-kumo")
        .set_app_path(&exe.to_string_lossy())
        .build()
        .ok()
}

fn build_menu() -> (Menu, Items) {
    let items = Items {
        kumospace: MenuItem::new("Kumospace: starting…", false, None),
        relay: MenuItem::new("Phone control: starting…", false, None),
        pair: MenuItem::new("Pair a phone…", true, None),
        open_kumospace: MenuItem::new("Open Kumospace", true, None),
        restart: MenuItem::new("Restart Kumospace for phone control", false, None),
        mirror: CheckMenuItem::new("Mirror notifications to phone", false, true, None),
        login: CheckMenuItem::new(
            "Start at login",
            true,
            autostart().and_then(|a| a.is_enabled().ok()).unwrap_or(false),
            None,
        ),
        log: MenuItem::new("Open log", true, None),
        quit: MenuItem::new(format!("Quit {APP_NAME}"), true, None),
    };
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &items.kumospace,
        &items.relay,
        &PredefinedMenuItem::separator(),
        &items.pair,
        &items.open_kumospace,
        &items.restart,
        &PredefinedMenuItem::separator(),
        &items.mirror,
        &items.login,
        &items.log,
        &PredefinedMenuItem::separator(),
        &items.quit,
    ]);
    (menu, items)
}

fn refresh(items: &Items, status: &Status) {
    let kumospace = match status.kumospace {
        Kumospace::NotInstalled => "Kumospace: not installed",
        Kumospace::NotRunning => "Kumospace: starting…",
        Kumospace::RunningWithoutPort { .. } => "Kumospace: opened without phone control",
        Kumospace::Running => "Kumospace: running",
    };
    items.kumospace.set_text(kumospace);
    items
        .restart
        .set_enabled(matches!(status.kumospace, Kumospace::RunningWithoutPort { .. }));
    let relay = match (&status.relay, &status.problem) {
        (Some(relay), _) => {
            let desktop = if relay.phone_in_call() {
                "phone is in the call"
            } else if relay.desktop_connected() {
                "desktop connected"
            } else {
                "waiting for desktop"
            };
            let phones = match relay.device_count() {
                0 => String::new(),
                1 => " · 1 phone".into(),
                n => format!(" · {n} phones"),
            };
            format!("Phone control: {desktop}{phones}")
        }
        (None, Some(problem)) => {
            let short: String = problem.chars().take(60).collect();
            format!("Problem: {short}")
        }
        (None, None) => "Phone control: starting…".into(),
    };
    items.relay.set_text(relay);
    match &status.relay {
        Some(relay) => {
            items.mirror.set_enabled(true);
            items.mirror.set_checked(relay.mirroring());
        }
        None => items.mirror.set_enabled(false),
    }
}

fn main() {
    if std::env::args().any(|a| a == "--foreground") {
        eprintln!("{APP_NAME}: logging to the terminal");
    } else {
        redirect_logs();
    }
    eprintln!("{APP_NAME} {} starting", env!("CARGO_PKG_VERSION"));

    let status: Shared = Arc::new(Mutex::new(Status::default()));
    let (stop_tx, stop_rx) = watch::channel(false);
    let stop_tx = Arc::new(stop_tx);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.spawn(supervisor::supervise(status.clone(), stop_rx));

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::new().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let mut tray: Option<TrayIcon> = None;
    let mut items: Option<Items> = None;
    let mut quitting: Option<Instant> = None;
    let menu_events = MenuEvent::receiver();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));
        if let Event::NewEvents(StartCause::Init) = event {
            let (menu, built) = build_menu();
            tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(APP_NAME)
                .with_icon_templated(icon::tray())
                .build()
                .ok();
            items = Some(built);
        }

        if let Some(started) = quitting {
            let stopped = status.lock().map(|s| s.relay.is_none()).unwrap_or(true);
            if stopped || started.elapsed() > Duration::from_secs(4) {
                tray.take();
                *control_flow = ControlFlow::Exit;
            }
            return;
        }

        let Some(items) = items.as_ref() else { return };
        let snapshot = status.lock().map(|s| s.clone()).unwrap_or_default();
        refresh(items, &snapshot);

        while let Ok(event) = menu_events.try_recv() {
            let id = event.id;
            if id == items.pair.id() {
                let _ = open::that(format!("http://127.0.0.1:{}/pair", snapshot.port));
            } else if id == items.open_kumospace.id() {
                if snapshot.kumospace == Kumospace::NotRunning {
                    let _ = kumospace::launch();
                } else if cfg!(target_os = "macos") {
                    let _ = std::process::Command::new("open").args(["-a", "Kumospace"]).spawn();
                }
            } else if id == items.restart.id() {
                std::thread::spawn(|| {
                    if let Err(e) = kumospace::restart_with_port() {
                        eprintln!("kumospace: restart failed: {e:#}");
                    }
                });
            } else if id == items.mirror.id() {
                if let Some(relay) = &snapshot.relay {
                    relay.set_enabled(!relay.mirroring());
                }
            } else if id == items.login.id() {
                if let Some(auto) = autostart() {
                    let enabled = auto.is_enabled().unwrap_or(false);
                    let result = if enabled { auto.disable() } else { auto.enable() };
                    if let Err(e) = result {
                        eprintln!("autostart: {e}");
                    }
                    items.login.set_checked(auto.is_enabled().unwrap_or(false));
                }
            } else if id == items.log.id() {
                let _ = open::that(log_path());
            } else if id == items.quit.id() {
                let stop = stop_tx.clone();
                let relay = snapshot.relay.clone();
                runtime.spawn(async move {
                    if let Some(desktop) = relay
                        .filter(|r| r.phone_in_call())
                        .and_then(|r| r.desktop.clone())
                    {
                        let _ = tokio::time::timeout(Duration::from_secs(2), desktop.reclaim()).await;
                    }
                    let _ = stop.send(true);
                });
                quitting = Some(Instant::now());
            }
        }
    });
}
