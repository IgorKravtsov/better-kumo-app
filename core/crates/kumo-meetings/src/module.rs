use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use futures_util::future::BoxFuture;
use serde::Serialize;

use crate::model::fetch;

pub const PROTOCOL: u32 = 1;
pub const RELEASES: &str = "https://github.com/IgorKravtsov/better-kumo-app/releases/download";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModuleState {
    #[default]
    Missing,
    Unsupported,
    Installing {
        step: String,
        percent: u8,
    },
    Ready {
        version: String,
    },
    Failed {
        error: String,
    },
}

impl ModuleState {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    pub dir: PathBuf,
    pub override_path: Option<PathBuf>,
    pub base_url: String,
    pub version: String,
}

impl Module {
    pub fn new(dir: PathBuf, override_path: Option<PathBuf>) -> Self {
        Self {
            dir,
            override_path,
            base_url: RELEASES.into(),
            version: VERSION.into(),
        }
    }

    pub fn binary(&self) -> Option<PathBuf> {
        match &self.override_path {
            Some(path) => path.exists().then(|| path.clone()),
            None => {
                let path = self.dir.join(binary_name());
                path.exists().then_some(path)
            }
        }
    }

    pub fn asset_url(&self) -> Option<String> {
        let asset = asset_name(&self.version)?;
        Some(format!("{}/v{}/{asset}", self.base_url, self.version))
    }

    pub async fn installed_version(&self) -> anyhow::Result<String> {
        let binary = self
            .binary()
            .context("the meetings module is not installed")?;
        let output = command(&binary)
            .arg("--version")
            .output()
            .await
            .with_context(|| format!("cannot start {}", binary.display()))?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut words = text.split_whitespace();
        let version = words.next().unwrap_or_default().to_string();
        let protocol: u32 = words.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        if !output.status.success() || version.is_empty() {
            bail!("the meetings module does not start");
        }
        if protocol != PROTOCOL {
            bail!(
                "the meetings module {version} is for another Better Kumo version, install it again"
            );
        }
        Ok(version)
    }

    pub async fn install(&self, progress: impl Fn(u8)) -> anyhow::Result<PathBuf> {
        if self.override_path.is_some() {
            bail!("transcriber is set in relay.toml, so nothing is downloaded");
        }
        let url = self
            .asset_url()
            .context("there is no meetings module for this computer")?;
        tokio::fs::create_dir_all(&self.dir).await?;
        let expected = sha256_of_listing(&fetch_text(&format!("{url}.sha256")).await?)?;
        let partial = self.dir.join(format!("{}.part", binary_name()));
        let actual = fetch(&url, &partial, progress).await?;
        if actual != expected {
            let _ = tokio::fs::remove_file(&partial).await;
            bail!("the meetings module download is damaged (checksum mismatch), try again");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755)).await?;
        }
        let target = self.dir.join(binary_name());
        tokio::fs::rename(&partial, &target).await?;
        Ok(target)
    }

    pub async fn remove(&self) -> anyhow::Result<()> {
        if self.override_path.is_none() {
            let path = self.dir.join(binary_name());
            if path.exists() {
                tokio::fs::remove_file(path).await?;
            }
        }
        Ok(())
    }
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) {
        "better-kumo-transcriber.exe"
    } else {
        "better-kumo-transcriber"
    }
}

pub fn asset_name(version: &str) -> Option<String> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macos-arm64",
        ("macos", "x86_64") => "macos-x64",
        ("windows", "x86_64") => "windows-x64",
        ("linux", "x86_64") => "linux-x64",
        _ => return None,
    };
    let ext = if cfg!(windows) { ".exe" } else { "" };
    Some(format!("better-kumo-transcriber-{platform}-{version}{ext}"))
}

fn sha256_of_listing(listing: &str) -> anyhow::Result<String> {
    let hex = listing
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("the meetings module checksum is missing");
    }
    Ok(hex)
}

async fn fetch_text(url: &str) -> anyhow::Result<String> {
    Ok(crate::model::client()?
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("cannot download {url}"))?
        .text()
        .await?)
}

pub fn command(binary: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    command.kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

pub struct Job<'a> {
    pub binary: &'a Path,
    pub model: &'a Path,
    pub language: &'a str,
    pub meeting: &'a Path,
}

pub trait Transcribe: Send + Sync {
    fn transcribe<'a>(&'a self, job: Job<'a>) -> BoxFuture<'a, anyhow::Result<()>>;
}

