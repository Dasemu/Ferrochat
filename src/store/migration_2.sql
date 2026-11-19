-- Schema version 2: disappearing-message expiry and the LID ↔ phone-number map.
ALTER TABLE messages ADD COLUMN expires_at INTEGER;
CREATE INDEX IF NOT EXISTS messages_by_expiry ON messages (expires_at) WHERE expires_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS lid_map (
    lid TEXT PRIMARY KEY,
    pn  TEXT NOT NULL
);
