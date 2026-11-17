use std::sync::Arc;

use futures_util::future::BoxFuture;
use tokio::sync::mpsc;

use super::store::DirectoryUser;
use crate::client::{ChatConfigChannel, KumoApi};
use crate::config::KumoConfig;
use crate::error::Result;
use crate::sendbird::{
    ChannelListQuery, FileSend, FileUpload, MessageChangelog, MessageQuery, SbChannel, SbEvent,
    SbMessage, SendbirdRest, SendbirdSocket, SocketParams,
};

pub type ChatCall<'a, T> = BoxFuture<'a, Result<T>>;

pub struct ChatLogin {
    pub self_id: String,
    pub self_name: String,
    pub file_key: String,
    pub unread: Vec<ChatConfigChannel>,
}

pub struct ChatConnection {
    pub login: ChatLogin,
    pub link: Arc<dyn ChatLink>,
    pub events: mpsc::Receiver<SbEvent>,
}

pub trait ChatTransport: Send + Sync {
    fn connect<'a>(&'a self, space_name: &'a str) -> ChatCall<'a, ChatConnection>;
}

pub trait ChatLink: Send + Sync {
    fn close(&self) -> BoxFuture<'_, ()>;

    fn send_message<'a>(
        &'a self,
        channel_url: &'a str,
        text: &'a str,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage>;

    fn edit_message<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        text: &'a str,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage>;

    fn send_file<'a>(
        &'a self,
        upload: &'a FileUpload,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage>;

    fn mark_read<'a>(&'a self, channel_url: &'a str, live: bool) -> ChatCall<'a, ()>;

    fn typing<'a>(&'a self, channel_url: &'a str, started: bool) -> ChatCall<'a, ()>;

    fn channels(&self) -> ChatCall<'_, Vec<SbChannel>>;

    fn channel<'a>(&'a self, channel_url: &'a str) -> ChatCall<'a, SbChannel>;

    fn messages<'a>(
        &'a self,
        channel_url: &'a str,
        query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>>;

    fn thread_replies<'a>(
        &'a self,
        channel_url: &'a str,
        parent_id: i64,
        query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>>;

    fn message_changelogs<'a>(
        &'a self,
        channel_url: &'a str,
        since_ts: i64,
        token: &'a str,
    ) -> ChatCall<'a, MessageChangelog>;

    fn react<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        key: &'a str,
        added: bool,
    ) -> ChatCall<'a, ()>;

    fn delete_message<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        is_file: bool,
    ) -> ChatCall<'a, ()>;

    fn delete_placeholder<'a>(&'a self, channel_url: &'a str, message_id: i64) -> ChatCall<'a, ()>;

    fn create_direct_channel<'a>(
        &'a self,
        space_uid: &'a str,
        user_ids: [&'a str; 2],
    ) -> ChatCall<'a, ()>;

    fn directory(&self) -> ChatCall<'_, Vec<DirectoryUser>>;
}

pub struct SendbirdTransport {
    api: KumoApi,
    app_id: String,
    http: reqwest::Client,
}

impl SendbirdTransport {
    pub fn new(api: KumoApi, config: &KumoConfig, http: reqwest::Client) -> Self {
        Self {
            api,
            app_id: config.sendbird_app_id.clone(),
            http,
        }
    }
}

impl ChatTransport for SendbirdTransport {
    fn connect<'a>(&'a self, space_name: &'a str) -> ChatCall<'a, ChatConnection> {
        Box::pin(async move {
            let chat = self.api.chat_config(space_name).await?;
            let connected = SendbirdSocket::connect(&SocketParams {
                app_id: self.app_id.clone(),
                user_id: chat.user_id.clone(),
                access_token: chat.session_token.clone(),
            })
            .await?;
            let rest = SendbirdRest::new(self.http.clone(), &self.app_id, &chat.user_id);
            rest.set_session_key(&connected.login.key);
            Ok(ChatConnection {
                login: ChatLogin {
                    self_id: chat.user_id,
                    self_name: connected.login.nickname,
                    file_key: connected.login.ekey,
                    unread: chat.channels,
                },
                link: Arc::new(SendbirdLink {
                    socket: connected.socket,
                    rest,
                    api: self.api.clone(),
                    space_name: space_name.to_string(),
                }),
                events: connected.events,
            })
        })
    }
}

struct SendbirdLink {
    socket: SendbirdSocket,
    rest: SendbirdRest,
    api: KumoApi,
    space_name: String,
}

