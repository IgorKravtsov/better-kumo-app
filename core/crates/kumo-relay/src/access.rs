//! A relay that Better Kumo set up answers only this computer until a phone pairs
//! (`Config::local_until_paired`, docs/architecture.md "Trust boundaries").

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::network::PairAddress;
use crate::{Config, Relay};

pub struct NetworkAccess {
    waiting: AtomicBool,
    /// Stays set after a phone pairs: the main listener only moves to `listen` on the next start.
    loopback_only: AtomicBool,
    /// Addresses a pairing listener was started for, bound or still binding.
    opened: Mutex<HashSet<IpAddr>>,
    /// Addresses whose pairing listener bound; only these get a code, since a code for an address
    /// another program holds would send it the secret.
    live: Mutex<HashSet<IpAddr>>,
    /// Where the main listener is bound, once `run` bound it.
    main: Mutex<Option<SocketAddr>>,
    closing: watch::Sender<bool>,
    listeners: Mutex<Vec<JoinHandle<()>>>,
}

impl Default for NetworkAccess {
    fn default() -> Self {
        Self {
            waiting: AtomicBool::default(),
            loopback_only: AtomicBool::default(),
            opened: Mutex::default(),
            live: Mutex::default(),
            main: Mutex::default(),
            closing: watch::Sender::new(false),
            listeners: Mutex::default(),
        }
    }
}

impl NetworkAccess {
    pub(crate) fn listening_on(&self, main: SocketAddr) {
        if let Ok(mut bound) = self.main.lock() {
            *bound = Some(main);
        }
    }

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

    /// The addresses a phone may be given: all of them once the Relay listens on `listen`, else
    /// only those with a pairing listener.
    /// A code may name only an address something of this Relay listens on: every address while the
    /// main listener is the wildcard, else the main one's and the pairing listeners'.
    fn reachable(&self, addresses: Vec<PairAddress>) -> Vec<PairAddress> {
        let main = self.main.lock().ok().and_then(|m| *m);
        let wildcard = main.is_none_or(|m| m.ip().to_canonical().is_unspecified());
        if wildcard && !self.loopback_only.load(Ordering::SeqCst) {
            return addresses;
        }
        let mut live = self.live.lock().map(|l| l.clone()).unwrap_or_default();
        live.extend(main.map(|m| m.ip()));
        addresses
            .into_iter()
            .filter(|a| {
                a.url
                    .strip_prefix("http://")
                    .and_then(|rest| rest.rsplit_once(':'))
                    .and_then(|(host, _)| host.parse::<IpAddr>().ok())
                    .is_some_and(|ip| live.contains(&ip))
            })
            .collect()
    }
}

impl Relay {
    pub fn waiting_for_phone(&self) -> bool {
        self.access.waiting_for_phone()
    }

    /// Re-run on every visit to `/pair` so an address that appeared since (another Wi-Fi,
    /// Tailscale) is added.
    pub fn pair_addresses(&self, port: u16) -> Vec<PairAddress> {
        self.access.reachable(crate::network::pair_addresses(port))
    }

    pub(crate) async fn open_for_pairing(self: &Arc<Self>, config: &Config) {
        if !self.access.loopback_only.load(Ordering::SeqCst) {
            return;
        }
        let port = config.port();
        for ip in self.access.claim(crate::network::pair_ips()) {
            let bound = tokio::net::TcpListener::bind(SocketAddr::new(ip, port)).await;
            // A /pair request taken before the Relay began to stop must not open a new address.
            if *self.access.closing.borrow() {
                self.access.release(ip);
                continue;
            }
            match bound {
                Ok(listener) => {
                    eprintln!("kumo-relay: listening on {ip}:{port} so a phone can pair");
                    let relay = self.clone();
                    let config = config.clone();
                    let mut closing = self.access.closing.subscribe();
                    let task = tokio::spawn(async move {
                        let closed = async move {
                            let _ = closing.wait_for(|closed| *closed).await;
                        };
                        if let Err(e) = crate::serve(listener, relay, config, closed).await {
                            eprintln!("kumo-relay: stopped listening on {ip}:{port}: {e}");
                        }
                    });
                    if let Ok(mut listeners) = self.access.listeners.lock() {
                        listeners.push(task);
                    }
                    if let Ok(mut live) = self.access.live.lock() {
                        live.insert(ip);
                    }
                }
                Err(e) => {
                    eprintln!("kumo-relay: cannot listen on {ip}:{port} for pairing: {e}");
                    self.access.release(ip);
                }
            }
        }
    }