pub struct ProcessTranscriber;

impl Transcribe for ProcessTranscriber {
    fn transcribe<'a>(&'a self, job: Job<'a>) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let output = command(job.binary)
                .arg("--model")
                .arg(job.model)
                .arg("--language")
                .arg(job.language)
                .arg(job.meeting)
                .output()
                .await
                .with_context(|| format!("cannot start {}", job.binary.display()))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let last = stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("no output");
                bail!("the transcriber failed: {last}");
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_computer_has_its_own_asset() {
        let name = asset_name("1.2.3");
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(
                name.as_deref(),
                Some("better-kumo-transcriber-macos-arm64-1.2.3")
            );
        }
        let module = Module {
            dir: PathBuf::from("/tmp/m"),
            override_path: None,
            base_url: "https://example.test/dl".into(),
            version: "1.2.3".into(),
        };
        if let Some(url) = module.asset_url() {
            assert!(url.starts_with("https://example.test/dl/v1.2.3/better-kumo-transcriber-"));
        }
    }

    #[test]
    fn reads_a_sha256sum_line() {
        let hex = "a".repeat(64);
        assert_eq!(sha256_of_listing(&format!("{hex}  file\n")).unwrap(), hex);
        assert!(sha256_of_listing("nope").is_err());
    }

    #[test]
    fn a_configured_transcriber_counts_only_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t");
        let module = Module::new(dir.path().join("module"), Some(path.clone()));
        assert_eq!(module.binary(), None);
        std::fs::write(&path, b"").unwrap();
        assert_eq!(module.binary(), Some(path));
    }

    #[cfg(unix)]
    async fn serve(files: Vec<(String, Vec<u8>)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let files = files.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let path = request
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    let reply = match files.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let mut r = format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                body.len()
                            )
                            .into_bytes();
                            r.extend_from_slice(body);
                            r
                        }
                        None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = socket.write_all(&reply).await;
                });
            }
        });
        base
    }

    #[cfg(unix)]
    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn installs_the_module_from_the_release_and_checks_it() {
        let Some(asset) = asset_name("1.2.3") else {
            return;
        };
        let program = format!("#!/bin/sh\necho \"1.2.3 {PROTOCOL}\"\n").into_bytes();
        let listing = format!("{}  {asset}\n", sha256_hex(&program));
        let base = serve(vec![
            (format!("/v1.2.3/{asset}"), program),
            (format!("/v1.2.3/{asset}.sha256"), listing.into_bytes()),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let module = Module {
            dir: dir.path().join("module"),
            override_path: None,
            base_url: base,
            version: "1.2.3".into(),
        };
        assert_eq!(module.binary(), None);
        let shown = std::sync::Mutex::new(Vec::new());
        let path = module
            .install(|p| shown.lock().unwrap().push(p))
            .await
            .unwrap();
        assert_eq!(module.binary(), Some(path));
        assert_eq!(shown.lock().unwrap().last(), Some(&100));
        assert_eq!(module.installed_version().await.unwrap(), "1.2.3");
        module.remove().await.unwrap();
        assert_eq!(module.binary(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_damaged_download_is_not_installed() {
        let Some(asset) = asset_name("1.2.3") else {
            return;
        };
        let listing = format!("{}  {asset}\n", "0".repeat(64));
        let base = serve(vec![
            (format!("/v1.2.3/{asset}"), b"tampered".to_vec()),
            (format!("/v1.2.3/{asset}.sha256"), listing.into_bytes()),
        ])
        .await;
        let dir = tempfile::tempdir().unwrap();
        let module = Module {
            dir: dir.path().join("module"),
            override_path: None,
            base_url: base,
            version: "1.2.3".into(),
        };
        let error = module.install(|_| {}).await.unwrap_err();
        assert!(format!("{error:#}").contains("checksum"), "{error:#}");
        assert_eq!(module.binary(), None);
        assert_eq!(
            std::fs::read_dir(dir.path().join("module"))
                .unwrap()
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checks_the_module_answers_with_this_protocol() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = |body: &str| {
            let path = dir.path().join(format!("t{}", body.len()));
            std::fs::write(&path, format!("#!/bin/sh\necho \"{body}\"\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Module::new(dir.path().join("module"), Some(path))
        };
        assert_eq!(
            script(&format!("0.9.0 {PROTOCOL}"))
                .installed_version()
                .await
                .unwrap(),
            "0.9.0"
        );
        assert!(script("0.9.0 999").installed_version().await.is_err());
    }
}
