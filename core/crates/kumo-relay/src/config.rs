use std::path::{Path, PathBuf};

use kumo_meetings::MeetingsConfig;
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
    #[serde(default)]
    pub meetings: MeetingsConfig,
    /// Only Better Kumo's first setup writes it, so older files keep listening on the network.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local_until_paired: bool,
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

pub fn has_secret(secret: &str) -> bool {
    !secret.trim().is_empty()
}

pub fn home() -> PathBuf {
    std::env::home_dir().unwrap_or_default()
}

pub fn default_config_path() -> PathBuf {
    home().join(".config").join("kumowatch").join("relay.toml")
}

impl Config {
    /// Refuses a blank secret so a misconfigured relay fails before it binds instead of looking operational.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            let hint = if e.kind() == std::io::ErrorKind::NotFound {
                "; run `kumo-relay init`"
            } else {
                ""
            };
            anyhow::anyhow!("cannot read relay.toml: {e} ({}){hint}", path.display())
        })?;
        Self::parse(&text).map_err(|problem| anyhow::anyhow!("{problem} ({})", path.display()))
    }

    /// Errors never quote the file: it holds the secret, and they end up in logs.
    fn parse(text: &str) -> Result<Self, String> {
        let config: Self = toml::from_str(text).map_err(|e| {
            let Some(before) = e.span().and_then(|span| text.get(..span.start)) else {
                return "relay.toml is not valid".to_string();
            };
            let line = before.matches('\n').count() + 1;
            let column = before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .chars()
                .count()
                + 1;
            format!("relay.toml is not valid at line {line}, column {column}")
        })?;
        if !has_secret(&config.secret) {
            return Err(
                "relay.toml has an empty secret; set a long random `secret` in it".to_string(),
            );
        }
        Ok(config)
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

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir().join("models")
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir().join("chat.db")
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Written aside and renamed so the Relay, Better Kumo and kumo-mcp never read a half-written file.
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        let tmp = path.with_file_name(name);
        let _ = std::fs::remove_file(&tmp);
        let mut file = std::fs::OpenOptions::new();
        file.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut file, 0o600);
        std::io::Write::write_all(
            &mut file.open(&tmp)?,
            toml::to_string_pretty(self)?.as_bytes(),
        )?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn port(&self) -> u16 {
        self.listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(8787)
    }

    pub fn bind_address(&self) -> String {
        match self.listen.parse::<std::net::SocketAddr>() {
            Ok(addr) if self.local_until_paired && addr.ip().is_unspecified() => {
                format!("127.0.0.1:{}", addr.port())
            }
            _ => self.listen.clone(),
        }
    }

    pub fn waiting_for_phone(&self) -> bool {
        self.bind_address() != self.listen
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
            meetings: MeetingsConfig::default(),
            local_until_paired: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(secret: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.toml");
        Config::template(secret, "office").save(&path).unwrap();
        (dir, path)
    }

    #[test]
    fn a_blank_secret_is_refused() {
        for blank in ["", "   ", "\t\n"] {
            let (_dir, path) = saved(blank);
            let before = std::fs::read(&path).unwrap();
            let error = format!("{:#}", Config::load(&path).unwrap_err());
            assert!(
                error.starts_with("relay.toml has an empty secret"),
                "{error}"
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "the file is left alone"
            );
        }
    }

    #[test]
    fn a_set_secret_loads_unchanged() {
        let (_dir, path) = saved("hand-written");
        let before = std::fs::read(&path).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.secret, "hand-written");
        assert_eq!(config.space, "office");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_wildcard_listen_stays_on_loopback_until_a_phone_pairs() {
        let mut config = Config::template("s", "office");
        assert_eq!(config.bind_address(), "0.0.0.0:8787");
        assert!(!config.waiting_for_phone());

        config.local_until_paired = true;
        assert_eq!(config.bind_address(), "127.0.0.1:8787");
        assert!(config.waiting_for_phone());

        config.listen = "[::]:9000".into();
        assert_eq!(config.bind_address(), "127.0.0.1:9000");

        for chosen in ["100.84.12.7:8787", "127.0.0.1:8787", "relay.local:8787"] {
            config.listen = chosen.into();
            assert_eq!(config.bind_address(), chosen);
            assert!(!config.waiting_for_phone());
        }
    }

    #[test]
    fn older_files_keep_listening_on_the_network() {
        let (_dir, path) = saved("s");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("local_until_paired"), "{text}");
        assert_eq!(Config::load(&path).unwrap().bind_address(), "0.0.0.0:8787");

        let mut config = Config::load(&path).unwrap();
        config.local_until_paired = true;
        config.save(&path).unwrap();
        assert_eq!(
            Config::load(&path).unwrap().bind_address(),
            "127.0.0.1:8787"
        );
    }

    #[test]
    fn parse_errors_give_the_place_but_not_the_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.toml");
        for text in [
            "listen = \"0.0.0.0:8787\"\nsecret = \"s3cr3t-token\nspace = \"office\"\n",
            "listen = \"0.0.0.0:8787\"\nsecret = 0x5ec3e7\nspace = \"office\"\n",
            "listen = \"s3cr3t-token\"\nsecret = [\"s3cr3t-token\"]\nspace = \"office\"\n",
        ] {
            std::fs::write(&path, text).unwrap();
            let error = format!("{:#}", Config::load(&path).unwrap_err());
            assert!(
                error.starts_with("relay.toml is not valid at line 2"),
                "{error}"
            );
            let suffix = format!(" ({})", path.display());
            let diagnostic = error.strip_suffix(suffix.as_str()).unwrap();
            for leak in ["s3cr3t", "5ec3e7", "secret =", "listen"] {
                assert!(!diagnostic.contains(leak), "{error}");
            }
        }
    }
}
