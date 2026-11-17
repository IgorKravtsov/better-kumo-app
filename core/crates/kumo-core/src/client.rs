use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api::{ApiError, Endpoint, HttpMethod, Paginated};
use crate::auth::Auth;
use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatConfigChannel {
    pub channel_url: String,
    #[serde(default)]
    pub unread_message_count: u64,
    #[serde(default)]
    pub unread_mention_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatConfig {
    pub user_id: String,
    pub session_token: String,
    #[serde(default)]
    pub channels: Vec<ChatConfigChannel>,
}

#[derive(Clone)]
pub struct KumoApi {
    http: reqwest::Client,
    base_url: String,
    auth: Arc<Auth>,
}

impl KumoApi {
    pub fn new(http: reqwest::Client, base_url: String, auth: Arc<Auth>) -> Self {
        Self {
            http,
            base_url,
            auth,
        }
    }

    pub fn auth(&self) -> &Arc<Auth> {
        &self.auth
    }

    pub async fn get<T: DeserializeOwned>(&self, endpoint: &Endpoint) -> Result<T> {
        self.request(endpoint, None).await
    }

    pub async fn send<T: DeserializeOwned>(&self, endpoint: &Endpoint, body: &Value) -> Result<T> {
        self.request(endpoint, Some(body)).await
    }

    pub async fn get_path(&self, path: &str) -> Result<Value> {
        let url = format!(
            "{}/v1/{}",
            self.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let token = self.auth.id_token().await?;
        let res = self.http.get(url).bearer_auth(token).send().await?;
        decode(res).await
    }

    async fn request<T: DeserializeOwned>(
        &self,
        endpoint: &Endpoint,
        body: Option<&Value>,
    ) -> Result<T> {
        let url = endpoint
            .url(&self.base_url)
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let mut forced = false;
        loop {
            let token = if forced {
                self.auth.refreshed_id_token().await?
            } else {
                self.auth.id_token().await?
            };
            let builder = match endpoint.method() {
                HttpMethod::Get => self.http.get(url.clone()),
                HttpMethod::Post => self.http.post(url.clone()),
                HttpMethod::Delete => self.http.delete(url.clone()),
            };
            let builder = builder.bearer_auth(token);
            let builder = match body {
                Some(b) => builder.json(b),
                None => builder,
            };
            let res = builder
                .send()
                .await
                .map_err(|_| ApiError::network_failure())?;
            if res.status().as_u16() == 401 && !forced {
                forced = true;
                continue;
            }
            return decode(res).await;
        }
    }

    pub async fn self_spaces(&self) -> Result<Vec<Value>> {
        let value: Value = self.get(&Endpoint::SelfSpaces).await?;
        Ok(items_of(value))
    }

    pub async fn recent_spaces(&self) -> Result<Vec<Value>> {
        let value: Value = self.get(&Endpoint::RecentSpaces).await?;
        Ok(items_of(value))
    }

    pub async fn default_space(&self) -> Result<Option<String>> {
        let value: Value = self.get(&Endpoint::DefaultSpace).await?;
        Ok(value.get("defaultSpace").and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.get("name")?.as_str().map(String::from))
        }))
    }

    pub async fn space(&self, space_name: &str) -> Result<Value> {
        self.get(&Endpoint::Space {
            space_name: space_name.into(),
        })
        .await
    }

    pub async fn chat_config(&self, space_name: &str) -> Result<ChatConfig> {
        self.get(&Endpoint::SpaceChatConfig {
            space_name: space_name.into(),
        })
        .await
    }

    pub async fn delete_chat_message(&self, channel_url: &str, message_id: i64) -> Result<()> {
        let _: Value = self
            .request(
                &Endpoint::DeleteChatMessage {
                    channel_url: channel_url.into(),
                    message_id: message_id.to_string(),
                },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn soft_delete_file_message(&self, channel_url: &str, message_id: i64) -> Result<()> {
        let _: Value = self
            .request(
                &Endpoint::SoftDeleteFileMessage {
                    channel_url: channel_url.into(),
                    message_id: message_id.to_string(),
                },
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn create_direct_channel(
        &self,
        space_uid: &str,
        user_ids: [&str; 2],
    ) -> Result<Value> {
        self.send(
            &Endpoint::CreateChatChannel,
            &serde_json::json!({ "channelType": "user", "spaceUid": space_uid, "userIds": user_ids }),
        )
        .await
    }

    pub async fn mint_login_token(&self) -> Result<String> {
        let value: Value = self.request(&Endpoint::DesktopLoginToken, None).await?;
        value
            .get("idToken")
            .or_else(|| value.get("token"))
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| CoreError::Decode("no token in desktop token response".into()))
    }

    pub async fn user_roles(&self, space_name: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut next = None;
        loop {
            let page: Paginated<Value> = self
                .get(&Endpoint::SpaceUserRoles {
                    space_name: space_name.into(),
                    next: next.take(),
                })
                .await?;
            out.extend(page.items);
            match page.metadata.next {
                Some(cursor) if !cursor.is_empty() => next = Some(cursor),
                _ => return Ok(out),
            }
        }
    }
}

fn items_of(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        Value::Object(mut map) => match map.remove("items") {
            Some(Value::Array(items)) => items,
            _ => vec![Value::Object(map)],
        },
        other => vec![other],
    }
}

async fn decode<T: DeserializeOwned>(res: reqwest::Response) -> Result<T> {
    let status = res.status();
    let text = res.text().await?;
    if !status.is_success() {
        return Err(ApiError::from_response(status.as_u16(), &text).into());
    }
    let text = if status.as_u16() == 204 || text.is_empty() {
        "null"
    } else {
        text.as_str()
    };
    Ok(serde_json::from_str(text)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_config() {
        let config: ChatConfig = serde_json::from_str(
            r#"{"userId":"u1","sessionToken":"s","channels":[{"channel_url":"c1","unread_message_count":3}]}"#,
        )
        .unwrap();
        assert_eq!(config.channels[0].unread_message_count, 3);
        assert_eq!(config.channels[0].unread_mention_count, 0);
    }

    #[test]
    fn extracts_items() {
        assert_eq!(items_of(serde_json::json!({"items":[1,2]})).len(), 2);
        assert_eq!(items_of(serde_json::json!([1])).len(), 1);
    }
}
