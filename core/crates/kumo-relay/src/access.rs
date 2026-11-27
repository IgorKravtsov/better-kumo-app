//! Who can reach the Relay over the network.
//!
//! A relay that Better Kumo set up answers only this computer until a phone pairs
//! (`Config::local_until_paired`). Opening the pairing page adds a listener on each of this
//! computer's network addresses, so a phone can scan a code and reach it; the first request that
//! comes from another device with the secret ends the wait for good (saved to relay.toml), and the
//! next start listens on `listen` as written.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{Config, Relay};

#[derive(Default)]
pub struct NetworkAccess {
    /// No phone has paired yet.
    waiting: AtomicBool,
    /// The main listener is on loopback; stays set until the Relay restarts.
    loopback_only: AtomicBool,
    /// Network addresses that got a pairing listener in this run.
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

    /// The pairing page opened: serve this computer's network addresses too, so a phone can reach
    /// the Relay. Addresses that appeared since the last time (another Wi-Fi) are added.
    pub(crate) async fn open_for_pairing(self: &Arc<Self>, config: &Config) {
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

    /// A request with the secret came from `peer`. From another device, that is a paired phone
    /// (or another app given the link): the Relay stops waiting and listens on the network from
    /// its next start.
    pub(crate) fn phone_reached(&self, peer: IpAddr) {
        if peer.is_loopback() || !self.access.waiting.swap(false, Ordering::SeqCst) {
            return;
        }
        eprintln!("kumo-relay: a phone reached the Relay from {peer}; it stays on the network");
        if let Some(path) = &self.config_path {
            let saved = Config::load(path).and_then(|mut config| {
                config.local_until_paired = false;
                config.save(path)
            });
            if let Err(e) = saved {
                eprintln!("kumo-relay: cannot save that a phone paired: {e:#}");
            }
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
        assert!(Config::load(&path).unwrap().local_until_paired);

        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(100, 84, 12, 7)));
        assert!(!relay.waiting_for_phone());
        let saved = Config::load(&path).unwrap();
        assert!(!saved.local_until_paired);
        assert_eq!(saved.bind_address(), "0.0.0.0:8787");
        assert_eq!(saved.secret, "s3cret", "the rest of relay.toml is kept");
    }

    /// Over real sockets. Needs a Wi-Fi or Tailscale address on this computer; without one there
    /// is nothing to pair over, and the test only checks that `/pair` still opens.
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
    fn a_relay_already_on_the_network_leaves_relay_toml_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        relay.access.wait_for_phone(false);
        let before = std::fs::read(&path).unwrap();
        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
