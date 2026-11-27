use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aws_lc_rs::hmac;
use kumo_core::desktop::{DesktopState, RelayError, WebhookInfo, WebhookRequest};
use kumo_core::events::LoggedEvent;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

pub const TIMEOUT: Duration = Duration::from_secs(3);
pub const SIGNATURE_HEADER: &str = "x-kumo-signature";
/// Deliveries a slow receiver may fall behind by before new ones are dropped; polling covers them.
const BACKLOG: usize = 64;
/// Chat may fill only part of a queue, so a burst of messages cannot crowd out an office event.
const CHAT_BACKLOG: usize = BACKLOG / 2;
pub const MAX_WEBHOOKS: usize = 16;
/// Failed reads of an attached page in a row that are retried before it counts as disconnected.
pub const TRANSIENT_READS: u32 = 3;

#[derive(Debug, Clone, Copy)]
pub struct WebhookTiming {
    pub debounce: Duration,
    pub poll: Duration,
}

impl Default for WebhookTiming {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(250),
            poll: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Delivery {
    Desktop(DesktopState),
    Event(LoggedEvent),
    Disconnected,
}

#[derive(Serialize)]
struct Body<'a> {
    boot: &'a str,
    seq: u64,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    desktop: Option<&'a DesktopState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<&'a LoggedEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Registration {
    urls: Vec<String>,
    secret: String,
}

struct Hook {
    registration: Registration,
    queue: Option<mpsc::Sender<Arc<String>>>,
    worker: Option<tokio::task::JoinHandle<()>>,
}

impl Hook {
    fn new(registration: Registration) -> Self {
        Self {
            registration,
            queue: None,
            worker: None,
        }
    }
}

/// A removed or replaced webhook must not keep posting its queue with the old URLs and secret.
impl Drop for Hook {
    fn drop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.abort();
        }
    }
}

#[derive(Default)]
struct Registry {
    hooks: BTreeMap<String, Hook>,
    seq: u64,
    stopped: bool,
}

pub struct Webhooks {
    path: PathBuf,
    boot: String,
    http: Option<reqwest::Client>,
    registry: Mutex<Registry>,
    registered: watch::Sender<u64>,
    writable: bool,
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

impl Webhooks {
    pub fn load(path: PathBuf, boot: &str) -> Self {
        let mut writable = true;
        let saved: BTreeMap<String, Registration> = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
        {
            Ok(saved) => saved,
            Err(_) if !path.exists() => BTreeMap::new(),
            Err(e) => {
                // Set aside so the next registration can't overwrite what may still be recovered by hand.
                let aside = path.with_extension(format!("json.bad-{}", kumo_core::http::now_ms()));
                match std::fs::rename(&path, &aside) {
                    Ok(()) => eprintln!(
                        "webhooks: could not read {} ({e}), moved to {}",
                        path.display(),
                        aside.display()
                    ),
                    Err(moving) => {
                        eprintln!(
                            "webhooks: could not read {} ({e}) or move it aside ({moving}); not saving webhooks",
                            path.display()
                        );
                        writable = false;
                    }
                }
                BTreeMap::new()
            }
        };
        let hooks = saved
            .into_iter()
            .map(|(id, registration)| (id, Hook::new(registration)))
            .collect();
        Self {
            path,
            boot: boot.to_string(),
            http: reqwest::Client::builder()
                .user_agent(concat!("better-kumo-relay/", env!("CARGO_PKG_VERSION")))
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .inspect_err(|e| {
                    eprintln!("webhooks: no HTTP client ({e}), nothing will be posted")
                })
                .ok(),
            registry: Mutex::new(Registry {
                hooks,
                seq: 0,
                stopped: false,
            }),
            registered: watch::Sender::new(0),
            writable,
        }
    }

    pub fn registered(&self) -> watch::Receiver<u64> {
        self.registered.subscribe()
    }

    pub fn is_empty(&self) -> bool {
        self.registry.lock().map_or(true, |r| r.hooks.is_empty())
    }

