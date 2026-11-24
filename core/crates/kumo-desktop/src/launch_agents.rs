//! Only bootout and bootstrap move a LaunchAgent to an updated program: the app is signed ad hoc, and
//! `kickstart -k` keeps the old signature's requirement (`OS_REASON_CODESIGNING`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static ADMISSION: Admission = Admission::new();

/// Quitting between bootout and bootstrap would leave the agent unloaded until someone loads it, so
/// quitting first closes the admission, then waits while a restart it admitted is still running.
pub fn close_and_busy() -> bool {
    ADMISSION.close()
}

/// (closed, restarting), under one lock so a restart cannot begin after quitting read it as idle.
struct Admission(Mutex<(bool, bool)>);

struct Admitted<'a>(&'a Admission);

impl Admission {
    const fn new() -> Self {
        Self(Mutex::new((false, false)))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, (bool, bool)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn enter(&self) -> Option<Admitted<'_>> {
        let mut state = self.state();
        if state.0 {
            return None;
        }
        state.1 = true;
        Some(Admitted(self))
    }

    fn close(&self) -> bool {
        let mut state = self.state();
        state.0 = true;
        state.1
    }
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.0.state().1 = false;
    }
}

/// On every start, not only after an in-app update, because Homebrew and a manual reinstall replace
/// the app without telling it.
#[cfg(target_os = "macos")]
pub fn restart_stale() {
    let crate::updater::Install::MacApp { bundle, .. } = crate::updater::Install::current() else {
        return;
    };
    let Some(home) = std::env::home_dir() else {
        return;
    };
    std::thread::spawn(move || {
        let own = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
        // SAFETY: getuid has no preconditions.
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        for (label, plist, program) in
            agents_in(&home.join("Library/LaunchAgents"), &bundle, own.as_deref())
        {
            let job = Job {
                domain: &domain,
                label: &label,
                plist: &plist,
                program: &program,
            };
            match job.restart_if_stale(&ADMISSION, &mut launchctl, &started_before_install) {
                Ok(true) => eprintln!("update: restarted {label} on the new version"),
                Ok(false) => {}
                Err(e) => eprintln!("update: {label}: {e:#}"),
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn restart_stale() {}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn agents_in(dir: &Path, bundle: &Path, own: Option<&Path>) -> Vec<(String, PathBuf, PathBuf)> {
    let Ok(bundle) = bundle.canonicalize() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut agents: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "plist"))
        .filter_map(|plist| {
            let (label, program) = runs_from(&read_plist(&plist)?, &bundle, own)?;
            Some((label, plist, program))
        })
        .collect();
    agents.sort();
    agents
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn runs_from(
    job: &serde_json::Value,
    bundle: &Path,
    own: Option<&Path>,
) -> Option<(String, PathBuf)> {
    let label = job.get("Label")?.as_str()?;
    let program = job
        .get("Program")
        .or_else(|| job.get("ProgramArguments")?.get(0))?
        .as_str()?;
    let program = in_bundle(Path::new(program), bundle, own)?;
    Some((label.into(), program))
}

/// Canonical, so a Homebrew link to `kumo-mcp` counts and `/var` equals `/private/var`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn in_bundle(program: &Path, bundle: &Path, own: Option<&Path>) -> Option<PathBuf> {
    if !program.is_absolute() {
        return None;
    }
    let program = program.canonicalize().ok()?;
    (program.starts_with(bundle) && Some(program.as_path()) != own).then_some(program)
}

/// plutil reads binary plists too.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn read_plist(path: &Path) -> Option<serde_json::Value> {
    let output = std::process::Command::new("plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).ok())
        .flatten()
}

#[cfg(target_os = "macos")]
fn launchctl(args: &[&str]) -> (bool, String) {
    match std::process::Command::new("launchctl").args(args).output() {
        Ok(o) => (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(e) => (false, e.to_string()),
    }
}

/// ctime, not mtime: unpacking keeps the build's mtime but sets ctime to the install time.
#[cfg(target_os = "macos")]
fn started_before_install(pid: u32, program: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(program) else {
        return false;
    };
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: zeroed is a valid proc_bsdinfo, and proc_pidinfo writes at most the size passed.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    written == size
        && before(
            (
                info.pbi_start_tvsec as i64,
                info.pbi_start_tvusec as i64 * 1000,
            ),
            (meta.ctime(), meta.ctime_nsec()),
        )
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn before(started: (i64, i64), installed: (i64, i64)) -> bool {
    started < installed
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Job<'a> {
    domain: &'a str,
    label: &'a str,
    plist: &'a Path,
    program: &'a Path,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl Job<'_> {
    /// Bootout cannot be undone, so anything uncertain leaves the job as it is.
    fn restart_if_stale(
        &self,
        admission: &Admission,
        launchctl: &mut dyn FnMut(&[&str]) -> (bool, String),
        stale: &dyn Fn(u32, &Path) -> bool,
    ) -> anyhow::Result<bool> {
        use anyhow::bail;

        let service = format!("{}/{}", self.domain, self.label);
        let (loaded, printed) = launchctl(&["print", &service]);
        if !loaded {
            return Ok(false);
        }
        let field = |name: &str| {
            printed.lines().find_map(|l| {
                l.strip_prefix('\t')?
                    .strip_prefix(name)?
                    .strip_prefix(" = ")
                    .map(str::trim)
            })
        };
        let canonical = |p: &str| Path::new(p).canonicalize().ok();
        if field("program").and_then(canonical).as_deref() != Some(self.program)
            || field("path").and_then(canonical) != Some(self.plist.canonicalize()?)
        {
            return Ok(false);
        }
        let old = match field("pid").and_then(|p| p.parse().ok()) {
            Some(pid) => stale(pid, self.program),
            // launchd checks the new binary against the old signature.
            None => field("last exit reason") == Some("OS_REASON_CODESIGNING"),
        };
        if !old {
            return Ok(false);
        }
        let (listed, disabled) = launchctl(&["print-disabled", self.domain]);
        if !listed {
            bail!("cannot tell whether it is disabled: {}", disabled.trim());
        }
        if disabled.contains(&format!("\"{}\" => disabled", self.label))
            || disabled.contains(&format!("\"{}\" => true", self.label))
        {
            return Ok(false);
        }

        let Some(_admitted) = admission.enter() else {
            return Ok(false);
        };
        let (out, why) = launchctl(&["bootout", &service]);
        if !out {
            bail!("cannot stop the old version: {}", why.trim());
        }
        let plist = self.plist.to_string_lossy();
        let mut last = String::new();
        // bootout can return before the old process is gone, and bootstrap fails until it is.
        for attempt in 0..20 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            let (ok, why) = launchctl(&["bootstrap", self.domain, &plist]);
            if ok {
                // A job without RunAtLoad or KeepAlive stays idle after bootstrap.
                let (started, why) = launchctl(&["kickstart", &service]);
                // RunAtLoad may have started it already.
                if !started
                    && !launchctl(&["print", &service])
                        .1
                        .lines()
                        .any(|l| l.starts_with("\tpid = "))
                {
                    bail!("loaded, but cannot start it: {}", why.trim());
                }
                return Ok(true);
            }
            last = why;
        }
        bail!(
            "stopped, but cannot start it again ({}); start it with: launchctl bootstrap {} '{}'",
            last.trim(),
            self.domain,
            plist
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn finds_the_jobs_that_run_a_program_from_the_bundle() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let bundle = root.join("Better Kumo.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let mcp = macos.join("kumo-mcp");
        let app = macos.join("better-kumo");
        std::fs::write(&mcp, "").unwrap();
        std::fs::write(&app, "").unwrap();
        let link = root.join("kumo-mcp");
        std::os::unix::fs::symlink(&mcp, &link).unwrap();
        let other = root.join("elsewhere");
        std::fs::write(&other, "").unwrap();

        let job = |program: &Path| {
            json!({
                "Label": "com.kumowatch.mcp-http",
                "ProgramArguments": [program, "serve-http", "--listen", "127.0.0.1:8788"],
            })
        };
        let found = Some(("com.kumowatch.mcp-http".to_string(), mcp.clone()));
        assert_eq!(runs_from(&job(&mcp), &bundle, Some(&app)), found);
        assert_eq!(runs_from(&job(&link), &bundle, Some(&app)), found);
        assert_eq!(runs_from(&job(&other), &bundle, Some(&app)), None);
        assert_eq!(runs_from(&job(&app), &bundle, Some(&app)), None);
        assert_eq!(runs_from(&job(Path::new("kumo-mcp")), &bundle, None), None);
        let program = json!({"Label": "x", "Program": mcp});
        assert_eq!(
            runs_from(&program, &bundle, None),
            Some(("x".into(), mcp.clone()))
        );
        assert_eq!(runs_from(&json!({"Program": mcp}), &bundle, None), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reads_the_launch_agents_folder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mcp = root.join("Better Kumo.app/Contents/MacOS/kumo-mcp");
        std::fs::create_dir_all(mcp.parent().unwrap()).unwrap();
        std::fs::write(&mcp, "").unwrap();
        let agents = root.join("LaunchAgents");
        std::fs::create_dir_all(&agents).unwrap();
        let plist = |label: &str, program: &Path| {
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
                 <key>Label</key><string>{label}</string>\
                 <key>ProgramArguments</key><array><string>{}</string><string>serve-http</string></array>\
                 </dict></plist>",
                program.display()
            )
        };
        let mine = agents.join("com.kumowatch.mcp-http.plist");
        std::fs::write(&mine, plist("com.kumowatch.mcp-http", &mcp)).unwrap();
        std::fs::write(
            agents.join("other.plist"),
            plist("other", Path::new("/bin/sh")),
        )
        .unwrap();
        std::fs::write(agents.join("broken.plist"), "not a plist").unwrap();

        assert_eq!(
            agents_in(&agents, &root.join("Better Kumo.app"), None),
            vec![("com.kumowatch.mcp-http".to_string(), mine, mcp)]
        );
    }

    struct Fake {
        print: (bool, String),
        disabled: (bool, String),
        bootout: bool,
        bootstrap_fails: usize,
        kickstart: bool,
        print_after_bootout: Option<(bool, String)>,
        admission: Admission,
        calls: Vec<String>,
    }

    impl Fake {
        fn running(program: &Path, plist: &Path) -> Self {
            Self {
                print: (
                    true,
                    format!(
                        "gui/501/l = {{\n\tpath = {}\n\tstate = running\n\tprogram = {}\n\targuments = {{\n\t\t{}\n\t}}\n\tpid = 42\n}}\n",
                        plist.display(),
                        program.display(),
                        program.display()
                    ),
                ),
                disabled: (true, "disabled services = {\n}\n".into()),
                bootout: true,
                bootstrap_fails: 0,
                kickstart: true,
                print_after_bootout: None,
                admission: Admission::new(),
                calls: Vec::new(),
            }
        }

        fn run(&mut self, job: &Job, stale: bool) -> (anyhow::Result<bool>, Vec<String>) {
            let mut launchctl = |args: &[&str]| {
                let booted_out = self.calls.iter().any(|c| c.starts_with("bootout"));
                self.calls.push(args.join(" "));
                match args[0] {
                    "print" if booted_out && self.print_after_bootout.is_some() => {
                        self.print_after_bootout.clone().unwrap()
                    }
                    "print" => self.print.clone(),
                    "print-disabled" => self.disabled.clone(),
                    "bootout" => {
                        assert!(self.admission.state().1, "bootout outside the admission");
                        (self.bootout, "Boot-out failed: 5".into())
                    }
                    "bootstrap" if self.bootstrap_fails > 0 => {
                        self.bootstrap_fails -= 1;
                        (false, "Bootstrap failed: 5".into())
                    }
                    "bootstrap" => (true, String::new()),
                    "kickstart" => (self.kickstart, "Could not kickstart".into()),
                    _ => unreachable!(),
                }
            };
            let result = job.restart_if_stale(&self.admission, &mut launchctl, &|pid, _| {
                assert_eq!(pid, 42);
                stale
            });
            (result, std::mem::take(&mut self.calls))
        }
    }

    struct Files {
        _dir: tempfile::TempDir,
        mcp: PathBuf,
        other: PathBuf,
        plist: PathBuf,
    }

    fn files() -> Files {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = |name: &str| {
            let path = root.join(name);
            std::fs::write(&path, "").unwrap();
            path
        };
        Files {
            mcp: file("kumo-mcp"),
            other: file("other"),
            plist: file("l.plist"),
            _dir: dir,
        }
    }

    fn job(f: &Files) -> Job<'_> {
        Job {
            domain: "gui/501",
            label: "l",
            plist: &f.plist,
            program: &f.mcp,
        }
    }

    #[test]
    fn restarts_only_a_running_old_copy_of_the_same_job() {
        let f = files();
        let job = job(&f);
        let plist = f.plist.display();

        let (result, calls) = Fake::running(&f.mcp, &f.plist).run(&job, true);
        assert!(result.unwrap());
        assert_eq!(
            calls,
            [
                "print gui/501/l".to_string(),
                "print-disabled gui/501".into(),
                "bootout gui/501/l".into(),
                format!("bootstrap gui/501 {plist}"),
                "kickstart gui/501/l".into(),
            ]
        );

        let (result, calls) = Fake::running(&f.mcp, &f.plist).run(&job, false);
        assert!(!result.unwrap());
        assert_eq!(calls, ["print gui/501/l"]);

        let (result, calls) = Fake::running(&f.other, &f.plist).run(&job, true);
        assert!(!result.unwrap(), "loaded with another program");
        assert_eq!(calls, ["print gui/501/l"]);

        let (result, calls) = Fake::running(&f.mcp, &f.other).run(&job, true);
        assert!(!result.unwrap(), "loaded from another plist");
        assert_eq!(calls, ["print gui/501/l"]);

        let mut unloaded = Fake::running(&f.mcp, &f.plist);
        unloaded.print = (false, String::new());
        assert!(!unloaded.run(&job, true).0.unwrap());

        let mut stopped = Fake::running(&f.mcp, &f.plist);
        stopped.print.1 = stopped.print.1.replace("\tpid = 42\n", "");
        assert!(!stopped.run(&job, true).0.unwrap());

        let mut refused = Fake::running(&f.mcp, &f.plist);
        refused.print.1 = refused.print.1.replace(
            "\tpid = 42\n",
            "\tlast exit reason = OS_REASON_CODESIGNING\n",
        );
        assert!(refused.run(&job, false).0.unwrap());

        let mut disabled = Fake::running(&f.mcp, &f.plist);
        disabled.disabled.1 = "\t\t\"l\" => disabled\n".into();
        let (result, calls) = disabled.run(&job, true);
        assert!(!result.unwrap());
        assert!(!calls.iter().any(|c| c.starts_with("bootout")));

        let mut unknown = Fake::running(&f.mcp, &f.plist);
        unknown.disabled = (false, "Operation not permitted".into());
        let (result, calls) = unknown.run(&job, true);
        assert!(result.is_err());
        assert!(!calls.iter().any(|c| c.starts_with("bootout")));
    }

    #[test]
    fn a_quit_first_stops_a_restart_and_one_already_running_holds_the_quit() {
        let f = files();
        let job = job(&f);

        let mut late = Fake::running(&f.mcp, &f.plist);
        assert!(!late.admission.close());
        let (result, calls) = late.run(&job, true);
        assert!(!result.unwrap());
        assert!(!calls.iter().any(|c| c.starts_with("bootout")));

        let admission = Admission::new();
        let held = admission.enter().unwrap();
        assert!(admission.close(), "a quit waits for the restart it let in");
        drop(held);
        assert!(!admission.close());
        assert!(admission.enter().is_none());
    }

    #[test]
    fn reads_only_the_top_level_fields() {
        let f = files();
        let job = job(&f);
        let mut nested = Fake::running(&f.mcp, &f.plist);
        nested.print.1 = format!(
            "gui/501/l = {{\n\tpath = {}\n\tprogram = {}\n\tendpoints = {{\n\t\tpid = 42\n\t}}\n}}\n",
            f.plist.display(),
            f.mcp.display()
        );
        assert!(!nested.run(&job, true).0.unwrap());
    }

    #[test]
    fn says_how_to_recover_when_the_restart_fails() {
        let f = files();
        let job = job(&f);

        let mut busy = Fake::running(&f.mcp, &f.plist);
        busy.bootstrap_fails = 2;
        let (result, calls) = busy.run(&job, true);
        assert!(result.unwrap());
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("bootstrap")).count(),
            3
        );

        let mut stuck = Fake::running(&f.mcp, &f.plist);
        stuck.bootout = false;
        let (result, calls) = stuck.run(&job, true);
        assert!(result.unwrap_err().to_string().contains("cannot stop"));
        assert!(!calls.iter().any(|c| c.starts_with("bootstrap")));

        let mut idle = Fake::running(&f.mcp, &f.plist);
        idle.kickstart = false;
        idle.print_after_bootout = Some((true, idle.print.1.replace("\tpid = 42\n", "")));
        let error = idle.run(&job, true).0.unwrap_err().to_string();
        assert!(error.contains("cannot start"), "{error}");

        let mut started_at_load = Fake::running(&f.mcp, &f.plist);
        started_at_load.kickstart = false;
        assert!(started_at_load.run(&job, true).0.unwrap());

        let mut broken = Fake::running(&f.mcp, &f.plist);
        broken.bootstrap_fails = usize::MAX;
        let error = broken.run(&job, true).0.unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "launchctl bootstrap gui/501 '{}'",
                f.plist.display()
            )),
            "{error}"
        );
    }

    #[test]
    fn compares_start_and_install_below_the_second() {
        assert!(before((100, 100_000), (100, 200_000_000)));
        assert!(!before((100, 300_000_000), (100, 200_000_000)));
        assert!(!before((101, 0), (100, 999_999_999)));
        assert!(before((99, 999_999_000), (100, 0)));
    }
}
