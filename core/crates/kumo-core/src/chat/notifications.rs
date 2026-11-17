use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::alert::AlertKind;
use crate::ws::Availability;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MyStatus {
    Available,
    Away,
    Focusing,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusSignals {
    pub known: bool,
    pub availability: Option<Availability>,
    pub idle: bool,
}

impl MyStatus {
    pub const ALL: [MyStatus; 4] = [
        MyStatus::Available,
        MyStatus::Away,
        MyStatus::Focusing,
        MyStatus::Unknown,
    ];

    pub fn of(signals: StatusSignals) -> Self {
        match signals {
            StatusSignals {
                known: true,
                idle: false,
                availability: Some(availability),
            } => match availability {
                Availability::Available => MyStatus::Available,
                Availability::Away => MyStatus::Away,
                Availability::Focusing => MyStatus::Focusing,
            },
            _ => MyStatus::Unknown,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            MyStatus::Available => "available",
            MyStatus::Away => "away",
            MyStatus::Focusing => "focusing",
            MyStatus::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MyStatus::Available => Availability::Available.label(),
            MyStatus::Away => Availability::Away.label(),
            MyStatus::Focusing => Availability::Focusing.label(),
            MyStatus::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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

impl Default for NotificationKinds {
    fn default() -> Self {
        Self {
            messages: true,
            mentions: true,
            nudges: true,
            knocks: true,
            broadcasts: true,
            public_chat: true,
            nearby_chat: true,
            floor_entries: false,
        }
    }
}

impl NotificationKinds {
    pub fn allows(&self, kind: AlertKind) -> bool {
        match kind {
            AlertKind::Message => self.messages,
            AlertKind::Mention => self.mentions,
            AlertKind::Nudge => self.nudges,
            AlertKind::Knock => self.knocks,
            AlertKind::Broadcast => self.broadcasts,
            AlertKind::PublicChat => self.public_chat,
            AlertKind::NearbyChat => self.nearby_chat,
            AlertKind::FloorEntry => self.floor_entries,
        }
    }

    pub fn merged(mut self, request: &KindsRequest) -> Self {
        let pairs = [
            (&mut self.messages, request.messages),
            (&mut self.mentions, request.mentions),
            (&mut self.nudges, request.nudges),
            (&mut self.knocks, request.knocks),
            (&mut self.broadcasts, request.broadcasts),
            (&mut self.public_chat, request.public_chat),
            (&mut self.nearby_chat, request.nearby_chat),
            (&mut self.floor_entries, request.floor_entries),
        ];
        for (field, value) in pairs {
            if let Some(value) = value {
                *field = value;
            }
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KindsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messages: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nudges: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knocks: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub broadcasts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_chat: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearby_chat: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor_entries: Option<bool>,
}

impl From<NotificationKinds> for KindsRequest {
    fn from(k: NotificationKinds) -> Self {
        Self {
            messages: Some(k.messages),
            mentions: Some(k.mentions),
            nudges: Some(k.nudges),
            knocks: Some(k.knocks),
            broadcasts: Some(k.broadcasts),
            public_chat: Some(k.public_chat),
            nearby_chat: Some(k.nearby_chat),
            floor_entries: Some(k.floor_entries),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct NotificationProfile {
    pub available: NotificationKinds,
    pub away: NotificationKinds,
    pub focusing: NotificationKinds,
    pub unknown: NotificationKinds,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ProfileFields {
    available: NotificationKinds,
    away: NotificationKinds,
    focusing: NotificationKinds,
    unknown: NotificationKinds,
}

impl<'de> Deserialize<'de> for NotificationProfile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let per_status = value
            .as_object()
            .is_some_and(|map| MyStatus::ALL.iter().any(|s| map.contains_key(s.key())));
        if per_status {
            let f = ProfileFields::deserialize(value).map_err(D::Error::custom)?;
            Ok(Self {
                available: f.available,
                away: f.away,
                focusing: f.focusing,
                unknown: f.unknown,
            })
        } else {
            NotificationKinds::deserialize(value)
                .map(Self::uniform)
                .map_err(D::Error::custom)
        }
    }
}

impl NotificationProfile {
    pub fn uniform(kinds: NotificationKinds) -> Self {
        Self {
            available: kinds,
            away: kinds,
            focusing: kinds,
            unknown: kinds,
        }
    }

    pub fn kinds(&self, status: MyStatus) -> NotificationKinds {
        match status {
            MyStatus::Available => self.available,
            MyStatus::Away => self.away,
            MyStatus::Focusing => self.focusing,
            MyStatus::Unknown => self.unknown,
        }
    }

    fn kinds_mut(&mut self, status: MyStatus) -> &mut NotificationKinds {
        match status {
            MyStatus::Available => &mut self.available,
            MyStatus::Away => &mut self.away,
            MyStatus::Focusing => &mut self.focusing,
            MyStatus::Unknown => &mut self.unknown,
        }
    }

    pub fn allows(&self, status: MyStatus, kind: AlertKind) -> bool {
        self.kinds(status).allows(kind)
    }

    pub fn with_kinds(mut self, status: MyStatus, kinds: NotificationKinds) -> Self {
        *self.kinds_mut(status) = kinds;
        self
    }

    pub fn merged(mut self, request: &NotificationsRequest) -> Self {
        let patches = match request {
            NotificationsRequest::Status(r) => vec![(r.status, r.kinds)],
            NotificationsRequest::Profile(r) => MyStatus::ALL
                .into_iter()
                .filter_map(|s| r.kinds(s).map(|k| (s, k)))
                .collect(),
            NotificationsRequest::All(kinds) => {
                MyStatus::ALL.into_iter().map(|s| (s, *kinds)).collect()
            }
        };
        for (status, kinds) in patches {
            let field = self.kinds_mut(status);
            *field = field.merged(&kinds);
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusKindsRequest {
    pub status: MyStatus,
    #[serde(default)]
    pub kinds: KindsRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<KindsRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub away: Option<KindsRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focusing: Option<KindsRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unknown: Option<KindsRequest>,
}

impl ProfileRequest {
    fn kinds(&self, status: MyStatus) -> Option<KindsRequest> {
        match status {
            MyStatus::Available => self.available,
            MyStatus::Away => self.away,
            MyStatus::Focusing => self.focusing,
            MyStatus::Unknown => self.unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotificationsRequest {
    Status(StatusKindsRequest),
    Profile(ProfileRequest),
    All(KindsRequest),
}

impl NotificationsRequest {
    pub fn status(status: MyStatus, kinds: impl Into<KindsRequest>) -> Self {
        Self::Status(StatusKindsRequest {
            status,
            kinds: kinds.into(),
        })
    }
}

impl From<NotificationProfile> for NotificationsRequest {
    fn from(p: NotificationProfile) -> Self {
        Self::Profile(ProfileRequest {
            available: Some(p.available.into()),
            away: Some(p.away.into()),
            focusing: Some(p.focusing.into()),
            unknown: Some(p.unknown.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const STATUSES: [Option<Availability>; 4] = [
        None,
        Some(Availability::Available),
        Some(Availability::Away),
        Some(Availability::Focusing),
    ];

    #[test]
    fn my_status_needs_a_known_awake_availability() {
        for availability in STATUSES {
            for idle in [false, true] {
                let unknown = MyStatus::of(StatusSignals {
                    known: false,
                    availability,
                    idle,
                });
                assert_eq!(unknown, MyStatus::Unknown);
                let known = MyStatus::of(StatusSignals {
                    known: true,
                    availability,
                    idle,
                });
                let expected = match (idle, availability) {
                    (true, _) | (false, None) => MyStatus::Unknown,
                    (false, Some(Availability::Available)) => MyStatus::Available,
                    (false, Some(Availability::Away)) => MyStatus::Away,
                    (false, Some(Availability::Focusing)) => MyStatus::Focusing,
                };
                assert_eq!(known, expected, "{availability:?} idle={idle}");
            }
        }
        assert_eq!(MyStatus::of(StatusSignals::default()), MyStatus::Unknown);
        assert_eq!(MyStatus::default(), MyStatus::Unknown);
        assert_eq!(
            MyStatus::ALL.map(MyStatus::label),
            ["Available", "Away", "Focusing", "Unknown"]
        );
        assert_eq!(
            serde_json::to_value(MyStatus::ALL).unwrap(),
            json!(["available", "away", "focusing", "unknown"])
        );
    }

    #[test]
    fn allows_looks_at_the_kinds_of_the_given_status_only() {
        let profile = NotificationProfile::default().with_kinds(
            MyStatus::Focusing,
            NotificationKinds {
                broadcasts: false,
                public_chat: false,
                ..NotificationKinds::default()
            },
        );
        assert!(!profile.allows(MyStatus::Focusing, AlertKind::Broadcast));
        assert!(!profile.allows(MyStatus::Focusing, AlertKind::PublicChat));
        assert!(profile.allows(MyStatus::Focusing, AlertKind::Knock));
        for status in [MyStatus::Available, MyStatus::Away, MyStatus::Unknown] {
            assert!(profile.allows(status, AlertKind::Broadcast));
            assert!(profile.allows(status, AlertKind::PublicChat));
        }
        let all_off = NotificationKinds {
            messages: false,
            mentions: false,
            nudges: false,
            knocks: false,
            broadcasts: false,
            public_chat: false,
            nearby_chat: false,
            floor_entries: false,
        };
        let away_off = NotificationProfile::default().with_kinds(MyStatus::Away, all_off);
        for kind in [
            AlertKind::Message,
            AlertKind::Mention,
            AlertKind::Nudge,
            AlertKind::Knock,
            AlertKind::Broadcast,
            AlertKind::PublicChat,
            AlertKind::NearbyChat,
        ] {
            assert!(!away_off.allows(MyStatus::Away, kind));
            assert!(away_off.allows(MyStatus::Available, kind));
        }
    }

    #[test]
    fn a_flat_profile_migrates_to_every_status() {
        let flat: NotificationProfile =
            serde_json::from_value(json!({"public_chat": false, "future_kind": false})).unwrap();
        let kinds = NotificationKinds {
            public_chat: false,
            ..NotificationKinds::default()
        };
        assert_eq!(flat, NotificationProfile::uniform(kinds));
        assert_eq!(
            serde_json::from_value::<NotificationProfile>(json!({})).unwrap(),
            NotificationProfile::default()
        );
        assert!(NotificationProfile::default().allows(MyStatus::Unknown, AlertKind::Nudge));
    }

    #[test]
    fn floor_entries_are_off_unless_turned_on() {
        for status in MyStatus::ALL {
            assert!(!NotificationProfile::default().allows(status, AlertKind::FloorEntry));
        }
        let saved: NotificationProfile =
            serde_json::from_value(json!({"away": {"messages": true}})).unwrap();
        assert!(!saved.allows(MyStatus::Away, AlertKind::FloorEntry));
        let on = saved.merged(&NotificationsRequest::status(
            MyStatus::Away,
            KindsRequest {
                floor_entries: Some(true),
                ..KindsRequest::default()
            },
        ));
        assert!(on.allows(MyStatus::Away, AlertKind::FloorEntry));
        assert!(!on.allows(MyStatus::Available, AlertKind::FloorEntry));
        let round_trip: NotificationProfile =
            serde_json::from_value(serde_json::to_value(on).unwrap()).unwrap();
        assert_eq!(round_trip, on);
    }

    #[test]
    fn a_per_status_profile_fills_missing_statuses_and_kinds_with_on() {
        let profile: NotificationProfile = serde_json::from_value(
            json!({"focusing": {"broadcasts": false}, "future_status": {"messages": false}}),
        )
        .unwrap();
        assert_eq!(
            profile,
            NotificationProfile::default().with_kinds(
                MyStatus::Focusing,
                NotificationKinds {
                    broadcasts: false,
                    ..NotificationKinds::default()
                }
            )
        );
        let round_trip: NotificationProfile =
            serde_json::from_value(serde_json::to_value(profile).unwrap()).unwrap();
        assert_eq!(round_trip, profile);
        assert_eq!(
            serde_json::to_value(NotificationProfile::default()).unwrap()["unknown"]["nearby_chat"],
            json!(true)
        );
    }

    #[test]
    fn requests_patch_one_status_a_profile_or_every_status() {
        let parse = |v: Value| serde_json::from_value::<NotificationsRequest>(v);
        let one = parse(json!({"status": "focusing", "kinds": {"broadcasts": false}})).unwrap();
        let merged = NotificationProfile::default().merged(&one);
        assert!(!merged.allows(MyStatus::Focusing, AlertKind::Broadcast));
        assert!(merged.allows(MyStatus::Away, AlertKind::Broadcast));
        assert!(merged.allows(MyStatus::Focusing, AlertKind::Knock));

        let flat = parse(json!({"knocks": false})).unwrap();
        assert_eq!(
            flat,
            NotificationsRequest::All(KindsRequest {
                knocks: Some(false),
                ..KindsRequest::default()
            })
        );
        let merged = merged.merged(&flat);
        for status in MyStatus::ALL {
            assert!(!merged.allows(status, AlertKind::Knock));
        }
        assert!(!merged.allows(MyStatus::Focusing, AlertKind::Broadcast));

        let partial_profile =
            parse(json!({"away": {"messages": false}, "unknown": {"nudges": false}})).unwrap();
        let merged = NotificationProfile::default().merged(&partial_profile);
        assert!(!merged.allows(MyStatus::Away, AlertKind::Message));
        assert!(!merged.allows(MyStatus::Unknown, AlertKind::Nudge));
        assert!(merged.allows(MyStatus::Available, AlertKind::Message));

        let full = NotificationsRequest::from(merged);
        assert_eq!(parse(serde_json::to_value(full).unwrap()).unwrap(), full);
        assert_eq!(NotificationProfile::default().merged(&full), merged);
        let status = NotificationsRequest::status(MyStatus::Away, merged.away);
        assert_eq!(
            parse(serde_json::to_value(status).unwrap()).unwrap(),
            status
        );

        assert_eq!(
            NotificationProfile::default().merged(&parse(json!({})).unwrap()),
            NotificationProfile::default()
        );
        assert!(parse(json!({"broadcast": false})).is_err());
        assert!(parse(json!({"status": "busy", "kinds": {}})).is_err());
        assert!(parse(json!({"status": "away", "kinds": {"broadcast": false}})).is_err());
        assert!(parse(json!({"away": {"broadcast": false}})).is_err());
    }
}
