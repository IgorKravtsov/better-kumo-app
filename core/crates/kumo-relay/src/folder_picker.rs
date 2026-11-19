use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{Context, bail};
use tokio::process::Command;
use tokio::sync::Mutex;

const PROMPT: &str = "Choose the folder for meeting transcripts";

static OPEN: Mutex<()> = Mutex::const_new(());

pub async fn choose_folder(start: &Path) -> anyhow::Result<Option<PathBuf>> {
    let Ok(_open) = OPEN.try_lock() else {
        bail!("a folder window is already open");
    };
    let start = existing_ancestor(start);
    let output = run_dialog(&start).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if cancelled(output.status.code(), &stderr) {
            return Ok(None);
        }
        bail!("the folder window failed: {}", stderr.trim());
    }
    let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if chosen.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(chosen)))
}

fn existing_ancestor(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .or_else(std::env::home_dir)
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
const MAC_SCRIPT: &str = r#"on run argv
activate
set chosen to choose folder with prompt (item 1 of argv) default location (POSIX file (item 2 of argv))
return POSIX path of chosen
end run"#;

#[cfg(target_os = "macos")]
async fn run_dialog(start: &Path) -> anyhow::Result<Output> {
    Command::new("osascript")
        .args(["-e", MAC_SCRIPT, PROMPT])
        .arg(start)
        .output()
        .await
        .context("could not open the folder window")
}

#[cfg(target_os = "macos")]
fn cancelled(_code: Option<i32>, stderr: &str) -> bool {
    stderr.contains("-128")
}

#[cfg(windows)]
const WINDOWS_SCRIPT: &str = r#"Add-Type -AssemblyName System.Windows.Forms
$owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true; ShowInTaskbar = $false }
$dialog = New-Object System.Windows.Forms.FolderBrowserDialog
$dialog.Description = $env:BETTER_KUMO_PROMPT
$dialog.SelectedPath = $env:BETTER_KUMO_START
$dialog.ShowNewFolderButton = $true
if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) { [Console]::Out.Write($dialog.SelectedPath) } else { exit 1 }"#;

#[cfg(windows)]
async fn run_dialog(start: &Path) -> anyhow::Result<Output> {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-Command",
            WINDOWS_SCRIPT,
        ])
        .env("BETTER_KUMO_PROMPT", PROMPT)
        .env("BETTER_KUMO_START", start)
        .output()
        .await
        .context("could not open the folder window")
}

#[cfg(windows)]
fn cancelled(code: Option<i32>, _stderr: &str) -> bool {
    code == Some(1)
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn run_dialog(start: &Path) -> anyhow::Result<Output> {
    let mut start = start.as_os_str().to_owned();
    start.push("/");
    let zenity = Command::new("zenity")
        .args([
            "--file-selection",
            "--directory",
            "--title",
            PROMPT,
            "--filename",
        ])
        .arg(&start)
        .output()
        .await;
    match zenity {
        Ok(output) => Ok(output),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Command::new("kdialog")
            .args(["--title", PROMPT, "--getexistingdirectory"])
            .arg(&start)
            .output()
            .await
            .context("install zenity or kdialog to choose a folder, or type the path"),
        Err(e) => Err(e).context("could not open the folder window"),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn cancelled(code: Option<i32>, _stderr: &str) -> bool {
    code == Some(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_folder_starts_the_window_at_its_nearest_existing_parent() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("Better Kumo").join("Meetings");
        assert_eq!(existing_ancestor(&missing), dir.path());
    }

    #[test]
    fn an_existing_folder_starts_the_window_there() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(existing_ancestor(dir.path()), dir.path());
    }
}
