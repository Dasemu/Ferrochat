-- Schema version 5: statuses (24 hour stories) and starred messages.
-- Before this version statuses were stored as messages of a fake chat
-- "status@broadcast"; they are not conversations, so that chat is dropped.
DELETE FROM messages  WHERE chat_id = 'status@broadcast';
DELETE FROM reactions WHERE chat_id = 'status@broadcast';
DELETE FROM receipts  WHERE chat_id = 'status@broadcast';
DELETE FROM chats     WHERE id      = 'status@broadcast';

CREATE TABLE IF NOT EXISTS statuses (
    author    TEXT NOT NULL,
    id        TEXT NOT NULL,
    ts        INTEGER NOT NULL,
    push_name TEXT,
    from_me   INTEGER NOT NULL DEFAULT 0,
    kind      TEXT NOT NULL,
    body      TEXT NOT NULL,
    thumb     BLOB,
    viewed    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (author, id)
);
CREATE INDEX IF NOT EXISTS statuses_ts ON statuses (ts);

CREATE TABLE IF NOT EXISTS starred (
    chat_id TEXT NOT NULL,
    msg_id  TEXT NOT NULL,
    ts      INTEGER NOT NULL,
    PRIMARY KEY (chat_id, msg_id)
);
CREATE INDEX IF NOT EXISTS starred_ts ON starred (ts);
