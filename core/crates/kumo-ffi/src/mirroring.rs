use std::sync::atomic::{AtomicBool, Ordering};

use kumo_core::chat::alert::Alert;
use kumo_core::chat::delivery::DeliveryStore;
use kumo_core::chat::notifications as core;

use crate::convert::{AlertItem, alert_item};
use crate::desktop::{RelayClient, RelayError, RelayStatus};
use crate::notifications::{MyStatus, NotificationKinds};

pub(crate) trait NotificationsRelay {
    async fn status(&self) -> Result<RelayStatus, RelayError>;
    async fn put(&self, request: core::NotificationsRequest) -> Result<RelayStatus, RelayError>;
}

impl NotificationsRelay for RelayClient {
    async fn status(&self) -> Result<RelayStatus, RelayError> {
        RelayClient::status(self).await
    }

    async fn put(&self, request: core::NotificationsRequest) -> Result<RelayStatus, RelayError> {
        self.put_notifications(request).await
    }
}

type StatusSource = Box<dyn Fn() -> core::MyStatus + Send + Sync>;

pub struct PhoneMirroring {
    enabled: AtomicBool,
    delivery: DeliveryStore,
    my_status: StatusSource,
}

fn failed(e: kumo_core::CoreError) -> RelayError {
    RelayError::Failed(e.to_string())
}

impl PhoneMirroring {
    pub fn new(
        delivery: DeliveryStore,
        my_status: impl Fn() -> core::MyStatus + Send + Sync + 'static,
    ) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            delivery,
            my_status: Box::new(my_status),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn set(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
    }

    pub fn delivery(&self) -> &DeliveryStore {
        &self.delivery
    }

    pub fn my_status(&self) -> core::MyStatus {
        (self.my_status)()
    }

    pub fn local_alert(&self, alert: Option<Alert>) -> Option<AlertItem> {
        let alert = alert.filter(|a| {
            self.is_enabled() && self.delivery.allows_local(self.my_status(), a.kind)
        })?;
        Some(alert_item(alert))
    }

    pub fn set_kinds(
        &self,
        status: MyStatus,
        kinds: NotificationKinds,
        paired: bool,
    ) -> Result<(), RelayError> {
        self.delivery
            .set_kinds(status.into(), kinds.into(), paired)
            .map_err(failed)
    }

    pub(crate) async fn apply_kinds(
        &self,
        relay: Option<&impl NotificationsRelay>,
        status: MyStatus,
        kinds: NotificationKinds,
    ) -> Result<Option<RelayStatus>, RelayError> {
        let was_pending = self.delivery.pending_sync();
        self.set_kinds(status, kinds, relay.is_some())?;
        let Some(relay) = relay else {
            return Ok(None);
        };
        let request = if was_pending {
            self.delivery.profile().into()
        } else {
            core::NotificationsRequest::status(status.into(), core::NotificationKinds::from(kinds))
        };
        self.push(relay, request).await.map(Some)
    }

    pub(crate) async fn sync(
        &self,
        relay: Option<&impl NotificationsRelay>,
    ) -> Result<Option<RelayStatus>, RelayError> {
        let Some(relay) = relay else {
            return Ok(None);
        };
        if !self.delivery.pending_sync() {
            let status = relay.status().await?;
            if self
                .delivery
                .adopt_relay(status.notifications.into())
                .map_err(failed)?
            {
                return Ok(Some(status));
            }
        }
        let request = self.delivery.profile().into();
        self.push(relay, request).await.map(Some)
    }

