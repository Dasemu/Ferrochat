//! Local cache: SQLite via rusqlite.
//!
//! The cache holds chats, messages (bodies + media references), contact
//! names and a few non-secret key/value pairs. Credentials and Signal keys
//! never go here (they are in the keyring and the SQLCipher session store).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::net::types::{ChatMeta, Content, Message, ReceiptStatus};

const MIGRATIONS: &[&str] = &[include_str!("schema.sql")];

/// Thread-safe handle; clone freely. Heavy calls should run in
/// `spawn_blocking` (the app does).
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Store")
    }
}

/// Chat row plus its newest message, for the chat list.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    /// Metadata.
    pub meta: ChatMeta,
    /// Newest message, if any.
    pub last: Option<Message>,
}

impl Store {
    /// Opens (creating if needed) the cache at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            crate::config::secure_dir(dir)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let store = Self::init(conn)?;
        crate::config::secure_db_files(path);
        Ok(store)
    }

    /// In-memory cache for tests and `--mock`.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let target = i as i64 + 1;
            if version < target {
                conn.execute_batch(sql)?;
                conn.pragma_update(None, "user_version", target)?;
            }
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| Error::Protocol("store mutex poisoned".into()))
    }

    /// Schema version (PRAGMA user_version).
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .lock()?
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Inserts or merges chat metadata. Names are only replaced by non-empty
    /// names, `last_ts` only grows.
    pub fn upsert_chats(&self, chats: &[ChatMeta]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO chats (id, name, unread, last_ts, pinned, archived, muted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    name = COALESCE(excluded.name, chats.name),
                    unread = excluded.unread,
                    last_ts = MAX(chats.last_ts, excluded.last_ts),
                    pinned = excluded.pinned,
                    archived = excluded.archived,
                    muted = excluded.muted",
            )?;
            for c in chats {
                st.execute(params![
                    c.id, c.name, c.unread, c.last_ts, c.pinned, c.archived, c.muted
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets the unread counter of a chat.
    pub fn set_unread(&self, chat: &str, unread: u32) -> Result<()> {
        self.lock()?.execute(
            "UPDATE chats SET unread = ?2 WHERE id = ?1",
            params![chat, unread],
        )?;
        Ok(())
    }

    /// Inserts messages (creating chats as needed). Existing messages keep
    /// their content unless the new one is a deletion; statuses go through
    /// the receipt state machine. Returns the ids that were new.
    pub fn insert_messages(&self, msgs: &[Message]) -> Result<Vec<String>> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut new_ids = Vec::new();
        {
            let mut existing =
                tx.prepare_cached("SELECT status FROM messages WHERE chat_id = ?1 AND id = ?2")?;
            let mut insert = tx.prepare_cached(
                "INSERT INTO messages
                    (chat_id, id, sender, sender_name, from_me, ts, kind, content, thumb, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            let mut update_status = tx
                .prepare_cached("UPDATE messages SET status = ?3 WHERE chat_id = ?1 AND id = ?2")?;
            let mut update_content = tx.prepare_cached(
                "UPDATE messages SET kind = ?3, content = ?4, thumb = NULL
                 WHERE chat_id = ?1 AND id = ?2",
            )?;
            let mut chat = tx.prepare_cached(
                "INSERT INTO chats (id, last_ts) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET last_ts = MAX(chats.last_ts, excluded.last_ts)",
            )?;
            for m in msgs {
                let prev: Option<i64> = existing
                    .query_row(params![m.chat, m.id], |r| r.get(0))
                    .optional()?;
                let (json, thumb) = encode_content(&m.content)?;
                match prev {
                    None => {
                        insert.execute(params![
                            m.chat,
                            m.id,
                            m.sender,
                            m.sender_name,
                            m.from_me,
                            m.timestamp,
                            m.content.kind_tag(),
                            json,
                            thumb,
                            m.status.to_i64(),
                        ])?;
                        new_ids.push(m.id.clone());
                    }
                    Some(s) => {
                        let merged = ReceiptStatus::from_i64(s).advance(m.status);
                        update_status.execute(params![m.chat, m.id, merged.to_i64()])?;
                        if matches!(m.content, Content::Deleted) {
                            update_content.execute(params![
                                m.chat,
                                m.id,
                                m.content.kind_tag(),
                                json
                            ])?;
                        }
                    }
                }
                chat.execute(params![m.chat, m.timestamp])?;
            }
        }
        tx.commit()?;
        Ok(new_ids)
    }

    /// Replaces a message's content (e.g. after a media upload filled in
    /// the CDN reference) and/or status.
    pub fn update_message(&self, chat: &str, id: &str, content: &Content) -> Result<()> {
        let (json, thumb) = encode_content(content)?;
        self.lock()?.execute(
            "UPDATE messages SET kind = ?3, content = ?4, thumb = COALESCE(?5, thumb)
             WHERE chat_id = ?1 AND id = ?2",
            params![chat, id, content.kind_tag(), json, thumb],
        )?;
        Ok(())
    }

    /// Applies a receipt. Returns `(id, new_status)` for rows that changed.
    pub fn apply_receipt(
        &self,
        chat: &str,
        ids: &[String],
        status: ReceiptStatus,
    ) -> Result<Vec<(String, ReceiptStatus)>> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut changed = Vec::new();
        {
            let mut get = tx.prepare_cached(
                "SELECT status FROM messages WHERE chat_id = ?1 AND id = ?2 AND from_me = 1",
            )?;
            let mut set = tx
                .prepare_cached("UPDATE messages SET status = ?3 WHERE chat_id = ?1 AND id = ?2")?;
            for id in ids {
                let Some(cur) = get
                    .query_row(params![chat, id], |r| r.get::<_, i64>(0))
                    .optional()?
                else {
                    continue;
                };
                let cur = ReceiptStatus::from_i64(cur);
                let next = cur.advance(status);
                if next != cur {
                    set.execute(params![chat, id, next.to_i64()])?;
                    changed.push((id.clone(), next));
                }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Messages of a chat, oldest first. `before` pages backwards from a
    /// `(timestamp, id)` cursor.
    pub fn load_messages(
        &self,
        chat: &str,
        before: Option<(i64, &str)>,
        limit: usize,
    ) -> Result<Vec<Message>> {
        let conn = self.lock()?;
        let (ts, id) = before.unwrap_or((i64::MAX, ""));
        let mut st = conn.prepare_cached(
            "SELECT chat_id, id, sender, sender_name, from_me, ts, content, thumb, status
             FROM messages
             WHERE chat_id = ?1 AND (ts < ?2 OR (ts = ?2 AND id < ?3))
             ORDER BY ts DESC, id DESC LIMIT ?4",
        )?;
        let id_bound = if before.is_some() { id } else { "\u{10FFFF}" };
        let mut rows: Vec<Message> = st
            .query_map(params![chat, ts, id_bound, limit as i64], row_to_message)?
            .collect::<std::result::Result<_, _>>()?;
        rows.reverse();
        Ok(rows)
    }

    /// One message.
    pub fn message(&self, chat: &str, id: &str) -> Result<Option<Message>> {
        let conn = self.lock()?;
        let mut st = conn.prepare_cached(
            "SELECT chat_id, id, sender, sender_name, from_me, ts, content, thumb, status
             FROM messages WHERE chat_id = ?1 AND id = ?2",
        )?;
        Ok(st.query_row(params![chat, id], row_to_message).optional()?)
    }

    /// All chats with their newest message, most recent first (pinned on top).
    pub fn chat_rows(&self) -> Result<Vec<ChatRow>> {
        let conn = self.lock()?;
        let mut st = conn.prepare_cached(
            "SELECT c.id, c.name, c.unread, c.last_ts, c.pinned, c.archived, c.muted,
                    m.chat_id, m.id, m.sender, m.sender_name, m.from_me, m.ts, m.content,
                    m.thumb, m.status
             FROM chats c
             LEFT JOIN messages m ON m.rowid = (
                SELECT rowid FROM messages WHERE chat_id = c.id ORDER BY ts DESC, id DESC LIMIT 1
             )
             ORDER BY c.pinned DESC, c.last_ts DESC",
        )?;
        let rows = st
            .query_map([], |r| {
                let meta = ChatMeta {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    unread: r.get(2)?,
                    last_ts: r.get(3)?,
                    pinned: r.get(4)?,
                    archived: r.get(5)?,
                    muted: r.get(6)?,
                };
                let last = match r.get::<_, Option<String>>(7)? {
                    Some(_) => Some(row_to_message_at(r, 7)?),
                    None => None,
                };
                Ok(ChatRow { meta, last })
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    /// Records a contact display name.
    pub fn set_contact_names(&self, names: &[(String, String)]) -> Result<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO contacts (jid, name) VALUES (?1, ?2)
                 ON CONFLICT(jid) DO UPDATE SET name = excluded.name",
            )?;
            for (jid, name) in names {
                st.execute(params![jid, name])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// All known contact names.
    pub fn contact_names(&self) -> Result<HashMap<String, String>> {
        let conn = self.lock()?;
        let mut st = conn.prepare_cached("SELECT jid, name FROM contacts")?;
        let map = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(map)
    }

    /// Non-secret settings (own JID, last open chat).
    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        self.lock()?.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Reads a setting.
    pub fn kv(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .lock()?
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// Wipes everything (logout).
    pub fn clear(&self) -> Result<()> {
        self.lock()?.execute_batch(
            "DELETE FROM messages; DELETE FROM chats; DELETE FROM contacts; DELETE FROM kv;",
        )?;
        Ok(())
    }
}

fn encode_content(c: &Content) -> Result<(String, Option<Vec<u8>>)> {
    let json = serde_json::to_string(c).map_err(|e| Error::Protocol(e.to_string()))?;
    let thumb = match c {
        Content::Image { thumbnail, .. } => thumbnail.clone(),
        _ => None,
    };
    Ok((json, thumb))
}

fn row_to_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    row_to_message_at(r, 0)
}

fn row_to_message_at(r: &rusqlite::Row<'_>, o: usize) -> rusqlite::Result<Message> {
    let json: String = r.get(o + 6)?;
    let thumb: Option<Vec<u8>> = r.get(o + 7)?;
    let mut content: Content = serde_json::from_str(&json)
        .unwrap_or_else(|_| Content::Unsupported("unreadable cache entry".into()));
    if let Content::Image { thumbnail, .. } = &mut content {
        *thumbnail = thumb;
    }
    Ok(Message {
        chat: r.get(o)?,
        id: r.get(o + 1)?,
        sender: r.get(o + 2)?,
        sender_name: r.get(o + 3)?,
        from_me: r.get(o + 4)?,
        timestamp: r.get(o + 5)?,
        content,
        status: ReceiptStatus::from_i64(r.get(o + 8)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(chat: &str, id: &str, ts: i64, from_me: bool, text: &str) -> Message {
        Message {
            id: id.into(),
            chat: chat.into(),
            sender: if from_me { "me".into() } else { chat.into() },
            sender_name: None,
            from_me,
            timestamp: ts,
            content: Content::Text(text.into()),
            status: if from_me {
                ReceiptStatus::Sent
            } else {
                ReceiptStatus::Read
            },
        }
    }

    #[test]
    fn migrations_set_version() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
    }

    #[test]
    fn insert_is_idempotent_and_creates_chat() {
        let s = Store::open_in_memory().unwrap();
        let m = msg("a@s.whatsapp.net", "1", 10, false, "hi");
        assert_eq!(
            s.insert_messages(std::slice::from_ref(&m)).unwrap(),
            vec!["1"]
        );
        assert!(s.insert_messages(&[m]).unwrap().is_empty());
        let rows = s.chat_rows().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].meta.last_ts, 10);
        assert_eq!(
            rows[0].last.as_ref().unwrap().content,
            Content::Text("hi".into())
        );
    }

    #[test]
    fn paging_returns_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let msgs: Vec<_> = (0..10)
            .map(|i| msg("c", &format!("{i:02}"), 100 + i, i % 2 == 0, "x"))
            .collect();
        s.insert_messages(&msgs).unwrap();
        let last3 = s.load_messages("c", None, 3).unwrap();
        assert_eq!(
            last3.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["07", "08", "09"]
        );
        let before = s.load_messages("c", Some((107, "07")), 3).unwrap();
        assert_eq!(
            before.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["04", "05", "06"]
        );
    }

    #[test]
    fn receipts_are_monotonic_in_store() {
        let s = Store::open_in_memory().unwrap();
        s.insert_messages(&[msg("c", "1", 1, true, "x")]).unwrap();
        let ch = s
            .apply_receipt("c", &["1".into(), "missing".into()], ReceiptStatus::Read)
            .unwrap();
        assert_eq!(ch, vec![("1".to_string(), ReceiptStatus::Read)]);
        assert!(
            s.apply_receipt("c", &["1".into()], ReceiptStatus::Delivered)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.message("c", "1").unwrap().unwrap().status,
            ReceiptStatus::Read
        );
        // Re-inserting an older copy (history sync) does not downgrade.
        s.insert_messages(&[msg("c", "1", 1, true, "x")]).unwrap();
        assert_eq!(
            s.message("c", "1").unwrap().unwrap().status,
            ReceiptStatus::Read
        );
    }

    #[test]
    fn deletion_replaces_content() {
        let s = Store::open_in_memory().unwrap();
        s.insert_messages(&[msg("c", "1", 1, false, "oops")])
            .unwrap();
        let mut del = msg("c", "1", 1, false, "");
        del.content = Content::Deleted;
        s.insert_messages(&[del]).unwrap();
        assert_eq!(
            s.message("c", "1").unwrap().unwrap().content,
            Content::Deleted
        );
    }

    #[test]
    fn chat_merge_keeps_names_and_orders_pinned_first() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_chats(&[
            ChatMeta {
                id: "a".into(),
                name: Some("Ana".into()),
                last_ts: 50,
                ..Default::default()
            },
            ChatMeta {
                id: "b".into(),
                name: None,
                last_ts: 10,
                pinned: true,
                ..Default::default()
            },
        ])
        .unwrap();
        s.upsert_chats(&[ChatMeta {
            id: "a".into(),
            name: None,
            last_ts: 20,
            unread: 3,
            ..Default::default()
        }])
        .unwrap();
        let rows = s.chat_rows().unwrap();
        assert_eq!(rows[0].meta.id, "b");
        assert_eq!(rows[1].meta.name.as_deref(), Some("Ana"));
        assert_eq!(rows[1].meta.last_ts, 50);
        assert_eq!(rows[1].meta.unread, 3);
    }

    #[test]
    fn image_thumbnail_is_stored_out_of_band() {
        let s = Store::open_in_memory().unwrap();
        let mut m = msg("c", "img", 1, false, "");
        m.content = Content::Image {
            caption: Some("cap".into()),
            width: 10,
            height: 20,
            thumbnail: Some(vec![1, 2, 3]),
            media: None,
        };
        s.insert_messages(std::slice::from_ref(&m)).unwrap();
        assert_eq!(s.message("c", "img").unwrap().unwrap(), m);
    }

    #[test]
    fn contacts_kv_and_clear() {
        let s = Store::open_in_memory().unwrap();
        s.set_contact_names(&[("a".into(), "Ana".into())]).unwrap();
        s.set_kv("own_jid", "me").unwrap();
        assert_eq!(
            s.contact_names().unwrap().get("a").map(String::as_str),
            Some("Ana")
        );
        assert_eq!(s.kv("own_jid").unwrap().as_deref(), Some("me"));
        s.clear().unwrap();
        assert!(s.contact_names().unwrap().is_empty());
        assert!(s.kv("own_jid").unwrap().is_none());
    }

    #[test]
    fn cache_file_is_plain_sqlite_not_sqlcipher_keyed() {
        // The cache is opened without a key, so it must stay readable by a
        // fresh connection (it is a cache, not a secret store).
        let dir = std::env::temp_dir().join(format!("ferrochat-test-{}", std::process::id()));
        let path = dir.join("cache.sqlite3");
        let s = Store::open(&path).unwrap();
        s.set_kv("k", "v").unwrap();
        drop(s);
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.kv("k").unwrap().as_deref(), Some("v"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
