use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::supervisor::DesktopApp;

const FRESH_SECONDS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopAppState {
    NotInstalled,
    NotRunning,
    RunningWithoutPort { fresh: bool },
    Running,
}

pub struct Process {
    pid: sysinfo::Pid,
    has_port: bool,
    age: u64,
}

pub fn devtools_url() -> String {
    kumo_devtools::local_url(kumo_devtools::DEFAULT_PORT)
}

fn port_flag() -> String {
    format!("--remote-debugging-port={}", kumo_devtools::DEFAULT_PORT)
}

fn is_kumospace(name: &str, exe: Option<&Path>) -> bool {
    let name = name.to_ascii_lowercase();
    let exe = exe
        .map(|p| p.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        exe.ends_with("kumospace.app/contents/macos/kumospace")
    } else if cfg!(windows) {
        name == "kumospace.exe"
    } else {
        name.starts_with("kumospace") || exe.contains("kumospace")
    }
}

pub fn find() -> Option<Process> {
    let mut system = System::new().ok()?;
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    system
        .processes()
        .values()
        .filter(|p| is_kumospace(&p.name().to_string_lossy(), p.exe()))
        .filter(|p| {
            !p.cmd()
                .iter()
                .any(|a| a.to_string_lossy().starts_with("--type="))
        })
        .map(|p| Process {
            pid: p.pid(),
            has_port: p.cmd().iter().any(|a| a.to_string_lossy() == port_flag()),
            age: p.run_time(),
        })
        .min_by_key(|p| p.age.wrapping_neg())
}

pub fn state() -> DesktopAppState {
    match find() {
        Some(p) if p.has_port => DesktopAppState::Running,
        Some(p) => DesktopAppState::RunningWithoutPort {
            fresh: p.age < FRESH_SECONDS,
        },
        None if installed_path().is_some() => DesktopAppState::NotRunning,
        None => DesktopAppState::NotInstalled,
    }
}

pub fn installed_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("KUMOSPACE_PATH").map(PathBuf::from) {
        return path.exists().then_some(path);
    }
    let home = std::env::home_dir().unwrap_or_default();
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/Applications/Kumospace.app"),
            home.join("Applications/Kumospace.app"),
        ]
    } else if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default();
        let program_files = std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .unwrap_or_default();
        vec![
            local.join("Programs/kumospace/Kumospace.exe"),
            local.join("kumospace/Kumospace.exe"),
            program_files.join("Kumospace/Kumospace.exe"),
        ]
    } else {
        let mut list = vec![
            home.join(".local/bin/kumospace"),
            PathBuf::from("/opt/Kumospace/kumospace"),
            PathBuf::from("/usr/bin/kumospace"),
        ];
        for dir in [
            home.join("Applications"),
            home.join("Downloads"),
            home.clone(),
        ] {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                list.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_ascii_lowercase())
                        .unwrap_or_default();
                    name.starts_with("kumospace") && name.ends_with(".appimage")
                }));
            }
        }
        list
    };
    candidates.into_iter().find(|p| p.exists())
}

pub fn launch() -> anyhow::Result<()> {
    let app =
        installed_path().ok_or_else(|| anyhow::anyhow!("Kumospace desktop is not installed"))?;
    if cfg!(target_os = "macos") {
        Command::new("open")
            .arg("-a")
            .arg(&app)
            .arg("--args")
            .arg(port_flag())
            .spawn()?;
    } else {
        Command::new(&app).arg(port_flag()).spawn()?;
    }
    Ok(())
}

pub fn quit() {
    let Some(process) = find() else { return };
    if cfg!(target_os = "macos") {
        let _ = Command::new("osascript")
            .args(["-e", "quit app \"Kumospace\""])
            .status();
        for _ in 0..40 {
            if find().is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Ok(mut system) = System::new() {
        system.refresh_processes(ProcessesToUpdate::Some(&[process.pid]), true);
        if let Some(p) = system.process(process.pid) {
            p.kill();
        }
    }
    for _ in 0..20 {
        if find().is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn restart_with_port() -> anyhow::Result<()> {
    quit();
    std::thread::sleep(Duration::from_millis(800));
    launch()
}

pub struct Kumospace;

impl DesktopApp for Kumospace {
    async fn state(&self) -> DesktopAppState {
        tokio::task::spawn_blocking(state)
            .await
            .unwrap_or(DesktopAppState::NotRunning)
    }

    fn launch(&self) -> anyhow::Result<()> {
        launch()
    }

    async fn restart_with_port(&self) -> anyhow::Result<()> {
        tokio::task::spawn_blocking(restart_with_port).await?
    }

    async fn devtools_ready(&self) -> bool {
        kumo_devtools::desktop_app_page_open(&devtools_url()).await
    }

    fn devtools_url(&self) -> String {
        devtools_url()
    }
}
