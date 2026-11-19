use std::time::Duration;

use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::api::{
    AgentsRequest, DeviceRequest, FloorRequest, FollowRequest, Handoff, LeaseRequest, MediaRequest,
    MirroringRequest, MoveRequest, NtfyRequest, NudgeRequest, OkReply, RelayError, RelayStatus,
    StatusRequest, ViewingRequest, ZoneRequest, routes,
};
use super::nearby::{NearbyChat, NearbyChatRequest};
use super::state::{DesktopCall, DesktopState, DesktopTarget};
use crate::chat::notifications::NotificationsRequest;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const PAIR_LINK_PREFIX: &str = "kumowatch://relay";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPairing {
    pub url: String,
    pub secret: String,
}

impl RelayPairing {
    pub fn from_link(link: &str) -> Option<Self> {
        let link = link.trim();
        if !link.starts_with(PAIR_LINK_PREFIX) {
            return None;
        }
        let parsed = url::Url::parse(link).ok()?;
        let value = |name: &str| {
            parsed
                .query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
                .filter(|v| !v.is_empty())
        };
        Some(Self {
            url: value("url")?,
            secret: value("secret")?,
        })
    }
}

#[derive(Clone)]
pub struct RelayClient {
    http: reqwest::Client,
    pairing: RelayPairing,
}

impl RelayClient {
    pub fn new(http: reqwest::Client, pairing: RelayPairing) -> Self {
        Self { http, pairing }
    }

    pub fn pairing(&self) -> &RelayPairing {
        &self.pairing
    }

    pub async fn status(&self) -> Result<RelayStatus, RelayError> {
        self.get(routes::STATUS).await
    }

    pub async fn set_mirroring(&self, enabled: bool) -> Result<RelayStatus, RelayError> {
        self.send(
            Method::PUT,
            routes::MIRRORING,
            &MirroringRequest { enabled },
        )
        .await
    }

    pub async fn set_agents(&self, enabled: bool) -> Result<RelayStatus, RelayError> {
        self.send(Method::PUT, routes::AGENTS, &AgentsRequest { enabled })
            .await
    }

    pub async fn set_ntfy_topic(&self, topic: Option<String>) -> Result<RelayStatus, RelayError> {
        self.send(
            Method::PUT,
            routes::NTFY,
            &NtfyRequest {
                topic,
                server: None,
            },
        )
        .await
    }

    pub async fn set_notifications(
        &self,
        request: &NotificationsRequest,
    ) -> Result<RelayStatus, RelayError> {
        self.send(Method::PUT, routes::NOTIFICATIONS, request).await
    }

    pub async fn set_viewing(&self, channel_url: Option<String>) -> Result<(), RelayError> {
        let _: OkReply = self
            .send(
                Method::PUT,
                routes::VIEWING,
                &ViewingRequest { channel_url },
            )
            .await?;
        Ok(())
    }

    pub async fn send_test_alert(&self) -> Result<RelayStatus, RelayError> {
        self.send(Method::POST, routes::TEST, &serde_json::json!({}))
            .await
    }

    pub async fn register_device(
        &self,
        token: &str,
        name: &str,
    ) -> Result<RelayStatus, RelayError> {
        self.send(
            Method::PUT,
            &routes::device(token),
            &DeviceRequest {
                enabled: true,
                name: name.to_string(),
            },
        )
        .await
    }

    pub async fn remove_device(&self, token: &str) -> Result<(), RelayError> {
        self.request(Method::DELETE, &routes::device(token), None::<&()>)
            .await
            .map(|_| ())
    }

    pub async fn desktop(&self) -> Result<DesktopState, RelayError> {
        self.get(routes::DESKTOP).await
    }

    pub async fn move_to(&self, spot: MoveRequest) -> Result<DesktopState, RelayError> {
        self.send(Method::POST, routes::MOVE, &spot).await
    }

    pub async fn enter_zone(&self, zone_id: &str) -> Result<DesktopState, RelayError> {
        self.send(
            Method::POST,
            routes::ZONE,
            &ZoneRequest {
                zone_id: zone_id.to_string(),
            },
        )
        .await
    }

    pub async fn go(&self, target: DesktopTarget) -> Result<DesktopState, RelayError> {
        match target {
            DesktopTarget::Zone(zone_id) => self.enter_zone(&zone_id).await,
            DesktopTarget::Spot(spot) => self.move_to(spot).await,
        }
    }

