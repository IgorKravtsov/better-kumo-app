use std::collections::BTreeMap;
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
}

pub struct Webhooks {
    path: PathBuf,
    boot: String,
    http: reqwest::Client,
    registry: Mutex<Registry>,
    registered: watch::Sender<u64>,
}

pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let tag = hmac::sign(&key, body);
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256={hex}")
}

impl Webhooks {
    pub fn load(path: PathBuf, boot: &str) -> Self {
        let saved: BTreeMap<String, Registration> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
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
                .unwrap_or_default(),
            registry: Mutex::new(Registry { hooks, seq: 0 }),
            registered: watch::Sender::new(0),
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

    pub fn publish(&self, delivery: &Delivery) {
        let Ok(mut registry) = self.registry.lock() else {
            return;
        };
        if registry.hooks.is_empty() {
            return;
        }
        registry.seq += 1;
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
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (body, seq) = (Arc::new(body), registry.seq);
        for (id, hook) in &mut registry.hooks {
            if hook.queue.is_none() {
                let (tx, rx) = mpsc::channel(BACKLOG);
                hook.worker = Some(runtime.spawn(work(
                    id.clone(),
                    self.http.clone(),
                    hook.registration.clone(),
                    rx,
                )));
                hook.queue = Some(tx);
            }
            let queue = hook.queue.as_ref().expect("set above");
            if queue.try_send(body.clone()).is_err() {
                eprintln!("webhook {id}: receiver is behind, dropped seq {seq}");
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
    written
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
