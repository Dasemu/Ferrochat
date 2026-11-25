-- Schema version 6: JIDs mentioned by a message (WhatsApp's
-- contextInfo.mentionedJid), a JSON array, so "@123…" in the text can be
-- shown as the person's name. NULL for older rows (resolved by number).
ALTER TABLE messages ADD COLUMN mentions TEXT;