    pub fn list(&self) -> Vec<WebhookInfo> {
        self.registry
            .lock()
            .map(|r| {
                r.hooks
                    .iter()
                    .map(|(id, hook)| WebhookInfo {
                        id: id.clone(),
                        urls: hook.registration.urls.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn register(&self, id: &str, request: WebhookRequest) -> Result<WebhookInfo, RelayError> {
        request.validate(id)?;
        let registration = Registration {
            urls: request.urls,
            secret: request.secret,
        };
        let mut registry = self.lock()?;
        if !registry.hooks.contains_key(id) && registry.hooks.len() >= MAX_WEBHOOKS {
            return Err(RelayError::Rejected(format!(
                "at most {MAX_WEBHOOKS} webhooks; remove one first"
            )));
        }
        // Clients re-register on a timer; an unchanged one keeps its worker and the URL that last worked.
        if registry
            .hooks
            .get(id)
            .is_none_or(|h| h.registration != registration)
        {
            let previous = registry
                .hooks
                .insert(id.to_string(), Hook::new(registration.clone()));
            if let Err(e) = self.save(&registry) {
                match previous {
                    Some(previous) => registry.hooks.insert(id.to_string(), previous),
                    None => registry.hooks.remove(id),
                };
                return Err(e);
            }
            self.registered.send_modify(|n| *n = n.wrapping_add(1));
        }
        Ok(WebhookInfo {
            id: id.to_string(),
            urls: registration.urls,
        })
    }

    pub fn remove(&self, id: &str) -> Result<(), RelayError> {
        let mut registry = self.lock()?;
        if let Some(removed) = registry.hooks.remove(id)
            && let Err(e) = self.save(&registry)
        {
            registry.hooks.insert(id.to_string(), removed);
            return Err(e);
        }
        Ok(())
    }

    pub fn publish(&self, delivery: &Delivery) -> BTreeSet<String> {
        self.publish_to(delivery, None)
    }

    /// Delivers to every receiver, or to `only` those; returns the ones whose full queue dropped it.
    pub fn publish_to(
        &self,
        delivery: &Delivery,
        only: Option<&BTreeSet<String>>,
    ) -> BTreeSet<String> {
        let mut dropped = BTreeSet::new();
        let Ok(mut registry) = self.registry.lock() else {
            return dropped;
        };
        if registry.hooks.is_empty() || registry.stopped {
            return dropped;
        }
        registry.seq += 1;
        let chat = matches!(delivery, Delivery::Event(e) if !e.event.kind.is_office());
        let (kind, desktop, event) = match delivery {
            Delivery::Desktop(state) => ("desktop", Some(state), None),
            Delivery::Event(event) => ("event", None, Some(event)),
            Delivery::Disconnected => ("disconnected", None, None),
        };
        let Ok(body) = serde_json::to_string(&Body {
            boot: &self.boot,
            seq: registry.seq,
            kind,
            desktop,
            event,
        }) else {
            return dropped;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return dropped;
        };
        let Some(http) = &self.http else {
            return dropped;
        };
        let (body, seq) = (Arc::new(body), registry.seq);
        for (id, hook) in &mut registry.hooks {
            if only.is_some_and(|only| !only.contains(id)) {
                continue;
            }
            if hook.queue.is_none() {
                let (tx, rx) = mpsc::channel(BACKLOG);
                hook.worker = Some(runtime.spawn(work(
                    id.clone(),
                    http.clone(),
                    hook.registration.clone(),
                    rx,
                )));
                hook.queue = Some(tx);
            }
            let queue = hook.queue.as_ref().expect("set above");
            let room = queue.capacity() > if chat { BACKLOG - CHAT_BACKLOG } else { 0 };
            if !room || queue.try_send(body.clone()).is_err() {
                eprintln!("webhook {id}: receiver is behind, dropped seq {seq}");
                dropped.insert(id.clone());
            }
        }
        dropped
    }

    /// Drops queued deliveries and accepts no more once the Relay stops.
    pub fn stop(&self) {
        if let Ok(mut registry) = self.registry.lock() {
            registry.stopped = true;
            for hook in registry.hooks.values_mut() {
                hook.queue = None;
                if let Some(worker) = hook.worker.take() {
                    worker.abort();
                }
            }
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Registry>, RelayError> {
        self.registry
            .lock()
            .map_err(|_| RelayError::Unavailable("webhook registry is poisoned".into()))
    }

    fn save(&self, registry: &Registry) -> Result<(), RelayError> {
        let saved: BTreeMap<&String, &Registration> = registry
            .hooks
            .iter()
            .map(|(id, hook)| (id, &hook.registration))
            .collect();
        let failed = |e: String| RelayError::Http {
            status: 500,
            message: format!("could not save webhooks: {e}"),
        };
        if !self.writable {
            return Err(failed(format!(
                "{} is unreadable and could not be moved aside",
                self.path.display()
            )));
        }
        let text = serde_json::to_string_pretty(&saved).map_err(|e| failed(e.to_string()))?;
        write_private(&self.path, &text).map_err(|e| failed(e.to_string()))
    }
}

/// The file holds signing secrets, so it is readable by the owner only, like `relay.toml`.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    });
    let written = written.and_then(|()| std::fs::rename(&tmp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    // Best effort: the rename already took effect, so a failure here must not be reported as a failed save.
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let _ = std::fs::File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

async fn work(
    id: String,
    http: reqwest::Client,
    registration: Registration,
    mut queue: mpsc::Receiver<Arc<String>>,
) {
    let mut working = 0;
    while let Some(body) = queue.recv().await {
        let signature = sign(&registration.secret, body.as_bytes());
        match deliver(&http, &registration.urls, working, &body, &signature).await {
            Some(index) => working = index,
            None => eprintln!("webhook {id}: no URL accepted the delivery"),
        }
    }
}

pub async fn deliver(
    http: &reqwest::Client,
    urls: &[String],
    start: usize,
    body: &str,
    signature: &str,
) -> Option<usize> {
    for index in (0..urls.len()).map(|n| (start + n) % urls.len()) {
        let accepted = http
            .post(&urls[index])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(SIGNATURE_HEADER, signature)
            .body(body.to_string())
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        if accepted {
            return Some(index);
        }
    }
    None
}

/// Positions change several times a second while anyone walks and must not trigger deliveries on their own.
pub fn unmoved(mut state: DesktopState) -> DesktopState {
    if let Some(me) = &mut state.me {
        (me.x, me.y) = (0.0, 0.0);
    }
    for person in &mut state.people {
        (person.x, person.y) = (0.0, 0.0);
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use kumo_core::events::{EventKind, OfficeEvent, Who};

    #[test]
    fn signature_is_hex_hmac_sha256_of_the_body() {
        // RFC 4231, test case 2.
        assert_eq!(
            sign("Jefe", b"what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    fn request(url: &str, secret: &str) -> WebhookRequest {
        WebhookRequest {
            urls: vec![url.into()],
            secret: secret.into(),
        }
    }

    #[test]
    fn registrations_survive_a_restart_in_a_private_file_and_never_list_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("webhooks.json");
        let secret = "s".repeat(64);
        let hooks = Webhooks::load(path.clone(), "boot-1");
        hooks
            .register(
                "entry-1",
                request("http://ha.local:8123/api/webhook/a", &secret),
            )
            .unwrap();
        hooks
            .register(
                "entry-2",
                request("https://ha.example.com/api/webhook/b", &secret),
            )
            .unwrap();
        hooks.remove("entry-2").unwrap();
        hooks.remove("never-registered").unwrap();

        let restarted = Webhooks::load(path.clone(), "boot-2");
        let listed = restarted.list();
        assert_eq!(
            listed,
            vec![WebhookInfo {
                id: "entry-1".into(),
                urls: vec!["http://ha.local:8123/api/webhook/a".into()],
            }]
        );
        assert!(!serde_json::to_string(&listed).unwrap().contains(&secret));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    fn chat() -> Delivery {
        Delivery::Event(LoggedEvent {
            seq: 1,
            event: OfficeEvent::new(EventKind::Message, 0, Who::default()),
        })
    }

    #[tokio::test]
    async fn publish_reports_receivers_a_full_queue_dropped_and_retries_only_those() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot-1");
        let secret = "s".repeat(64);
        hooks
            .register("slow", request("http://127.0.0.1:9/a", &secret))
            .unwrap();
        // The worker cannot run before this test yields, so the queue only fills.
        for _ in 0..BACKLOG {
            assert!(hooks.publish(&Delivery::Disconnected).is_empty());
        }
        hooks
            .register("fresh", request("http://127.0.0.1:9/b", &secret))
            .unwrap();
        let dropped = hooks.publish(&Delivery::Disconnected);
        assert_eq!(dropped, BTreeSet::from(["slow".to_string()]));
        hooks.publish_to(&Delivery::Disconnected, Some(&dropped));
        let registry = hooks.registry.lock().unwrap();
        let fresh = registry.hooks["fresh"].queue.as_ref().unwrap();
        assert_eq!(fresh.capacity(), BACKLOG - 1);
        drop(registry);
        hooks.stop();
    }

    #[tokio::test]
    async fn chat_cannot_crowd_office_events_out_of_a_queue() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot-1");
        hooks
            .register("slow", request("http://127.0.0.1:9/a", &"s".repeat(64)))
            .unwrap();
        for _ in 0..CHAT_BACKLOG {
            assert!(hooks.publish(&chat()).is_empty());
        }
        assert!(!hooks.publish(&chat()).is_empty());
        for _ in CHAT_BACKLOG..BACKLOG {
            assert!(hooks.publish(&Delivery::Disconnected).is_empty());
        }
        hooks.stop();
    }

    #[test]
    fn a_bad_registration_is_rejected_and_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot");
        assert!(matches!(
            hooks.register("entry", request("ftp://x", &"s".repeat(64))),
            Err(RelayError::Rejected(_))
        ));
        assert!(hooks.is_empty());
        assert!(!dir.path().join("webhooks.json").exists());
    }

    #[test]
    fn re_registering_the_same_webhook_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot");
        let registered = hooks.registered();
        let secret = "s".repeat(64);
        hooks
            .register("entry", request("http://ha.local/a", &secret))
            .unwrap();
        assert!(registered.has_changed().unwrap());
        let mut registered = hooks.registered();
        hooks
            .register("entry", request("http://ha.local/a", &secret))
            .unwrap();
        assert!(!registered.has_changed().unwrap());
        hooks
            .register("entry", request("http://ha.local/b", &secret))
            .unwrap();
        assert!(registered.has_changed().unwrap());
        registered.mark_unchanged();
    }

    #[test]
    fn new_ids_stop_at_the_limit_while_existing_ones_still_update() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot");
        let secret = "s".repeat(64);
        for n in 0..MAX_WEBHOOKS {
            hooks
                .register(&format!("e{n}"), request("http://ha.local/a", &secret))
                .unwrap();
        }
        assert!(matches!(
            hooks.register("one-more", request("http://ha.local/a", &secret)),
            Err(RelayError::Rejected(_))
        ));
        hooks
            .register("e0", request("http://ha.local/b", &secret))
            .unwrap();
        hooks.remove("e1").unwrap();
        hooks
            .register("one-more", request("http://ha.local/a", &secret))
            .unwrap();
    }

    #[test]
    fn an_unreadable_registry_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("webhooks.json");
        std::fs::write(&path, "{ not json").unwrap();
        let hooks = Webhooks::load(path.clone(), "boot");
        assert!(hooks.is_empty());
        hooks
            .register("entry", request("http://ha.local/a", &"s".repeat(64)))
            .unwrap();
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains("webhooks.json.bad-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "{ not json");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_registry_that_cannot_be_moved_is_never_overwritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("webhooks.json");
        std::fs::write(&path, "{ not json").unwrap();
        let readonly = std::fs::Permissions::from_mode(0o500);
        std::fs::set_permissions(dir.path(), readonly).unwrap();
        // Root ignores the directory's mode, so the rename can't be made to fail there.
        if std::fs::write(dir.path().join("probe"), "").is_ok() {
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            return;
        }
        let hooks = Webhooks::load(path.clone(), "boot");
        let registered = hooks.register("entry", request("http://ha.local/a", &"s".repeat(64)));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(
            registered,
            Err(RelayError::Http { status: 500, .. })
        ));
        assert!(hooks.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[tokio::test]
    async fn a_stopped_registry_queues_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let hooks = Webhooks::load(dir.path().join("webhooks.json"), "boot");
        hooks
            .register("entry", request("http://127.0.0.1:9/a", &"s".repeat(64)))
            .unwrap();
        hooks.stop();
        hooks.publish(&Delivery::Disconnected);
        let registry = hooks.registry.lock().unwrap();
        assert_eq!(registry.seq, 0);
        assert!(registry.hooks["entry"].worker.is_none());
    }
}
