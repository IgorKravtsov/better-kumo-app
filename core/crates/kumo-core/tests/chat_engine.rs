use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use kumo_core::CoreError;
use kumo_core::chat::alert::{Alert, AlertKind, message_alert};
use kumo_core::chat::store::DirectoryUser;
use kumo_core::chat::transport::ChatCall;
use kumo_core::chat::{
    Attachment, ChatConnection, ChatEngine, ChatEngineDeps, ChatEvent, ChatLink, ChatListener,
    ChatLogin, ChatStore, ChatTransport, ConnectionState, MessageCursor, MessageRow, MessageState,
    OutgoingMessage,
};
use kumo_core::sendbird::{
    DeletedItem, FileUpload, Frame, MessageChangelog, MessageQuery, SbChannel, SbEvent, SbMember,
    SbMessage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

const SPACE: &str = "office";
const SPACE_UID: &str = "sp";
const DM: &str = "sp_amy_me";
const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Remote {
    channels: Vec<SbChannel>,
    messages: HashMap<String, Vec<SbMessage>>,
    changelogs: HashMap<String, MessageChangelog>,
    sockets: Vec<mpsc::Sender<SbEvent>>,
    next_id: i64,
    sent: Vec<Sent>,
    groups: Vec<Option<String>>,
    fail_at: Option<usize>,
    live_during_page: Option<(Arc<ChatStore>, SbMessage)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    File {
        name: String,
        bytes: usize,
        parent_id: Option<i64>,
    },
    Text {
        text: String,
        parent_id: Option<i64>,
    },
}

impl Remote {
    fn accept(&mut self, sent: Sent, data: Option<&str>) -> Result<i64, CoreError> {
        if self.fail_at == Some(self.sent.len()) {
            self.fail_at = None;
            return Err(CoreError::Network("upload interrupted".into()));
        }
        self.sent.push(sent);
        self.groups.push(data.map(|d| {
            serde_json::from_str::<Value>(d).unwrap()["msgGroupId"]
                .as_str()
                .unwrap()
                .to_string()
        }));
        self.next_id += 1;
        Ok(self.next_id)
    }
}

#[derive(Clone, Default)]
struct FakeSendbird {
    remote: Arc<Mutex<Remote>>,
}

impl FakeSendbird {
    fn with<T>(&self, f: impl FnOnce(&mut Remote) -> T) -> T {
        f(&mut self.remote.lock().unwrap())
    }

    fn socket(&self, index: usize) -> mpsc::Sender<SbEvent> {
        self.with(|r| r.sockets[index].clone())
    }
}

impl ChatTransport for FakeSendbird {
    fn connect<'a>(&'a self, _space_name: &'a str) -> ChatCall<'a, ChatConnection> {
        Box::pin(async move {
            let (tx, events) = mpsc::channel(64);
            self.with(|r| r.sockets.push(tx));
            Ok(ChatConnection {
                login: ChatLogin {
                    self_id: "me".into(),
                    self_name: "Me".into(),
                    file_key: String::new(),
                    unread: Vec::new(),
                },
                link: Arc::new(self.clone()),
                events,
            })
        })
    }
}

impl ChatLink for FakeSendbird {
    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }

    fn send_message<'a>(
        &'a self,
        channel_url: &'a str,
        text: &'a str,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        Box::pin(async move {
            self.with(|r| {
                let next = r.accept(
                    Sent::Text {
                        text: text.to_string(),
                        parent_id,
                    },
                    data,
                )?;
                let mut acked = message(channel_url, 1000 + next, 500 + next, text);
                acked.user = Some(
                    serde_json::from_value(json!({"user_id": "me", "nickname": "Me"})).unwrap(),
                );
                acked.req_id = Some(req_id.to_string());
                acked.parent_message_id = parent_id;
                r.messages
                    .entry(channel_url.to_string())
                    .or_default()
                    .push(acked.clone());
                Ok(acked)
            })
        })
    }

    fn edit_message<'a>(
        &'a self,
        _channel_url: &'a str,
        _message_id: i64,
        _text: &'a str,
        _data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        unsupported()
    }

    fn send_file<'a>(
        &'a self,
        upload: &'a FileUpload,
        req_id: &'a str,
        parent_id: Option<i64>,
        data: Option<&'a str>,
    ) -> ChatCall<'a, SbMessage> {
        Box::pin(async move {
            self.with(|r| {
                let next = r.accept(
                    Sent::File {
                        name: upload.name.clone(),
                        bytes: upload.data.len(),
                        parent_id,
                    },
                    data,
                )?;
                Ok(serde_json::from_value::<SbMessage>(json!({
                    "msg_id": 1000 + next, "type": "FILE", "channel_url": upload.channel_url,
                    "message": upload.name, "ts": 500 + next, "req_id": req_id,
                    "url": format!("https://file-us-3.sendbird.com/{}", upload.name),
                    "name": upload.name, "size": upload.data.len(), "parent_message_id": parent_id,
                    "user": {"user_id": "me", "nickname": "Me"}
                }))
                .unwrap()
                .normalized())
            })
        })
    }

    fn mark_read<'a>(&'a self, _channel_url: &'a str, _live: bool) -> ChatCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn typing<'a>(&'a self, _channel_url: &'a str, _started: bool) -> ChatCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn channels(&self) -> ChatCall<'_, Vec<SbChannel>> {
        Box::pin(async move { Ok(self.with(|r| r.channels.clone())) })
    }

    fn channel<'a>(&'a self, channel_url: &'a str) -> ChatCall<'a, SbChannel> {
        Box::pin(async move {
            self.with(|r| {
                r.channels
                    .iter()
                    .find(|c| c.channel_url == channel_url)
                    .cloned()
            })
            .ok_or(CoreError::Sendbird {
                code: 400201,
                message: "not found".into(),
            })
        })
    }

    fn messages<'a>(
        &'a self,
        channel_url: &'a str,
        query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>> {
        Box::pin(async move {
            if let Some((store, live)) = self.with(|r| r.live_during_page.take()) {
                store.upsert_messages(&[live]).unwrap();
            }
            let mut all = self.with(|r| r.messages.get(channel_url).cloned().unwrap_or_default());
            all.sort_by_key(|m| m.created_at);
            let after: Vec<SbMessage> = all
                .iter()
                .filter(|m| m.created_at > query.anchor_ts)
                .take(query.next_limit as usize)
                .cloned()
                .collect();
            let before: Vec<SbMessage> = all
                .iter()
                .filter(|m| m.created_at < query.anchor_ts)
                .cloned()
                .collect();
            let skip = before.len().saturating_sub(query.prev_limit as usize);
            Ok(before.into_iter().skip(skip).chain(after).collect())
        })
    }

    fn thread_replies<'a>(
        &'a self,
        _channel_url: &'a str,
        _parent_id: i64,
        _query: &'a MessageQuery,
    ) -> ChatCall<'a, Vec<SbMessage>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn message_changelogs<'a>(
        &'a self,
        channel_url: &'a str,
        _since_ts: i64,
        _token: &'a str,
    ) -> ChatCall<'a, MessageChangelog> {
        Box::pin(
            async move { Ok(self.with(|r| r.changelogs.remove(channel_url).unwrap_or_default())) },
        )
    }

    fn react<'a>(
        &'a self,
        _channel_url: &'a str,
        _message_id: i64,
        _key: &'a str,
        _added: bool,
    ) -> ChatCall<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn delete_message<'a>(
        &'a self,
        _channel_url: &'a str,
        _message_id: i64,
        _is_file: bool,
    ) -> ChatCall<'a, ()> {
        unsupported()
    }

    fn delete_placeholder<'a>(
        &'a self,
        _channel_url: &'a str,
        _message_id: i64,
    ) -> ChatCall<'a, ()> {
        unsupported()
    }

    fn create_direct_channel<'a>(
        &'a self,
        _space_uid: &'a str,
        _user_ids: [&'a str; 2],
    ) -> ChatCall<'a, ()> {
        unsupported()
    }

    fn directory(&self) -> ChatCall<'_, Vec<DirectoryUser>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn unsupported<'a, T: Send + 'a>() -> ChatCall<'a, T> {
    Box::pin(async { Err(CoreError::Socket("not scripted".into())) })
}