impl ChatLink for SendbirdLink {
    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.socket.close())
    }

    fn send_message<'a>(
        &'a self,
        channel_url: &'a str,
        text: &'a str,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        Box::pin(
            self.socket
                .send_message(channel_url, text, req_id, parent_id, data),
        )
    }

    fn edit_message<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        text: &'a str,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        Box::pin(
            self.socket
                .edit_message_with_data(channel_url, message_id, text, data),
        )
    }

    fn send_file<'a>(
        &'a self,
        upload: &'a FileUpload,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        Box::pin(async move {
            let uploaded = self.rest.upload_file(upload).await?;
            let mut msg = self
                .socket
                .send_file(&FileSend {
                    channel_url: &upload.channel_url,
                    file: &uploaded,
                    name: &upload.name,
                    mime_type: &upload.mime_type,
                    req_id,
                    parent_id,
                    data,
                })
                .await?;
            let file = msg.file.get_or_insert_with(Default::default);
            if file.url.is_empty() {
                file.url = uploaded.url.clone();
            }
            if file.name.is_empty() {
                file.name = upload.name.clone();
            }
            if file.mime_type.is_empty() || file.mime_type == "FILE" {
                file.mime_type = upload.mime_type.clone();
            }
            if file.size == 0 {
                file.size = uploaded.size;
            }
            Ok(msg)
        })
    }

    fn mark_read<'a>(&'a self, channel_url: &'a str, live: bool) -> ChatCall<'a, ()> {
        Box::pin(async move {
            if live {
                self.socket.mark_read(channel_url).await
            } else {
                self.rest.mark_as_read(channel_url).await
            }
        })
    }

    fn typing<'a>(&'a self, channel_url: &'a str, started: bool) -> ChatCall<'a, ()> {
        Box::pin(self.socket.typing(channel_url, started))
    }

    fn channels(&self) -> ChatCall<'_, Vec<SbChannel>> {
        Box::pin(async move {
            self.rest
                .all_group_channels(&ChannelListQuery {
                    include_empty: true,
                    limit: 100,
                    ..Default::default()
                })
                .await
        })
    }

    fn channel<'a>(&'a self, channel_url: &'a str) -> ChatCall<'a, SbChannel> {
        Box::pin(self.rest.group_channel(channel_url))
    }

    fn messages<'a>(
        &'a self,
        channel_url: &'a str,
        query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>> {
        Box::pin(self.rest.messages(channel_url, query))
    }

    fn thread_replies<'a>(
        &'a self,
        channel_url: &'a str,
        parent_id: i64,
        query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>> {
        Box::pin(self.rest.thread_replies(channel_url, parent_id, query))
    }

    fn message_changelogs<'a>(
        &'a self,
        channel_url: &'a str,
        since_ts: i64,
        token: &'a str,
    ) -> ChatCall<'a, MessageChangelog> {
        Box::pin(self.rest.message_changelogs(channel_url, since_ts, token))
    }

    fn react<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        key: &'a str,
        added: bool,
    ) -> ChatCall<'a, ()> {
        Box::pin(async move {
            if added {
                self.rest.add_reaction(channel_url, message_id, key).await
            } else {
                self.rest
                    .remove_reaction(channel_url, message_id, key)
                    .await
            }
        })
    }

    fn delete_message<'a>(
        &'a self,
        channel_url: &'a str,
        message_id: i64,
        is_file: bool,
    ) -> ChatCall<'a, ()> {
        Box::pin(async move {
            if is_file
                && self
                    .api
                    .soft_delete_file_message(channel_url, message_id)
                    .await
                    .is_ok()
            {
                return Ok(());
            }
            self.rest.delete_message(channel_url, message_id).await
        })
    }

    fn delete_placeholder<'a>(&'a self, channel_url: &'a str, message_id: i64) -> ChatCall<'a, ()> {
        Box::pin(self.api.delete_chat_message(channel_url, message_id))
    }

    fn create_direct_channel<'a>(
        &'a self,
        space_uid: &'a str,
        user_ids: [&'a str; 2],
    ) -> ChatCall<'a, ()> {
        Box::pin(async move {
            self.api
                .create_direct_channel(space_uid, user_ids)
                .await
                .map(|_| ())
        })
    }

    fn directory(&self) -> ChatCall<'_, Vec<DirectoryUser>> {
        Box::pin(async move {
            let roles = self.api.user_roles(&self.space_name).await?;
            Ok(roles
                .iter()
                .filter_map(|r| {
                    Some(DirectoryUser {
                        user_id: r["userId"].as_str()?.to_string(),
                        name: r["displayName"].as_str().unwrap_or_default().to_string(),
                        avatar_url: r["avatarUrl"].as_str().unwrap_or_default().to_string(),
                    })
                })
                .collect())
        })
    }
}
