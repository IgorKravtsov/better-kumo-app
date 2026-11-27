//! A relay that Better Kumo set up answers only this computer until a phone pairs
//! (`Config::local_until_paired`, docs/architecture.md "Trust boundaries").

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Config, Relay};

#[derive(Default)]
pub struct NetworkAccess {
    waiting: AtomicBool,
    /// Another device reached the Relay with the secret since it started.
    phone_seen: AtomicBool,
    /// Stays set after a phone pairs: the main listener only moves to `listen` on the next start.
    loopback_only: AtomicBool,
    opened: Mutex<HashSet<IpAddr>>,
}

impl NetworkAccess {
    pub(crate) fn wait_for_phone(&self, waiting: bool) {
        self.waiting.store(waiting, Ordering::SeqCst);
        self.loopback_only.store(waiting, Ordering::SeqCst);
    }

    pub fn waiting_for_phone(&self) -> bool {
        self.waiting.load(Ordering::SeqCst)
    }

    /// The addresses of `ips` that have no pairing listener yet, now marked as having one.
    fn claim(&self, ips: impl IntoIterator<Item = IpAddr>) -> Vec<IpAddr> {
        let Ok(mut opened) = self.opened.lock() else {
            return Vec::new();
        };
        ips.into_iter().filter(|ip| opened.insert(*ip)).collect()
    }

    fn release(&self, ip: IpAddr) {
        if let Ok(mut opened) = self.opened.lock() {
            opened.remove(&ip);
        }
    }
}

impl Relay {
    pub fn waiting_for_phone(&self) -> bool {
        self.access.waiting_for_phone()
    }

    /// Another device (a phone, or an app given the pairing link) reached the Relay with the
    /// secret since it started.
    pub fn phone_seen(&self) -> bool {
        self.access.phone_seen.load(Ordering::SeqCst)
    }

    /// Re-run on every visit to `/pair` (and the Setup guide's phone step) so an address that
    /// appeared since (another Wi-Fi, Tailscale) is added.
    pub async fn open_for_pairing(self: &Arc<Self>, config: &Config) {
        if !self.access.loopback_only.load(Ordering::SeqCst) {
            return;
        }
        let port = config.port();
        for ip in self.access.claim(crate::network::pair_ips()) {
            match tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await {
                Ok(listener) => {
                    eprintln!("kumo-relay: listening on {ip}:{port} so a phone can pair");
                    let relay = self.clone();
                    let config = config.clone();
                    tokio::spawn(async move {
                        let served =
                            crate::serve(listener, relay, config, std::future::pending()).await;
                        if let Err(e) = served {
                            eprintln!("kumo-relay: stopped listening on {ip}:{port}: {e}");
                        }
                    });
                }
                Err(e) => {
                    eprintln!("kumo-relay: cannot listen on {ip}:{port} for pairing: {e}");
                    self.access.release(ip);
                }
            }
        }
    }

    /// `peer` sent the secret. Another device doing so is a paired phone (or another app given the
    /// link), so the next start listens on the network. Until that is saved, every such request
    /// tries again.
    pub(crate) fn phone_reached(&self, peer: IpAddr) {
        if peer.to_canonical().is_loopback() {
            return;
        }
        self.access.phone_seen.store(true, Ordering::SeqCst);
        if !self.access.waiting_for_phone() {
            return;
        }
        let saved = self.update_config(|config| config.local_until_paired = false);
        if let Err(e) = saved {
            eprintln!("kumo-relay: cannot save that a phone paired: {e:#}");
            return;
        }
        if self.access.waiting.swap(false, Ordering::SeqCst) {
            eprintln!("kumo-relay: a phone reached the Relay from {peer}; it stays on the network");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn each_address_is_opened_once() {
        let access = NetworkAccess::default();
        let wifi = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        let tailscale = IpAddr::V4(Ipv4Addr::new(100, 84, 12, 7));
        assert_eq!(access.claim([wifi]), vec![wifi]);
        assert_eq!(access.claim([wifi, tailscale]), vec![tailscale]);
        access.release(wifi);
        assert_eq!(access.claim([wifi, tailscale]), vec![wifi]);
    }

    fn relay_waiting(dir: &std::path::Path) -> (Arc<Relay>, std::path::PathBuf) {
        let path = dir.join("relay.toml");
        let mut config = Config::template("s3cret", "office");
        config.local_until_paired = true;
        config.save(&path).unwrap();
        let relay = Relay::new(crate::RelayParts {
            uid: "uid-me".into(),
            data_dir: dir.to_path_buf(),
            apns: None,
            ntfy: None,
            config_path: Some(path.clone()),
            agents_path: dir.join("agents.json"),
            watch_path: dir.join("watch.json"),
            desktop: None,
            meetings: None,
        });
        relay.access.wait_for_phone(true);
        (relay, path)
    }

    #[test]
    fn the_first_phone_takes_the_relay_onto_the_network_for_good() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());

        relay.phone_reached(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(relay.waiting_for_phone(), "this computer is not a phone");
        assert!(!relay.phone_seen());
        assert!(Config::load(&path).unwrap().local_until_paired);

        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(100, 84, 12, 7)));
        assert!(!relay.waiting_for_phone());
        assert!(relay.phone_seen());
        let saved = Config::load(&path).unwrap();
        assert!(!saved.local_until_paired);
        assert_eq!(saved.bind_address(), "0.0.0.0:8787");
        assert_eq!(saved.secret, "s3cret", "the rest of relay.toml is kept");
    }

    /// Without a Wi-Fi or Tailscale address (CI) this only checks that `/pair` still opens.
    #[tokio::test]
    async fn the_pairing_page_lets_a_phone_in_and_the_phone_keeps_it_open() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut config = Config::load(&path).unwrap();
        config.listen = format!("0.0.0.0:{port}");
        config.save(&path).unwrap();
        tokio::spawn(crate::serve(
            listener,
            relay.clone(),
            config,
            std::future::pending(),
        ));
        let http = reqwest::Client::new();
        let lan = crate::network::pair_ips().into_iter().next();
        let status = |ip: IpAddr| {
            http.get(format!(
                "http://{ip}:{port}{}",
                kumo_core::desktop::routes::STATUS
            ))
            .bearer_auth("s3cret")
            .send()
        };

        if let Some(ip) = lan {
            assert!(
                status(ip).await.is_err(),
                "nothing listens on {ip} before pairing"
            );
        }
        let page = http
            .get(format!("http://127.0.0.1:{port}/pair"))
            .send()
            .await
            .unwrap();
        assert!(page.status().is_success());
        let Some(ip) = lan else { return };

        status(ip)
            .await
            .expect("the pairing page opened this address");
        assert!(!relay.waiting_for_phone());
        assert!(!Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn a_pairing_that_could_not_be_saved_is_saved_by_the_next_request() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        let text = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let phone = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));

        relay.phone_reached(phone);
        assert!(relay.waiting_for_phone());

        std::fs::write(&path, text).unwrap();
        relay.phone_reached(phone);
        assert!(!relay.waiting_for_phone());
        assert!(!Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn a_mapped_loopback_peer_is_this_computer() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, _) = relay_waiting(dir.path());
        relay.phone_reached("::ffff:127.0.0.1".parse().unwrap());
        assert!(relay.waiting_for_phone());
    }

    #[test]
    fn a_relay_already_on_the_network_leaves_relay_toml_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        relay.access.wait_for_phone(false);
        let before = std::fs::read(&path).unwrap();
        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(
            relay.phone_seen(),
            "a phone of an older install still counts"
        );
    }
}
