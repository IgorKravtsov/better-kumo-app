//! A LaunchAgent the user set up to run a program from this app, such as `kumo-mcp serve-http`, keeps
//! the old one running after an update until it is restarted. `launchctl kickstart -k` cannot do it:
//! the binaries are signed ad hoc, so the new one fails launchd's check against the old signature
//! (`OS_REASON_CODESIGNING`). Booting the agent out and in again lets launchd take the new one.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
pub fn restart_after_update() {
    let crate::updater::Install::MacApp { bundle, .. } = crate::updater::Install::current() else {
        return;
    };
    let Some(home) = std::env::home_dir() else {
        return;
    };
    std::thread::spawn(move || {
        let own = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
        for (label, plist) in agents_in(&home.join("Library/LaunchAgents"), &bundle, own.as_deref())
        {
            match restart(&label, &plist) {
                Ok(true) => eprintln!("update: restarted {label} on the new version"),
                Ok(false) => {}
                Err(e) => eprintln!("update: cannot restart {label}: {e:#}"),
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn restart_after_update() {}

/// The agents in `dir` that start a program inside `bundle`, other than this app itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn agents_in(dir: &Path, bundle: &Path, own: Option<&Path>) -> Vec<(String, PathBuf)> {
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
            let label = runs_from(&read_plist(&plist)?, &bundle, own)?;
            Some((label, plist))
        })
        .collect();
    agents.sort();
    agents
}

/// The job's label, if its program lives in `bundle` (also through a symlink, as Homebrew links
/// `kumo-mcp`) and is not `own`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn runs_from(job: &serde_json::Value, bundle: &Path, own: Option<&Path>) -> Option<String> {
    let label = job.get("Label")?.as_str()?;
    let program = job
        .get("Program")
        .or_else(|| job.get("ProgramArguments")?.get(0))?
        .as_str()?;
    let program = Path::new(program).canonicalize().ok()?;
    (program.starts_with(bundle) && Some(program.as_path()) != own).then(|| label.into())
}

/// Through `plutil`, which reads binary plists too.
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

/// Only an agent that is loaded now, so one the user stopped stays stopped. False if it was not.
#[cfg(target_os = "macos")]
fn restart(label: &str, plist: &Path) -> anyhow::Result<bool> {
    use anyhow::bail;
    use std::process::{Command, Stdio};

    // SAFETY: getuid cannot fail.
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let service = format!("{domain}/{label}");
    let quiet = |args: &[&str]| {
        Command::new("launchctl")
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    if !quiet(&["print", &service]) {
        return Ok(false);
    }
    quiet(&["bootout", &service]);
    let plist = plist.to_string_lossy();
    // bootout may return before the old process is gone, and bootstrap fails until it is.
    let mut last = String::new();
    for _ in 0..20 {
        let output = Command::new("launchctl")
            .args(["bootstrap", &domain, &plist])
            .output()?;
        if output.status.success() {
            return Ok(true);
        }
        last = String::from_utf8_lossy(&output.stderr).trim().into();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    bail!("launchctl bootstrap failed: {last}")
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
        let label = Some("com.kumowatch.mcp-http".to_string());
        assert_eq!(runs_from(&job(&mcp), &bundle, Some(&app)), label);
        assert_eq!(runs_from(&job(&link), &bundle, Some(&app)), label);
        assert_eq!(runs_from(&job(&other), &bundle, Some(&app)), None);
        assert_eq!(runs_from(&job(&app), &bundle, Some(&app)), None);
        assert_eq!(runs_from(&job(Path::new("kumo-mcp")), &bundle, None), None);
        let program = json!({"Label": "x", "Program": mcp});
        assert_eq!(runs_from(&program, &bundle, None), Some("x".into()));
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
            vec![("com.kumowatch.mcp-http".to_string(), mine)]
        );
    }
}
