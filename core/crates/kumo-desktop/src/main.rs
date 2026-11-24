#![cfg_attr(windows, windows_subsystem = "windows")]

mod icon;
mod kumospace;
mod launch_agents;
mod relay;
mod settings;
mod supervisor;
mod updater;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use auto_launch::{AutoLaunch, AutoLaunchBuilder, WindowsEnableMode};
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tokio::sync::watch;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

use kumo_core::desktop::{AgentAccess, AutoUpdate};
use kumo_relay::Relay;
use kumospace::{DesktopAppState, Kumospace};
use relay::ConfiguredRelay;
use supervisor::{Shared, Status, Supervisor};
use updater::{UpdateState, Updater};

const APP_NAME: &str = "Better Kumo";

struct Items {
    kumospace: MenuItem,
    relay: MenuItem,
    pair: MenuItem,
    open_kumospace: MenuItem,
    restart: MenuItem,
    keep_open: CheckMenuItem,
    agents: CheckMenuItem,
    meetings: MenuItem,
    record_now: MenuItem,
    auto_start: CheckMenuItem,
    open_meetings: MenuItem,
    settings: MenuItem,
    login: CheckMenuItem,
    log: MenuItem,
    update: MenuItem,
    update_in_terminal: MenuItem,
    auto_update: CheckMenuItem,
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
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        unsafe {
            libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
            libc::dup2(file.as_raw_fd(), libc::STDOUT_FILENO);
        }
        std::mem::forget(file);
    }
}

#[cfg(windows)]
fn redirect_logs() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let handle = file.as_raw_handle();
        unsafe {
            SetStdHandle(STD_ERROR_HANDLE, handle);
            SetStdHandle(STD_OUTPUT_HANDLE, handle);
        }
        std::mem::forget(file);
    }
}

#[cfg(not(any(unix, windows)))]
fn redirect_logs() {}

fn autostart() -> Option<AutoLaunch> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_string_lossy();
    // auto-launch writes the Windows Run value as a command line without quoting the path.
    let exe = if cfg!(windows) {
        format!("\"{exe}\"")
    } else {
        exe.into_owned()
    };
    AutoLaunchBuilder::new()
        .set_app_name("better-kumo")
        .set_app_path(&exe)
        // HKCU like the installer's entry; the default tries HKLM first when elevated.
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .build()
        .ok()
}

fn build_menu(keep_open: bool, updater: &Updater) -> (Menu, Items) {
    let items = Items {
        kumospace: MenuItem::new("Kumospace: starting…", false, None),
        relay: MenuItem::new("Phone control: starting…", false, None),
        pair: MenuItem::new("Pair a phone…", true, None),
        open_kumospace: MenuItem::new("Open Kumospace", true, None),
        restart: MenuItem::new("Restart Kumospace for phone control", false, None),
        keep_open: CheckMenuItem::new("Keep Kumospace open", true, keep_open, None),
        agents: CheckMenuItem::new(
            "AI agents can use my chat (MCP)",
            true,
            AgentAccess::load(&AgentAccess::default_path()).enabled,
            None,
        ),
        meetings: MenuItem::new("Meetings: starting…", false, None),
        record_now: MenuItem::new("Start recording now", false, None),
        auto_start: CheckMenuItem::new("Start recording automatically", false, false, None),
        open_meetings: MenuItem::new("Open meetings folder", false, None),
        settings: MenuItem::new("Settings…", true, None),
        login: CheckMenuItem::new(
            "Start at login",
            true,
            autostart()
                .and_then(|a| a.is_enabled().ok())
                .unwrap_or(false),
            None,
        ),
        log: MenuItem::new("Open log", true, None),
        update: MenuItem::new("Check for updates", true, None),
        update_in_terminal: MenuItem::new(
            // muda reads a single `&` as a mnemonic marker, which would show `&&` as `&`.
            format!(
                "Update in Terminal: {}",
                updater
                    .install
                    .terminal_command()
                    .unwrap_or_default()
                    .replace('&', "&&")
            ),
            true,
            None,
        ),
        auto_update: CheckMenuItem::new(
            "Install updates automatically",
            updater.install.installs_by_itself(),
            AutoUpdate::load(&AutoUpdate::default_path()).enabled,
            None,
        ),
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
        &items.keep_open,
        &PredefinedMenuItem::separator(),
        &items.meetings,
        &items.record_now,
        &items.auto_start,
        &items.open_meetings,
        &PredefinedMenuItem::separator(),
        &items.agents,
        &items.settings,
        &items.login,
        &items.log,
        &PredefinedMenuItem::separator(),
        &items.update,
        &items.auto_update,
        &PredefinedMenuItem::separator(),
        &items.quit,
        &PredefinedMenuItem::separator(),
        &MenuItem::new(
            format!("Version {}", env!("CARGO_PKG_VERSION")),
            false,
            None,
        ),
    ]);
    (menu, items)
}

