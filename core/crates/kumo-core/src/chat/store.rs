use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::error::{CoreError, Result};
use crate::sendbird::{SbChannel, SbMessage};

use super::model::{Attachment, ChannelRow, MemberRow, MessageRow, MessageState, ReactionRow};

const SCHEMA: &str = r#"
PRAGMA busy_timeout = 5000;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS channels (
    url TEXT PRIMARY KEY,
    custom_type TEXT NOT NULL,
    name TEXT NOT NULL,
    cover_url TEXT NOT NULL,
    member_count INTEGER NOT NULL,
    unread INTEGER NOT NULL,
    mentions INTEGER NOT NULL,
    frozen INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    last_message_id INTEGER NOT NULL,
    last_message_at INTEGER NOT NULL,
    last_message_text TEXT NOT NULL,
    last_sender_name TEXT NOT NULL,
    members_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS channels_by_activity ON channels(last_message_at DESC);
CREATE TABLE IF NOT EXISTS messages (
    channel_url TEXT NOT NULL,
    id INTEGER NOT NULL,
    req_id TEXT,
    state INTEGER NOT NULL,
    kind TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    sender_id TEXT NOT NULL,
    sender_name TEXT NOT NULL,
    sender_avatar TEXT NOT NULL,
    text TEXT NOT NULL,
    parent_id INTEGER NOT NULL,
    reply_count INTEGER NOT NULL,
    reactions_json TEXT NOT NULL,
    file_url TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_type TEXT NOT NULL,
    PRIMARY KEY (channel_url, id)
);
CREATE INDEX IF NOT EXISTS messages_timeline ON messages(channel_url, parent_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS messages_req ON messages(req_id) WHERE req_id IS NOT NULL;
CREATE TABLE IF NOT EXISTS sync_state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS users (
    user_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    avatar_url TEXT NOT NULL
);
"#;

const SEARCH_SCAN_LIMIT: u32 = 50_000;

const MIGRATIONS: &[&str] = &[
    "UPDATE messages SET file_type = kind, file_name = CASE WHEN file_name = '' THEN text ELSE file_name END, kind = 'FILE'
     WHERE kind LIKE '%/%'",
    "ALTER TABLE messages ADD COLUMN batch_id TEXT;
     ALTER TABLE messages ADD COLUMN upload BLOB;",
];

fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let applied: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(applied.max(0) as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

pub struct ChatStore {
    conn: Mutex<Connection>,
}

fn db(e: rusqlite::Error) -> CoreError {
    CoreError::Storage(e.to_string())
}

impl ChatStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| CoreError::Storage(e.to_string()))?;
        }
        Self::init(Connection::open(path).map_err(db)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch(SCHEMA).map_err(db)?;
        migrate(&mut conn).map_err(db)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| CoreError::Storage("poisoned".into()))?;
        f(&mut conn).map_err(db)
    }

    pub fn get_state(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            c.query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()
        })
    }

    pub fn set_state(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO sync_state(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map(|_| ())
        })
    }

    /// Read-modify-write of one sync_state value. IMMEDIATE takes the write lock before the read,
    /// so another process on the same file (kumo-mcp) cannot interleave.
    pub fn update_state(&self, key: &str, f: impl FnOnce(Option<String>) -> String) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let old = tx
                .query_row("SELECT value FROM sync_state WHERE key = ?1", [key], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO sync_state(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, &f(old)],
            )?;
            tx.commit()
        })
    }

    pub fn upsert_channels(&self, channels: &[SbChannel]) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO channels(url, custom_type, name, cover_url, member_count, unread, mentions, frozen, created_at,
                        last_message_id, last_message_at, last_message_text, last_sender_name, members_json)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                     ON CONFLICT(url) DO UPDATE SET custom_type = excluded.custom_type, name = excluded.name,
                        cover_url = excluded.cover_url, member_count = excluded.member_count, unread = excluded.unread,
                        mentions = excluded.mentions, frozen = excluded.frozen, created_at = excluded.created_at,
                        last_message_id = excluded.last_message_id, last_message_at = excluded.last_message_at,
                        last_message_text = excluded.last_message_text, last_sender_name = excluded.last_sender_name,
                        members_json = excluded.members_json",
                )?;
                for ch in channels {
                    let members: Vec<MemberRow> = ch.members.iter().map(MemberRow::from).collect();
                    let last = ch.last_message.as_ref();
                    stmt.execute(params![
                        ch.channel_url,
                        ch.custom_type,
                        ch.name,
                        ch.cover_url,
                        ch.member_count as i64,
                        ch.unread_message_count as i64,
                        ch.unread_mention_count as i64,
                        ch.freeze,
                        ch.created_at,
                        last.map(|m| m.id()).unwrap_or_default(),
                        ch.last_activity(),
                        last.map(preview).unwrap_or_default(),
                        last.and_then(|m| m.user.as_ref()).map(|u| u.nickname.clone()).unwrap_or_default(),
                        serde_json::to_string(&members).unwrap_or_else(|_| "[]".into()),
                    ])?;
                }
            }
            tx.commit()
        })
    }

    pub fn remove_channels(&self, urls: &[String]) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            for url in urls {
                tx.execute("DELETE FROM channels WHERE url = ?1", [url])?;
                tx.execute("DELETE FROM messages WHERE channel_url = ?1", [url])?;
            }
            tx.commit()
        })
    }

    pub fn channels(&self, url_prefix: &str) -> Result<Vec<ChannelRow>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT url, custom_type, name, cover_url, member_count, unread, mentions, frozen, last_message_at,
                        last_message_text, last_sender_name, members_json, last_message_id
                 FROM channels WHERE url LIKE ?1 ESCAPE '\\' ORDER BY last_message_at DESC",
            )?;
            let pattern = format!("{}%", escape_like(url_prefix));
            stmt.query_map([pattern], |r| {
                let members_json: String = r.get(11)?;
                Ok(ChannelRow {
                    url: r.get(0)?,
                    custom_type: r.get(1)?,
                    raw_name: r.get(2)?,
                    title: String::new(),
                    cover_url: r.get(3)?,
                    member_count: r.get::<_, i64>(4)? as u64,
                    unread: r.get::<_, i64>(5)? as u64,
                    mentions: r.get::<_, i64>(6)? as u64,
                    frozen: r.get(7)?,
                    last_message_at: r.get(8)?,
                    last_message_text: r.get(9)?,
                    last_sender_name: r.get(10)?,
                    members: serde_json::from_str(&members_json).unwrap_or_default(),
                    last_message_id: r.get(12)?,
                })
            })?
            .collect()
        })
    }

    pub fn channel(&self, url: &str) -> Result<Option<ChannelRow>> {
        Ok(self.channels(url)?.into_iter().find(|c| c.url == url))
    }

    pub fn set_unread(&self, url: &str, unread: u64, mentions: u64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE channels SET unread = ?2, mentions = ?3 WHERE url = ?1",
                params![url, unread as i64, mentions as i64],
            )
            .map(|_| ())
        })
    }

    pub fn apply_live_message(
        &self,
        msg: &SbMessage,
        counts_as_unread: bool,
        mentions_me: bool,
    ) -> Result<()> {
        self.upsert_messages(std::slice::from_ref(msg))?;
        if msg.is_thread_reply() {
            return Ok(());
        }
        self.with(|c| {
            c.execute(
                "UPDATE channels SET last_message_id = ?2, last_message_at = ?3, last_message_text = ?4,
                    last_sender_name = ?5, unread = unread + ?6, mentions = mentions + ?7
                 WHERE url = ?1 AND last_message_at <= ?3",
                params![
                    msg.channel_url,
                    msg.id(),
                    msg.created_at,
                    preview(msg),
                    msg.user.as_ref().map(|u| u.nickname.clone()).unwrap_or_default(),
                    counts_as_unread as i64,
                    (counts_as_unread && mentions_me) as i64,
                ],
            )
            .map(|_| ())
        })
    }

    pub fn upsert_messages(&self, messages: &[SbMessage]) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut clear_pending = tx.prepare_cached("DELETE FROM messages WHERE req_id = ?1 AND state != 0")?;
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO messages(channel_url, id, req_id, state, kind, created_at, updated_at, sender_id, sender_name,
                        sender_avatar, text, parent_id, reply_count, reactions_json, file_url, file_name, file_type)
                     VALUES(?1, ?2, NULL, 0, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                     ON CONFLICT(channel_url, id) DO UPDATE SET kind = excluded.kind, updated_at = excluded.updated_at,
                        sender_name = excluded.sender_name, sender_avatar = excluded.sender_avatar, text = excluded.text,
                        reply_count = excluded.reply_count, reactions_json = excluded.reactions_json,
                        file_url = excluded.file_url, file_name = excluded.file_name, file_type = excluded.file_type
                     WHERE excluded.updated_at >= messages.updated_at",
                )?;
                for m in messages {
                    if m.id() <= 0 {
                        continue;
                    }
                    if let Some(req) = m.req_id.as_deref().filter(|r| !r.is_empty()) {
                        clear_pending.execute([req])?;
                    }
                    let user = m.user.clone().unwrap_or_default();
                    let reactions: Vec<ReactionRow> = m
                        .reactions
                        .iter()
                        .map(|r| ReactionRow {
                            key: r.key.clone(),
                            user_ids: r.user_ids.clone(),
                        })
                        .collect();
                    let file = m.file.clone().unwrap_or_default();
                    stmt.execute(params![
                        m.channel_url,
                        m.id(),
                        m.kind,
                        m.created_at,
                        m.updated_at.max(m.created_at),
                        user.user_id,
                        user.nickname,
                        user.profile_url,
                        m.message,
                        m.parent_message_id.unwrap_or_default(),
                        m.thread_info.as_ref().map(|t| t.reply_count as i64).unwrap_or_default(),
                        serde_json::to_string(&reactions).unwrap_or_else(|_| "[]".into()),
                        if file.url.is_empty() { m.url.clone().unwrap_or_default() } else { file.url },
                        file.name,
                        file.mime_type,
                    ])?;
                }
            }
            tx.commit()
        })
    }

    pub fn insert_pending(&self, pending: &PendingMessage<'_>) -> Result<i64> {
        self.with(|c| {
            let local_id: i64 = c.query_row("SELECT MIN(0, COALESCE(MIN(id), 0)) - 1 FROM messages", [], |r| r.get(0))?;
            let (kind, file_name, file_type, upload) = match &pending.file {
                Some(file) => ("FILE", file.name, file.mime_type, Some(file.data)),
                None => ("MESG", "", "", None),
            };
            c.execute(
                "INSERT INTO messages(channel_url, id, req_id, state, kind, created_at, updated_at, sender_id, sender_name,
                    sender_avatar, text, parent_id, reply_count, reactions_json, file_url, file_name, file_type, batch_id, upload)
                 VALUES(?1, ?2, ?3, 1, ?4, ?5, ?5, ?6, ?7, '', ?8, ?9, 0, '[]', '', ?10, ?11, ?12, ?13)",
                params![
                    pending.channel_url,
                    local_id,
                    pending.req_id,
                    kind,
                    pending.created_at,
                    pending.sender_id,
                    pending.sender_name,
                    pending.text,
                    pending.parent_id.unwrap_or_default(),
                    file_name,
                    file_type,
                    pending.batch_id,
                    upload,
                ],
            )?;
            Ok(local_id)
        })
    }

    pub fn mark_failed(&self, req_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE messages SET state = 2 WHERE req_id = ?1 AND state = 1",
                [req_id],
            )
            .map(|_| ())
        })
    }

    pub fn unsent_batch(&self, channel_url: &str, local_id: i64) -> Result<Vec<OutboxItem>> {
        self.with(|c| {
            let batch: Option<String> = c
                .query_row(
                    "SELECT COALESCE(batch_id, req_id) FROM messages WHERE channel_url = ?1 AND id = ?2 AND state != 0",
                    params![channel_url, local_id],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let Some(batch) = batch else {
                return Ok(Vec::new());
            };
            let mut stmt = c.prepare_cached(
                "SELECT id, req_id, text, parent_id, kind, file_name, file_type, upload FROM messages
                 WHERE channel_url = ?1 AND state != 0 AND COALESCE(batch_id, req_id) = ?2
                 ORDER BY created_at ASC, id DESC",
            )?;
            stmt.query_map(params![channel_url, batch], |r| {
                let kind: String = r.get(4)?;
                let parent: i64 = r.get(3)?;
                let upload: Option<Vec<u8>> = r.get(7)?;
                Ok(OutboxItem {
                    local_id: r.get(0)?,
                    req_id: r.get(1)?,
                    text: r.get(2)?,
                    parent_id: (parent > 0).then_some(parent),
                    file: match (kind.as_str(), upload) {
                        ("FILE", Some(data)) => Some(Attachment {
                            name: r.get(5)?,
                            mime_type: r.get(6)?,
                            data,
                        }),
                        _ => None,
                    },
                })
            })?
            .collect()
        })
    }

    pub fn reset_pending(&self, channel_url: &str, local_id: i64, req_id: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE messages SET state = 1, req_id = ?3 WHERE channel_url = ?1 AND id = ?2",
                params![channel_url, local_id, req_id],
            )
            .map(|_| ())
        })
    }

    pub fn message(&self, channel_url: &str, id: i64) -> Result<Option<MessageRow>> {
        Ok(self.page_where(channel_url, id)?.into_iter().next())
    }

    fn page_where(&self, channel_url: &str, id: i64) -> Result<Vec<MessageRow>> {
        let parent = self.with(|c| {
            c.query_row(
                "SELECT parent_id, created_at FROM messages WHERE channel_url = ?1 AND id = ?2",
                params![channel_url, id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()
        })?;
        let Some((parent_id, created_at)) = parent else {
            return Ok(Vec::new());
        };
        Ok(self
            .page(
                channel_url,
                parent_id,
                MessageCursor::before_ts(created_at + 1),
                5,
            )?
            .into_iter()
            .filter(|m| m.id == id)
            .collect())
    }

    pub fn set_reaction(&self, reaction: &ReactionChange<'_>) -> Result<()> {
        let Some(row) = self.message(reaction.channel_url, reaction.message_id)? else {
            return Ok(());
        };
        let mut reactions = row.reactions;
        match reactions.iter_mut().find(|r| r.key == reaction.key) {
            Some(r) => {
                r.user_ids.retain(|u| u != reaction.user_id);
                if reaction.added {
                    r.user_ids.push(reaction.user_id.to_string());
                }
            }
            None if reaction.added => reactions.push(ReactionRow {
                key: reaction.key.to_string(),
                user_ids: vec![reaction.user_id.to_string()],
            }),
            None => {}
        }
        reactions.retain(|r| !r.user_ids.is_empty());
        self.with(|c| {
            c.execute(
                "UPDATE messages SET reactions_json = ?3 WHERE channel_url = ?1 AND id = ?2",
                params![
                    reaction.channel_url,
                    reaction.message_id,
                    serde_json::to_string(&reactions).unwrap_or_else(|_| "[]".into())
                ],
            )
            .map(|_| ())
        })
    }

    pub fn delete_message(&self, channel_url: &str, id: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "DELETE FROM messages WHERE channel_url = ?1 AND id = ?2",
                params![channel_url, id],
            )
            .map(|_| ())
        })
    }

    pub fn top_level_sent(&self, channel_url: &str, before_ts: i64) -> Result<Vec<MessageCursor>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT created_at, id FROM messages
                 WHERE channel_url = ?1 AND parent_id = 0 AND state = 0 AND created_at < ?2",
            )?;
            stmt.query_map(params![channel_url, before_ts], |r| {
                Ok(MessageCursor {
                    created_at: r.get(0)?,
                    id: r.get(1)?,
                })
            })?
            .collect()
        })
    }

    pub fn delete_messages(&self, channel_url: &str, ids: &[i64]) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut delete =
                    tx.prepare_cached("DELETE FROM messages WHERE channel_url = ?1 AND id = ?2")?;
                for id in ids {
                    delete.execute(params![channel_url, id])?;
                }
            }
            tx.commit()
        })
    }

    pub fn latest_messages(&self, channel_url: &str, limit: u32) -> Result<Vec<MessageRow>> {
        self.page(channel_url, 0, MessageCursor::before_ts(i64::MAX), limit)
    }

    pub fn messages_before_message(
        &self,
        channel_url: &str,
        before: MessageCursor,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        self.page(channel_url, 0, before, limit)
    }

    pub fn messages_before(
        &self,
        channel_url: &str,
        before_ts: i64,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        self.page(channel_url, 0, MessageCursor::before_ts(before_ts), limit)
    }

    pub fn thread_messages(
        &self,
        channel_url: &str,
        parent_id: i64,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        self.page(
            channel_url,
            parent_id,
            MessageCursor::before_ts(i64::MAX),
            limit,
        )
    }

    fn page(
        &self,
        channel_url: &str,
        parent_id: i64,
        before: MessageCursor,
        limit: u32,
    ) -> Result<Vec<MessageRow>> {
        let mut rows = self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT channel_url, id, state, kind, created_at, updated_at, sender_id, sender_name, sender_avatar, text,
                        parent_id, reply_count, reactions_json, file_url, file_name, file_type
                 FROM messages WHERE channel_url = ?1 AND parent_id = ?2
                   AND (created_at < ?3 OR (created_at = ?3 AND id < ?5))
                 ORDER BY created_at DESC, id DESC LIMIT ?4",
            )?;
            stmt.query_map(
                params![channel_url, parent_id, before.created_at, limit as i64, before.id],
                message_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        rows.reverse();
        Ok(rows)
    }

    pub fn search_messages(&self, search: &MessageSearch<'_>) -> Result<Vec<MessageRow>> {
        let needle = search.text.map(str::to_lowercase).filter(|t| !t.is_empty());
        let scan_limit = if needle.is_some() {
            SEARCH_SCAN_LIMIT
        } else {
            search.limit
        };
        let rows = self.with(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT channel_url, id, state, kind, created_at, updated_at, sender_id, sender_name, sender_avatar, text,
                        parent_id, reply_count, reactions_json, file_url, file_name, file_type
                 FROM messages
                 WHERE substr(channel_url, 1, length(?1)) = ?1 AND state = 0
                   AND (?2 IS NULL OR channel_url = ?2)
                   AND (?3 IS NULL OR sender_id = ?3)
                   AND (?4 IS NULL OR sender_id != ?4)
                   AND created_at >= ?5
                 ORDER BY created_at DESC, id DESC LIMIT ?6",
            )?;
            stmt.query_map(
                params![
                    search.url_prefix,
                    search.channel_url,
                    search.sender_id,
                    search.exclude_sender_id,
                    search.since_ts,
                    scan_limit as i64
                ],
                message_row,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()
        })?;
        Ok(match needle {
            Some(needle) => rows
                .into_iter()
                .filter(|m| {
                    m.text.to_lowercase().contains(&needle)
                        || m.file_name.to_lowercase().contains(&needle)
                })
                .take(search.limit as usize)
                .collect(),
            None => rows,
        })
    }

    pub fn newest_message(&self, channel_url: &str) -> Result<Option<(i64, i64)>> {
        self.with(|c| {
            c.query_row(
                "SELECT id, created_at FROM messages WHERE channel_url = ?1 AND parent_id = 0 AND state = 0
                 ORDER BY created_at DESC, id DESC LIMIT 1",
                [channel_url],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    pub fn oldest_message_ts(&self, channel_url: &str) -> Result<Option<i64>> {
        self.with(|c| {
            c.query_row(
                "SELECT MIN(created_at) FROM messages WHERE channel_url = ?1 AND state = 0",
                [channel_url],
                |r| r.get::<_, Option<i64>>(0),
            )
        })
    }

    pub fn has_message(&self, channel_url: &str, id: i64) -> Result<bool> {
        self.with(|c| {
            c.query_row(
                "SELECT 1 FROM messages WHERE channel_url = ?1 AND id = ?2",
                params![channel_url, id],
                |_| Ok(()),
            )
            .optional()
            .map(|r| r.is_some())
        })
    }

    pub fn upsert_users(&self, users: &[DirectoryUser]) -> Result<()> {
        self.with(|c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO users(user_id, name, avatar_url) VALUES(?1, ?2, ?3)
                     ON CONFLICT(user_id) DO UPDATE SET name = excluded.name, avatar_url = excluded.avatar_url",
                )?;
                for u in users {
                    stmt.execute(params![u.user_id, u.name, u.avatar_url])?;
                }
            }
            tx.commit()
        })
    }

    pub fn directory(&self) -> Result<HashMap<String, DirectoryUser>> {
        self.with(|c| {
            let mut stmt = c.prepare_cached("SELECT user_id, name, avatar_url FROM users")?;
            stmt.query_map([], |r| {
                Ok(DirectoryUser {
                    user_id: r.get(0)?,
                    name: r.get(1)?,
                    avatar_url: r.get(2)?,
                })
            })?
            .map(|u| u.map(|u| (u.user_id.clone(), u)))
            .collect()
        })
    }

    pub fn clear(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch(
                "DELETE FROM channels; DELETE FROM messages; DELETE FROM sync_state; DELETE FROM users;",
            )
        })
    }
}

