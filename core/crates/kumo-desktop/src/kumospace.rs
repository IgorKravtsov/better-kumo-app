use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use sysinfo::{ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

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

#[cfg(target_os = "macos")]
fn is_kumospace(_name: &str, exe: Option<&Path>) -> bool {
    exe.is_some_and(|p| {
        p.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with("kumospace.app/contents/macos/kumospace")
    })
}

#[cfg(windows)]
fn is_kumospace(name: &str, _exe: Option<&Path>) -> bool {
    name.eq_ignore_ascii_case("kumospace.exe")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn is_kumospace(name: &str, exe: Option<&Path>) -> bool {
    name.to_ascii_lowercase().starts_with("kumospace")
        || exe.is_some_and(|p| {
            p.to_string_lossy()
                .to_ascii_lowercase()
                .contains("kumospace")
        })
}

/// The arguments of a command line. On Linux, Chromium (and so Electron) rewrites its process
/// title, and `/proc/<pid>/cmdline` of the Kumospace main process then holds one string with every
/// argument joined by spaces instead of one entry per argument. Only that flattened form is split
/// on whitespace, after the executable path, so a path with spaces or flag-like text is not read as
/// flags. Separate arguments are kept whole, so flag-like text inside a value is no flag.
fn cmd_words(cmd: &[OsString], exe: Option<&Path>) -> Vec<String> {
    match cmd {
        [title] => {
            let title = title.to_string_lossy();
            let rest = exe
                .and_then(|e| title.strip_prefix(e.to_string_lossy().as_ref()))
                .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                .unwrap_or(&title);
            rest.split_whitespace().map(str::to_owned).collect()
        }
        args => args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
    }
}

fn is_helper_process(cmd: &[OsString], exe: Option<&Path>) -> bool {
    cmd_words(cmd, exe).iter().any(|w| w.starts_with("--type="))
}

fn has_port_flag(cmd: &[OsString], exe: Option<&Path>) -> bool {
    cmd_words(cmd, exe).contains(&port_flag())
}

pub fn find() -> Option<Process> {
    let mut system = System::new().ok()?;
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        // Without tasks: on Linux, sysinfo otherwise lists every thread as a process of its own,
        // and a thread that just started looked like a Kumospace that just started.
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    system
        .processes()
        .values()
        .filter(|p| p.thread_kind().is_none())
        // A Kumospace that exited but was not reaped yet has no command line any more.
        .filter(|p| !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead))
        .filter(|p| is_kumospace(&p.name().to_string_lossy(), p.exe()))
        .filter(|p| !is_helper_process(p.cmd(), p.exe()))
        .map(|p| Process {
            pid: p.pid(),
            has_port: has_port_flag(p.cmd(), p.exe()),
            age: p.run_time(),
        })
        .max_by_key(|p| p.age)
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
    // Reap the child when it exits, so it doesn't stay behind as a zombie. The reaper thread is
    // started before the app, so a thread that can't be created fails the launch instead of
    // leaving a Kumospace without a reaper.
    let (send_child, receive_child) = std::sync::mpsc::channel::<std::process::Child>();
    std::thread::Builder::new()
        .name("kumospace-reaper".into())
        .spawn(move || {
            if let Ok(mut child) = receive_child.recv() {
                let _ = child.wait();
            }
        })?;
    let child = if cfg!(target_os = "macos") {
        Command::new("open")
            .arg("-a")
            .arg(&app)
            .arg("--args")
            .arg(port_flag())
            .spawn()?
    } else {
        Command::new(&app).arg(port_flag()).spawn()?
    };
    // The reaper only stops before receiving the child if it panicked, which it can't.
    let _ = send_child.send(child);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn finds_the_port_flag_in_separate_arguments() {
        assert!(has_port_flag(
            &cmd(&["/opt/Kumospace/kumospace", "--remote-debugging-port=9222",]),
            None
        ));
    }

    #[test]
    fn finds_the_port_flag_in_a_rewritten_process_title() {
        assert!(has_port_flag(
            &cmd(
                &["/tmp/.mount_KumospAbCdEf/kumospace --no-sandbox --remote-debugging-port=9222",]
            ),
            None
        ));
    }

    #[test]
    fn needs_the_exact_port_flag() {
        assert!(!has_port_flag(
            &cmd(&["/opt/Kumospace/kumospace --no-sandbox"]),
            None
        ));
        assert!(!has_port_flag(
            &cmd(&["/opt/Kumospace/kumospace", "--remote-debugging-port=92220",]),
            None
        ));
        assert!(!has_port_flag(&[], None));
    }

    #[test]
    fn recognises_helper_processes_in_both_forms() {
        assert!(is_helper_process(
            &cmd(&["/opt/Kumospace/kumospace", "--type=renderer",]),
            None
        ));
        assert!(is_helper_process(
            &cmd(&["/tmp/.mount_KumospAbCdEf/kumospace --type=gpu-process --no-sandbox",]),
            None
        ));
        assert!(!is_helper_process(
            &cmd(&["/opt/Kumospace/kumospace --remote-debugging-port=9222",]),
            None
        ));
    }

    #[test]
    fn keeps_separate_arguments_whole() {
        let args = cmd(&[
            "/opt/Kumospace/kumospace",
            "--user-data-dir=/home/me/Profile --type=renderer --remote-debugging-port=9222",
        ]);
        assert!(!is_helper_process(&args, None));
        assert!(!has_port_flag(&args, None));
    }

    #[test]
    fn ignores_flag_like_text_in_the_executable_path() {
        let exe = Path::new("/home/me/Kumospace --type=renderer build/kumospace");
        let title = cmd(&[
            "/home/me/Kumospace --type=renderer build/kumospace --remote-debugging-port=9222",
        ]);
        assert!(!is_helper_process(&title, Some(exe)));
        assert!(has_port_flag(&title, Some(exe)));
        assert!(!is_helper_process(
            &cmd(&[exe.to_str().unwrap()]),
            Some(exe)
        ));
    }
}
