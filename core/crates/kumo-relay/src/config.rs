use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub listen: String,
    pub secret: String,
    pub space: String,
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub ntfy: Option<NtfyConfig>,
    #[serde(default)]
    pub apns: Option<ApnsConfig>,
    #[serde(default = "default_desktop")]
    pub desktop_cdp: Option<String>,
}

fn default_desktop() -> Option<String> {
    Some(kumo_devtools::local_url(kumo_devtools::DEFAULT_PORT))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtfyConfig {
    #[serde(default = "default_ntfy_server")]
    pub server: String,
    pub topic: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApnsConfig {
    pub team_id: String,
    pub key_id: String,
    pub key_path: PathBuf,
    pub topic: String,
    #[serde(default = "default_true")]
    pub sandbox: bool,
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".into()
}

fn default_true() -> bool {
    true
}

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_default()
}

pub fn default_config_path() -> PathBuf {
    home().join(".config").join("kumowatch").join("relay.toml")
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}; run `kumo-relay init`", path.display()))?;
        Ok(toml::from_str(&text)?)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| home().join(".local").join("share").join("kumowatch-relay"))
    }

    pub fn session_path(&self) -> PathBuf {
        self.data_dir().join("session.json")
    }

    pub fn devices_path(&self) -> PathBuf {
        self.data_dir().join("devices.json")
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir().join("chat.db")
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787)
    }

    pub fn template(secret: &str, space: &str) -> Self {
        Self {
            listen: "0.0.0.0:8787".into(),
            secret: secret.into(),
            space: space.into(),
            data_dir: None,
            ntfy: None,
            apns: None,
            desktop_cdp: default_desktop(),
        }
    }
}