struct Events(mpsc::UnboundedSender<ChatEvent>);

impl ChatListener for Events {
    fn on_event(&self, event: ChatEvent) {
        let _ = self.0.send(event);
    }
}

struct Harness {
    engine: ChatEngine,
    remote: FakeSendbird,
    events: mpsc::UnboundedReceiver<ChatEvent>,
}

impl Harness {
    fn new(store: ChatStore, remote: FakeSendbird) -> Self {
        let (tx, events) = mpsc::unbounded_channel();
        let engine = ChatEngine::new(ChatEngineDeps {
            transport: Arc::new(remote.clone()),
            store: Arc::new(store),
            listener: Arc::new(Events(tx)),
        });
        Self {
            engine,
            remote,
            events,
        }
    }

    async fn wait_for<T>(&mut self, mut pick: impl FnMut(ChatEvent) -> Option<T>) -> T {
        tokio::time::timeout(WAIT, async {
            loop {
                let event = self.events.recv().await.expect("listener dropped");
                if let Some(found) = pick(event) {
                    return found;
                }
            }
        })
        .await
        .expect("timed out waiting for engine event")
    }

    async fn start_online(&mut self) {
        self.engine.start(SPACE, SPACE_UID);
        self.wait_for(|e| (e == ChatEvent::Connection(ConnectionState::Online)).then_some(()))
            .await;
    }

