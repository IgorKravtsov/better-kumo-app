use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream;

use super::model::{
    Attachment, ChannelCategory, ChannelRow, MAX_ATTACHMENTS, MessageRow, mentions_user,
};
use super::store::{
    ChatStore, DirectoryUser, MessageCursor, OutboxItem, PendingFile, PendingMessage,
    ReactionChange,
};
use super::transport::{ChatLink, ChatTransport};
use crate::error::{CoreError, Result};
use crate::http::now_ms;
use crate::sendbird::{DeletedItem, FileUpload, MessageQuery, SbEvent, SbMessage};

const PAGE: u32 = 50;
pub const DELETED_TEXT: &str = "This message has been deleted.";
const CATCH_UP_PAGE: u32 = 100;
const CATCH_UP_MAX_PAGES: usize = 5;
const PREFETCH_CHANNELS: usize = 40;
const PREFETCH_PARALLEL: usize = 4;
const MAX_BACKOFF_SECS: u64 = 30;
const RECENT_REACTIONS_KEY: &str = "recent_reactions";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Offline,
    Connecting,
    Syncing,
    Online,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    ChannelsChanged,
    Incoming(IncomingMessage),
    MessagesChanged {
        channel_url: String,
    },
    Typing {
        channel_url: String,
        user_names: Vec<String>,
    },
    Connection(ConnectionState),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    pub channel_url: String,
    pub text: String,
    pub files: Vec<Attachment>,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReactionRequest<'a> {
    pub channel_url: &'a str,
    pub message_id: i64,
    pub key: &'a str,
    pub added: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadedPage {
    pub count: u32,
    pub oldest_ts: Option<i64>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingMessage {
    pub channel_url: String,
    pub channel_title: String,
    pub category: ChannelCategory,
    pub message_id: i64,
    pub parent_id: Option<i64>,
    pub created_at: i64,
    pub sender_id: String,
    pub sender_name: String,
    pub text: String,
    pub mentions_me: bool,
    pub channel_is_open: bool,
}

pub trait ChatListener: Send + Sync {
    fn on_event(&self, event: ChatEvent);
}

type TypingUsers = HashMap<String, (String, u64)>;

#[derive(Clone, Default)]
struct Session {
    space_name: String,
    space_uid: String,
    self_id: String,
    self_name: String,
    file_key: String,
    link: Option<Arc<dyn ChatLink>>,
    live: bool,
}

impl Session {
    fn live_link(&self) -> Option<Arc<dyn ChatLink>> {
        self.link.clone().filter(|_| self.live)
    }

    fn take_live(&mut self) -> Option<Arc<dyn ChatLink>> {
        if std::mem::take(&mut self.live) {
            self.link.clone()
        } else {
            None
        }
    }
}

struct Inner {
    transport: Arc<dyn ChatTransport>,
    store: Arc<ChatStore>,
    listener: Arc<dyn ChatListener>,
    session: Mutex<Session>,
    open_channel: Mutex<Option<String>>,
    typing: Mutex<HashMap<String, TypingUsers>>,
    last_read_sent: Mutex<HashMap<String, u64>>,
    directory: Mutex<Option<HashMap<String, DirectoryUser>>>,
    generation: AtomicU64,
}

#[derive(Clone)]
pub struct ChatEngine {
    inner: Arc<Inner>,
}

pub struct ChatEngineDeps {
    pub transport: Arc<dyn ChatTransport>,
    pub store: Arc<ChatStore>,
    pub listener: Arc<dyn ChatListener>,
}

impl ChatEngine {
    pub fn new(deps: ChatEngineDeps) -> Self {
        Self {
            inner: Arc::new(Inner {
                transport: deps.transport,
                store: deps.store,
                listener: deps.listener,
                session: Mutex::new(Session::default()),
                open_channel: Mutex::new(None),
                typing: Mutex::new(HashMap::new()),
                last_read_sent: Mutex::new(HashMap::new()),
                directory: Mutex::new(None),
                generation: AtomicU64::new(0),
            }),
        }
    }

    pub fn store(&self) -> &Arc<ChatStore> {
        &self.inner.store
    }

    pub fn start(&self, space_name: &str, space_uid: &str) {
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut s) = self.inner.session.lock() {
            if let Some(link) = s.take_live() {
                tokio::spawn(async move { link.close().await });
            }
            *s = Session {
                space_name: space_name.to_string(),
                space_uid: space_uid.to_string(),
                ..Default::default()
            };
        }
        self.inner.emit(ChatEvent::ChannelsChanged);
        let inner = self.inner.clone();
        tokio::spawn(async move { inner.run(generation).await });
    }

    pub fn stop(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut s) = self.inner.session.lock()
            && let Some(link) = s.take_live()
        {
            tokio::spawn(async move { link.close().await });
        }
        self.inner
            .emit(ChatEvent::Connection(ConnectionState::Offline));
    }

    pub fn self_id(&self) -> String {
        self.inner.session().self_id
    }

    pub fn directory_snapshot(&self) -> HashMap<String, DirectoryUser> {
        self.inner.directory()
    }

    pub fn authorized_file_url(&self, url: &str) -> String {
        let key = self.inner.session().file_key;
        if key.is_empty()
            || url.is_empty()
            || !url.contains(".sendbird.com/")
            || url.contains("auth=")
        {
            return url.to_string();
        }
        let sep = if url.contains('?') { '&' } else { '?' };
        format!("{url}{sep}auth={key}")
    }

    pub fn channels(&self) -> Result<Vec<ChannelRow>> {
        let session = self.inner.session();
        if session.space_uid.is_empty() {
            return Ok(Vec::new());
        }
        let self_id = self.inner.self_id_or_cached(&session);
        let mut rows = self
            .inner
            .store
            .channels(&format!("{}_", session.space_uid))?;
        let directory = self.inner.directory();
        for row in &mut rows {
            for member in &mut row.members {
                if let Some(avatar) = directory.get(&member.user_id).map(|u| &u.avatar_url)
                    && !avatar.is_empty()
                {
                    member.profile_url = avatar.clone();
                }
            }
            row.title = row.compute_title(&self_id);
        }
        Ok(rows)
    }

    pub fn messages(&self, channel_url: &str, limit: u32) -> Result<Vec<MessageRow>> {
        self.inner
            .with_avatars(self.inner.store.latest_messages(channel_url, limit))
    }

    pub fn messages_before(
        &self,
        channel_url: &str,
        before_ts: i64,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        self.inner.with_avatars(
            self.inner
                .store
                .messages_before(channel_url, before_ts, limit),
        )
    }

    pub fn thread(&self, channel_url: &str, parent_id: i64) -> Result<Vec<MessageRow>> {
        self.inner.with_avatars(
            self.inner
                .store
                .thread_messages(channel_url, parent_id, 500),
        )
    }

    pub fn open(&self, channel_url: &str) {
        if let Ok(mut open) = self.inner.open_channel.lock() {
            *open = Some(channel_url.to_string());
        }
        let inner = self.inner.clone();
        let url = channel_url.to_string();
        tokio::spawn(async move {
            if let Err(e) = inner.catch_up(&url).await {
                inner.emit(ChatEvent::Error(e.to_string()));
            }
            // Closed during the catch-up: what arrived meanwhile was never shown.
            let still_open = inner.open_channel.lock().is_ok_and(|open| open.as_deref() == Some(url.as_str()));
            if still_open {
                inner.mark_read_if_needed(&url).await;
            }
        });
    }

    pub fn close(&self, channel_url: &str) {
        if let Ok(mut open) = self.inner.open_channel.lock()
            && open.as_deref() == Some(channel_url)
        {
            *open = None;
        }
    }

    pub async fn load_older(&self, channel_url: &str, before_ts: i64) -> Result<u32> {
        Ok(self.load_page(channel_url, before_ts).await?.count)
    }

    pub async fn load_page(&self, channel_url: &str, before_ts: i64) -> Result<LoadedPage> {
        let link = self.inner.link()?;
        let cached = self.inner.store.top_level_sent(channel_url, before_ts)?;
        let page = link
            .messages(channel_url, &MessageQuery::before(before_ts, PAGE))
            .await?;
        self.inner.store.upsert_messages(&page)?;
        let loaded = LoadedPage {
            count: page.len() as u32,
            oldest_ts: page.iter().map(|m| m.created_at).filter(|t| *t > 0).min(),
            has_more: page.len() as u32 >= PAGE,
        };
        let covered_after = match (loaded.has_more, loaded.oldest_ts) {
            (true, Some(ts)) => ts,
            (true, None) => before_ts,
            (false, _) => i64::MIN,
        };
        let returned: HashSet<i64> = page.iter().map(|m| m.id()).collect();
        let gone: Vec<i64> = cached
            .iter()
            .filter(|c| c.created_at > covered_after && !returned.contains(&c.id))
            .map(|c| c.id)
            .collect();
        self.inner.store.delete_messages(channel_url, &gone)?;
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(loaded)
    }

    pub async fn load_before(
        &self,
        channel_url: &str,
        before: MessageCursor,
        limit: u32,
    ) -> Result<()> {
        let latest = now_ms() as i64 + 60_000;
        let mut cursor = if before.id == i64::MIN {
            before.created_at.min(latest)
        } else {
            before.created_at.saturating_add(1).min(latest)
        };
        loop {
            let page = self.load_page(channel_url, cursor).await?;
            let (Some(oldest), true) = (page.oldest_ts, page.has_more) else {
                return Ok(());
            };
            let covered = self
                .inner
                .store
                .messages_before_message(channel_url, before, limit)?
                .iter()
                .filter(|m| m.created_at >= oldest)
                .count();
            let next = oldest + 1;
            if covered >= limit as usize || next >= cursor {
                return Ok(());
            }
            cursor = next;
        }
    }

    pub async fn load_thread(&self, channel_url: &str, parent_id: i64) -> Result<()> {
        let link = self.inner.link()?;
        let mut query = MessageQuery::after(0, CATCH_UP_PAGE);
        query.include_threads = true;
        let replies = link.thread_replies(channel_url, parent_id, &query).await?;
        self.inner.store.upsert_messages(&replies)?;
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    pub async fn send_text(
        &self,
        channel_url: &str,
        text: &str,
        parent_id: Option<i64>,
    ) -> Result<()> {
        self.send(OutgoingMessage {
            channel_url: channel_url.to_string(),
            text: text.to_string(),
            files: Vec::new(),
            parent_id,
        })
        .await
    }

    pub async fn send(&self, message: OutgoingMessage) -> Result<()> {
        self.send_batch(message).await.map(|_| ())
    }

    pub async fn send_batch(&self, message: OutgoingMessage) -> Result<Vec<i64>> {
        let text = message.text.trim();
        if message.files.len() > MAX_ATTACHMENTS {
            return Err(CoreError::Invalid(format!(
                "at most {MAX_ATTACHMENTS} attachments per message"
            )));
        }
        if text.is_empty() && message.files.is_empty() {
            return Ok(Vec::new());
        }
        let channel_url = message.channel_url.as_str();
        let session = self.inner.session();
        let link = session.live_link();
        let batch_id = format!(
            "rq-{}-{}",
            now_ms(),
            self.inner.generation.load(Ordering::SeqCst)
        );
        let created_at = now_ms() as i64;
        let caption = (!text.is_empty()).then_some(());
        let parts = message
            .files
            .iter()
            .map(Some)
            .chain(caption.map(|_| None))
            .enumerate();
        let mut first = None;
        for (index, file) in parts {
            let local_id = self.inner.store.insert_pending(&PendingMessage {
                channel_url,
                req_id: &format!("{batch_id}-{index}"),
                batch_id: &batch_id,
                text: file.map(|f| f.name.as_str()).unwrap_or(text),
                sender_id: &session.self_id,
                sender_name: &session.self_name,
                created_at: created_at + index as i64,
                parent_id: message.parent_id,
                file: file.map(|f| PendingFile {
                    name: &f.name,
                    mime_type: &f.mime_type,
                    data: &f.data,
                }),
            })?;
            first.get_or_insert(local_id);
        }
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        match first {
            Some(local_id) => self.inner.flush(link, channel_url, local_id).await,
            None => Ok(Vec::new()),
        }
    }

    pub async fn retry(&self, channel_url: &str, local_id: i64) -> Result<()> {
        let items = self.inner.store.unsent_batch(channel_url, local_id)?;
        if items.is_empty() {
            return Ok(());
        }
        let batch_id = format!("rq-{}-r", now_ms());
        for (index, item) in items.iter().enumerate() {
            self.inner.store.reset_pending(
                channel_url,
                item.local_id,
                &format!("{batch_id}-{index}"),
            )?;
        }
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        let link = self.inner.session().live_link();
        self.inner
            .flush(link, channel_url, local_id)
            .await
            .map(|_| ())
    }

    pub async fn open_direct(&self, user_id: &str) -> Result<String> {
        let session = self.inner.session();
        let self_id = self.inner.self_id_or_cached(&session);
        let url =
            crate::channel::ChannelUrl::direct_message(&session.space_uid, [&self_id, user_id]);
        if self.inner.store.channel(&url)?.is_some() {
            return Ok(url);
        }
        let link = self.inner.link()?;
        match link.channel(&url).await {
            Ok(channel) => self
                .inner
                .store
                .upsert_channels(std::slice::from_ref(&channel))?,
            Err(_) => {
                link.create_direct_channel(&session.space_uid, [&self_id, user_id])
                    .await?;
                let channel = link.channel(&url).await?;
                self.inner
                    .store
                    .upsert_channels(std::slice::from_ref(&channel))?;
            }
        }
        self.inner.emit(ChatEvent::ChannelsChanged);
        Ok(url)
    }

    fn is_my_reaction(&self, channel_url: &str, message_id: i64, key: &str) -> Result<bool> {
        let self_id = self.inner.session().self_id;
        Ok(self
            .inner
            .store
            .message(channel_url, message_id)?
            .is_some_and(|m| {
                m.reactions
                    .iter()
                    .any(|r| r.key == key && r.user_ids.contains(&self_id))
            }))
    }

    /// Adds `key`, or does nothing when it is already this user's reaction (the picker shows no
    /// selected state, so picking an applied emoji must not remove it).
    pub async fn add_reaction(&self, channel_url: &str, message_id: i64, key: &str) -> Result<()> {
        if self.is_my_reaction(channel_url, message_id, key)? {
            return Ok(());
        }
        self.set_reaction(&ReactionRequest {
            channel_url,
            message_id,
            key,
            added: true,
        })
        .await
    }

    pub async fn toggle_reaction(
        &self,
        channel_url: &str,
        message_id: i64,
        key: &str,
    ) -> Result<()> {
        let mine = self.is_my_reaction(channel_url, message_id, key)?;
        self.set_reaction(&ReactionRequest {
            channel_url,
            message_id,
            key,
            added: !mine,
        })
        .await
    }

    pub fn recent_reactions(&self) -> Vec<String> {
        self.inner
            .store
            .get_state(RECENT_REACTIONS_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    pub async fn set_reaction(&self, request: &ReactionRequest<'_>) -> Result<()> {
        let ReactionRequest {
            channel_url,
            message_id,
            key,
            added,
        } = *request;
        let self_id = self.inner.session().self_id;
        self.inner
            .link()?
            .react(channel_url, message_id, key, added)
            .await?;
        self.inner.store.set_reaction(&ReactionChange {
            channel_url,
            message_id,
            key,
            user_id: &self_id,
            added,
        })?;
        if added {
            // Best effort: the reaction is already sent, a lost entry only reorders the quick row.
            let _ = self.inner.store.update_state(RECENT_REACTIONS_KEY, |old| {
                let recent = old
                    .and_then(|json| serde_json::from_str(&json).ok())
                    .unwrap_or_default();
                serde_json::to_string(&super::emoji::remember(recent, key))
                    .unwrap_or_else(|_| "[]".into())
            });
        }
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    pub async fn edit_message(&self, channel_url: &str, message_id: i64, text: &str) -> Result<()> {
        let updated = self
            .inner
            .live_link()?
            .edit_message(channel_url, message_id, text, None)
            .await?;
        self.inner
            .store
            .upsert_messages(std::slice::from_ref(&updated))?;
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    pub async fn delete_message(&self, channel_url: &str, message_id: i64) -> Result<()> {
        let row = self.inner.store.message(channel_url, message_id)?;
        let link = self.inner.link()?;
        let is_file = row.as_ref().is_some_and(|m| m.kind == "FILE");
        match link.delete_message(channel_url, message_id, is_file).await {
            Ok(()) => {}
            Err(CoreError::Sendbird { code: 400920, .. }) => {
                let updated = self
                    .inner
                    .live_link()?
                    .edit_message(
                        channel_url,
                        message_id,
                        DELETED_TEXT,
                        Some(r#"{"deleted":true}"#),
                    )
                    .await?;
                self.inner
                    .store
                    .upsert_messages(std::slice::from_ref(&updated))?;
                self.inner.emit(ChatEvent::MessagesChanged {
                    channel_url: channel_url.to_string(),
                });
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        self.inner.store.delete_message(channel_url, message_id)?;
        if let Some(parent_id) = row.as_ref().map(|r| r.parent_id).filter(|p| *p > 0)
            && let Some(parent) = self.inner.store.message(channel_url, parent_id)?
            && parent.text == DELETED_TEXT
        {
            link.delete_placeholder(channel_url, parent_id).await?;
            self.inner.store.delete_message(channel_url, parent_id)?;
        }
        self.inner.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    pub async fn set_typing(&self, channel_url: &str, typing: bool) {
        if let Some(link) = self.inner.session().live_link() {
            let _ = link.typing(channel_url, typing).await;
        }
    }

    pub async fn mark_read(&self, channel_url: &str) -> Result<()> {
        self.inner.mark_read(channel_url).await
    }
}

impl Inner {
    fn emit(&self, event: ChatEvent) {
        self.listener.on_event(event);
    }

    fn directory(&self) -> HashMap<String, DirectoryUser> {
        let Ok(mut cached) = self.directory.lock() else {
            return HashMap::new();
        };
        if cached.is_none() {
            *cached = Some(self.store.directory().unwrap_or_default());
        }
        cached.clone().unwrap_or_default()
    }

    fn with_avatars(&self, rows: Result<Vec<MessageRow>>) -> Result<Vec<MessageRow>> {
        let mut rows = rows?;
        let directory = self.directory();
        for row in &mut rows {
            if let Some(user) = directory.get(&row.sender_id)
                && !user.avatar_url.is_empty()
            {
                row.sender_avatar = user.avatar_url.clone();
            }
        }
        Ok(rows)
    }

    async fn refresh_directory(&self) -> Result<()> {
        let users = self.link()?.directory().await?;
        self.store.upsert_users(&users)?;
        if let Ok(mut cached) = self.directory.lock() {
            *cached = None;
        }
        self.emit(ChatEvent::ChannelsChanged);
        if let Some(open) = self.open_channel.lock().ok().and_then(|o| o.clone()) {
            self.emit(ChatEvent::MessagesChanged { channel_url: open });
        }
        Ok(())
    }

    fn session(&self) -> Session {
        self.session.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn self_id_or_cached(&self, session: &Session) -> String {
        if !session.self_id.is_empty() {
            return session.self_id.clone();
        }
        self.store
            .get_state("self_id")
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn link(&self) -> Result<Arc<dyn ChatLink>> {
        self.session()
            .link
            .ok_or(CoreError::Socket("chat is offline".into()))
    }

    fn live_link(&self) -> Result<Arc<dyn ChatLink>> {
        self.session()
            .live_link()
            .ok_or(CoreError::Socket("chat is offline".into()))
    }

    fn alive(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn is_open(&self, channel_url: &str) -> bool {
        self.open_channel
            .lock()
            .map(|o| o.as_deref() == Some(channel_url))
            .unwrap_or(false)
    }

    async fn run(self: Arc<Self>, generation: u64) {
        let mut attempt: u32 = 0;
        while self.alive(generation) {
            self.emit(ChatEvent::Connection(ConnectionState::Connecting));
            match self.connect(generation).await {
                Ok(mut events) => {
                    attempt = 0;
                    self.emit(ChatEvent::Connection(ConnectionState::Syncing));
                    let syncer = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = syncer.sync(generation).await {
                            syncer.emit(ChatEvent::Error(e.to_string()));
                        }
                        if syncer.alive(generation) {
                            syncer.emit(ChatEvent::Connection(ConnectionState::Online));
                        }
                    });
                    while let Some(event) = events.recv().await {
                        if !self.alive(generation) {
                            return;
                        }
                        if let SbEvent::Disconnected { reason } = &event {
                            self.emit(ChatEvent::Error(format!("chat disconnected: {reason}")));
                            break;
                        }
                        self.handle(event).await;
                    }
                }
                Err(e) => self.emit(ChatEvent::Error(e.to_string())),
            }
            if let Ok(mut s) = self.session.lock() {
                s.live = false;
            }
            if !self.alive(generation) {
                return;
            }
            self.emit(ChatEvent::Connection(ConnectionState::Offline));
            attempt += 1;
            let delay = (1u64 << attempt.min(5)).min(MAX_BACKOFF_SECS);
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
    }

    async fn connect(&self, generation: u64) -> Result<tokio::sync::mpsc::Receiver<SbEvent>> {
        let space_name = self.session().space_name;
        let connected = self.transport.connect(&space_name).await?;
        if !self.alive(generation) {
            connected.link.close().await;
            return Err(CoreError::Socket("stale".into()));
        }
        let login = connected.login;
        let _ = self.store.set_state("self_id", &login.self_id);
        let mut file_key_changed = false;
        if let Ok(mut s) = self.session.lock() {
            s.self_id = login.self_id.clone();
            s.self_name = login.self_name.clone();
            file_key_changed = s.file_key != login.file_key;
            s.file_key = login.file_key.clone();
            s.link = Some(connected.link);
            s.live = true;
        }
        if file_key_changed {
            let open = self.open_channel.lock().ok().and_then(|o| o.clone());
            if let Some(channel_url) = open {
                self.emit(ChatEvent::MessagesChanged { channel_url });
            }
        }
        let unread: Vec<_> = login
            .unread
            .iter()
            .map(|c| {
                (
                    c.channel_url.clone(),
                    c.unread_message_count,
                    c.unread_mention_count,
                )
            })
            .collect();
        for (url, unread, mentions) in unread {
            let _ = self.store.set_unread(&url, unread, mentions);
        }
        Ok(connected.events)
    }

    async fn sync(self: &Arc<Self>, generation: u64) -> Result<()> {
        let channels = self.link()?.channels().await?;
        if !self.alive(generation) {
            return Ok(());
        }
        let prefix = format!("{}_", self.session().space_uid);
        let known: HashSet<String> = channels.iter().map(|c| c.channel_url.clone()).collect();
        let stale: Vec<String> = self
            .store
            .channels(&prefix)?
            .into_iter()
            .filter(|c| !known.contains(&c.url))
            .map(|c| c.url)
            .collect();
        self.store.remove_channels(&stale)?;
        self.store.upsert_channels(&channels)?;
        self.store
            .set_state("channels_synced_at", &now_ms().to_string())?;
        self.emit(ChatEvent::ChannelsChanged);
        let directory = self.clone();
        tokio::spawn(async move {
            if let Err(e) = directory.refresh_directory().await {
                directory.emit(ChatEvent::Error(format!("members: {e}")));
            }
        });

        let mut targets: Vec<_> = channels
            .iter()
            .filter(|c| c.channel_url.starts_with(&prefix))
            .collect();
        targets.sort_by_key(|c| std::cmp::Reverse((c.unread_message_count > 0, c.last_activity())));
        let mut urls: Vec<String> = Vec::new();
        for (rank, c) in targets.into_iter().enumerate() {
            let Some(last) = &c.last_message else {
                continue;
            };
            if rank >= PREFETCH_CHANNELS && c.unread_message_count == 0 {
                break;
            }
            if !self.store.has_message(&c.channel_url, last.id())? {
                urls.push(c.channel_url.clone());
            }
        }
        let open = self.open_channel.lock().ok().and_then(|o| o.clone());
        if let Some(open) = open {
            urls.retain(|u| *u != open);
            urls.insert(0, open);
        }
        stream::iter(urls)
            .map(|url| {
                let me = self.clone();
                async move {
                    if me.alive(generation)
                        && let Err(e) = me.catch_up(&url).await
                    {
                        me.emit(ChatEvent::Error(e.to_string()));
                    }
                }
            })
            .buffer_unordered(PREFETCH_PARALLEL)
            .collect::<Vec<_>>()
            .await;
        Ok(())
    }

    async fn apply_changelog(&self, link: &dyn ChatLink, channel_url: &str) -> Result<bool> {
        let key = format!("changelog_ts:{channel_url}");
        let started = now_ms() as i64;
        let stored = self
            .store
            .get_state(&key)?
            .and_then(|v| v.parse::<i64>().ok());
        let Some(since) = stored.or(self.store.oldest_message_ts(channel_url)?) else {
            self.store.set_state(&key, &started.to_string())?;
            return Ok(false);
        };
        let mut token = String::new();
        let mut changed = false;
        for _ in 0..10 {
            let log = link.message_changelogs(channel_url, since, &token).await?;
            if !log.updated.is_empty() {
                self.store.upsert_messages(&log.updated)?;
                changed = true;
            }
            for deleted in &log.deleted {
                let id = match deleted {
                    DeletedItem::Id(id) => Some(*id),
                    DeletedItem::Object(v) => {
                        v["message_id"].as_i64().or_else(|| v["msg_id"].as_i64())
                    }
                    DeletedItem::Url(_) => None,
                };
                if let Some(id) = id {
                    self.store.delete_message(channel_url, id)?;
                    changed = true;
                }
            }
            if !log.has_more || log.next.is_empty() {
                break;
            }
            token = log.next;
        }
        self.store.set_state(&key, &started.to_string())?;
        Ok(changed)
    }

    async fn catch_up(&self, channel_url: &str) -> Result<()> {
        let link = self.link()?;
        if self.apply_changelog(link.as_ref(), channel_url).await? {
            self.emit(ChatEvent::MessagesChanged {
                channel_url: channel_url.to_string(),
            });
        }
        let newest = self.store.newest_message(channel_url)?;
        let mut fetched_any = false;
        if let Some((_, mut ts)) = newest {
            for _ in 0..CATCH_UP_MAX_PAGES {
                let page = link
                    .messages(channel_url, &MessageQuery::after(ts, CATCH_UP_PAGE))
                    .await?;
                let done = (page.len() as u32) < CATCH_UP_PAGE;
                if let Some(last) = page.iter().map(|m| m.created_at).max() {
                    ts = last;
                }
                fetched_any |= !page.is_empty();
                self.store.upsert_messages(&page)?;
                if done {
                    if fetched_any {
                        self.emit(ChatEvent::MessagesChanged {
                            channel_url: channel_url.to_string(),
                        });
                    }
                    return Ok(());
                }
            }
        }
        let page = link
            .messages(channel_url, &MessageQuery::latest(PAGE))
            .await?;
        self.store.upsert_messages(&page)?;
        self.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    async fn flush(
        &self,
        link: Option<Arc<dyn ChatLink>>,
        channel_url: &str,
        local_id: i64,
    ) -> Result<Vec<i64>> {
        let items = self.store.unsent_batch(channel_url, local_id)?;
        let group = (items.len() > 1).then(group_data);
        let mut delivered = false;
        let mut ids = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            match self
                .deliver(link.as_deref(), channel_url, item, group.as_deref())
                .await
            {
                Ok(acked) => {
                    self.store.apply_live_message(&acked, false, false)?;
                    ids.push(acked.id());
                    delivered = true;
                    self.emit(ChatEvent::MessagesChanged {
                        channel_url: channel_url.to_string(),
                    });
                }
                Err(e) => {
                    for rest in &items[index..] {
                        self.store.mark_failed(&rest.req_id)?;
                    }
                    self.emit(ChatEvent::MessagesChanged {
                        channel_url: channel_url.to_string(),
                    });
                    if delivered {
                        self.emit(ChatEvent::ChannelsChanged);
                    }
                    return Err(e);
                }
            }
        }
        if delivered {
            self.emit(ChatEvent::ChannelsChanged);
        }
        Ok(ids)
    }

    async fn deliver(
        &self,
        link: Option<&dyn ChatLink>,
        channel_url: &str,
        item: &OutboxItem,
        group: Option<&str>,
    ) -> Result<SbMessage> {
        let Some(link) = link else {
            return Err(CoreError::Socket("chat is offline".into()));
        };
        match &item.file {
            Some(file) => {
                let upload = FileUpload {
                    channel_url: channel_url.to_string(),
                    name: file.name.clone(),
                    mime_type: file.mime_type.clone(),
                    data: file.data.clone(),
                };
                link.send_file(&upload, &item.req_id, item.parent_id, group)
                    .await
            }
            None => {
                link.send_message(channel_url, &item.text, &item.req_id, item.parent_id, group)
                    .await
            }
        }
    }

    async fn mark_read(&self, channel_url: &str) -> Result<()> {
        let now = now_ms();
        if let Ok(mut sent) = self.last_read_sent.lock() {
            if sent.get(channel_url).is_some_and(|t| now < t + 1000) {
                return Ok(());
            }
            sent.insert(channel_url.to_string(), now);
        }
        match self.session().live_link() {
            Some(link) => link.mark_read(channel_url, true).await?,
            None => self.link()?.mark_read(channel_url, false).await?,
        }
        self.store.set_unread(channel_url, 0, 0)?;
        self.emit(ChatEvent::ChannelsChanged);
        Ok(())
    }

    async fn mark_read_if_needed(&self, channel_url: &str) {
        let unread = self
            .store
            .channel(channel_url)
            .ok()
            .flatten()
            .map(|c| c.unread)
            .unwrap_or_default();
        if unread > 0
            && let Err(e) = self.mark_read(channel_url).await
        {
            self.emit(ChatEvent::Error(e.to_string()));
        }
    }

    async fn handle(&self, event: SbEvent) {
        let self_id = self.session().self_id;
        let result: Result<()> = match event {
            SbEvent::Message(msg) => self.on_message(&msg, &self_id).await,
            SbEvent::Updated(msg) => {
                let r = self.store.upsert_messages(std::slice::from_ref(&msg));
                self.emit(ChatEvent::MessagesChanged {
                    channel_url: msg.channel_url.clone(),
                });
                r
            }
            SbEvent::Deleted {
                channel_url,
                message_id,
            } => {
                let r = self.store.delete_message(&channel_url, message_id);
                self.emit(ChatEvent::MessagesChanged { channel_url });
                r
            }
            SbEvent::Read {
                channel_url,
                user_id,
                ..
            } if user_id == self_id => {
                let r = self.store.set_unread(&channel_url, 0, 0);
                self.emit(ChatEvent::ChannelsChanged);
                r
            }
            SbEvent::Typing {
                channel_url,
                user_id,
                started,
            } => {
                self.on_typing(&channel_url, &user_id, started);
                Ok(())
            }
            SbEvent::Reaction { channel_url, .. } | SbEvent::ThreadInfo { channel_url, .. } => {
                self.refresh_recent(&channel_url).await
            }
            SbEvent::Channel { channel_url, .. } if !channel_url.is_empty() => {
                self.refresh_channel(&channel_url).await
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.emit(ChatEvent::Error(e.to_string()));
        }
    }

    async fn on_message(&self, msg: &SbMessage, self_id: &str) -> Result<()> {
        let mine = msg.sender_id() == Some(self_id);
        let open = self.is_open(&msg.channel_url);
        let known = self.store.channel(&msg.channel_url)?.is_some();
        self.store
            .apply_live_message(msg, !mine && !open, mentions_user(&msg.message, self_id))?;
        self.on_typing(&msg.channel_url, msg.sender_id().unwrap_or_default(), false);
        self.emit(ChatEvent::MessagesChanged {
            channel_url: msg.channel_url.clone(),
        });
        self.emit(ChatEvent::ChannelsChanged);
        if !known {
            self.refresh_channel(&msg.channel_url).await?;
        }
        if !mine && let Some(channel) = self.store.channel(&msg.channel_url)? {
            self.emit(ChatEvent::Incoming(IncomingMessage {
                channel_url: msg.channel_url.clone(),
                channel_title: channel.compute_title(self_id),
                category: channel.category(),
                message_id: msg.id(),
                parent_id: msg.parent_message_id.filter(|id| *id > 0),
                created_at: msg.created_at,
                sender_id: msg.sender_id().unwrap_or_default().to_string(),
                sender_name: msg
                    .user
                    .as_ref()
                    .map(|u| u.nickname.clone())
                    .unwrap_or_default(),
                text: msg.message.clone(),
                mentions_me: mentions_user(&msg.message, self_id),
                channel_is_open: open,
            }));
        }
        if open && !mine {
            self.mark_read(&msg.channel_url).await?;
        }
        Ok(())
    }

    async fn refresh_channel(&self, channel_url: &str) -> Result<()> {
        let channel = self.link()?.channel(channel_url).await?;
        self.store.upsert_channels(std::slice::from_ref(&channel))?;
        self.emit(ChatEvent::ChannelsChanged);
        Ok(())
    }

    async fn refresh_recent(&self, channel_url: &str) -> Result<()> {
        let page = self
            .link()?
            .messages(channel_url, &MessageQuery::latest(PAGE))
            .await?;
        self.store.upsert_messages(&page)?;
        self.emit(ChatEvent::MessagesChanged {
            channel_url: channel_url.to_string(),
        });
        Ok(())
    }

    fn on_typing(&self, channel_url: &str, user_id: &str, started: bool) {
        if user_id.is_empty() || user_id == self.session().self_id {
            return;
        }
        let names = {
            let Ok(mut typing) = self.typing.lock() else {
                return;
            };
            let entry = typing.entry(channel_url.to_string()).or_default();
            let changed = if started {
                let name = self
                    .store
                    .channel(channel_url)
                    .ok()
                    .flatten()
                    .and_then(|c| c.members.into_iter().find(|m| m.user_id == user_id))
                    .map(|m| m.nickname)
                    .unwrap_or_default();
                entry
                    .insert(user_id.to_string(), (name, now_ms()))
                    .is_none()
            } else {
                entry.remove(user_id).is_some()
            };
            if !changed {
                return;
            }
            entry.values().map(|(n, _)| n.clone()).collect::<Vec<_>>()
        };
        self.emit(ChatEvent::Typing {
            channel_url: channel_url.to_string(),
            user_names: names,
        });
    }
}

fn group_data() -> String {
    use std::hash::{BuildHasher, Hasher};
    let random = || {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    };
    let (high, low) = (random(), random());
    let bits = (u128::from(high) << 64 | u128::from(low)) & !(0xF000u128 << 64) & !(0xC0u128 << 56)
        | (0x4000u128 << 64)
        | (0x80u128 << 56);
    let hex = format!("{bits:032x}");
    serde_json::json!({
        "msgGroupId": format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
    })
    .to_string()
}
