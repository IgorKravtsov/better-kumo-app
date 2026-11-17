use kumo_core::chat::notifications as core;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MyStatus {
    Available,
    Away,
    Focusing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct NotificationKinds {
    pub messages: bool,
    pub mentions: bool,
    pub nudges: bool,
    pub knocks: bool,
    pub broadcasts: bool,
    pub public_chat: bool,
    pub nearby_chat: bool,
    pub floor_entries: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct NotificationProfile {
    pub available: NotificationKinds,
    pub away: NotificationKinds,
    pub focusing: NotificationKinds,
    pub unknown: NotificationKinds,
}

#[uniffi::export]
pub fn my_status_choices() -> Vec<MyStatus> {
    core::MyStatus::ALL
        .into_iter()
        .map(MyStatus::from)
        .collect()
}

#[uniffi::export]
pub fn my_status_label(status: MyStatus) -> String {
    core::MyStatus::from(status).label().to_string()
}

#[uniffi::export]
pub fn profile_kinds(profile: NotificationProfile, status: MyStatus) -> NotificationKinds {
    core::NotificationProfile::from(profile)
        .kinds(status.into())
        .into()
}

impl From<core::MyStatus> for MyStatus {
    fn from(s: core::MyStatus) -> Self {
        match s {
            core::MyStatus::Available => MyStatus::Available,
            core::MyStatus::Away => MyStatus::Away,
            core::MyStatus::Focusing => MyStatus::Focusing,
            core::MyStatus::Unknown => MyStatus::Unknown,
        }
    }
}

impl From<MyStatus> for core::MyStatus {
    fn from(s: MyStatus) -> Self {
        match s {
            MyStatus::Available => core::MyStatus::Available,
            MyStatus::Away => core::MyStatus::Away,
            MyStatus::Focusing => core::MyStatus::Focusing,
            MyStatus::Unknown => core::MyStatus::Unknown,
        }
    }
}

impl From<core::NotificationKinds> for NotificationKinds {
    fn from(k: core::NotificationKinds) -> Self {
        Self {
            messages: k.messages,
            mentions: k.mentions,
            nudges: k.nudges,
            knocks: k.knocks,
            broadcasts: k.broadcasts,
            public_chat: k.public_chat,
            nearby_chat: k.nearby_chat,
            floor_entries: k.floor_entries,
        }
    }
}

impl From<NotificationKinds> for core::NotificationKinds {
    fn from(k: NotificationKinds) -> Self {
        Self {
            messages: k.messages,
            mentions: k.mentions,
            nudges: k.nudges,
            knocks: k.knocks,
            broadcasts: k.broadcasts,
            public_chat: k.public_chat,
            nearby_chat: k.nearby_chat,
            floor_entries: k.floor_entries,
        }
    }
}

impl From<core::NotificationProfile> for NotificationProfile {
    fn from(p: core::NotificationProfile) -> Self {
        Self {
            available: p.available.into(),
            away: p.away.into(),
            focusing: p.focusing.into(),
            unknown: p.unknown.into(),
        }
    }
}

impl From<NotificationProfile> for core::NotificationProfile {
    fn from(p: NotificationProfile) -> Self {
        Self {
            available: p.available.into(),
            away: p.away.into(),
            focusing: p.focusing.into(),
            unknown: p.unknown.into(),
        }
    }
}