    fn store(&self) -> &ChatStore {
        self.engine.store()
    }
}

fn message(channel_url: &str, id: i64, ts: i64, text: &str) -> SbMessage {
    serde_json::from_value(json!({
        "message_id": id, "type": "MESG", "channel_url": channel_url, "message": text,
        "created_at": ts, "updated_at": ts, "user": {"user_id": "amy", "nickname": "Amy"}
    }))
    .unwrap()
}

fn direct_channel(last: Option<SbMessage>) -> SbChannel {
    SbChannel {
        channel_url: DM.into(),
        custom_type: "user".into(),
        member_count: 2,
        members: ["amy", "me"]
            .into_iter()
            .map(|id| SbMember {
                user_id: id.into(),
                nickname: if id == "amy" { "Amy" } else { "Me" }.into(),
                ..Default::default()
            })
            .collect(),
        last_message: last,
        ..Default::default()
    }
}

fn live(frame: &str) -> SbEvent {
    SbEvent::from_frame(Frame::decode_all(frame).remove(0))
}

fn remote_with_dm() -> FakeSendbird {
    let remote = FakeSendbird::default();
    remote.with(|r| r.channels.push(direct_channel(None)));
    remote
}

#[tokio::test]
async fn live_file_message_is_stored_as_file_and_alerts() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    h.remote
        .socket(0)
        .send(live(
            r#"FILE{"msg_id":77,"channel_url":"sp_amy_me","type":"image/png","url":"https://f.sendbird.com/cat.png","name":"cat.png","size":3,"ts":900,"user":{"user_id":"amy","nickname":"Amy"}}"#,
        ))
        .await
        .unwrap();
    let incoming = h
        .wait_for(|e| match e {
            ChatEvent::Incoming(m) => Some(m),
            _ => None,
        })
        .await;

    let row = h.store().message(DM, 77).unwrap().unwrap();
    assert_eq!(
        (
            row.kind.as_str(),
            row.file_type.as_str(),
            row.file_name.as_str(),
            row.file_url.as_str()
        ),
        (
            "FILE",
            "image/png",
            "cat.png",
            "https://f.sendbird.com/cat.png"
        )
    );
    assert_eq!(h.engine.channels().unwrap()[0].unread, 1);
    assert_eq!(
        message_alert(&incoming),
        Some(Alert {
            kind: AlertKind::Message,
            title: "Amy".into(),
            body: "Sent a file".into(),
            channel_url: Some(DM.into()),
            person_id: None,
        })
    );
}

