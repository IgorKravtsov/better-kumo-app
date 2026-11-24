use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, bail};
use minisign_verify::{PublicKey, Signature};

const REPO: &str = "https://github.com/IgorKravtsov/better-kumo-app";
/// Signatures sit in the service prerelease, next to the meetings module, not on the release page.
const SIGNATURES: &str =
    "https://github.com/IgorKravtsov/better-kumo-app/releases/download/release-files";
const PUBLIC_KEY: &str = include_str!("../../../../packaging/release-minisign.pub");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const STAGING: &str = ".better-kumo-update-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    MacApp { bundle: PathBuf, homebrew: bool },
    Folder { dir: PathBuf },
    Apt,
    Source,
}

impl Install {
    pub fn current() -> Self {
        match std::env::current_exe().and_then(|p| p.canonicalize()) {
            Ok(exe) => Self::detect(&exe, Path::new("/")),
            Err(_) => Self::Source,
        }
    }

    fn detect(exe: &Path, root: &Path) -> Self {
        let names: Vec<_> = exe
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let profile = |name: &String| name == "release" || name == "debug";
        let built = names.windows(2).any(|w| w[0] == "target" && profile(&w[1]))
            || names.windows(3).any(|w| w[0] == "target" && profile(&w[2]));
        if built {
            return Self::Source;
        }
        if let Some(bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
        {
            let cask = [
                "opt/homebrew/Caskroom/better-kumo",
                "usr/local/Caskroom/better-kumo",
            ]
            .iter()
            .any(|cask| root.join(cask).is_dir());
            let homebrew = cask && bundle == root.join("Applications/Better Kumo.app");
            return Self::MacApp {
                bundle: bundle.to_path_buf(),
                homebrew,
            };
        }
        if cfg!(target_os = "linux") && exe == root.join("usr/bin/better-kumo") {
            return Self::Apt;
        }
        match exe.parent() {
            Some(dir) => Self::Folder { dir: dir.into() },
            None => Self::Source,
        }
    }

    fn asset(&self, version: &str) -> Option<String> {
        let arch = std::env::consts::ARCH;
        match self {
            Self::MacApp { .. } => Some(format!("Better-Kumo-macOS-{version}.zip")),
            Self::Folder { .. } if cfg!(windows) && arch == "x86_64" => {
                Some(format!("Better-Kumo-Windows-x64-{version}.zip"))
            }
            Self::Folder { .. } if cfg!(target_os = "linux") && arch == "x86_64" => {
                Some(format!("Better-Kumo-Linux-x64-{version}.tar.gz"))
            }
            Self::Apt if arch == "x86_64" => Some(format!("better-kumo_{version}_amd64.deb")),
            _ => None,
        }
    }

    /// The app swaps its own files, unlike APT, whose password prompt must not appear unasked.
    pub fn installs_by_itself(&self) -> bool {
        matches!(self, Self::MacApp { .. } | Self::Folder { .. }) && self.asset(VERSION).is_some()
    }

    /// No release file fits this computer, so the menu offers the download page instead.
    pub fn downloads_only(&self, version: &str) -> bool {
        self.asset(version).is_none()
    }

    pub fn terminal_command(&self) -> Option<&'static str> {
        match self {
            // The tap is refreshed only once a day on its own, too late right after a release.
            Self::MacApp { homebrew: true, .. } => {
                Some("brew update && brew upgrade --cask better-kumo")
            }
            Self::Apt => Some("sudo apt update && sudo apt install --only-upgrade better-kumo"),
            _ => None,
        }
    }

    /// The executable a package manager replaces, to tell whether it actually updated anything.
    fn installed_executable(&self) -> Option<PathBuf> {
        match self {
            Self::MacApp { bundle, .. } => Some(bundle.join("Contents/MacOS/better-kumo")),
            Self::Apt => Some("/usr/bin/better-kumo".into()),
            Self::Folder { dir } => Some(dir.join(folder_executable())),
            Self::Source => None,
        }
    }

    fn relaunch(&self) -> Option<Relaunch> {
        let pid = std::process::id().to_string();
        Some(match self {
            Self::MacApp { bundle, .. } => Relaunch {
                program: "open".into(),
                args: vec![
                    "-n".into(),
                    bundle.to_string_lossy().into(),
                    "--args".into(),
                    "--after-update".into(),
                    pid,
                ],
            },
            Self::Folder { .. } | Self::Apt => Relaunch {
                program: self.installed_executable()?,
                args: vec!["--after-update".into(), pid],
            },
            Self::Source => return None,
        })
    }

