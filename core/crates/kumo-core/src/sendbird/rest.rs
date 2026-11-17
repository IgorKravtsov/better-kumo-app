use std::sync::{Arc, RwLock};

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::model::{
    ChannelChangelog, ChannelPage, MessageChangelog, MessagePage, SbChannel, SbMessage,
};
use super::{SDK_VERSION, sdk_user_agent, user_agent};
use crate::error::{CoreError, Result};
use crate::http::now_ms;

#[derive(Debug, Clone, Default)]
pub struct ChannelListQuery {
    pub custom_types: Vec<String>,
    pub metadata_key: Option<String>,
    pub metadata_values: Vec<String>,
    pub unread_only: bool,
    pub include_empty: bool,
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct MessageQuery {
    pub anchor_ts: i64,
    pub prev_limit: u32,
    pub next_limit: u32,
    pub include_threads: bool,
}

impl MessageQuery {
    pub fn after(ts: i64, limit: u32) -> Self {
        Self {
            anchor_ts: ts,
            prev_limit: 0,
            next_limit: limit,
            include_threads: false,
        }
    }

    pub fn before(ts: i64, limit: u32) -> Self {
        Self {
            anchor_ts: ts,
            prev_limit: limit,
            next_limit: 0,
            include_threads: false,
        }
    }