#[tokio::test]
async fn a_live_mention_in_a_public_channel_alerts_in_every_wire_format() {
    let remote = FakeSendbird::default();
    remote.with(|r| {
        r.channels.push(SbChannel {
            channel_url: "sp_c_design".into(),
            custom_type: "custom".into(),
            name: "Design | sp".into(),
            ..direct_channel(None)
        })
    });
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote);
    h.start_online().await;

    let cases = [
        ("hey @mention(type=user&id=me) look", true),
        ("hey @mention(type=user&id=me&name=Me%20Myself) look", true),
        ("hey @mention(type=user&name=Me&id=me) look", true),
        ("hey @mention(type=here) look", true),
        ("hey @mention(type=channel) look", true),
        ("[FYI] @mention(id=me) [spec](https://x.io/spec)", true),
        ("hey @mention(type=user&id=bo&name=Bo) look", false),
        ("type `@mention(type=user&id=me&name=Me)` to tag", false),
        ("see https://x.io/?q=@mention(type=user&id=me)&p=2", false),
    ];
    for (id, (text, mentioned)) in (1..).zip(cases) {
        let frame = serde_json::json!({
            "msg_id": id, "channel_url": "sp_c_design", "message": text, "ts": 900 + id,
            "user": {"user_id": "amy", "nickname": "Amy"},
        });
        h.remote
            .socket(0)
            .send(live(&format!("MESG{frame}")))
            .await
            .unwrap();
        let incoming = h
            .wait_for(|e| match e {
                ChatEvent::Incoming(m) => Some(m),
                _ => None,
            })
            .await;
        assert_eq!(incoming.mentions_me, mentioned, "{text}");
        assert_eq!(
            message_alert(&incoming).map(|a| (a.kind, a.title)),
            mentioned.then(|| (AlertKind::Mention, "Amy · Design".to_string())),
            "{text}"
        );
    }
    let alert = |text: &str| {
        message_alert(&kumo_core::chat::IncomingMessage {
            channel_url: "sp_c_design".into(),
            channel_title: "Design".into(),
            category: kumo_core::chat::ChannelCategory::Channel,
            message_id: 1,
            parent_id: None,
            created_at: 0,
            sender_id: "amy".into(),
            sender_name: "Amy".into(),
            text: text.into(),
            mentions_me: true,
            channel_is_open: false,
        })
        .unwrap()
        .body
    };
    assert_eq!(
        alert("hey @mention(type=user&id=me&name=Me%20Myself) see https://x.io/?q=@mention(id=me)"),
        "hey @Me Myself see https://x.io/?q=@mention(id=me)"
    );
    assert_eq!(h.engine.channels().unwrap()[0].mentions, 6);
}

#[tokio::test]
async fn events_from_a_replaced_connection_are_ignored() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    h.start_online().await;
    let stale = h.remote.socket(0);

    stale
        .send(live(
            r#"MESG{"msg_id":5,"channel_url":"sp_amy_me","message":"late","ts":800,"user":{"user_id":"amy","nickname":"Amy"}}"#,
        ))
        .await
        .unwrap();
    tokio::time::timeout(WAIT, stale.closed())
        .await
        .expect("stale connection was not dropped");

    assert!(h.store().message(DM, 5).unwrap().is_none());

    h.remote
        .socket(1)
        .send(live(
            r#"MESG{"msg_id":6,"channel_url":"sp_amy_me","message":"fresh","ts":810,"user":{"user_id":"amy","nickname":"Amy"}}"#,
        ))
        .await
        .unwrap();
    h.wait_for(|e| matches!(e, ChatEvent::Incoming(_)).then_some(()))
        .await;
    assert_eq!(h.store().message(DM, 6).unwrap().unwrap().text, "fresh");
}

#[tokio::test]
async fn failed_send_is_delivered_on_retry_once_connected() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());

    assert!(h.engine.send_text(DM, "hello", None).await.is_err());
    let failed = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].state, MessageState::Failed);

    h.start_online().await;
    h.engine.retry(DM, failed[0].id).await.unwrap();

    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].state, rows[0].text.as_str(), rows[0].id > 0),
        (MessageState::Sent, "hello", true)
    );
    assert_eq!(h.remote.with(|r| r.messages[DM].len()), 1);
}

#[tokio::test]
async fn catch_up_on_connect_applies_changelog_and_new_messages() {
    let store = ChatStore::in_memory().unwrap();
    store.upsert_channels(&[direct_channel(None)]).unwrap();
    store
        .upsert_messages(&[
            message(DM, 1, 10, "before edit"),
            message(DM, 2, 20, "doomed"),
        ])
        .unwrap();

    let remote = FakeSendbird::default();
    let newest = message(DM, 3, 30, "while away");
    let mut edited = message(DM, 1, 10, "after edit");
    edited.updated_at = 25;
    remote.with(|r| {
        r.channels.push(direct_channel(Some(newest.clone())));
        r.messages.insert(DM.into(), vec![edited.clone(), newest]);
        r.changelogs.insert(
            DM.into(),
            MessageChangelog {
                updated: vec![edited],
                deleted: vec![DeletedItem::Id(2)],
                ..Default::default()
            },
        );
    });

    let mut h = Harness::new(store, remote);
    h.start_online().await;

    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(
        rows.iter()
            .map(|m| (m.id, m.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "after edit"), (3, "while away")]
    );
}