    pub async fn switch_floor(&self, floor_id: &str) -> Result<DesktopState, RelayError> {
        self.send(
            Method::POST,
            routes::FLOOR,
            &FloorRequest {
                floor_id: floor_id.to_string(),
            },
        )
        .await
    }

    pub async fn follow(&self, user_id: Option<String>) -> Result<DesktopState, RelayError> {
        self.send(Method::POST, routes::FOLLOW, &FollowRequest { user_id })
            .await
    }

    pub async fn set_status(&self, status: &StatusRequest) -> Result<DesktopState, RelayError> {
        self.send(Method::POST, routes::AVAILABILITY, status).await
    }

    pub async fn set_media(&self, media: MediaRequest) -> Result<DesktopState, RelayError> {
        self.send(Method::POST, routes::MEDIA, &media).await
    }

    pub async fn go_home(&self) -> Result<DesktopState, RelayError> {
        self.send(Method::POST, routes::HOME, &serde_json::json!({}))
            .await
    }

    pub async fn nudge(&self, user_id: &str) -> Result<(), RelayError> {
        let _: OkReply = self
            .send(
                Method::POST,
                routes::NUDGE,
                &NudgeRequest {
                    user_id: user_id.to_string(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn nearby_chat(&self) -> Result<NearbyChat, RelayError> {
        self.get(routes::NEARBY_CHAT).await
    }

    pub async fn send_nearby(&self, message: &str) -> Result<(), RelayError> {
        let _: OkReply = self
            .send(
                Method::POST,
                routes::NEARBY_CHAT,
                &NearbyChatRequest {
                    message: message.to_string(),
                },
            )
            .await?;
        Ok(())
    }

    pub async fn call(&self) -> Result<DesktopCall, RelayError> {
        self.get(routes::CALL).await
    }

    pub async fn handoff(&self) -> Result<Handoff, RelayError> {
        self.send(Method::POST, routes::HANDOFF, &serde_json::json!({}))
            .await
    }

    pub async fn renew(&self, lease_id: &str) -> Result<(), RelayError> {
        self.lease_request(routes::LEASE, lease_id).await
    }

    pub async fn reclaim(&self, lease_id: &str) -> Result<(), RelayError> {
        self.lease_request(routes::RECLAIM, lease_id).await
    }

    async fn lease_request(&self, path: &str, lease_id: &str) -> Result<(), RelayError> {
        let _: OkReply = self
            .send(
                Method::POST,
                path,
                &LeaseRequest {
                    lease_id: lease_id.to_string(),
                },
            )
            .await?;
        Ok(())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, RelayError> {
        let body = self.request(Method::GET, path, None::<&()>).await?;
        decode(&body)
    }

    async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: &B,
    ) -> Result<T, RelayError> {
        let body = self.request(method, path, Some(body)).await?;
        decode(&body)
    }

    async fn request<B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<String, RelayError> {
        let url = format!("{}{path}", self.pairing.url.trim_end_matches('/'));
        let mut request = self
            .http
            .request(method, url)
            .timeout(REQUEST_TIMEOUT)
            .bearer_auth(&self.pairing.secret);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| RelayError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response
            .text()
            .await
            .map_err(|e| RelayError::Network(e.to_string()))?;
        if (200..300).contains(&status) {
            Ok(text)
        } else {
            Err(RelayError::from_reply(status, &text))
        }
    }
}

fn decode<T: DeserializeOwned>(body: &str) -> Result<T, RelayError> {
    serde_json::from_str(body).map_err(|e| RelayError::Protocol(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_links_carry_the_relay_address_and_secret() {
        assert_eq!(
            RelayPairing::from_link(
                "kumowatch://relay?url=http%3A%2F%2F10.0.0.2%3A8787&secret=s3cr%2Bt"
            ),
            Some(RelayPairing {
                url: "http://10.0.0.2:8787".into(),
                secret: "s3cr+t".into(),
            })
        );
        assert_eq!(RelayPairing::from_link("kumowatch://relay?url=x"), None);
        assert_eq!(
            RelayPairing::from_link("https://evil/?url=a&secret=b"),
            None
        );
    }
}