fn message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    let reactions: String = r.get(12)?;
    Ok(MessageRow {
        channel_url: r.get(0)?,
        id: r.get(1)?,
        state: MessageState::from_code(r.get(2)?),
        kind: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        sender_id: r.get(6)?,
        sender_name: r.get(7)?,
        sender_avatar: r.get(8)?,
        text: r.get(9)?,
        parent_id: r.get(10)?,
        reply_count: r.get::<_, i64>(11)? as u64,
        reactions: serde_json::from_str(&reactions).unwrap_or_default(),
        file_url: r.get(13)?,
        file_name: r.get(14)?,
        file_type: r.get(15)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageCursor {
    pub created_at: i64,
    pub id: i64,
}

impl MessageCursor {
    pub fn before_ts(created_at: i64) -> Self {
        Self {
            created_at,
            id: i64::MIN,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MessageSearch<'a> {
    pub url_prefix: &'a str,
    pub channel_url: Option<&'a str>,
    pub text: Option<&'a str>,
    pub sender_id: Option<&'a str>,
    pub exclude_sender_id: Option<&'a str>,
    pub since_ts: i64,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryUser {
    pub user_id: String,
    pub name: String,
    pub avatar_url: String,
}

pub struct ReactionChange<'a> {
    pub channel_url: &'a str,
    pub message_id: i64,
    pub key: &'a str,
    pub user_id: &'a str,
    pub added: bool,
}

pub struct PendingMessage<'a> {
    pub channel_url: &'a str,
    pub req_id: &'a str,
    pub batch_id: &'a str,
    pub text: &'a str,
    pub sender_id: &'a str,
    pub sender_name: &'a str,
    pub created_at: i64,
    pub parent_id: Option<i64>,
    pub file: Option<PendingFile<'a>>,
}

pub struct PendingFile<'a> {
    pub name: &'a str,
    pub mime_type: &'a str,
    pub data: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub local_id: i64,
    pub req_id: String,
    pub text: String,
    pub parent_id: Option<i64>,
    pub file: Option<Attachment>,
}

fn preview(m: &SbMessage) -> String {
    if m.kind == "FILE" {
        return m
            .file
            .as_ref()
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "File".into());
    }
    m.message.chars().take(200).collect()
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sendbird::{SbMember, SbUser};

    fn msg(id: i64, ts: i64, text: &str) -> SbMessage {
        serde_json::from_value(serde_json::json!({
            "message_id": id, "type": "MESG", "channel_url": "sp_c", "message": text, "created_at": ts,
            "user": {"user_id": "u2", "nickname": "Amy"}
        }))
        .unwrap()
    }

    fn channel() -> SbChannel {
        SbChannel {
            channel_url: "sp_c".into(),
            custom_type: "user".into(),
            members: vec![SbMember {
                user_id: "u2".into(),
                nickname: "Amy".into(),
                ..Default::default()
            }],
            last_message: Some(msg(1, 10, "first")),
            ..Default::default()
        }
    }

    #[test]
    fn stores_channels_and_pages_messages() {
        let store = ChatStore::in_memory().unwrap();
        store.upsert_channels(&[channel()]).unwrap();
        store
            .upsert_messages(
                &(1..=5)
                    .map(|i| msg(i, i * 10, &format!("m{i}")))
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let latest = store.latest_messages("sp_c", 3).unwrap();
        assert_eq!(
            latest.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        let older = store.messages_before("sp_c", 30, 10).unwrap();
        assert_eq!(older.iter().map(|m| m.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(store.newest_message("sp_c").unwrap(), Some((5, 50)));
        assert_eq!(store.channels("sp_").unwrap()[0].members[0].nickname, "Amy");
        assert!(store.channels("other_").unwrap().is_empty());
    }

    #[test]
    fn searches_cached_messages_ignoring_case() {
        let store = ChatStore::in_memory().unwrap();
        store
            .upsert_messages(&[
                msg(1, 10, "Привет, нужен РЕЛИЗ"),
                msg(2, 20, "unrelated"),
                msg(3, 30, "релиз готов"),
            ])
            .unwrap();
        let ids = |search: &MessageSearch<'_>| {
            store
                .search_messages(search)
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
        };
        let base = MessageSearch {
            url_prefix: "sp_",
            limit: 10,
            ..Default::default()
        };
        assert_eq!(
            ids(&MessageSearch {
                text: Some("релиз"),
                ..base.clone()
            }),
            vec![3, 1]
        );
        assert_eq!(
            ids(&MessageSearch {
                since_ts: 15,
                ..base.clone()
            }),
            vec![3, 2]
        );
        assert!(
            ids(&MessageSearch {
                exclude_sender_id: Some("u2"),
                ..base.clone()
            })
            .is_empty()
        );
        assert!(
            ids(&MessageSearch {
                url_prefix: "other_",
                ..base
            })
            .is_empty()
        );
    }

    #[test]
    fn pages_by_timestamp_and_id() {
        let store = ChatStore::in_memory().unwrap();
        let mut reply = msg(4, 20, "reply");
        reply.parent_message_id = Some(1);
        store
            .upsert_messages(&[msg(1, 10, "a"), msg(2, 20, "b"), msg(3, 20, "c"), reply])
            .unwrap();
        let ids = |rows: Vec<MessageRow>| rows.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            ids(store
                .messages_before_message(
                    "sp_c",
                    MessageCursor {
                        created_at: 20,
                        id: 3
                    },
                    10
                )
                .unwrap()),
            vec![1, 2]
        );
        assert_eq!(ids(store.messages_before("sp_c", 20, 10).unwrap()), vec![1]);
        let sent = store.top_level_sent("sp_c", 20).unwrap();
        assert_eq!(sent.iter().map(|c| c.id).collect::<Vec<_>>(), vec![1]);
        store.delete_messages("sp_c", &[1, 3]).unwrap();
        assert_eq!(ids(store.latest_messages("sp_c", 10).unwrap()), vec![2]);
        assert_eq!(store.thread_messages("sp_c", 1, 10).unwrap().len(), 1);
    }

    #[test]
    fn live_message_updates_channel_and_unread() {
        let store = ChatStore::in_memory().unwrap();
        store.upsert_channels(&[channel()]).unwrap();
        store
            .apply_live_message(&msg(9, 90, "hello"), true, true)
            .unwrap();
        let ch = store.channel("sp_c").unwrap().unwrap();
        assert_eq!(
            (ch.unread, ch.mentions, ch.last_message_text.as_str()),
            (1, 1, "hello")
        );
        store
            .apply_live_message(&msg(3, 30, "late"), true, false)
            .unwrap();
        assert_eq!(
            store.channel("sp_c").unwrap().unwrap().last_message_text,
            "hello"
        );
    }

    #[test]
    fn pending_message_is_replaced_by_ack() {
        let store = ChatStore::in_memory().unwrap();
        let local = store
            .insert_pending(&PendingMessage {
                channel_url: "sp_c",
                req_id: "rq-1",
                batch_id: "rq-1",
                text: "hi",
                sender_id: "u1",
                sender_name: "Me",
                created_at: 100,
                parent_id: None,
                file: None,
            })
            .unwrap();
        assert!(local < 0);
        assert_eq!(
            store.latest_messages("sp_c", 10).unwrap()[0].state,
            MessageState::Pending
        );
        let mut acked = msg(42, 101, "hi");
        acked.user = Some(SbUser {
            user_id: "u1".into(),
            ..Default::default()
        });
        acked.req_id = Some("rq-1".into());
        store.upsert_messages(&[acked]).unwrap();
        let rows = store.latest_messages("sp_c", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].id, rows[0].state), (42, MessageState::Sent));
    }

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kumo-store-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        dir.join("chat.db")
    }

    fn user_version(store: &ChatStore) -> i64 {
        store
            .with(|c| c.pragma_query_value(None, "user_version", |r| r.get(0)))
            .unwrap()
    }

    fn insert_raw_file_row(conn: &Connection, id: i64, kind: &str) {
        conn.execute(
            "INSERT INTO messages(channel_url, id, req_id, state, kind, created_at, updated_at, sender_id, sender_name,
                sender_avatar, text, parent_id, reply_count, reactions_json, file_url, file_name, file_type)
             VALUES('sp_c', ?1, NULL, 0, ?2, ?1, ?1, 'u2', 'Amy', '', 'cat.png', 0, 0, '[]', 'https://f/cat.png', '', '')",
            params![id, kind],
        )
        .unwrap();
    }

    #[test]
    fn fresh_database_is_at_latest_version() {
        let path = temp_db("fresh");
        let store = ChatStore::open(&path).unwrap();
        assert_eq!(user_version(&store), MIGRATIONS.len() as i64);
        drop(store);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn unversioned_database_upgrades_once() {
        let path = temp_db("legacy");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            insert_raw_file_row(&conn, 1, "image/png");
        }

        let store = ChatStore::open(&path).unwrap();
        assert_eq!(user_version(&store), MIGRATIONS.len() as i64);
        let row = store.message("sp_c", 1).unwrap().unwrap();
        assert_eq!(
            (
                row.kind.as_str(),
                row.file_type.as_str(),
                row.file_name.as_str()
            ),
            ("FILE", "image/png", "cat.png")
        );
        store
            .with(|c| {
                insert_raw_file_row(c, 2, "image/gif");
                Ok(())
            })
            .unwrap();
        drop(store);

        let reopened = ChatStore::open(&path).unwrap();
        assert_eq!(user_version(&reopened), MIGRATIONS.len() as i64);
        assert_eq!(
            reopened.message("sp_c", 2).unwrap().unwrap().kind,
            "image/gif"
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