fn photo(name: &str, bytes: usize) -> Attachment {
    Attachment {
        name: name.into(),
        mime_type: "image/jpeg".into(),
        data: vec![7; bytes],
    }
}

fn with_photos(text: &str, photos: Vec<Attachment>, parent_id: Option<i64>) -> OutgoingMessage {
    OutgoingMessage {
        channel_url: DM.into(),
        text: text.into(),
        files: photos,
        parent_id,
    }
}

fn summary(rows: &[MessageRow]) -> Vec<(&str, &str, MessageState)> {
    rows.iter()
        .map(|m| {
            let label = if m.kind == "FILE" {
                m.file_name.as_str()
            } else {
                m.text.as_str()
            };
            (m.kind.as_str(), label, m.state)
        })
        .collect()
}

fn file(name: &str, bytes: usize, parent_id: Option<i64>) -> Sent {
    Sent::File {
        name: name.into(),
        bytes,
        parent_id,
    }
}

fn text(body: &str, parent_id: Option<i64>) -> Sent {
    Sent::Text {
        text: body.into(),
        parent_id,
    }
}

#[tokio::test]
async fn photos_are_sent_in_order_before_the_text() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    h.engine
        .send(with_photos(
            "  look at these  ",
            vec![photo("a.jpg", 3), photo("b.jpg", 5)],
            Some(9),
        ))
        .await
        .unwrap();

    assert_eq!(
        h.remote.with(|r| r.sent.clone()),
        vec![
            file("a.jpg", 3, Some(9)),
            file("b.jpg", 5, Some(9)),
            text("look at these", Some(9)),
        ]
    );
    let rows = h.store().thread_messages(DM, 9, 10).unwrap();
    assert_eq!(
        summary(&rows),
        vec![
            ("FILE", "a.jpg", MessageState::Sent),
            ("FILE", "b.jpg", MessageState::Sent),
            ("MESG", "look at these", MessageState::Sent),
        ]
    );
    assert!(rows.iter().all(|m| m.id > 0));
    assert_eq!(rows[0].file_url, "https://file-us-3.sendbird.com/a.jpg");
    let groups = h.remote.with(|r| r.groups.clone());
    let group = groups[0].clone().unwrap();
    assert_eq!(group.len(), 36);
    assert_eq!(&group[14..15], "4");
    assert_eq!(groups, vec![Some(group); 3]);
}

#[tokio::test]
async fn photos_without_text_send_only_the_photos() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    h.engine
        .send(with_photos("   ", vec![photo("a.jpg", 1)], None))
        .await
        .unwrap();
    h.engine
        .send(with_photos(" ", Vec::new(), None))
        .await
        .unwrap();

    assert_eq!(
        h.remote.with(|r| r.sent.clone()),
        vec![file("a.jpg", 1, None)]
    );
    assert_eq!(h.remote.with(|r| r.groups.clone()), vec![None]);
    assert_eq!(h.store().latest_messages(DM, 10).unwrap().len(), 1);
}

#[tokio::test]
async fn too_many_attachments_are_rejected_before_anything_is_queued() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    let photos = (0..11).map(|i| photo(&format!("{i}.jpg"), 1)).collect();
    assert!(
        h.engine
            .send(with_photos("hi", photos, None))
            .await
            .is_err()
    );

    assert!(h.remote.with(|r| r.sent.is_empty()));
    assert!(h.store().latest_messages(DM, 10).unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_photo_fails_the_rest_and_retry_resends_them_in_order() {
    let remote = remote_with_dm();
    remote.with(|r| r.fail_at = Some(1));
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote);
    h.start_online().await;

    let sent = h
        .engine
        .send(with_photos(
            "trip",
            vec![photo("a.jpg", 2), photo("b.jpg", 4), photo("c.jpg", 6)],
            None,
        ))
        .await;
    assert!(sent.is_err());

    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(
        summary(&rows),
        vec![
            ("FILE", "a.jpg", MessageState::Sent),
            ("FILE", "b.jpg", MessageState::Failed),
            ("FILE", "c.jpg", MessageState::Failed),
            ("MESG", "trip", MessageState::Failed),
        ]
    );

    let tapped = rows.iter().find(|m| m.text == "trip").unwrap().id;
    h.engine.retry(DM, tapped).await.unwrap();

    assert_eq!(
        h.remote.with(|r| r.sent.clone()),
        vec![
            file("a.jpg", 2, None),
            file("b.jpg", 4, None),
            file("c.jpg", 6, None),
            text("trip", None),
        ]
    );
    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(
        summary(&rows),
        vec![
            ("FILE", "a.jpg", MessageState::Sent),
            ("FILE", "b.jpg", MessageState::Sent),
            ("FILE", "c.jpg", MessageState::Sent),
            ("MESG", "trip", MessageState::Sent),
        ]
    );
}