    fn staging_parent(&self) -> Option<PathBuf> {
        match self {
            // Next to the app, so swapping it in is a rename on one filesystem.
            Self::MacApp { bundle, .. } => bundle.parent().map(Path::to_path_buf),
            Self::Folder { dir } => Some(dir.clone()),
            Self::Apt => Some(std::env::temp_dir()),
            Self::Source => None,
        }
    }
}

/// The name the release archive gives it, whatever this copy was renamed to.
fn folder_executable() -> &'static str {
    if cfg!(windows) {
        "Better Kumo.exe"
    } else {
        "better-kumo"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relaunch {
    program: PathBuf,
    args: Vec<String>,
}

impl Relaunch {
    pub fn spawn(&self) -> std::io::Result<()> {
        Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
    }

    fn shell(&self) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .map(|word| shell_quote(&word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum UpdateState {
    #[default]
    Unknown,
    Checking,
    UpToDate,
    Available(String),
    Downloading {
        version: String,
        percent: u8,
    },
    Installing(String),
    Installed(Relaunch),
    /// Ends when the script removes `dir`, its last step, or when the user gives up on it.
    InTerminal {
        version: String,
        dir: PathBuf,
        since: std::time::Instant,
    },
    Failed {
        version: Option<String>,
        error: String,
    },
}

impl UpdateState {
    pub fn offered(&self) -> Option<&str> {
        match self {
            Self::Available(v) => Some(v),
            Self::Failed {
                version: Some(v), ..
            } => Some(v),
            _ => None,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. }
                | Self::Installing(_)
                | Self::Installed(_)
                | Self::InTerminal { .. }
        )
    }
}

/// The old app could not be put back, so its copy in the staging folder must survive.
#[derive(Debug)]
struct OldAppKept;

/// Marks a staging folder holding the only old copy, so `clean_up` leaves it alone.
const KEPT_MARKER: &str = "KEPT";

impl std::fmt::Display for OldAppKept {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "old version kept, see the log")
    }
}

impl std::error::Error for OldAppKept {}

#[derive(Clone)]
pub struct Updater {
    pub install: Install,
    state: Arc<Mutex<UpdateState>>,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            install: Install::current(),
            state: Arc::default(),
        }
    }

    pub fn state(&self) -> UpdateState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn set(&self, state: UpdateState) {
        if let Ok(mut s) = self.state.lock() {
            *s = state;
        }
    }

    /// One lock, so two clicks cannot both win.
    fn claim(&self, next: UpdateState) -> Option<UpdateState> {
        let mut s = self.state.lock().ok()?;
        if s.busy() || (*s == UpdateState::Checking && next == UpdateState::Checking) {
            return None;
        }
        Some(std::mem::replace(&mut *s, next))
    }

    pub fn fail(&self, error: String) {
        eprintln!("update: {error}");
        self.set(UpdateState::Failed {
            version: None,
            error,
        });
    }

    pub async fn run(self) {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            self.check(false).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    }

    /// A failed background check stays silent; a failed click says so.
    pub async fn check(&self, clicked: bool) {
        let Some(before) = self.claim(UpdateState::Checking) else {
            return;
        };
        let state = match latest_version().await {
            Ok(latest) if is_newer(&latest, VERSION) => {
                eprintln!("update: {latest} is available");
                UpdateState::Available(latest)
            }
            Ok(_) => UpdateState::UpToDate,
            Err(e) => {
                eprintln!("update: cannot check: {e:#}");
                if clicked {
                    UpdateState::Failed {
                        version: before.offered().map(str::to_string),
                        error: "cannot reach GitHub".into(),
                    }
                } else {
                    before
                }
            }
        };
        if let Ok(mut s) = self.state.lock()
            && *s == UpdateState::Checking
        {
            *s = state;
        }
    }

    pub async fn install(&self, version: String) {
        let downloading = UpdateState::Downloading {
            version: version.clone(),
            percent: 0,
        };
        if self.claim(downloading).is_none() {
            return;
        }
        match self.download_and_install(&version).await {
            Ok(relaunch) => {
                eprintln!("update: {version} installed, restarting");
                self.set(UpdateState::Installed(relaunch));
            }
            Err(e) => {
                eprintln!("update: {version} failed: {e:#}");
                self.set(UpdateState::Failed {
                    version: Some(version),
                    error: format!("{e:#}"),
                });
            }
        }
    }

    async fn download_and_install(&self, version: &str) -> anyhow::Result<Relaunch> {
        let install = self.install.clone();
        let asset = install
            .asset(version)
            .context("there is no update for this computer")?;
        if let Install::MacApp { bundle, .. } = &install
            && bundle.to_string_lossy().contains("/AppTranslocation/")
        {
            bail!("move Better Kumo to Applications first");
        }
        let relaunch = install.relaunch().context("cannot find this app")?;
        let parent = install.staging_parent().context("cannot find this app")?;
        // Random and private (0700), so nobody else can swap the file between the check and apt-get.
        let staging = tempfile::Builder::new()
            .prefix(STAGING)
            .tempdir_in(&parent)
            .with_context(|| format!("cannot write to {}", parent.display()))?;

        let url = format!("{REPO}/releases/download/v{version}/{asset}");
        let signature = fetch_text(&format!("{SIGNATURES}/{asset}.minisig")).await?;
        let file = staging.path().join(&asset);
        kumo_meetings::model::fetch(&url, &file, |percent| {
            self.set(UpdateState::Downloading {
                version: version.into(),
                percent,
            })
        })
        .await?;
        let bytes = tokio::fs::read(&file).await?;
        verify(&bytes, &signature, &asset)?;
        drop(bytes);

        self.set(UpdateState::Installing(version.into()));
        let staging = staging.keep();
        let result = tokio::task::spawn_blocking(move || put_in_place(&install, &file)).await?;
        let kept = result
            .as_ref()
            .err()
            .is_some_and(|e| e.downcast_ref::<OldAppKept>().is_some());
        if kept {
            eprintln!(
                "update: the previous version is kept in {}",
                staging.display()
            );
            let _ = std::fs::write(staging.join(KEPT_MARKER), "");
        } else {
            // Windows cannot delete the running old .exe, so it stays until a later clean_up.
            let _ = std::fs::remove_dir_all(&staging);
        }
        result.map(|()| relaunch)
    }

    pub fn leave_terminal(&self) {
        if let Ok(mut s) = self.state.lock()
            && let UpdateState::InTerminal { version, .. } = &*s
        {
            *s = UpdateState::Available(version.clone());
        }
    }

    /// Called on every tick: the script deletes its directory as its last step, unless the user
    /// closed the window first, which the hour also covers.
    pub fn notice_terminal_done(&self) {
        let done = matches!(
            self.state(),
            UpdateState::InTerminal { dir, since, .. }
                if !dir.exists() || since.elapsed() > Duration::from_secs(60 * 60)
        );
        if done {
            self.leave_terminal();
        }
    }

    pub fn update_in_terminal(&self, version: String) {
        let dir = match tempfile::Builder::new().prefix(STAGING).tempdir() {
            Ok(dir) => dir,
            Err(e) => return self.fail(format!("cannot open a terminal: {e}")),
        };
        let claimed = UpdateState::InTerminal {
            version: version.clone(),
            dir: dir.path().to_path_buf(),
            since: std::time::Instant::now(),
        };
        if self.claim(claimed).is_none() {
            return;
        }
        if let Err(e) = self.open_terminal(dir) {
            eprintln!("update: {e:#}");
            self.set(UpdateState::Failed {
                version: Some(version),
                error: format!("{e:#}"),
            });
        }
    }

    /// `dir` must be private: a predictable path could be swapped before the terminal runs it.
    fn open_terminal(&self, dir: tempfile::TempDir) -> anyhow::Result<()> {
        let command = self
            .install
            .terminal_command()
            .context("this install has no package manager")?;
        let relaunch = self.install.relaunch().context("cannot find this app")?;
        let executable = self
            .install
            .installed_executable()
            .context("cannot find this app")?;
        let script = dir.path().join("update.command");
        let pid = std::process::id();
        // Read now, while this process surely owns the PID, and in the C locale the script also uses.
        let output = Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()?;
        let started = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if started.is_empty() {
            bail!("cannot read this app's start time");
        }
        let text = terminal_script(&Script {
            command,
            pid,
            started: &started,
            executable: &executable,
            relaunch: &relaunch,
            dir: dir.path(),
        });
        std::fs::write(&script, text)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
        }
        let opened = if cfg!(target_os = "macos") {
            match Command::new("open")
                .args(["-a", "Terminal"])
                .arg(&script)
                .spawn()
            {
                Ok(mut child) => {
                    std::thread::spawn(move || child.wait());
                    true
                }
                Err(_) => false,
            }
        } else {
            let terminals: [(&str, &[&str]); 5] = [
                ("x-terminal-emulator", &["-e"]),
                ("gnome-terminal", &["--"]),
                ("konsole", &["-e"]),
                ("xfce4-terminal", &["-x"]),
                ("xterm", &["-e"]),
            ];
            terminals.iter().any(|(terminal, flag)| {
                match Command::new(terminal).args(*flag).arg(&script).spawn() {
                    Ok(mut child) => {
                        std::thread::spawn(move || child.wait());
                        true
                    }
                    Err(_) => false,
                }
            })
        };
        if !opened {
            bail!("no terminal app found, run: {command}");
        }
        // The script removes the directory once it runs.
        let _ = dir.keep();
        Ok(())
    }
}

