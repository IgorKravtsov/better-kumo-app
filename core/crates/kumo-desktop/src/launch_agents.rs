//! A LaunchAgent the user set up to run a program from this app, such as `kumo-mcp serve-http`, keeps
//! running the old one after an update. `kickstart -k` cannot replace it: the app is signed ad hoc and
//! launchd keeps the old signature's requirement (`OS_REASON_CODESIGNING`), so only bootout and
//! bootstrap work.

use std::path::{Path, PathBuf};

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
            match job.restart_if_stale(&mut launchctl, &started_before_install) {
                Ok(true) => eprintln!("update: restarted {label} on the new version"),
                Ok(false) => {}
                Err(e) => eprintln!("update: {label}: {e:#}"),
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn restart_stale() {}

/// Label, plist and canonical program of each agent in `dir` that runs a program inside `bundle`
/// other than `own`.
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
    let Ok(installed) = std::fs::metadata(program).map(|m| m.ctime()) else {
        return false;
    };
    let pid = sysinfo::Pid::from_u32(pid);
    let Ok(mut system) = sysinfo::System::new() else {
        return false;
    };
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system
        .process(pid)
        .is_some_and(|p| (p.start_time() as i64) < installed)
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
    /// Leaves alone a job that is unloaded, disabled, stopped, already new, or loaded from
    /// another definition than its plist now holds: bootout would stop something bootstrap does not
    /// bring back.
    fn restart_if_stale(
        &self,
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
        let loaded_program = field("program").and_then(|p| Path::new(p).canonicalize().ok());
        if loaded_program.as_deref() != Some(self.program) {
            return Ok(false);
        }
        let old = match field("pid").and_then(|p| p.parse().ok()) {
            Some(pid) => stale(pid, self.program),
            // Exited after the update, and launchd now refuses the new binary.
            None => field("last exit reason") == Some("OS_REASON_CODESIGNING"),
        };
        if !old {
            return Ok(false);
        }
        let (_, disabled) = launchctl(&["print-disabled", self.domain]);
        if disabled.contains(&format!("\"{}\" => disabled", self.label))
            || disabled.contains(&format!("\"{}\" => true", self.label))
        {
            return Ok(false);
        }

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
        disabled: String,
        bootout: bool,
        bootstrap_fails: usize,
        calls: Vec<String>,
    }

    impl Fake {
        fn running(program: &Path) -> Self {
            Self {
                print: (
                    true,
                    format!(
                        "gui/501/l = {{\n\tstate = running\n\tprogram = {}\n\targuments = {{\n\t}}\n\tpid = 42\n}}\n",
                        program.display()
                    ),
                ),
                disabled: "disabled services = {\n}\n".into(),
                bootout: true,
                bootstrap_fails: 0,
                calls: Vec::new(),
            }
        }

        fn run(&mut self, job: &Job, stale: bool) -> (anyhow::Result<bool>, Vec<String>) {
            let mut launchctl = |args: &[&str]| {
                self.calls.push(args[0].to_string());
                match args[0] {
                    "print" => self.print.clone(),
                    "print-disabled" => (true, self.disabled.clone()),
                    "bootout" => (self.bootout, "Boot-out failed: 5".into()),
                    "bootstrap" if self.bootstrap_fails > 0 => {
                        self.bootstrap_fails -= 1;
                        (false, "Bootstrap failed: 5".into())
                    }
                    "bootstrap" => (true, String::new()),
                    _ => unreachable!(),
                }
            };
            let result = job.restart_if_stale(&mut launchctl, &|pid, _| {
                assert_eq!(pid, 42);
                stale
            });
            (result, std::mem::take(&mut self.calls))
        }
    }

    #[test]
    fn restarts_only_a_running_old_copy_of_the_same_job() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mcp = root.join("kumo-mcp");
        let other = root.join("other");
        std::fs::write(&mcp, "").unwrap();
        std::fs::write(&other, "").unwrap();
        let plist = root.join("l.plist");
        let job = Job {
            domain: "gui/501",
            label: "l",
            plist: &plist,
            program: &mcp,
        };

        let (result, calls) = Fake::running(&mcp).run(&job, true);
        assert!(result.unwrap());
        assert_eq!(calls, ["print", "print-disabled", "bootout", "bootstrap"]);

        let (result, calls) = Fake::running(&mcp).run(&job, false);
        assert!(!result.unwrap());
        assert_eq!(calls, ["print"]);

        let (result, calls) = Fake::running(&other).run(&job, true);
        assert!(!result.unwrap(), "loaded from a different definition");
        assert_eq!(calls, ["print"]);

        let mut unloaded = Fake::running(&mcp);
        unloaded.print = (false, String::new());
        assert!(!unloaded.run(&job, true).0.unwrap());

        let mut stopped = Fake::running(&mcp);
        stopped.print.1 = stopped.print.1.replace("\tpid = 42\n", "");
        assert!(!stopped.run(&job, true).0.unwrap());

        let mut refused = Fake::running(&mcp);
        refused.print.1 = refused.print.1.replace(
            "\tpid = 42\n",
            "\tlast exit reason = OS_REASON_CODESIGNING\n",
        );
        assert!(refused.run(&job, false).0.unwrap());

        let mut disabled = Fake::running(&mcp);
        disabled.disabled = "\t\t\"l\" => disabled\n".into();
        let (result, calls) = disabled.run(&job, true);
        assert!(!result.unwrap());
        assert!(!calls.contains(&"bootout".to_string()));
    }

    #[test]
    fn says_how_to_recover_when_the_restart_fails() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mcp = root.join("kumo-mcp");
        std::fs::write(&mcp, "").unwrap();
        let plist = root.join("l.plist");
        let job = Job {
            domain: "gui/501",
            label: "l",
            plist: &plist,
            program: &mcp,
        };

        let mut busy = Fake::running(&mcp);
        busy.bootstrap_fails = 2;
        let (result, calls) = busy.run(&job, true);
        assert!(result.unwrap());
        assert_eq!(calls.iter().filter(|c| *c == "bootstrap").count(), 3);

        let mut stuck = Fake::running(&mcp);
        stuck.bootout = false;
        let (result, calls) = stuck.run(&job, true);
        assert!(result.unwrap_err().to_string().contains("cannot stop"));
        assert!(!calls.contains(&"bootstrap".to_string()));

        let mut broken = Fake::running(&mcp);
        broken.bootstrap_fails = usize::MAX;
        let error = broken.run(&job, true).0.unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "launchctl bootstrap gui/501 '{}'",
                plist.display()
            )),
            "{error}"
        );
    }
}