#[tokio::test]
async fn photos_queued_offline_keep_their_bytes_for_retry() {
    let store_path = std::env::temp_dir().join(format!(
        "kumowatch-outbox-{}-{}.sqlite",
        std::process::id(),
        kumo_core::http::now_ms()
    ));
    {
        let h = Harness::new(ChatStore::open(&store_path).unwrap(), remote_with_dm());
        assert!(
            h.engine
                .send(with_photos("later", vec![photo("a.jpg", 8)], None))
                .await
                .is_err()
        );
    }

    let mut h = Harness::new(ChatStore::open(&store_path).unwrap(), remote_with_dm());
    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(
        summary(&rows),
        vec![
            ("FILE", "a.jpg", MessageState::Failed),
            ("MESG", "later", MessageState::Failed),
        ]
    );
    h.start_online().await;
    h.engine.retry(DM, rows[0].id).await.unwrap();

    assert_eq!(
        h.remote.with(|r| r.sent.clone()),
        vec![file("a.jpg", 8, None), text("later", None)]
    );
    let _ = std::fs::remove_file(&store_path);
}

#[tokio::test]
async fn a_page_drops_cached_messages_the_server_no_longer_has() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    h.store()
        .upsert_messages(&[
            message(DM, 1, 10, "a"),
            message(DM, 2, 20, "deleted"),
            message(DM, 3, 30, "c"),
        ])
        .unwrap();
    h.remote.with(|r| {
        r.messages.insert(
            DM.into(),
            vec![message(DM, 1, 10, "a"), message(DM, 3, 30, "c")],
        )
    });

    let page = h.engine.load_page(DM, i64::MAX).await.unwrap();

    assert_eq!((page.count, page.has_more), (2, false));
    let ids: Vec<i64> = h
        .store()
        .latest_messages(DM, 10)
        .unwrap()
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec![1, 3]);
}

#[tokio::test]
async fn a_message_arriving_while_a_page_loads_is_kept() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    let store = h.engine.store().clone();
    h.remote.with(|r| {
        r.messages.insert(DM.into(), vec![message(DM, 1, 10, "a")]);
        r.live_during_page = Some((store, message(DM, 9, 40, "live")));
    });

    h.engine.load_page(DM, i64::MAX).await.unwrap();

    let ids: Vec<i64> = h
        .store()
        .latest_messages(DM, 10)
        .unwrap()
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(ids, vec![1, 9]);
}

#[tokio::test]
async fn send_batch_returns_the_acknowledged_ids_in_order() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    let ids = h
        .engine
        .send_batch(OutgoingMessage {
            channel_url: DM.into(),
            text: "caption".into(),
            files: vec![Attachment {
                name: "a.png".into(),
                mime_type: "image/png".into(),
                data: vec![1, 2, 3],
            }],
            parent_id: None,
        })
        .await
        .unwrap();

    let rows = h.store().latest_messages(DM, 10).unwrap();
    assert_eq!(ids, rows.iter().map(|m| m.id).collect::<Vec<_>>());
    assert_eq!(ids.len(), 2);
}

fn long_history(count: i64) -> FakeSendbird {
    let remote = remote_with_dm();
    remote.with(|r| {
        r.messages.insert(
            DM.into(),
            (1..=count)
                .map(|i| message(DM, i, i * 10, &format!("m{i}")))
                .collect(),
        )
    });
    remote
}