    pub fn latest(limit: u32) -> Self {
        Self {
            anchor_ts: i64::MAX,
            prev_limit: limit,
            next_limit: 0,
            include_threads: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileUpload {
    pub channel_url: String,
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub url: String,
    pub size: u64,
    pub require_auth: bool,
    pub thumbnails: Value,
}

#[derive(Clone)]
pub struct SendbirdRest {
    http: reqwest::Client,
    base_url: String,
    app_id: String,
    user_id: String,
    session_key: Arc<RwLock<String>>,
}

impl SendbirdRest {
    pub fn new(http: reqwest::Client, app_id: &str, user_id: &str) -> Self {
        Self {
            http,
            base_url: format!("https://api-{}.sendbird.com", app_id.to_lowercase()),
            app_id: app_id.to_string(),
            user_id: user_id.to_string(),
            session_key: Arc::new(RwLock::new(String::new())),
        }
    }

    pub fn set_session_key(&self, key: &str) {
        if let Ok(mut k) = self.session_key.write() {
            *k = key.to_string();
        }
    }

    fn key(&self) -> String {
        self.session_key
            .read()
            .map(|k| k.clone())
            .unwrap_or_default()
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        self.call(reqwest::Method::GET, path, query, None).await
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<T> {
        let url = format!("{}/v3{}", self.base_url, path);
        let mut builder = self
            .http
            .request(method, url)
            .query(query)
            .header("SendBird", format!("JS,,{SDK_VERSION},{}", self.app_id))
            .header("SB-User-Agent", user_agent())
            .header("SB-SDK-User-Agent", sdk_user_agent())
            .header("Request-Sent-Timestamp", now_ms().to_string())
            .header("App-Id", &self.app_id)
            .header("Session-Key", self.key())
            .header("Content-Type", "application/json; charset=utf-8");
        if let Some(b) = body {
            builder = builder.body(b.to_string());
        }
        let res = builder.send().await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            return Err(CoreError::Sendbird {
                code: parsed["code"].as_i64().unwrap_or(status.as_u16().into()),
                message: parsed["message"].as_str().unwrap_or(&text).to_string(),
            });
        }
        Ok(serde_json::from_str(&text)?)
    }

    fn user_path(&self, suffix: &str) -> String {
        format!("/users/{}{}", urlencode(&self.user_id), suffix)
    }

    pub async fn my_group_channels(
        &self,
        query: &ChannelListQuery,
        token: &str,
    ) -> Result<ChannelPage> {
        let mut q = vec![
            ("token", token.to_string()),
            ("limit", query.limit.max(1).to_string()),
            ("order", "latest_last_message".into()),
            ("show_member", "true".into()),
            ("show_read_receipt", "true".into()),
            ("show_delivery_receipt", "true".into()),
            ("show_empty", query.include_empty.to_string()),
            ("show_frozen", "true".into()),
            ("show_metadata", "true".into()),
            ("member_state_filter", "all".into()),
            ("distinct_mode", "all".into()),
            ("public_mode", "all".into()),
            ("super_mode", "all".into()),
            (
                "unread_filter",
                if query.unread_only {
                    "unread_message"
                } else {
                    "all"
                }
                .into(),
            ),
        ];
        if !query.custom_types.is_empty() {
            q.push(("custom_types", query.custom_types.join(",")));
        }
        if let Some(key) = &query.metadata_key {
            q.push(("metadata_key", key.clone()));
            q.push(("metadata_values", query.metadata_values.join(",")));
        }
        self.get(&self.user_path("/my_group_channels"), &q).await
    }

    pub async fn all_group_channels(&self, query: &ChannelListQuery) -> Result<Vec<SbChannel>> {
        let mut out = Vec::new();
        let mut token = String::new();
        loop {
            let page = self.my_group_channels(query, &token).await?;
            out.extend(page.channels);
            if page.next.is_empty() || page.next == token {
                return Ok(out);
            }
            token = page.next;
        }
    }

    pub async fn group_channel(&self, channel_url: &str) -> Result<SbChannel> {
        let q = [
            ("show_member", "true".to_string()),
            ("show_read_receipt", "true".into()),
            ("show_delivery_receipt", "true".into()),
            ("show_metadata", "true".into()),
        ];
        self.get(&format!("/group_channels/{}", urlencode(channel_url)), &q)
            .await
    }

    pub async fn messages(
        &self,
        channel_url: &str,
        query: &MessageQuery,
    ) -> Result<Vec<SbMessage>> {
        let ts = if query.anchor_ts == i64::MAX {
            (now_ms() as i64 + 60_000).to_string()
        } else {
            query.anchor_ts.to_string()
        };
        let q = [
            ("is_sdk", "true".to_string()),
            ("message_ts", ts),
            ("prev_limit", query.prev_limit.to_string()),
            ("next_limit", query.next_limit.to_string()),
            ("include", "false".into()),
            ("reverse", "false".into()),
            (
                "include_reply_type",
                if query.include_threads { "all" } else { "none" }.into(),
            ),
            ("include_thread_info", "true".into()),
            ("include_reactions", "true".into()),
            ("include_parent_message_info", "false".into()),
            ("with_sorted_meta_array", "false".into()),
        ];
        let page: MessagePage = self
            .get(
                &format!("/group_channels/{}/messages", urlencode(channel_url)),
                &q,
            )
            .await?;
        Ok(page.messages)
    }

    pub async fn thread_replies(
        &self,
        channel_url: &str,
        parent_id: i64,
        query: &MessageQuery,
    ) -> Result<Vec<SbMessage>> {
        let q = [
            ("is_sdk", "true".to_string()),
            ("message_ts", query.anchor_ts.to_string()),
            ("prev_limit", query.prev_limit.to_string()),
            ("next_limit", query.next_limit.to_string()),
            ("include", "true".into()),
            ("reverse", "false".into()),
            ("parent_message_id", parent_id.to_string()),
            ("include_reply_type", "all".into()),
            ("include_thread_info", "true".into()),
            ("include_reactions", "true".into()),
        ];
        let page: MessagePage = self
            .get(
                &format!("/group_channels/{}/messages", urlencode(channel_url)),
                &q,
            )
            .await?;
        Ok(page.messages)
    }

    pub async fn message_changelogs(
        &self,
        channel_url: &str,
        since_ts: i64,
        token: &str,
    ) -> Result<MessageChangelog> {
        let mut q = vec![
            ("include_thread_info", "true".to_string()),
            ("include_reactions", "true".into()),
            ("include_reply_type", "all".into()),
        ];
        if token.is_empty() {
            q.push(("change_ts", since_ts.to_string()));
        } else {
            q.push(("token", token.to_string()));
        }
        self.get(
            &format!(
                "/group_channels/{}/messages/changelogs",
                urlencode(channel_url)
            ),
            &q,
        )
        .await
    }

    pub async fn channel_changelogs(&self, since_ts: i64, token: &str) -> Result<ChannelChangelog> {
        let mut q = vec![
            ("show_member", "true".to_string()),
            ("show_read_receipt", "true".into()),
            ("show_delivery_receipt", "true".into()),
            ("show_empty", "true".into()),
            ("show_frozen", "true".into()),
            ("show_metadata", "true".into()),
        ];
        if token.is_empty() {
            q.push(("change_ts", since_ts.to_string()));
        } else {
            q.push(("token", token.to_string()));
        }
        self.get(&self.user_path("/my_group_channels/changelogs"), &q)
            .await
    }

    async fn call_raw(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Value> {
        let text_or_null: Value =
            self.call(method, path, query, body)
                .await
                .or_else(|e| match e {
                    CoreError::Decode(_) => Ok(Value::Null),
                    other => Err(other),
                })?;
        Ok(text_or_null)
    }

    pub async fn add_reaction(&self, channel_url: &str, message_id: i64, key: &str) -> Result<()> {
        let body = serde_json::json!({ "reaction": key, "user_id": self.user_id });
        self.call_raw(
            reqwest::Method::POST,
            &format!(
                "/group_channels/{}/messages/{message_id}/reactions",
                urlencode(channel_url)
            ),
            &[],
            Some(&body),
        )
        .await
        .map(|_| ())
    }

    pub async fn remove_reaction(
        &self,
        channel_url: &str,
        message_id: i64,
        key: &str,
    ) -> Result<()> {
        self.call_raw(
            reqwest::Method::DELETE,
            &format!(
                "/group_channels/{}/messages/{message_id}/reactions",
                urlencode(channel_url)
            ),
            &[
                ("reaction", key.to_string()),
                ("user_id", self.user_id.clone()),
            ],
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn delete_message(&self, channel_url: &str, message_id: i64) -> Result<()> {
        self.call_raw(
            reqwest::Method::DELETE,
            &format!(
                "/group_channels/{}/messages/{message_id}",
                urlencode(channel_url)
            ),
            &[],
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn upload_file(&self, upload: &FileUpload) -> Result<UploadedFile> {
        let part = reqwest::multipart::Part::bytes(upload.data.clone())
            .file_name(upload.name.clone())
            .mime_str(&upload.mime_type)
            .map_err(|e| CoreError::Network(e.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("channel_url", upload.channel_url.clone());
        let res = self
            .http
            .post(format!("{}/v3/storage/file", self.base_url))
            .header("SendBird", format!("JS,,{SDK_VERSION},{}", self.app_id))
            .header("SB-User-Agent", user_agent())
            .header("SB-SDK-User-Agent", sdk_user_agent())
            .header("Request-Sent-Timestamp", now_ms().to_string())
            .header("App-Id", &self.app_id)
            .header("Session-Key", self.key())
            .multipart(form)
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            return Err(CoreError::Sendbird {
                code: parsed["code"].as_i64().unwrap_or(status.as_u16().into()),
                message: parsed["message"].as_str().unwrap_or(&text).to_string(),
            });
        }
        let v: Value = serde_json::from_str(&text)?;
        Ok(UploadedFile {
            url: v["url"].as_str().unwrap_or_default().to_string(),
            size: v["file_size"].as_u64().unwrap_or(upload.data.len() as u64),
            require_auth: v["require_auth"].as_bool().unwrap_or(false),
            thumbnails: v.get("thumbnails").cloned().unwrap_or(Value::Null),
        })
    }

    pub async fn total_unread(&self) -> Result<u64> {
        let v: Value = self
            .get(&self.user_path("/unread_message_count"), &[])
            .await?;
        Ok(v["unread_count"].as_u64().unwrap_or_default())
    }

    pub async fn mark_as_read(&self, channel_url: &str) -> Result<()> {
        let body = serde_json::json!({ "user_id": self.user_id });
        let _: Value = self
            .call(
                reqwest::Method::PUT,
                &format!(
                    "/group_channels/{}/messages/mark_as_read",
                    urlencode(channel_url)
                ),
                &[],
                Some(&body),
            )
            .await?;
        Ok(())
    }
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