fn refresh(items: &Items, status: &Status<Arc<Relay>>) {
    let kumospace = match status.desktop_app {
        DesktopAppState::NotInstalled => "Kumospace: not installed",
        DesktopAppState::NotRunning if items.keep_open.is_checked() => "Kumospace: starting…",
        DesktopAppState::NotRunning => "Kumospace: closed",
        DesktopAppState::RunningWithoutPort { .. } => "Kumospace: opened without phone control",
        DesktopAppState::Running => "Kumospace: running",
    };
    items.kumospace.set_text(kumospace);
    items.restart.set_enabled(matches!(
        status.desktop_app,
        DesktopAppState::RunningWithoutPort { .. }
    ));
    let relay = match (&status.relay, &status.problem) {
        (Some(relay), _) => {
            let relay = relay.status();
            let desktop = if relay.phone_in_call {
                "phone is in the call"
            } else if relay.desktop_connected {
                "desktop connected"
            } else {
                "waiting for desktop"
            };
            let phones = match relay.phones {
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
    let agents = AgentAccess::load(&AgentAccess::default_path()).enabled;
    if items.agents.is_checked() != agents {
        items.agents.set_checked(agents);
    }

    let meetings = status.relay.as_ref().and_then(|r| r.meetings());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    items.meetings.set_text(match &meetings {
        Some(m) => m.summary(now_ms),
        None => "Meetings: need the desktop app".into(),
    });
    let ready = meetings.as_ref().is_some_and(|m| m.module.is_ready());
    let recording = status.relay.as_ref().is_some_and(|r| r.meeting_recording());
    items.record_now.set_enabled(ready);
    items.record_now.set_text(if recording {
        "Stop recording"
    } else {
        "Start recording now"
    });
    items.auto_start.set_enabled(ready);
    items.auto_start.set_checked(
        status
            .relay
            .as_ref()
            .is_some_and(|r| r.meetings_auto_start()),
    );
    items.open_meetings.set_enabled(meetings.is_some());
}

fn refresh_update(
    menu: &Menu,
    items: &Items,
    updater: &Updater,
    state: &UpdateState,
    deferred: bool,
) {
    let (text, enabled) = match state {
        UpdateState::Installed(_) if deferred => (
            "Update ready · restarts after the meeting, or now".into(),
            true,
        ),
        UpdateState::Unknown => ("Check for updates".into(), true),
        UpdateState::Checking => ("Checking for updates…".into(), false),
        UpdateState::UpToDate => ("Up to date · check again".into(), true),
        UpdateState::Available(v) if updater.install.downloads_only(v) => {
            (format!("Version {v} is out · open downloads"), true)
        }
        UpdateState::Available(v) => (format!("Update to {v} and restart"), true),
        UpdateState::Downloading { version, percent } => {
            (format!("Downloading {version}… {percent}%"), false)
        }
        UpdateState::Installing(v) => (format!("Installing {v}…"), false),
        UpdateState::Installed(_) => ("Restarting…".into(), false),
        UpdateState::InTerminal { .. } => {
            ("Updating in Terminal… · click if it stopped".into(), true)
        }
        UpdateState::Failed { error, .. } => {
            let short: String = error.chars().take(50).collect();
            (format!("Update failed: {short} · try again"), true)
        }
    };
    items.update.set_text(text);
    items.update.set_enabled(enabled);

    let in_terminal = state.offered().is_some() && updater.install.terminal_command().is_some();
    let position = menu
        .items()
        .iter()
        .position(|item| item.id() == items.update_in_terminal.id());
    match (in_terminal, position) {
        (true, None) => {
            if let Some(update) = menu
                .items()
                .iter()
                .position(|i| i.id() == items.update.id())
            {
                let _ = menu.insert(&items.update_in_terminal, update + 1);
            }
        }
        (false, Some(_)) => {
            let _ = menu.remove(&items.update_in_terminal);
        }
        _ => {}
    }
}

fn main() {
    if std::env::args().any(|a| a == "--foreground") {
        eprintln!("{APP_NAME}: logging to the terminal");
    } else {
        redirect_logs();
    }
    eprintln!("{APP_NAME} {} starting", env!("CARGO_PKG_VERSION"));
    updater::wait_for_previous();
    launch_agents::restart_stale();
    updater::clean_up();

    let settings_path = settings::path();
    let keep_open = Arc::new(AtomicBool::new(settings::keep_kumospace_open(
        &settings_path,
    )));
    let status: Shared<Arc<Relay>> = Arc::new(Mutex::new(Status::default()));
    let (stop_tx, stop_rx) = watch::channel(false);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.spawn(
        Supervisor {
            status: status.clone(),
            shutdown: stop_rx,
            app: Kumospace,
            relays: ConfiguredRelay::new(),
            keep_open: keep_open.clone(),
        }
        .run(),
    );
    let updater = Updater::new();
    runtime.spawn(updater.clone().run());
    let terminated = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    {
        // The terminal update stops the app with SIGTERM; quit as from the menu, not abruptly.
        let terminated = terminated.clone();
        runtime.spawn(async move {
            use tokio::signal::unix::{SignalKind, signal};
            if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
                sigterm.recv().await;
                terminated.store(true, Ordering::Relaxed);
            }
        });
    }

    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::new().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let mut tray: Option<TrayIcon> = None;
    let mut items: Option<Items> = None;
    let mut tray_menu: Option<Menu> = None;
    let mut auto_installing = false;
    let mut auto_tried: Option<String> = None;
    let mut quitting: Option<Instant> = None;
    let menu_events = MenuEvent::receiver();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500));
        if let Event::NewEvents(StartCause::Init) = event {
            let (menu, built) = build_menu(keep_open.load(Ordering::Relaxed), &updater);
            tray_menu = Some(menu.clone());
            let builder = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(APP_NAME);
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_templated(icon::tray());
            #[cfg(not(target_os = "macos"))]
            let builder = builder.with_icon(icon::tray());
            tray = builder.build().ok();
            items = Some(built);
        }

        if quitting.is_none() && terminated.load(Ordering::Relaxed) {
            let _ = stop_tx.send(true);
            quitting = Some(Instant::now());
        }
        if let Some(started) = quitting {
            let stopped = status.lock().map(|s| s.relay.is_none()).unwrap_or(true);
            // Exiting between the two renames of the app swap would leave no app in place.
            let installing = updater.install.installs_by_itself()
                && matches!(updater.state(), UpdateState::Installing(_))
                && started.elapsed() < Duration::from_secs(120);
            if !installing && (stopped || started.elapsed() > Duration::from_secs(4)) {
                tray.take();
                *control_flow = ControlFlow::Exit;
            }
            return;
        }

        let Some(items) = items.as_ref() else { return };
        let snapshot = status.lock().map(|s| s.clone()).unwrap_or_default();
        refresh(items, &snapshot);
        // After a terminal attempt only a click installs: brew may still be swapping the bundle.
        if let UpdateState::InTerminal { version, .. } = updater.state() {
            updater.notice_terminal_done();
            if !matches!(updater.state(), UpdateState::InTerminal { .. }) {
                auto_tried = Some(version);
            }
        }
        let update = updater.state();
        let supported = updater.install.installs_by_itself();
        if let Some(relay) = &snapshot.relay {
            relay.set_auto_update_support(supported);
        }
        let auto = supported && AutoUpdate::load(&AutoUpdate::default_path()).enabled;
        if items.auto_update.is_checked() != auto {
            items.auto_update.set_checked(auto);
        }
        let busy = snapshot
            .relay
            .as_ref()
            .is_some_and(|relay| relay.busy_for_update());
        // One automatic try per version: a failure after the download would only repeat.
        if let UpdateState::Available(version) = &update
            && auto
            && !busy
            && auto_tried.as_deref() != Some(version.as_str())
        {
            auto_tried = Some(version.clone());
            auto_installing = true;
            let (updater, version) = (updater.clone(), version.clone());
            runtime.spawn(async move { updater.install(version).await });
        }
        let deferred = auto_installing && busy && matches!(update, UpdateState::Installed(_));
        if let Some(menu) = &tray_menu {
            refresh_update(menu, items, &updater, &update, deferred);
        }
        if !deferred && let UpdateState::Installed(relaunch) = &update {
            match relaunch.spawn() {
                Ok(()) => {
                    let _ = stop_tx.send(true);
                    quitting = Some(Instant::now());
                    return;
                }
                Err(e) => updater.fail(format!("cannot start the new version: {e}")),
            }
        }

        while let Ok(event) = menu_events.try_recv() {
            let id = event.id;
            if id == items.pair.id() {
                let _ = open::that(format!("http://127.0.0.1:{}/pair", snapshot.port));
            } else if id == items.open_kumospace.id() {
                if snapshot.desktop_app == DesktopAppState::NotRunning {
                    let _ = kumospace::launch();
                } else if cfg!(target_os = "macos") {
                    let _ = std::process::Command::new("open")
                        .args(["-a", "Kumospace"])
                        .spawn();
                }
            } else if id == items.restart.id() {
                std::thread::spawn(|| {
                    if let Err(e) = kumospace::restart_with_port() {
                        eprintln!("kumospace: restart failed: {e:#}");
                    }
                });
            } else if id == items.agents.id() {
                let path = AgentAccess::default_path();
                let enabled = !AgentAccess::load(&path).enabled;
                if let Err(e) = (AgentAccess { enabled }).save(&path) {
                    eprintln!("agents setting: {e}");
                }
                items.agents.set_checked(AgentAccess::load(&path).enabled);
            } else if id == items.record_now.id() {
                if let Some(relay) = &snapshot.relay {
                    if relay.meeting_recording() {
                        relay.stop_meeting_now();
                    } else if let Err(e) = relay.start_meeting_now() {
                        eprintln!("meetings: {e:#}");
                    }
                }
            } else if id == items.auto_start.id() {
                if let Some(relay) = &snapshot.relay {
                    let auto_start = !relay.meetings_auto_start();
                    if let Err(e) = relay.set_meetings_auto_start(auto_start) {
                        eprintln!("meetings: {e:#}");
                    }
                    items.auto_start.set_checked(relay.meetings_auto_start());
                }
            } else if id == items.settings.id() {
                let _ = open::that(format!("http://127.0.0.1:{}/settings", snapshot.port));
            } else if id == items.open_meetings.id() {
                if let Some(folder) = snapshot.relay.as_ref().and_then(|r| r.meetings_folder()) {
                    let _ = std::fs::create_dir_all(&folder);
                    let _ = open::that(folder);
                }
            } else if id == items.keep_open.id() {
                let keep = items.keep_open.is_checked();
                keep_open.store(keep, Ordering::Relaxed);
                if let Err(e) = settings::set_keep_kumospace_open(&settings_path, keep) {
                    eprintln!("settings: {e:#}");
                }
            } else if id == items.login.id() {
                if let Some(auto) = autostart() {
                    let enabled = auto.is_enabled().unwrap_or(false);
                    let result = if enabled {
                        auto.disable()
                    } else {
                        auto.enable()
                    };
                    if let Err(e) = result {
                        eprintln!("autostart: {e}");
                    }
                    items.login.set_checked(auto.is_enabled().unwrap_or(false));
                }
            } else if id == items.log.id() {
                let _ = open::that(log_path());
            } else if id == items.update.id() {
                match update.offered() {
                    _ if matches!(update, UpdateState::Installed(_)) => auto_installing = false,
                    _ if let UpdateState::InTerminal { version, .. } = &update => {
                        auto_tried = Some(version.clone());
                        updater.leave_terminal()
                    }
                    Some(version) if updater.install.downloads_only(version) => {
                        let _ = open::that(
                            "https://github.com/IgorKravtsov/better-kumo-app/releases/latest",
                        );
                    }
                    Some(version) => {
                        auto_installing = false;
                        let (updater, version) = (updater.clone(), version.to_string());
                        runtime.spawn(async move { updater.install(version).await });
                    }
                    None => {
                        let updater = updater.clone();
                        runtime.spawn(async move { updater.check(true).await });
                    }
                }
            } else if id == items.update_in_terminal.id() {
                if let Some(version) = update.offered() {
                    updater.update_in_terminal(version.into());
                }
            } else if id == items.auto_update.id() {
                let path = AutoUpdate::default_path();
                let enabled = !AutoUpdate::load(&path).enabled;
                if let Err(e) = (AutoUpdate { enabled }).save(&path) {
                    eprintln!("auto-update setting: {e}");
                }
                items
                    .auto_update
                    .set_checked(AutoUpdate::load(&path).enabled);
            } else if id == items.quit.id() {
                let _ = stop_tx.send(true);
                quitting = Some(Instant::now());
            }
        }
    });
}