#[tokio::test]
async fn load_before_fills_the_limit_from_a_cold_cache() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), long_history(160));
    h.start_online().await;

    h.engine
        .load_before(DM, MessageCursor::before_ts(i64::MAX), 100)
        .await
        .unwrap();

    let rows = h.store().latest_messages(DM, 100).unwrap();
    assert_eq!(rows.len(), 100);
    assert_eq!(
        rows.iter().map(|m| m.id).collect::<Vec<_>>(),
        (61..=160).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn load_before_pages_back_from_a_message() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), long_history(160));
    h.start_online().await;
    let before = MessageCursor {
        created_at: 1200,
        id: 120,
    };

    h.engine.load_before(DM, before, 100).await.unwrap();

    let rows = h.store().messages_before_message(DM, before, 100).unwrap();
    assert_eq!(
        rows.iter().map(|m| m.id).collect::<Vec<_>>(),
        (20..=119).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn thread_replies_arrive_with_their_sender_but_raise_no_alert() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;

    h.remote
        .socket(0)
        .send(live(
            r#"MESG{"msg_id":9,"parent_message_id":4,"channel_url":"sp_amy_me","message":"in thread","ts":820,"user":{"user_id":"amy","nickname":"Amy"}}"#,
        ))
        .await
        .unwrap();
    let incoming = h
        .wait_for(|e| match e {
            ChatEvent::Incoming(m) => Some(m),
            _ => None,
        })
        .await;

    assert_eq!(
        (
            incoming.message_id,
            incoming.parent_id,
            incoming.sender_id.as_str(),
            incoming.created_at
        ),
        (9, Some(4), "amy", 820)
    );
    assert_eq!(message_alert(&incoming), None);
}

#[tokio::test]
async fn my_own_live_message_is_not_incoming() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    let socket = h.remote.socket(0);
    socket
        .send(live(
            r#"MESG{"msg_id":10,"channel_url":"sp_amy_me","message":"from me","ts":830,"user":{"user_id":"me","nickname":"Me"}}"#,
        ))
        .await
        .unwrap();
    socket
        .send(live(
            r#"MESG{"msg_id":11,"channel_url":"sp_amy_me","message":"from Amy","ts":840,"user":{"user_id":"amy","nickname":"Amy"}}"#,
        ))
        .await
        .unwrap();
    let incoming = h
        .wait_for(|e| match e {
            ChatEvent::Incoming(m) => Some(m),
            _ => None,
        })
        .await;
    assert_eq!(incoming.message_id, 11);
    assert!(h.store().message(DM, 10).unwrap().is_some());
}

#[tokio::test]
async fn a_failed_directory_read_is_not_cached() {
    let path = std::env::temp_dir().join(format!("kumo-directory-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let h = Harness::new(ChatStore::open(&path).unwrap(), remote_with_dm());
    let other = rusqlite::Connection::open(&path).unwrap();
    other
        .execute_batch("ALTER TABLE users RENAME TO users_away")
        .unwrap();
    assert_eq!(h.engine.user_name("amy"), None);
    other
        .execute_batch("ALTER TABLE users_away RENAME TO users")
        .unwrap();
    h.store()
        .upsert_users(&[DirectoryUser {
            user_id: "amy".into(),
            name: "Amy".into(),
            avatar_url: String::new(),
        }])
        .unwrap();
    assert_eq!(h.engine.user_name("amy").as_deref(), Some("Amy"));
    drop((h, other));
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn added_reactions_become_recent_and_removing_one_keeps_it() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    h.store()
        .upsert_messages(&[message(DM, 5, 100, "hi")])
        .unwrap();

    assert!(h.engine.recent_reactions().is_empty());
    h.engine.toggle_reaction(DM, 5, "🎉").await.unwrap();
    h.engine.toggle_reaction(DM, 5, "🔥").await.unwrap();
    h.engine.toggle_reaction(DM, 5, "🎉").await.unwrap();

    assert_eq!(h.engine.recent_reactions(), vec!["🔥", "🎉"]);
    let row = h.store().message(DM, 5).unwrap().unwrap();
    assert_eq!(
        row.reactions
            .iter()
            .map(|r| r.key.as_str())
            .collect::<Vec<_>>(),
        vec!["🔥"]
    );
}

#[tokio::test]
async fn adding_a_reaction_that_is_already_mine_keeps_it() {
    let mut h = Harness::new(ChatStore::in_memory().unwrap(), remote_with_dm());
    h.start_online().await;
    h.store()
        .upsert_messages(&[message(DM, 5, 100, "hi")])
        .unwrap();

    h.engine.add_reaction(DM, 5, "🎉").await.unwrap();
    h.engine.add_reaction(DM, 5, "🎉").await.unwrap();

    let row = h.store().message(DM, 5).unwrap().unwrap();
    assert_eq!(row.reactions.len(), 1);
    assert_eq!(row.reactions[0].key, "🎉");
}
