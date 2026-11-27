use std::path::PathBuf;
use std::sync::Arc;

use kumo_relay::{Config, Relay};
use tokio::task::JoinHandle;

use crate::supervisor::{OnReady, RelayHandle, RelayStarter};

impl RelayHandle for Arc<Relay> {
    async fn stop(&self) {
        Relay::stop(self);
    }
}

pub struct ConfiguredRelay {
    config_path: PathBuf,
}

impl ConfiguredRelay {
    pub fn new() -> Self {
        Self {
            config_path: kumo_relay::config::default_config_path(),
        }
    }
}

impl RelayStarter for ConfiguredRelay {
    type Handle = Arc<Relay>;

    fn configured_port(&self) -> anyhow::Result<Option<u16>> {
        if !std::fs::exists(&self.config_path).map_err(|e| {
            anyhow::anyhow!(
                "cannot check relay.toml: {e} ({})",
                self.config_path.display()
            )
        })? {
            return Ok(None);
        }
        let config = Config::load(&self.config_path)?;
        Ok(kumo_relay::is_signed_in(&config).then(|| config.port()))
    }

    async fn set_up_from_desktop_app(&self, devtools_url: String) -> anyhow::Result<u16> {
        kumo_relay::setup_from_desktop(&self.config_path, &devtools_url)
            .await
            .map(|config| config.port())
    }

    fn start(&self, on_ready: OnReady<Arc<Relay>>) -> JoinHandle<anyhow::Result<()>> {
        let path = self.config_path.clone();
        tokio::spawn(async move { kumo_relay::run(&path, std::future::pending(), on_ready).await })
    }
}
