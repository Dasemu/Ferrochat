//! Pure state types and reducers (unit-tested without iced).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::net::types::{
    ChatId, ChatMeta, Content, Message, ReceiptStatus, fallback_name, is_group_jid,
};
use crate::store::ChatRow;

/// Max messages kept in memory for the open conversation (architecture §3).
pub const WINDOW: usize = 200;

/// Chat list filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// Everything.
    #[default]
    All,
    /// Chats with unread messages.
    Unread,
    /// Pinned chats (stand-in for "Favourites").
    Favourites,
    /// Group chats.
    Groups,
}

impl Filter {
    /// Chip label.
    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Unread => "Unread",
            Filter::Favourites => "Favourites",
            Filter::Groups => "Groups",
        }
    }

    /// All chips in display order.
    pub const ALL: [Filter; 4] = [
        Filter::All,
        Filter::Unread,
        Filter::Favourites,
        Filter::Groups,
    ];
}

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatEntry {
    /// Metadata.
    pub meta: ChatMeta,
    /// Newest message.
    pub last: Option<Message>,
}

impl ChatEntry {
    /// True for group chats.
    pub fn is_group(&self) -> bool {
        is_group_jid(&self.meta.id)
    }
}

/// The chat list plus the contact-name directory.
#[derive(Debug, Clone, Default)]
pub struct ChatList {
    /// Rows, sorted pinned-first then by recency. Read-only outside; use the
    /// methods to mutate so the index stays in sync.
    pub entries: Vec<ChatEntry>,
    /// JID → display name.
    pub names: HashMap<String, String>,
    /// JID → position in `entries` (O(1) lookups during history sync).
    index: HashMap<String, usize>,
}

impl ChatList {
    /// Builds from cache rows.
    pub fn from_rows(rows: Vec<ChatRow>, names: HashMap<String, String>) -> Self {
        let mut list = Self {
            entries: rows
                .into_iter()
                .map(|r| ChatEntry {
                    meta: r.meta,
                    last: r.last,
                })
                .collect(),
            names,
            index: HashMap::new(),
        };
        list.sort();
        list
    }

