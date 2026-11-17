use std::sync::Arc;

use futures_util::future::BoxFuture;

use super::ports::RelayCalls;
use super::types::{CallZone, Credentials, Grant, HandoffFailure, RenewOutcome};
use crate::desktop::{MediaRequest, RelayClient, RelayError};

pub type RelayClients = Arc<dyn Fn() -> Option<RelayClient> + Send + Sync>;

pub struct RelayCallSource {
    clients: RelayClients,
}

impl RelayCallSource {
    pub fn new(clients: RelayClients) -> Self {
        Self { clients }
    }
}

impl RelayCalls for RelayCallSource {
    fn handoff(&self) -> BoxFuture<'static, Result<Grant, HandoffFailure>> {
        let client = (self.clients)();
        Box::pin(async move {
            let client = client.ok_or(HandoffFailure::Unreachable)?;
            let _ = client
                .set_media(MediaRequest {
                    audio: Some(false),
                    video: Some(false),
                })
                .await;
            match client.handoff().await {
                Ok(handoff) => Ok(Grant {
                    lease_id: handoff.lease_id,
                    credentials: Credentials {
                        url: handoff.call.url,
                        token: handoff.call.token,
                    },
                    room_name: handoff.call.room_name,
                    peers: handoff.call.peers,
                }),
                Err(RelayError::NotInCall) => Err(HandoffFailure::NotInCall),
                Err(_) => Err(HandoffFailure::Unreachable),
            }
        })
    }

    fn renew(&self, lease_id: String) -> BoxFuture<'static, RenewOutcome> {
        let client = (self.clients)();
        Box::pin(async move {
            let Some(client) = client else {
                return RenewOutcome::Unreachable;
            };
            match client.renew(&lease_id).await {
                Ok(()) => RenewOutcome::Renewed,
                Err(RelayError::StaleLease) => RenewOutcome::Stale,
                Err(_) => RenewOutcome::Unreachable,
            }
        })
    }

    fn reclaim(&self, lease_id: String) -> BoxFuture<'static, ()> {
        let client = (self.clients)();
        Box::pin(async move {
            if let Some(client) = client {
                let _ = client.reclaim(&lease_id).await;
            }
        })
    }

    fn zone(&self) -> BoxFuture<'static, Option<CallZone>> {
        let client = (self.clients)();
        Box::pin(async move { Some(client?.desktop().await.ok()?.call_zone()) })
    }
}
