-- Schema version 4: contact names keep their source (address book, business
-- verified name, push name) so the display priority is decidable, and chat
-- lists (custom lists, favourites) with their members.
CREATE TABLE IF NOT EXISTS contact_names (
    jid    TEXT NOT NULL,
    source INTEGER NOT NULL,
    name   TEXT NOT NULL,
    PRIMARY KEY (jid, source)
);
-- Everything stored so far came from push names and history sync.
INSERT OR IGNORE INTO contact_names (jid, source, name) SELECT jid, 1, name FROM contacts;
DROP TABLE contacts;

CREATE TABLE IF NOT EXISTS lists (
    id    TEXT PRIMARY KEY,
    name  TEXT NOT NULL,
    color INTEGER NOT NULL DEFAULT 0,
    kind  INTEGER NOT NULL DEFAULT 0,
    ord   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS list_chats (
    list_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    PRIMARY KEY (list_id, chat_id)
);