    fn reindex(&mut self) {
        self.index.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.index.insert(e.meta.id.clone(), i);
        }
    }

    /// Re-sorts: pinned first, then newest activity.
    pub fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            b.meta
                .pinned
                .cmp(&a.meta.pinned)
                .then(b.meta.last_ts.cmp(&a.meta.last_ts))
                .then(a.meta.id.cmp(&b.meta.id))
        });
        self.reindex();
    }

    /// Display name for a chat or a sender.
    pub fn display_name(&self, jid: &str) -> String {
        if let Some(e) = self.get(jid)
            && let Some(n) = &e.meta.name
        {
            return n.clone();
        }
        self.names
            .get(jid)
            .cloned()
            .unwrap_or_else(|| fallback_name(jid))
    }

    /// Mutable entry, created if missing.
    fn entry_mut(&mut self, chat: &str) -> &mut ChatEntry {
        if let Some(&i) = self.index.get(chat) {
            return &mut self.entries[i];
        }
        self.index.insert(chat.to_string(), self.entries.len());
        self.entries.push(ChatEntry {
            meta: ChatMeta {
                id: chat.to_string(),
                ..Default::default()
            },
            last: None,
        });
        let last = self.entries.len() - 1;
        &mut self.entries[last]
    }

    /// Finds an entry.
    pub fn get(&self, chat: &str) -> Option<&ChatEntry> {
        self.index.get(chat).and_then(|&i| self.entries.get(i))
    }

    fn get_mut(&mut self, chat: &str) -> Option<&mut ChatEntry> {
        let i = *self.index.get(chat)?;
        self.entries.get_mut(i)
    }

    /// Merges history-sync metadata.
    pub fn apply_history(&mut self, chats: &[ChatMeta], names: &[(String, String)]) {
        for (jid, name) in names {
            self.names.insert(jid.clone(), name.clone());
        }
        for c in chats {
            let e = self.entry_mut(&c.id);
            let name = c.name.clone().or_else(|| e.meta.name.clone());
            let last_ts = e.meta.last_ts.max(c.last_ts);
            e.meta = ChatMeta {
                name,
                last_ts,
                ..c.clone()
            };
        }
        self.sort();
    }

    /// Applies incoming/outgoing messages. Incoming messages whose id is in
    /// `fresh` (newly stored: live or drained from the offline queue) count
    /// as unread unless that chat is open and the window focused. History
    /// passes `None`: it carries its own unread counters.
    pub fn apply_messages(
        &mut self,
        msgs: &[Message],
        open_chat: Option<&str>,
        focused: bool,
        fresh: Option<&HashSet<String>>,
    ) {
        for m in msgs {
            if !m.from_me
                && let Some(n) = &m.sender_name
                && !self.names.contains_key(&m.sender)
            {
                self.names.insert(m.sender.clone(), n.clone());
            }
            let viewing = open_chat == Some(m.chat.as_str()) && focused;
            let e = self.entry_mut(&m.chat);
            let newer = e
                .last
                .as_ref()
                .is_none_or(|l| (m.timestamp, &m.id) >= (l.timestamp, &l.id));
            let is_fresh = fresh.is_some_and(|f| f.contains(&m.id));
            if is_fresh && !m.from_me && !viewing && !matches!(m.content, Content::Deleted) {
                e.meta.unread += 1;
            }
            if newer {
                e.last = Some(m.clone());
                e.meta.last_ts = e.meta.last_ts.max(m.timestamp);
            } else if let Some(l) = &mut e.last
                && l.id == m.id
            {
                l.content = m.content.clone();
            }
        }
        self.sort();
    }

    /// Receipt for the preview tick.
    pub fn apply_receipt(&mut self, chat: &str, ids: &[String], status: ReceiptStatus) {
        if let Some(e) = self.get_mut(chat)
            && let Some(l) = &mut e.last
            && l.from_me
            && ids.contains(&l.id)
        {
            l.status = l.status.advance(status);
        }
    }

    /// Clears the unread badge.
    pub fn mark_read(&mut self, chat: &str) {
        if let Some(e) = self.get_mut(chat) {
            e.meta.unread = 0;
        }
    }

    /// Rows matching the search query and filter.
    pub fn filtered(&self, query: &str, filter: Filter) -> Vec<&ChatEntry> {
        let q = query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| !e.meta.archived)
            .filter(|e| match filter {
                Filter::All => true,
                Filter::Unread => e.meta.unread > 0,
                Filter::Favourites => e.meta.pinned,
                Filter::Groups => e.is_group(),
            })
            .filter(|e| {
                q.is_empty()
                    || self.display_name(&e.meta.id).to_lowercase().contains(&q)
                    || e.last
                        .as_ref()
                        .is_some_and(|l| l.content.preview().to_lowercase().contains(&q))
            })
            .collect()
    }

    /// Number of archived chats (for the "Archived" row).
    pub fn archived_count(&self) -> usize {
        self.entries.iter().filter(|e| e.meta.archived).count()
    }
}

/// The open conversation's message window.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    /// Chat id.
    pub chat: ChatId,
    /// Messages, oldest first, at most [`WINDOW`].
    pub messages: VecDeque<Message>,
    /// More history exists in the cache.
    pub has_more: bool,
}

impl Conversation {
    /// New window from cached messages.
    pub fn new(chat: ChatId, msgs: Vec<Message>, has_more: bool) -> Self {
        let mut c = Self {
            chat,
            messages: VecDeque::new(),
            has_more,
        };
        c.merge(msgs);
        c
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.messages.iter().position(|m| m.id == id)
    }

