use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::alert::AlertKind;
use super::notifications::{MyStatus, NotificationKinds, NotificationProfile};
use crate::error::{CoreError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDelivery {
    Ntfy,
    #[default]
    Phone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    delivery: Option<NotificationDelivery>,
    settings: NotificationProfile,
    pending_sync: bool,
}

pub struct DeliveryStore {
    path: PathBuf,
    prefs: Mutex<Prefs>,
}

impl DeliveryStore {
    pub fn open(path: PathBuf) -> Self {
        let prefs = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            prefs: Mutex::new(prefs),
        }
    }

    pub fn delivery(&self) -> NotificationDelivery {
        self.snapshot().delivery.unwrap_or_default()
    }

    pub fn set_delivery(&self, delivery: NotificationDelivery) -> Result<()> {
        self.update(|p| p.delivery = Some(delivery))
    }

    pub fn profile(&self) -> NotificationProfile {
        self.snapshot().settings
    }

    pub fn pending_sync(&self) -> bool {
        self.snapshot().pending_sync
    }

    pub fn set_kinds(
        &self,
        status: MyStatus,
        kinds: NotificationKinds,
        pending: bool,
    ) -> Result<()> {
        self.update(|p| {
            p.settings = p.settings.with_kinds(status, kinds);
            p.pending_sync |= pending;
        })
    }

    pub fn adopt_relay(&self, relay: NotificationProfile) -> Result<bool> {
        let mut adopted = false;
        self.update(|p| {
            if !p.pending_sync {
                p.settings = relay;
                adopted = true;
            }
        })?;
        Ok(adopted)
    }

    pub fn synced(&self, sent: NotificationProfile, relay: NotificationProfile) -> Result<()> {
        self.update(|p| {
            if p.settings == sent {
                p.settings = relay;
                p.pending_sync = false;
            }
        })
    }

    pub fn allows_local(&self, status: MyStatus, kind: AlertKind) -> bool {
        let prefs = self.snapshot();
        prefs.delivery == Some(NotificationDelivery::Ntfy) || prefs.settings.allows(status, kind)
    }

    fn snapshot(&self) -> Prefs {
        self.prefs.lock().map(|p| *p).unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut Prefs)) -> Result<()> {
        let mut prefs = self
            .prefs
            .lock()
            .map_err(|_| CoreError::Storage("poisoned".into()))?;
        let mut next = *prefs;
        change(&mut next);
        self.write(&next)?;
        *prefs = next;
        Ok(())
    }

    fn write(&self, prefs: &Prefs) -> Result<()> {
        let storage = |e: std::io::Error| CoreError::Storage(e.to_string());
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(storage)?;
        }
        let tmp = self.path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_string_pretty(prefs)?).map_err(storage)?;
        fs::rename(&tmp, &self.path).map_err(storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "kumo-delivery-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("notifications.json")
    }

    #[test]
    fn delivery_is_this_phone_until_chosen() {
        let path = temp_path("unset");
        let store = DeliveryStore::open(path.clone());
        assert_eq!(store.delivery(), NotificationDelivery::Phone);
        assert!(!path.exists());

        store.set_delivery(NotificationDelivery::Ntfy).unwrap();
        assert_eq!(
            DeliveryStore::open(path).delivery(),
            NotificationDelivery::Ntfy
        );
    }

    fn quiet() -> NotificationKinds {
        NotificationKinds {
            nudges: false,
            public_chat: false,
            ..NotificationKinds::default()
        }
    }

    #[test]
    fn delivery_and_profile_persist() {
        let path = temp_path("persist");
        let store = DeliveryStore::open(path.clone());
        store.set_delivery(NotificationDelivery::Phone).unwrap();
        store.set_kinds(MyStatus::Focusing, quiet(), true).unwrap();

        let reopened = DeliveryStore::open(path);
        assert_eq!(reopened.delivery(), NotificationDelivery::Phone);
        assert_eq!(
            reopened.profile(),
            NotificationProfile::default().with_kinds(MyStatus::Focusing, quiet())
        );
        assert!(reopened.pending_sync());
    }

    #[test]
    fn missing_or_broken_file_means_defaults() {
        let path = temp_path("broken");
        assert_eq!(
            DeliveryStore::open(path.clone()).profile(),
            NotificationProfile::default()
        );
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        let store = DeliveryStore::open(path.clone());
        assert_eq!(store.profile(), NotificationProfile::default());
        assert_eq!(store.delivery(), NotificationDelivery::Phone);
        assert!(!store.pending_sync());
    }

    #[test]
    fn a_flat_settings_file_applies_to_every_status() {
        let path = temp_path("flat");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"delivery":"ntfy","settings":{"knocks":false}}"#).unwrap();
        let store = DeliveryStore::open(path);
        assert_eq!(store.delivery(), NotificationDelivery::Ntfy);
        for status in MyStatus::ALL {
            assert!(!store.profile().allows(status, AlertKind::Knock));
            assert!(store.profile().allows(status, AlertKind::Message));
        }
        assert!(!store.pending_sync());
    }

    #[test]
    fn local_alerts_are_filtered_by_status_and_kind_only_for_phone_delivery() {
        let store = DeliveryStore::open(temp_path("filter"));
        store.set_kinds(MyStatus::Focusing, quiet(), false).unwrap();
        assert!(!store.allows_local(MyStatus::Focusing, AlertKind::Nudge));
        assert!(store.allows_local(MyStatus::Available, AlertKind::Nudge));
        assert!(store.allows_local(MyStatus::Unknown, AlertKind::Nudge));

        store.set_delivery(NotificationDelivery::Ntfy).unwrap();
        assert!(store.allows_local(MyStatus::Focusing, AlertKind::Nudge));

        store.set_delivery(NotificationDelivery::Phone).unwrap();
        assert!(!store.allows_local(MyStatus::Focusing, AlertKind::Nudge));
        assert!(store.allows_local(MyStatus::Focusing, AlertKind::Message));
    }

    #[test]
    fn a_pending_change_is_kept_until_the_relay_has_it() {
        let store = DeliveryStore::open(temp_path("pending"));
        let relay = NotificationProfile::default().with_kinds(MyStatus::Away, quiet());
        assert!(store.adopt_relay(relay).unwrap());
        assert_eq!(store.profile(), relay);

        store.set_kinds(MyStatus::Unknown, quiet(), true).unwrap();
        let local = store.profile();
        assert!(!store.adopt_relay(relay).unwrap());
        assert_eq!(store.profile(), local);
        assert!(store.pending_sync());

        store.synced(relay, relay).unwrap();
        assert!(store.pending_sync());
        store.synced(local, local).unwrap();
        assert!(!store.pending_sync());
        assert_eq!(store.profile(), local);
        assert!(store.adopt_relay(relay).unwrap());
        assert_eq!(store.profile(), relay);
    }
}