    async fn push(
        &self,
        relay: &impl NotificationsRelay,
        request: core::NotificationsRequest,
    ) -> Result<RelayStatus, RelayError> {
        let sent = self.delivery.profile();
        let status = relay.put(request).await?;
        self.delivery
            .synced(sent, status.notifications.into())
            .map_err(failed)?;
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use kumo_core::chat::alert::AlertKind;
    use kumo_core::chat::delivery::NotificationDelivery;

    use super::*;
    use crate::desktop::RelayCompatibility;
    use crate::notifications::NotificationProfile;

    struct FakeRelay {
        profile: Mutex<core::NotificationProfile>,
        online: AtomicBool,
        puts: Mutex<Vec<core::NotificationsRequest>>,
    }

    impl FakeRelay {
        fn new(profile: core::NotificationProfile) -> Self {
            Self {
                profile: Mutex::new(profile),
                online: AtomicBool::new(true),
                puts: Mutex::new(Vec::new()),
            }
        }

        fn reply(&self) -> Result<RelayStatus, RelayError> {
            if !self.online.load(Ordering::SeqCst) {
                return Err(RelayError::Unavailable("relay is asleep".into()));
            }
            Ok(RelayStatus {
                uid: "uid-me".into(),
                mirroring: true,
                chat_online: true,
                ntfy: true,
                ntfy_topic: None,
                ntfy_server: None,
                ntfy_url: None,
                apns: false,
                phones: 0,
                desktop_connected: true,
                notifications: NotificationProfile::from(*self.profile.lock().unwrap()),
                my_status: MyStatus::Focusing,
                relay_version: None,
                compatibility: RelayCompatibility::Compatible,
                agents: Some(true),
            })
        }

        fn profile(&self) -> core::NotificationProfile {
            *self.profile.lock().unwrap()
        }

        fn puts(&self) -> Vec<core::NotificationsRequest> {
            self.puts.lock().unwrap().clone()
        }
    }

    impl NotificationsRelay for FakeRelay {
        async fn status(&self) -> Result<RelayStatus, RelayError> {
            self.reply()
        }

        async fn put(
            &self,
            request: core::NotificationsRequest,
        ) -> Result<RelayStatus, RelayError> {
            self.reply()?;
            self.puts.lock().unwrap().push(request);
            let mut profile = self.profile.lock().unwrap();
            *profile = profile.merged(&request);
            drop(profile);
            self.reply()
        }
    }

    fn mirroring(name: &str, status: Arc<Mutex<core::MyStatus>>) -> PhoneMirroring {
        let path = std::env::temp_dir()
            .join(format!(
                "kumo-ffi-mirroring-{name}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
            .join("notifications.json");
        PhoneMirroring::new(DeliveryStore::open(path), move || *status.lock().unwrap())
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn broadcast() -> Option<Alert> {
        Some(kumo_core::chat::alert::broadcast_alert(
            kumo_core::space::BroadcastKind::Screen,
            Some("Amy"),
        ))
    }

    fn no_broadcasts() -> NotificationKinds {
        core::NotificationKinds {
            broadcasts: false,
            ..core::NotificationKinds::default()
        }
        .into()
    }

    #[test]
    fn phone_delivery_filters_by_my_office_status() {
        let status = Arc::new(Mutex::new(core::MyStatus::Focusing));
        let phone = mirroring("status", status.clone());
        phone
            .delivery()
            .set_delivery(NotificationDelivery::Phone)
            .unwrap();
        phone.set(true);
        phone
            .set_kinds(MyStatus::Focusing, no_broadcasts(), false)
            .unwrap();
        assert!(phone.local_alert(broadcast()).is_none());
        let knock = kumo_core::chat::alert::office_alert(
            &serde_json::json!({"notificationType": "doorKnock"}),
            Some("Bo"),
        );
        assert_eq!(knock.as_ref().map(|k| k.kind), Some(AlertKind::Knock));
        assert!(phone.local_alert(knock).is_some());

        *status.lock().unwrap() = core::MyStatus::Away;
        assert!(phone.local_alert(broadcast()).is_some());

        *status.lock().unwrap() = core::MyStatus::Unknown;
        assert!(phone.local_alert(broadcast()).is_some());
        phone
            .set_kinds(MyStatus::Unknown, no_broadcasts(), false)
            .unwrap();
        assert!(phone.local_alert(broadcast()).is_none());
        assert_eq!(phone.my_status(), core::MyStatus::Unknown);
    }

    #[test]
    fn a_change_the_relay_missed_is_pushed_on_the_next_sync() {
        let phone = mirroring("pending", Arc::new(Mutex::new(core::MyStatus::Unknown)));
        let relay = FakeRelay::new(core::NotificationProfile::default());
        relay.online.store(false, Ordering::SeqCst);

        let result = block_on(phone.apply_kinds(Some(&relay), MyStatus::Focusing, no_broadcasts()));
        assert!(matches!(result, Err(RelayError::Unavailable(_))));
        assert!(phone.delivery().pending_sync());
        let local = phone.delivery().profile();
        assert!(!local.allows(core::MyStatus::Focusing, AlertKind::Broadcast));

        relay.online.store(true, Ordering::SeqCst);
        let status = block_on(phone.sync(Some(&relay))).unwrap().unwrap();
        assert_eq!(relay.puts(), vec![core::NotificationsRequest::from(local)]);
        assert_eq!(relay.profile(), local);
        assert_eq!(core::NotificationProfile::from(status.notifications), local);
        assert!(!phone.delivery().pending_sync());
        assert_eq!(phone.delivery().profile(), local);

        *relay.profile.lock().unwrap() = core::NotificationProfile::default();
        block_on(phone.sync(Some(&relay))).unwrap();
        assert_eq!(relay.puts().len(), 1);
        assert_eq!(
            phone.delivery().profile(),
            core::NotificationProfile::default()
        );
    }

    #[test]
    fn a_reachable_relay_gets_only_the_changed_status() {
        let phone = mirroring("direct", Arc::new(Mutex::new(core::MyStatus::Unknown)));
        let theirs = core::NotificationProfile::default().with_kinds(
            core::MyStatus::Away,
            core::NotificationKinds {
                messages: false,
                ..core::NotificationKinds::default()
            },
        );
        let relay = FakeRelay::new(theirs);
        let status = block_on(phone.apply_kinds(Some(&relay), MyStatus::Focusing, no_broadcasts()))
            .unwrap()
            .unwrap();
        assert_eq!(
            relay.puts(),
            vec![core::NotificationsRequest::status(
                core::MyStatus::Focusing,
                core::NotificationKinds::from(no_broadcasts())
            )]
        );
        let merged = theirs.with_kinds(core::MyStatus::Focusing, no_broadcasts().into());
        assert_eq!(
            core::NotificationProfile::from(status.notifications),
            merged
        );
        assert_eq!(phone.delivery().profile(), merged);
        assert!(!phone.delivery().pending_sync());
    }

    #[test]
    fn without_a_relay_changes_stay_local_and_a_new_relay_wins() {
        let phone = mirroring("alone", Arc::new(Mutex::new(core::MyStatus::Unknown)));
        let none: Option<&FakeRelay> = None;
        assert_eq!(
            block_on(phone.apply_kinds(none, MyStatus::Away, no_broadcasts())).unwrap(),
            None
        );
        assert!(!phone.delivery().pending_sync());
        assert_eq!(block_on(phone.sync(none)).unwrap(), None);

        let relay = FakeRelay::new(core::NotificationProfile::default());
        block_on(phone.sync(Some(&relay))).unwrap();
        assert!(relay.puts().is_empty());
        assert_eq!(
            phone.delivery().profile(),
            core::NotificationProfile::default()
        );
    }
}
