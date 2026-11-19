-- Schema version 3: replies, edits, reactions, per-participant group
-- receipts and message search.
ALTER TABLE messages ADD COLUMN reply TEXT;
ALTER TABLE messages ADD COLUMN edited INTEGER NOT NULL DEFAULT 0;
-- Lower-cased searchable text (body, caption or file name); NULL for media
-- without text. Back-filled by `Store::init` for rows written before v3.
ALTER TABLE messages ADD COLUMN search TEXT;

CREATE TABLE IF NOT EXISTS reactions (
    chat_id TEXT NOT NULL,
    msg_id  TEXT NOT NULL,
    reactor TEXT NOT NULL,
    emoji   TEXT NOT NULL,
    ts      INTEGER NOT NULL,
    PRIMARY KEY (chat_id, msg_id, reactor)
);

CREATE TABLE IF NOT EXISTS receipts (
    chat_id     TEXT NOT NULL,
    msg_id      TEXT NOT NULL,
    participant TEXT NOT NULL,
    status      INTEGER NOT NULL,
    PRIMARY KEY (chat_id, msg_id, participant)
);
