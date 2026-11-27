use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use kumo_core::chat::alert::{Alert, AlertKind};
use serde_json::json;

use crate::config::{ApnsConfig, NtfyConfig};

const PERSON_CATEGORY: &str = "PERSON";

pub enum Outcome {
    Delivered,
    InvalidToken,
}

pub struct Apns {
    config: ApnsConfig,
    key: EncodingKey,
    http: reqwest::Client,
    jwt: Mutex<Option<(String, u64)>>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Apns {
    pub fn new(config: ApnsConfig, http: reqwest::Client) -> anyhow::Result<Self> {
        let pem = std::fs::read(&config.key_path)
            .with_context(|| format!("cannot read APNs key {}", config.key_path.display()))?;
        let key =
            EncodingKey::from_ec_pem(&pem).context("APNs key must be the .p8 file from Apple")?;
        Ok(Self {
            config,
            key,
            http,
            jwt: Mutex::new(None),
        })
    }

    fn token(&self) -> anyhow::Result<String> {
        let mut cached = self.jwt.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
        if let Some((jwt, issued)) = cached.as_ref()
            && now_secs() < issued + 50 * 60
        {
            return Ok(jwt.clone());
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.config.key_id.clone());
        let issued = now_secs();
        let claims = json!({ "iss": self.config.team_id, "iat": issued });
        let jwt = jsonwebtoken::encode(&header, &claims, &self.key)?;
        *cached = Some((jwt.clone(), issued));
        Ok(jwt)
    }

    pub async fn send(&self, device_token: &str, alert: &Alert) -> anyhow::Result<Outcome> {
        let host = if self.config.sandbox {
            "https://api.sandbox.push.apple.com"
        } else {
            "https://api.push.apple.com"
        };
        let mut aps = json!({
            "alert": { "title": alert.title, "body": alert.body },
            "sound": "default",
        });
        if let Some(url) = &alert.channel_url {
            aps["thread-id"] = json!(url);
        }
        if person_actions(alert).is_some() {
            aps["category"] = json!(PERSON_CATEGORY);
        }
        let body = json!({
            "aps": aps,
            "channel_url": alert.channel_url,
            "person_id": alert.person_id,
            "kind": kind_name(alert.kind),
        });
        let res = self
            .http
            .post(format!("{host}/3/device/{device_token}"))
            .bearer_auth(self.token()?)
            .header("apns-topic", &self.config.topic)
            .header("apns-push-type", "alert")
            .header("apns-priority", "10")
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        match res.status().as_u16() {
            200 => Ok(Outcome::Delivered),
            400 | 410 => {
                let text = res.text().await.unwrap_or_default();
                if text.contains("BadDeviceToken") || text.contains("Unregistered") {
                    Ok(Outcome::InvalidToken)
                } else {
                    bail!("APNs rejected: {text}")
                }
            }
            code => bail!("APNs {code}: {}", res.text().await.unwrap_or_default()),
        }
    }
}

#[derive(Clone)]
pub struct Ntfy {
    config: NtfyConfig,
    http: reqwest::Client,
}

impl Ntfy {
    pub fn new(config: NtfyConfig, http: reqwest::Client) -> Self {
        Self { config, http }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn config(&self) -> &NtfyConfig {
        &self.config
    }

    pub async fn send(&self, alert: &Alert) -> anyhow::Result<()> {
        let mut body = json!({
            "topic": self.config.topic,
            "title": alert.title,
            "message": alert.body,
            "priority": priority(alert.kind),
            "tags": [tag(alert.kind)],
        });
        if let Some(url) = &alert.channel_url {
            body["click"] = json!(format!("kumowatch://chat?channel={}", encode(url)));
        }
        if let Some(person) = person_actions(alert) {
            body["click"] = json!(person_link(person, None));
            body["actions"] = json!([
                { "action": "view", "label": "Go to on desktop", "url": person_link(person, Some("desktop")), "clear": true },
                { "action": "view", "label": "Join on phone", "url": person_link(person, Some("phone")), "clear": true },
            ]);
        }
        let res = self
            .http
            .post(&self.config.server)
            .json(&body)
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        if !res.status().is_success() {
            // Only the status: a server may echo the notification text back.
            bail!("ntfy {}", res.status());
        }
        Ok(())
    }
}

fn person_actions(alert: &Alert) -> Option<&str> {
    alert
        .person_id
        .as_deref()
        .filter(|_| alert.kind == AlertKind::Nudge)
}

fn person_link(person: &str, via: Option<&str>) -> String {
    let link = format!("kumowatch://person?id={}", encode(person));
    match via {
        Some(via) => format!("{link}&via={via}"),
        None => link,
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn kind_name(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Message => "message",
        AlertKind::Mention => "mention",
        AlertKind::Nudge => "nudge",
        AlertKind::Knock => "knock",
        AlertKind::Broadcast => "broadcast",
        AlertKind::PublicChat => "public_chat",
        AlertKind::NearbyChat => "nearby_chat",
        AlertKind::FloorEntry => "floor_entry",
    }
}

pub fn priority(kind: AlertKind) -> u8 {
    match kind {
        AlertKind::Nudge | AlertKind::Knock | AlertKind::Mention | AlertKind::Broadcast => 4,
        AlertKind::Message
        | AlertKind::PublicChat
        | AlertKind::NearbyChat
        | AlertKind::FloorEntry => 3,
    }
}

pub fn tag(kind: AlertKind) -> &'static str {
    match kind {
        AlertKind::Message => "speech_balloon",
        AlertKind::Mention => "bell",
        AlertKind::Nudge => "wave",
        AlertKind::Knock => "door",
        AlertKind::Broadcast => "movie_camera",
        AlertKind::PublicChat => "loudspeaker",
        AlertKind::NearbyChat => "busts_in_silhouette",
        AlertKind::FloorEntry => "walking",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgO1tRImgfoxucZ3ov
L91F1/6E161tbBWByrm3pllvcNChRANCAAQq4michGbNlyLBIPpx2icv9KgraS30
yFZoHbshe5hHGmeZAyghIZ8vktMST5G7S9AFaYGbigH81DrGZZ8ImGb3
-----END PRIVATE KEY-----";

    #[test]
    fn signs_apns_jwt() {
        let dir = std::env::temp_dir().join(format!("kumo-relay-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key.p8");
        std::fs::write(&path, TEST_KEY).unwrap();
        let apns = Apns::new(
            ApnsConfig {
                team_id: "TEAM123456".into(),
                key_id: "KEY1234567".into(),
                key_path: path,
                topic: "com.kravtsov.kumowatch".into(),
                sandbox: true,
            },
            reqwest::Client::new(),
        )
        .unwrap();
        let jwt = apns.token().unwrap();
        assert_eq!(jwt.split('.').count(), 3);
        assert_eq!(apns.token().unwrap(), jwt);
    }

    #[test]
    fn encodes_channel_links() {
        assert_eq!(encode("a_b c/d"), "a_b%20c%2Fd");
    }

    #[test]
    fn links_to_a_person() {
        assert_eq!(person_link("c 1", None), "kumowatch://person?id=c%201");
        assert_eq!(
            person_link("c1", Some("desktop")),
            "kumowatch://person?id=c1&via=desktop"
        );
    }
}