    /// Inserts messages in timestamp order, merging duplicates (status via
    /// the receipt state machine, deletions replace content). Trims the
    /// oldest when over [`WINDOW`].
    pub fn merge(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            if m.chat != self.chat {
                continue;
            }
            if let Some(i) = self.position(&m.id) {
                let cur = &mut self.messages[i];
                cur.status = cur.status.advance(m.status);
                let deleted = matches!(m.content, Content::Deleted)
                    && crate::store::revoke_allowed(cur.from_me, &cur.sender, &m);
                let gained_media = cur.content.media().is_none() && m.content.media().is_some();
                if deleted || gained_media {
                    cur.content = m.content;
                }
                continue;
            }
            if matches!(m.content, Content::Deleted) {
                // Revoke for a message outside the window / never seen.
                continue;
            }
            let idx = self
                .messages
                .iter()
                .rposition(|x| (x.timestamp, &x.id) <= (m.timestamp, &m.id))
                .map(|i| i + 1)
                .unwrap_or(0);
            self.messages.insert(idx, m);
        }
        while self.messages.len() > WINDOW {
            self.messages.pop_front();
            self.has_more = true;
        }
    }

    /// Prepends an older page.
    pub fn prepend(&mut self, older: Vec<Message>, has_more: bool) {
        for m in older.into_iter().rev() {
            if self.position(&m.id).is_none() {
                self.messages.push_front(m);
            }
        }
        while self.messages.len() > WINDOW {
            self.messages.pop_back();
        }
        self.has_more = has_more;
    }

    /// Applies a receipt to outgoing messages.
    pub fn apply_receipt(&mut self, ids: &[String], status: ReceiptStatus) -> bool {
        let mut changed = false;
        for m in self
            .messages
            .iter_mut()
            .filter(|m| m.from_me && ids.contains(&m.id))
        {
            let next = m.status.advance(status);
            changed |= next != m.status;
            m.status = next;
        }
        changed
    }

    /// Replaces the content of a message (upload finished).
    pub fn set_content(&mut self, id: &str, content: Content) {
        if let Some(i) = self.position(id) {
            self.messages[i].content = content;
        }
    }

    /// Finds a message.
    pub fn get(&self, id: &str) -> Option<&Message> {
        self.messages.iter().find(|m| m.id == id)
    }

    /// Cursor for paging older messages.
    pub fn oldest_cursor(&self) -> Option<(i64, String)> {
        self.messages.front().map(|m| (m.timestamp, m.id.clone()))
    }
}

/// Groups incoming message ids per sender for read receipts (groups need
/// the participant). Returns `(sender_for_group, ids)`.
pub fn read_receipt_batches(chat: &str, msgs: &[Message]) -> Vec<(Option<String>, Vec<String>)> {
    let group = is_group_jid(chat);
    let mut out: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for m in msgs.iter().filter(|m| !m.from_me && m.chat == chat) {
        let key = if group { Some(m.sender.clone()) } else { None };
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, ids)) => ids.push(m.id.clone()),
            None => out.push((key, vec![m.id.clone()])),
        }
    }
    out
}

/// Whether a live message should raise a desktop notification.
pub fn should_notify(m: &Message, open_chat: Option<&str>, focused: bool, muted: bool) -> bool {
    if m.from_me || muted || matches!(m.content, Content::Deleted) {
        return false;
    }
    !(focused && open_chat == Some(m.chat.as_str()))
}

