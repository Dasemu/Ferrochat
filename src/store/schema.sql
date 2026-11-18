-- Ferrochat local cache, schema version 1.
-- This database is a cache: it can be deleted at any time and is rebuilt
-- from history sync. It never contains credentials or Signal keys.
CREATE TABLE IF NOT EXISTS chats (
    id        TEXT PRIMARY KEY,
    name      TEXT,
    unread    INTEGER NOT NULL DEFAULT 0,
    last_ts   INTEGER NOT NULL DEFAULT 0,
    pinned    INTEGER NOT NULL DEFAULT 0,
    archived  INTEGER NOT NULL DEFAULT 0,
    muted     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    chat_id     TEXT NOT NULL,
    id          TEXT NOT NULL,
    sender      TEXT NOT NULL,
    sender_name TEXT,
    from_me     INTEGER NOT NULL,
    ts          INTEGER NOT NULL,
    kind        TEXT NOT NULL,
    content     TEXT NOT NULL,
    thumb       BLOB,
    status      INTEGER NOT NULL,
    PRIMARY KEY (chat_id, id)
);

CREATE INDEX IF NOT EXISTS messages_by_time ON messages (chat_id, ts, id);

CREATE TABLE IF NOT EXISTS contacts (
    jid  TEXT PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