/// Only day-old staging counts as abandoned: a younger one may belong to another copy updating now.
pub fn clean_up() {
    const ABANDONED: Duration = Duration::from_secs(24 * 60 * 60);
    let install = Install::current();
    if !matches!(install, Install::MacApp { .. } | Install::Folder { .. }) {
        return;
    }
    let Some(entries) = install
        .staging_parent()
        .and_then(|p| std::fs::read_dir(p).ok())
    else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if !entry.file_name().to_string_lossy().starts_with(STAGING)
            || entry.path().join(KEPT_MARKER).exists()
        {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > ABANDONED);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Started by the app it replaces: wait for that one to quit, so they do not share the relay port.
pub fn wait_for_previous() {
    let mut args = std::env::args()
        .skip_while(|a| a != "--after-update")
        .skip(1);
    let Some(pid) = args.next().and_then(|p| p.parse::<usize>().ok()) else {
        return;
    };
    let pid = sysinfo::Pid::from(pid);
    let Ok(mut system) = sysinfo::System::new() else {
        return;
    };
    for _ in 0..100 {
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        if system.process(pid).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("update: the previous Better Kumo is still running");
}

async fn latest_version() -> anyhow::Result<String> {
    let response = reqwest::Client::builder()
        .user_agent(concat!("better-kumo/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()?
        .get(format!("{REPO}/releases/latest"))
        .send()
        .await?;
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|l| l.to_str().ok())
        .with_context(|| format!("GitHub answered {}", response.status()))?;
    version_in_location(location).context("GitHub has no release yet")
}

fn version_in_location(location: &str) -> Option<String> {
    let tag = location
        .trim_end_matches('/')
        .rsplit_once("/releases/tag/")?
        .1;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(version).map(|_| version.to_string())
}

fn parse_version(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

async fn fetch_text(url: &str) -> anyhow::Result<String> {
    Ok(kumo_meetings::model::client()?
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("cannot download {url}"))?
        .text()
        .await?)
}

/// Binding the signed file name prevents substituting another signed version or platform.
fn verify(bytes: &[u8], signature: &str, asset: &str) -> anyhow::Result<()> {
    verify_with(PUBLIC_KEY, bytes, signature, asset)
}

fn verify_with(key: &str, bytes: &[u8], signature: &str, asset: &str) -> anyhow::Result<()> {
    let key = PublicKey::decode(key).context("the release key in this build is damaged")?;
    let signature = Signature::decode(signature).context("the update signature is damaged")?;
    key.verify(bytes, &signature, false)
        .context("the update is not signed by the Better Kumo release key")?;
    let named = signature
        .trusted_comment()
        .split('\t')
        .any(|field| field == format!("file:{asset}"));
    if !named {
        bail!("the update signature is for another file");
    }
    Ok(())
}

fn put_in_place(install: &Install, file: &Path) -> anyhow::Result<()> {
    let staging = file.parent().context("no staging folder")?;
    let unpacked = staging.join("new");
    match install {
        Install::MacApp { bundle, .. } => {
            run(Command::new("ditto")
                .arg("-x")
                .arg("-k")
                .arg(file)
                .arg(&unpacked))?;
            let new = unpacked.join("Better Kumo.app");
            if !new.join("Contents/MacOS/better-kumo").is_file() {
                bail!("the update has no Better Kumo.app");
            }
            let old = staging.join("old.app");
            // After an earlier failed swap the bundle may be gone; installing the new one still helps.
            if !bundle.exists() {
                return std::fs::rename(&new, bundle)
                    .context("cannot put the new Better Kumo.app in place");
            }
            if let Err(e) = std::fs::rename(bundle, &old) {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    bail!(
                        "macOS did not let Better Kumo replace itself, allow it in System Settings → Privacy & Security → App Management"
                    );
                }
                return Err(e).with_context(|| format!("cannot replace {}", bundle.display()));
            }
            if let Err(e) = std::fs::rename(&new, bundle) {
                if std::fs::rename(&old, bundle).is_err() {
                    return Err(OldAppKept.into());
                }
                return Err(e).context("cannot put the new Better Kumo.app in place");
            }
            Ok(())
        }
        Install::Folder { dir } => {
            std::fs::create_dir_all(&unpacked)?;
            let mut tar = Command::new("tar");
            tar.arg(if cfg!(windows) { "-xf" } else { "-xzf" })
                .arg(file)
                .arg("-C")
                .arg(&unpacked);
            run(&mut tar)?;
            let mut folders = std::fs::read_dir(&unpacked)?
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir());
            let new = folders.next().context("the update is empty")?.path();
            replace_files(&new, dir, &staging.join("old"))
        }
        Install::Apt => {
            // apt-get, not dpkg -i, so a new dependency is installed instead of breaking the package.
            let output = Command::new("pkexec")
                .args(["apt-get", "install", "-y"])
                .arg(file)
                .output()
                .context("cannot ask for the password (pkexec), update in the terminal instead")?;
            match output.status.code() {
                Some(0) => Ok(()),
                Some(126 | 127) => bail!("the password prompt was closed"),
                _ => bail!(
                    "apt-get could not install the update: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            }
        }
        Install::Source => bail!("a build from source updates with git pull"),
    }
}

/// Windows permits renaming a running executable but prevents overwriting it.
fn replace_files(new: &Path, dir: &Path, old: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(old)?;
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> anyhow::Result<()> {
        for entry in std::fs::read_dir(new)? {
            let entry = entry?;
            let target = dir.join(entry.file_name());
            let backup = old.join(entry.file_name());
            let moved_old = if target.exists() {
                std::fs::rename(&target, &backup)
                    .with_context(|| format!("cannot replace {}", target.display()))?;
                Some(backup)
            } else {
                None
            };
            done.push((target.clone(), moved_old));
            std::fs::rename(entry.path(), &target)
                .with_context(|| format!("cannot write {}", target.display()))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let mut restored = true;
        for (target, backup) in done.into_iter().rev() {
            if let Some(backup) = backup {
                let _ = std::fs::remove_file(&target);
                restored &= std::fs::rename(backup, &target).is_ok();
            }
        }
        if !restored {
            return Err(OldAppKept.into());
        }
    }
    result
}

fn run(command: &mut Command) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .with_context(|| format!("cannot start {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

struct Script<'a> {
    command: &'a str,
    pid: u32,
    started: &'a str,
    executable: &'a Path,
    relaunch: &'a Relaunch,
    dir: &'a Path,
}

/// The package manager may exit 0 without a newer version (repository not published yet, a `.deb`
/// installed by hand), so the app restarts as new only if its executable changed. Homebrew may also
/// have quit it already, so it is started again whenever it no longer runs.
fn terminal_script(s: &Script) -> String {
    format!(
        "#!/bin/sh\n\
         started={started}\n\
         alive() {{ [ \"$(LC_ALL=C ps -p {pid} -o lstart= 2>/dev/null | xargs)\" = \"$started\" ]; }}\n\
         before=\"$(cksum {executable} 2>/dev/null)\"\n\
         echo 'Updating Better Kumo:'\n\
         echo '  {command}'\n\
         echo\n\
         if {{ {command}; }}; then ok=1; else ok=; fi\n\
         echo\n\
         if [ \"$(cksum {executable} 2>/dev/null)\" != \"$before\" ]; then\n\
         \x20 if alive; then kill {pid}; sleep 1; fi\n\
         \x20 nohup {relaunch} </dev/null >/dev/null 2>&1 &\n\
         \x20 echo 'Better Kumo is updated and starting again.'\n\
         else\n\
         \x20 if [ -n \"$ok\" ]; then\n\
         \x20   echo 'There is no newer Better Kumo here yet. Try again later, or use Update to … in its menu.'\n\
         \x20 else\n\
         \x20   echo 'The update did not finish.'\n\
         \x20 fi\n\
         \x20 if ! alive; then\n\
         \x20   nohup {relaunch} </dev/null >/dev/null 2>&1 &\n\
         \x20   echo 'Better Kumo is starting again.'\n\
         \x20 fi\n\
         fi\n\
         echo\n\
         rm -rf {dir}\n\
         printf 'Press Enter to close this window. '\n\
         read -r _\n",
        command = s.command,
        pid = s.pid,
        started = shell_quote(s.started),
        executable = shell_quote(&s.executable.to_string_lossy()),
        relaunch = s.relaunch.shell(),
        dir = shell_quote(&s.dir.to_string_lossy()),
    )
}

fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:".contains(c))
    {
        word.into()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_github_redirects_to() {
        let location = "https://github.com/IgorKravtsov/better-kumo-app/releases/tag/v0.2.6";
        assert_eq!(version_in_location(location).as_deref(), Some("0.2.6"));
        assert_eq!(
            version_in_location("https://github.com/IgorKravtsov/better-kumo-app/releases"),
            None
        );
        assert_eq!(version_in_location(".../releases/tag/nightly"), None);
    }

    #[test]
    fn compares_versions_by_number() {
        assert!(is_newer("0.2.10", "0.2.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.5", "0.2.5"));
        assert!(!is_newer("0.2.4", "0.2.5"));
        assert!(!is_newer("garbage", "0.2.5"));
    }

    #[test]
    fn tells_how_the_app_was_installed() {
        let root = tempfile::tempdir().unwrap();
        let exe = root
            .path()
            .join("Applications/Better Kumo.app/Contents/MacOS/better-kumo");
        let bundle = root.path().join("Applications/Better Kumo.app");
        assert_eq!(
            Install::detect(&exe, root.path()),
            Install::MacApp {
                bundle: bundle.clone(),
                homebrew: false
            }
        );
        std::fs::create_dir_all(root.path().join("opt/homebrew/Caskroom/better-kumo")).unwrap();
        assert_eq!(
            Install::detect(&exe, root.path()),
            Install::MacApp {
                bundle,
                homebrew: true
            }
        );
        let copy = root
            .path()
            .join("Downloads/Better Kumo.app/Contents/MacOS/better-kumo");
        assert_eq!(
            Install::detect(&copy, root.path()),
            Install::MacApp {
                bundle: root.path().join("Downloads/Better Kumo.app"),
                homebrew: false
            }
        );
        let built = root.path().join("src/core/target/release/better-kumo");
        assert_eq!(Install::detect(&built, root.path()), Install::Source);
        let unzipped = root.path().join("Tools/Better Kumo/Better Kumo.exe");
        assert_eq!(
            Install::detect(&unzipped, root.path()),
            Install::Folder {
                dir: root.path().join("Tools/Better Kumo")
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_debian_package_lives_in_usr_bin() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("usr/bin/better-kumo");
        assert_eq!(Install::detect(&exe, root.path()), Install::Apt);
    }

    #[test]
    fn package_managers_get_their_own_command() {
        assert_eq!(
            Install::Apt.terminal_command(),
            Some("sudo apt update && sudo apt install --only-upgrade better-kumo")
        );
        let brew = Install::MacApp {
            bundle: "/Applications/Better Kumo.app".into(),
            homebrew: true,
        };
        assert_eq!(
            brew.terminal_command(),
            Some("brew update && brew upgrade --cask better-kumo")
        );
        let folder = Install::Folder { dir: "/opt".into() };
        assert_eq!(folder.terminal_command(), None);
    }

    #[test]
    fn only_one_install_can_start() {
        let updater = Updater {
            install: Install::Source,
            state: Arc::new(Mutex::new(UpdateState::Available("9.9.9".into()))),
        };
        let downloading = UpdateState::Downloading {
            version: "9.9.9".into(),
            percent: 0,
        };
        assert!(updater.claim(downloading.clone()).is_some());
        assert!(updater.claim(downloading).is_none());
        assert!(updater.claim(UpdateState::Checking).is_none());
    }

    #[test]
    fn a_terminal_update_blocks_the_in_app_one_until_it_ends() {
        let updater = Updater {
            install: Install::Source,
            state: Arc::new(Mutex::new(UpdateState::Available("9.9.9".into()))),
        };
        updater.update_in_terminal("9.9.9".into());
        assert!(matches!(
            updater.state(),
            UpdateState::Failed { version: Some(v), .. } if v == "9.9.9"
        ));

        let dir = tempfile::tempdir().unwrap();
        updater.set(UpdateState::InTerminal {
            version: "9.9.9".into(),
            dir: dir.path().to_path_buf(),
            since: std::time::Instant::now(),
        });
        let downloading = UpdateState::Downloading {
            version: "9.9.9".into(),
            percent: 0,
        };
        assert!(updater.claim(downloading.clone()).is_none());
        updater.notice_terminal_done();
        assert!(updater.claim(downloading.clone()).is_none());
        drop(dir);
        updater.notice_terminal_done();
        assert_eq!(updater.state(), UpdateState::Available("9.9.9".into()));
        assert!(updater.claim(downloading).is_some());
    }

    #[test]
    fn the_shipped_release_key_is_valid() {
        PublicKey::decode(PUBLIC_KEY).unwrap();
    }

    // Made with: minisign -G -W, then minisign -S -m Better-Kumo-macOS-9.9.9.zip on b"hello".
    const TEST_KEY: &str = "untrusted comment: minisign public key 4B4CCD9D5A105E88\nRWSIXhBanc1MSz8ogsvByfpLh3GmdE/9rkn0ivE9Fl6u0sLq/tNh2EIU\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRUSIXhBanc1MS6cbqVTzLBR/X2So/OL+AOq/veIoNZKHCAOgwuuXccnutrmo8todz9x2Hn0OKHdSMREZEKd4EZ2zq+hTsudBkgA=\ntrusted comment: timestamp:1795518136\tfile:Better-Kumo-macOS-9.9.9.zip\thashed\ngbaAyfS4Hnzxb4Tki+7XCXsCsqlI61dnE8fWRl8HQNqWSeBn4upt37ZlTJZwIFGv51BaKVRoUmaH1L6TUn/vCQ==\n";

    #[test]
    fn accepts_only_a_signed_file_with_its_own_name() {
        let name = "Better-Kumo-macOS-9.9.9.zip";
        verify_with(TEST_KEY, b"hello", TEST_SIGNATURE, name).unwrap();
        assert!(verify_with(TEST_KEY, b"hellO", TEST_SIGNATURE, name).is_err());
        assert!(
            verify_with(
                TEST_KEY,
                b"hello",
                TEST_SIGNATURE,
                "Better-Kumo-macOS-1.0.0.zip"
            )
            .is_err()
        );
        assert!(verify_with(PUBLIC_KEY, b"hello", TEST_SIGNATURE, name).is_err());
    }

    #[test]
    fn replaces_the_files_and_keeps_the_old_ones_aside() {
        let root = tempfile::tempdir().unwrap();
        let (new, dir, old) = (
            root.path().join("new"),
            root.path().join("app"),
            root.path().join("old"),
        );
        std::fs::create_dir_all(&new).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(new.join("better-kumo"), "2").unwrap();
        std::fs::write(new.join("kumo-mcp"), "2").unwrap();
        std::fs::write(dir.join("better-kumo"), "1").unwrap();
        std::fs::write(dir.join("notes.txt"), "mine").unwrap();
        replace_files(&new, &dir, &old).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("better-kumo")).unwrap(),
            "2"
        );
        assert_eq!(std::fs::read_to_string(dir.join("kumo-mcp")).unwrap(), "2");
        assert_eq!(
            std::fs::read_to_string(dir.join("notes.txt")).unwrap(),
            "mine"
        );
        assert_eq!(
            std::fs::read_to_string(old.join("better-kumo")).unwrap(),
            "1"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn swaps_the_mac_app_bundle() {
        let root = tempfile::tempdir().unwrap();
        let fake_app = |dir: &Path, version: &str| {
            let exe = dir.join("Better Kumo.app/Contents/MacOS/better-kumo");
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(exe, version).unwrap();
        };
        let built = root.path().join("built");
        fake_app(&built, "2");
        let apps = root.path().join("Applications");
        fake_app(&apps, "1");
        let bundle = apps.join("Better Kumo.app");
        let install = Install::MacApp {
            bundle: bundle.clone(),
            homebrew: false,
        };
        let staging = tempfile::Builder::new()
            .prefix(STAGING)
            .tempdir_in(install.staging_parent().unwrap())
            .unwrap()
            .keep();
        let zip = staging.join("Better-Kumo-macOS-2.zip");
        run(Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(built.join("Better Kumo.app"))
            .arg(&zip))
        .unwrap();
        put_in_place(&install, &zip).unwrap();
        let exe = bundle.join("Contents/MacOS/better-kumo");
        assert_eq!(std::fs::read_to_string(exe).unwrap(), "2");
    }

    #[cfg(unix)]
    #[test]
    fn unpacks_the_linux_tarball_over_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let built = root.path().join("built/better-kumo-2");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("better-kumo"), "2").unwrap();
        std::fs::write(built.join("kumo-mcp"), "2").unwrap();
        let dir = root.path().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("better-kumo"), "1").unwrap();
        let install = Install::Folder { dir: dir.clone() };
        let staging = tempfile::Builder::new()
            .prefix(STAGING)
            .tempdir_in(install.staging_parent().unwrap())
            .unwrap()
            .keep();
        let tarball = staging.join("Better-Kumo-Linux-x64-2.tar.gz");
        run(Command::new("tar")
            .arg("-czf")
            .arg(&tarball)
            .arg("-C")
            .arg(root.path().join("built"))
            .arg("better-kumo-2"))
        .unwrap();
        put_in_place(&install, &tarball).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("better-kumo")).unwrap(),
            "2"
        );
        assert_eq!(std::fs::read_to_string(dir.join("kumo-mcp")).unwrap(), "2");
    }

    #[cfg(unix)]
    #[test]
    fn the_terminal_script_restarts_only_when_needed() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("better kumo");
        let marker = root.path().join("relaunched");
        let relaunch = Relaunch {
            program: "touch".into(),
            args: vec![marker.to_string_lossy().into()],
        };
        let start_app = || {
            let app = Command::new("sleep").arg("30").spawn().unwrap();
            let output = Command::new("ps")
                .env("LC_ALL", "C")
                .args(["-p", &app.id().to_string(), "-o", "lstart="])
                .output()
                .unwrap();
            let started = String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            (app, started)
        };
        let run_script = |command: &str, app: &mut std::process::Child, started: &str| {
            std::fs::write(&executable, "1").unwrap();
            let _ = std::fs::remove_file(&marker);
            let dir = root.path().join("script");
            std::fs::create_dir_all(&dir).unwrap();
            let text = terminal_script(&Script {
                command,
                pid: app.id(),
                started,
                executable: &executable,
                relaunch: &relaunch,
                dir: &dir,
            });
            let output = Command::new("sh")
                .args(["-c", &text])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(!dir.exists(), "the script removes its directory");
            for _ in 0..40 {
                if marker.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let alive = app.try_wait().unwrap().is_none();
            (
                String::from_utf8_lossy(&output.stdout).into_owned(),
                marker.exists(),
                alive,
            )
        };
        let change = format!("echo 2 > {}", shell_quote(&executable.to_string_lossy()));

        let (mut app, started) = start_app();
        let (said, relaunched, alive) = run_script("true && true", &mut app, &started);
        assert!(
            said.contains("There is no newer Better Kumo here yet"),
            "{said}"
        );
        assert!(!relaunched && alive);

        let (said, relaunched, alive) =
            run_script(&format!("false && {change}"), &mut app, &started);
        assert!(said.contains("The update did not finish"), "{said}");
        assert!(!relaunched && alive);

        let (said, relaunched, alive) =
            run_script(&format!("true && {change}"), &mut app, &started);
        assert!(
            said.contains("Better Kumo is updated and starting again"),
            "{said}"
        );
        assert!(relaunched && !alive);

        let (said, relaunched, _) = run_script("true && true", &mut app, &started);
        assert!(said.contains("Better Kumo is starting again"), "{said}");
        assert!(relaunched);
    }
}