/// Day bucket label for date separators ("Today", "Yesterday", weekday,
/// or a date), computed against `today` (local days since epoch).
pub fn day_label(day: i64, today: i64, weekday: &str, date: &str) -> String {
    match today - day {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => weekday.into(),
        _ => date.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(chat: &str, id: &str, ts: i64, from_me: bool) -> Message {
        Message {
            id: id.into(),
            chat: chat.into(),
            sender: if from_me {
                "me".into()
            } else {
                format!("s-{id}")
            },
            sender_name: Some("Bob".into()),
            from_me,
            timestamp: ts,
            content: Content::Text(format!("msg {id}")),
            status: if from_me {
                ReceiptStatus::Pending
            } else {
                ReceiptStatus::Read
            },
            expires_at: None,
        }
    }

    fn fresh(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn list() -> ChatList {
        let mut l = ChatList::default();
        l.apply_history(
            &[
                ChatMeta {
                    id: "a".into(),
                    name: Some("Alice".into()),
                    last_ts: 10,
                    ..Default::default()
                },
                ChatMeta {
                    id: "g@g.us".into(),
                    name: Some("Group".into()),
                    last_ts: 20,
                    unread: 2,
                    ..Default::default()
                },
            ],
            &[],
        );
        l
    }

    #[test]
    fn new_message_moves_chat_to_top_and_counts_unread() {
        let mut l = list();
        assert_eq!(l.entries[0].meta.id, "g@g.us");
        l.apply_messages(&[m("a", "1", 5, false)], None, true, None);
        assert_eq!(l.get("a").unwrap().meta.unread, 0, "history does not count");
        l.apply_messages(&[m("a", "2", 30, false)], None, true, Some(&fresh(&["2"])));
        assert_eq!(l.entries[0].meta.id, "a");
        assert_eq!(l.get("a").unwrap().meta.unread, 1);
        // Same message again (not newly stored) does not double count.
        l.apply_messages(&[m("a", "2", 30, false)], None, true, Some(&fresh(&[])));
        assert_eq!(l.get("a").unwrap().meta.unread, 1);
        // Offline-drained messages older than the preview still count.
        l.apply_messages(&[m("a", "0", 1, false)], None, true, Some(&fresh(&["0"])));
        assert_eq!(l.get("a").unwrap().meta.unread, 2);
        assert_eq!(l.get("a").unwrap().last.as_ref().unwrap().id, "2");
    }

    #[test]
    fn open_focused_chat_does_not_count_unread() {
        let mut l = list();
        l.apply_messages(
            &[m("a", "2", 12, false)],
            Some("a"),
            true,
            Some(&fresh(&["2"])),
        );
        assert_eq!(l.get("a").unwrap().meta.unread, 0);
        l.apply_messages(
            &[m("a", "3", 13, false)],
            Some("a"),
            false,
            Some(&fresh(&["3"])),
        );
        assert_eq!(l.get("a").unwrap().meta.unread, 1);
        l.mark_read("a");
        assert_eq!(l.get("a").unwrap().meta.unread, 0);
    }

    #[test]
    fn filters_and_search() {
        let mut l = list();
        l.apply_messages(&[m("a", "1", 11, false)], None, true, None);
        assert_eq!(l.filtered("", Filter::Groups).len(), 1);
        assert_eq!(l.filtered("", Filter::Unread).len(), 1);
        assert_eq!(l.filtered("ali", Filter::All).len(), 1);
        assert_eq!(l.filtered("msg 1", Filter::All).len(), 1);
        assert_eq!(l.filtered("zzz", Filter::All).len(), 0);
    }

    #[test]
    fn display_names_fall_back() {
        let mut l = list();
        assert_eq!(l.display_name("a"), "Alice");
        assert_eq!(l.display_name("34600111222@s.whatsapp.net"), "+34600111222");
        l.apply_history(&[], &[("34600111222@s.whatsapp.net".into(), "Zoe".into())]);
        assert_eq!(l.display_name("34600111222@s.whatsapp.net"), "Zoe");
    }

    #[test]
    fn conversation_orders_and_dedupes() {
        let mut c = Conversation::new("a".into(), vec![m("a", "2", 20, false)], false);
        c.merge(vec![
            m("a", "1", 10, true),
            m("a", "3", 30, false),
            m("b", "x", 1, false),
        ]);
        c.merge(vec![m("a", "2", 20, false)]);
        let ids: Vec<_> = c.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[test]
    fn conversation_receipts_and_deletes() {
        let mut c = Conversation::new("a".into(), vec![m("a", "1", 10, true)], false);
        assert!(c.apply_receipt(&["1".into()], ReceiptStatus::Delivered));
        assert!(!c.apply_receipt(&["1".into()], ReceiptStatus::Sent));
        assert_eq!(c.get("1").unwrap().status, ReceiptStatus::Delivered);
        // Someone else cannot revoke my message.
        let mut forged = m("a", "1", 10, false);
        forged.content = Content::Deleted;
        c.merge(vec![forged]);
        assert_ne!(c.get("1").unwrap().content, Content::Deleted);
        let mut del = m("a", "1", 10, true);
        del.content = Content::Deleted;
        c.merge(vec![del.clone()]);
        assert_eq!(c.get("1").unwrap().content, Content::Deleted);
        // Revokes never create bubbles.
        del.id = "unknown".into();
        c.merge(vec![del]);
        assert!(c.get("unknown").is_none());
    }

    #[test]
    fn index_survives_many_chats() {
        let mut l = ChatList::default();
        let chats: Vec<ChatMeta> = (0..5000)
            .map(|i| ChatMeta {
                id: format!("{i}@s.whatsapp.net"),
                last_ts: i,
                ..Default::default()
            })
            .collect();
        l.apply_history(&chats, &[]);
        assert_eq!(l.entries.len(), 5000);
        assert_eq!(l.entries[0].meta.id, "4999@s.whatsapp.net");
        assert_eq!(l.get("17@s.whatsapp.net").unwrap().meta.last_ts, 17);
        l.mark_read("17@s.whatsapp.net");
    }

    #[test]
    fn window_is_bounded() {
        let msgs: Vec<_> = (0..WINDOW as i64 + 50)
            .map(|i| m("a", &format!("{i:05}"), i, false))
            .collect();
        let c = Conversation::new("a".into(), msgs, false);
        assert_eq!(c.messages.len(), WINDOW);
        assert!(c.has_more);
        assert_eq!(c.messages.front().unwrap().id, "00050");
    }

    #[test]
    fn prepend_older_page() {
        let mut c = Conversation::new("a".into(), vec![m("a", "5", 50, false)], true);
        c.prepend(vec![m("a", "3", 30, false), m("a", "4", 40, false)], false);
        let ids: Vec<_> = c.messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["3", "4", "5"]);
        assert!(!c.has_more);
        assert_eq!(c.oldest_cursor(), Some((30, "3".into())));
    }

    #[test]
    fn read_receipts_grouped_per_sender_in_groups() {
        let msgs = vec![
            m("g@g.us", "1", 1, false),
            m("g@g.us", "2", 2, false),
            m("g@g.us", "3", 3, true),
        ];
        let b = read_receipt_batches("g@g.us", &msgs);
        assert_eq!(b.len(), 2);
        assert!(b.iter().all(|(s, _)| s.is_some()));
        let dm = vec![m("a", "1", 1, false), m("a", "2", 2, false)];
        assert_eq!(
            read_receipt_batches("a", &dm),
            vec![(None, vec!["1".into(), "2".into()])]
        );
    }

    #[test]
    fn notification_rules() {
        let msg = m("a", "1", 1, false);
        assert!(should_notify(&msg, None, true, false));
        assert!(!should_notify(&msg, Some("a"), true, false));
        assert!(should_notify(&msg, Some("a"), false, false));
        assert!(!should_notify(&msg, None, false, true));
        assert!(!should_notify(&m("a", "2", 1, true), None, false, false));
    }

    #[test]
    fn day_labels() {
        assert_eq!(day_label(10, 10, "Monday", "1/1/2026"), "Today");
        assert_eq!(day_label(9, 10, "Monday", "1/1/2026"), "Yesterday");
        assert_eq!(day_label(6, 10, "Monday", "1/1/2026"), "Monday");
        assert_eq!(day_label(1, 10, "Monday", "1/1/2026"), "1/1/2026");
    }
}