    /// Stops the pairing listeners and waits for their requests, so none runs while the Relay
    /// tears down what they use.
    pub(crate) async fn close_pairing_listeners(&self) {
        self.stop_pairing_listeners();
        let listeners = self
            .access
            .listeners
            .lock()
            .map(|mut l| std::mem::take(&mut *l))
            .unwrap_or_default();
        for listener in listeners {
            let _ = listener.await;
        }
    }

    /// `peer` sent the secret. Another device doing so is a paired phone (or another app given the
    /// link), so the next start listens on the network. Until that is saved, every such request
    /// tries again.
    /// Tells the pairing listeners to stop taking connections, without waiting for them.
    pub(crate) fn stop_pairing_listeners(&self) {
        self.access.closing.send_replace(true);
    }

    /// `secret` is the one the request was checked against: a relay.toml with another secret
    /// (changed since the start) is left waiting.
    pub(crate) fn phone_reached(&self, peer: IpAddr, secret: &str) {
        if peer.to_canonical().is_loopback() || !self.access.waiting_for_phone() {
            return;
        }
        // This computer reaching itself over its Wi-Fi or Tailscale address is not a phone.
        if crate::network::is_own_address(peer) {
            return;
        }
        let mut same_secret = true;
        let saved = self.update_config_if(|config| {
            same_secret = config.secret == secret;
            if same_secret {
                config.local_until_paired = false;
            }
            same_secret
        });
        if let Err(e) = saved {
            eprintln!("kumo-relay: cannot save that a phone paired: {e:#}");
            return;
        }
        if !same_secret {
            eprintln!("kumo-relay: relay.toml has another secret now; still waiting for a phone");
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

        relay.phone_reached(IpAddr::V4(Ipv4Addr::LOCALHOST), "s3cret");
        assert!(relay.waiting_for_phone(), "this computer is not a phone");
        assert!(Config::load(&path).unwrap().local_until_paired);

        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)), "s3cret");
        assert!(!relay.waiting_for_phone());
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
        let status = |ip: IpAddr| {
            http.get(format!(
                "http://{ip}:{port}{}",
                kumo_core::desktop::routes::STATUS
            ))
            .bearer_auth("s3cret")
            .send()
        };

        let mut closed_before = HashSet::new();
        for ip in crate::network::pair_ips() {
            if status(ip).await.is_err() {
                closed_before.insert(ip);
            }
        }
        let page = http
            .get(format!("http://127.0.0.1:{port}/pair"))
            .send()
            .await
            .unwrap();
        assert!(page.status().is_success());
        if closed_before.is_empty() {
            return; // no Wi-Fi or Tailscale address free on this port: nothing to pair over
        }
        // An address another program holds on this port gets no listener and no code.
        let opened = relay.pair_addresses(port);
        let Some(ip) = opened
            .first()
            .and_then(|a| a.url.strip_prefix("http://"))
            .and_then(|rest| rest.rsplit_once(':'))
            .and_then(|(host, _)| host.parse::<IpAddr>().ok())
        else {
            panic!("/pair opened none of the free addresses {closed_before:?}");
        };
        assert!(
            closed_before.contains(&ip),
            "nothing listened on {ip} before pairing"
        );

        status(ip)
            .await
            .expect("the pairing page opened this address");
        assert!(
            relay.waiting_for_phone(),
            "this computer reaching its own address is not a phone"
        );
        assert!(Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn a_pairing_that_could_not_be_saved_is_saved_by_the_next_request() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        let text = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let phone = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20));

        relay.phone_reached(phone, "s3cret");
        assert!(relay.waiting_for_phone());

        std::fs::write(&path, text).unwrap();
        relay.phone_reached(phone, "s3cret");
        assert!(!relay.waiting_for_phone());
        assert!(!Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn a_phone_checked_against_an_old_secret_does_not_open_a_changed_relay_toml() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        let mut changed = Config::load(&path).unwrap();
        changed.secret = "rotated".into();
        changed.save(&path).unwrap();

        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20)), "s3cret");
        assert!(relay.waiting_for_phone());
        assert!(Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn while_waiting_only_addresses_with_a_listener_get_a_code() {
        let access = NetworkAccess::default();
        let address = |label, ip: &str| PairAddress {
            label,
            url: format!("http://{ip}:8787"),
        };
        let all = vec![
            address("Tailscale", "100.84.12.7"),
            address("Same Wi-Fi", "192.168.1.20"),
        ];
        assert_eq!(
            access.reachable(all.clone()),
            all,
            "on the network: every address"
        );

        access.wait_for_phone(true);
        assert_eq!(access.reachable(all.clone()), vec![]);
        let wifi = "192.168.1.20".parse().unwrap();
        access.claim([wifi]);
        assert_eq!(access.reachable(all.clone()), vec![], "still binding");
        access.live.lock().unwrap().insert(wifi);
        assert_eq!(access.reachable(all.clone()), vec![all[1].clone()]);
    }

    #[tokio::test]
    async fn only_the_secret_from_another_device_counts_as_a_phone() {
        use axum::extract::connect_info::MockConnectInfo;
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        let config = Config::load(&path).unwrap();
        let send = |peer: &str, bearer: Option<&str>| {
            let app = crate::server::router(relay.clone(), config.clone())
                .layer(MockConnectInfo(peer.parse::<SocketAddr>().unwrap()));
            let mut request = axum::http::Request::get(kumo_core::desktop::routes::STATUS);
            if let Some(bearer) = bearer {
                request = request.header("authorization", format!("Bearer {bearer}"));
            }
            app.oneshot(request.body(axum::body::Body::empty()).unwrap())
        };

        send("198.51.100.20:50000", None).await.unwrap();
        send("198.51.100.20:50000", Some("wrong")).await.unwrap();
        send("127.0.0.1:50000", Some("s3cret")).await.unwrap();
        assert!(relay.waiting_for_phone());
        assert!(Config::load(&path).unwrap().local_until_paired);

        send("198.51.100.20:50000", Some("s3cret")).await.unwrap();
        assert!(!relay.waiting_for_phone());
        assert!(!Config::load(&path).unwrap().local_until_paired);
    }

    #[test]
    fn a_relay_bound_to_one_address_gives_codes_only_for_it() {
        let access = NetworkAccess::default();
        let address = |label, ip: &str| PairAddress {
            label,
            url: format!("http://{ip}:8787"),
        };
        let all = vec![
            address("Tailscale", "100.84.12.7"),
            address("Same Wi-Fi", "192.168.1.20"),
        ];

        access.listening_on("127.0.0.1:8787".parse().unwrap());
        assert_eq!(access.reachable(all.clone()), vec![], "listen = 127.0.0.1");
        access.listening_on("100.84.12.7:8787".parse().unwrap());
        assert_eq!(access.reachable(all.clone()), vec![all[0].clone()]);
        access.listening_on("0.0.0.0:8787".parse().unwrap());
        assert_eq!(access.reachable(all.clone()), all);
    }

    #[tokio::test]
    async fn a_stopping_relay_opens_no_new_pairing_listener() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        relay.stop_pairing_listeners();
        relay.open_for_pairing(&Config::load(&path).unwrap()).await;
        assert!(relay.access.listeners.lock().unwrap().is_empty());
        assert!(relay.access.live.lock().unwrap().is_empty());
    }

    #[test]
    fn a_mapped_loopback_peer_is_this_computer() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, _) = relay_waiting(dir.path());
        relay.phone_reached("::ffff:127.0.0.1".parse().unwrap(), "s3cret");
        assert!(relay.waiting_for_phone());
    }

    #[test]
    fn a_relay_already_on_the_network_leaves_relay_toml_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, path) = relay_waiting(dir.path());
        relay.access.wait_for_phone(false);
        let before = std::fs::read(&path).unwrap();
        relay.phone_reached(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20)), "s3cret");
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
