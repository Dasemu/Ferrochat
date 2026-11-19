# Protocol

Ferrochat is a **linked device** (companion) on the WhatsApp multi-device
protocol, the same role WhatsApp Web and Desktop play. The phone stays the
primary device; Ferrochat pairs by scanning a QR code.

> Ferrochat is unofficial. Third-party clients break WhatsApp's terms of
> service and can get the account banned.

## 1. Library decision: `whatsapp-rust` (pure Rust), no Go sidecar

The brief asks for a maintained pure-Rust implementation if one exists and
works, otherwise `whatsmeow` (Go) as a sidecar. Evaluation of
[`whatsapp-rust`](https://github.com/jlucaso1/whatsapp-rust) 0.7.0
(time-boxed, done before writing app code):

| Question | Finding |
|---|---|
| Maintained? | Yes. 10 releases from 0.1.0 (2025-10) to 0.7.0 (2026-08-07), ~186k downloads, MIT. Inspired by whatsmeow and Baileys, which it tracks closely. |
| Builds here? | Yes, on Rust 1.97 with `default-features = false` and `tokio-runtime, tokio-native, tokio-transport, ureq-client`. Its storage crate builds against SQLCipher (see §3). |
| QR pairing as a linked device | Yes: `Event::PairingQrCode { code, timeout }` rotates codes, `PairingQrCodesExhausted`, `PairSuccess`, `PairError`; `DevicePropsOverride` names the device ("Ferrochat") in the phone's Linked devices list. Pair-code login also exists (not used). |
| Send / receive text | Yes: `Client::send_message_with_options` (caller-chosen message id), `Event::Messages` batches with decrypted `wa::Message` + `MessageInfo`. |
| Media upload / download | Yes: `Client::upload` (encrypts, returns `direct_path`, `media_key`, hashes, streaming sidecar for audio) and `Client::download_from_params` (decrypts + verifies). Works for images, documents and audio/PTT. |
| Receipts | Yes: `Event::Receipt` (delivered / read / played / read-self), `Client::mark_as_read`, `Client::mark_as_played`. |
| History sync | Yes: `Event::HistorySync(LazyHistorySync)` with lazily inflated `wa::HistorySync` (conversations, messages, push names). |
| Logout | `Client::logout()` deregisters the companion; deleting the store wipes keys. |
| Storage | Pluggable `Backend`; the bundled SQLite store has a per-connection init hook documented for SQLCipher keying. |

**Decision: use `whatsapp-rust`.** It covers every core feature in the list
natively. A Go sidecar would add a second toolchain, a second process to
supervise, an IPC protocol to version and a larger memory footprint, for no
feature gain.

The library is wrapped behind the `net::WaClient` trait plus the
`net::Event` enum, so the app never sees library types and is driven by
`net::fake::FakeClient` in `--mock` mode and tests. All protobuf ↔ domain
conversion lives in pure functions in `src/net/convert.rs`.

## 2. Event mapping

| Library event | `net::Event` | App effect |
|---|---|---|
| `PairingQrCode` | `Qr { code, valid_for_secs }` | QR rendered with `qrcode` → RGBA image; `AuthState::ShowingQr { generation }` increments |
| `PairingQrCodesExhausted` | `QrExhausted` | "Click to reload QR code" (restarts the client) |
| `PairSuccess` / `PairError` | same | logged in / error under the QR |
| `Connected` | `Connection(Connected)` + `Connected { jid }` | banner cleared |
| `Disconnected`, `StreamError`, `ConnectFailure` 500/503 | `Connection(Reconnecting { attempt })` | banner; the library reconnects with backoff |
| `ConnectFailure` other 4xx | `Connection(Offline { reason })` | library stopped; app retries with its own backoff (1 s → 60 s + jitter) and shows "Retry now" |
| `ConnectFailure` 401/403/406, `LoggedOut` | `LoggedOut { reason }` | session store and key deleted, cache wiped, back to QR (the only automatic wipe) |
| `StreamReplaced` | `Connection(Replaced)` | **non-destructive**: library stopped, banner "WhatsApp is open on another computer" with "Use here" |
| `TemporaryBan`, `ClientOutdated` | `Fatal` | shown on the login screen |
| `Messages` | `LidMappings` + `Messages { origin: Live \| Offline }` | stored first; ids the store did not have count as unread (live and offline-drained alike); one notification per live message, one summary per offline drain |
| `Receipt` | `Receipt` or `ChatRead` (read-self) | tick state machine |
| `HistorySync` | `LidMappings` + `History` / `Messages { origin: History }` in chunks of 500 | chats, names, cached messages; never counted as unread |
| `PushNameUpdate`, `ContactUpdate` | `ContactName` | display names |
| `MarkChatAsReadUpdate` | `ChatRead` | badge cleared |

Message bodies handled: `conversation`, `extendedTextMessage`,
`imageMessage`, `documentMessage` (also wrapped in
`documentWithCaptionMessage`), `audioMessage` (PTT and audio files),
`protocolMessage{REVOKE}` (shown as "This message was deleted", applied only
when the revoker is the original sender: our own devices for our messages,
the same participant otherwise; group-admin revokes are ignored because they
cannot be verified locally). Ephemeral wrappers are unwrapped; disappearing
messages carry `expires_at` and are pruned from the cache when it passes.
**View-once** messages are rendered as "View once message: open it on your
phone": their media reference is never exposed, downloaded or stored. Video, stickers, locations,
contacts and polls show an "unsupported" placeholder; reactions and key
distribution messages are not bubbles.

## 3. What lands on disk

| Path | Content | Protection |
|---|---|---|
| OS keyring, service `dev.ferro.ferrochat`, account `signal-store-key` | 256-bit random key (hex) for the session store | Secret Service (KWallet / GNOME Keyring), Keychain, Credential Manager |
| `$XDG_DATA_HOME/ferrochat/session.sqlcipher` (+ `-wal`, `-shm`) | The library's whole Signal / device state: identity key pair, noise key, signed pre-keys, sessions, sender keys, app-state sync keys, LID mappings | **SQLCipher** (AES-256, HMAC per page) keyed with the raw key above (`PRAGMA key = "x'…'"`, no PBKDF2). File mode 0600. Tested: no `SQLite format 3` header and no schema text in the file; a wrong key is rejected. |
| `$XDG_DATA_HOME/ferrochat/cache.sqlite3` (+ `-wal`, `-shm`) | Cache: chats, message bodies, media references (CDN path + per-file media key, needed to download later), contact names, own JID | Plain SQLite, mode 0600, like the official desktop client's local database. Deletable at any time; rebuilt from history sync. Never contains credentials or Signal keys. |
| `$XDG_STATE_HOME/ferrochat/log` (rotated at 5 MB) | Logs | Bodies, phone numbers, QR payloads, keys and file names are redacted (`Redacted`, `MaskedJid`, manual `Debug` impls; covered by `tests/redaction.rs`). |
| `~/Downloads/<name>` | Documents the user opened | Only on explicit click. |

The data directory is created with mode 0700 and every database file
(including `-wal` / `-shm`) is set to 0600. Downloads are capped at 100 MB
and images are probed (header only) and rejected above 40 Mpx before
decoding. Notification titles and bodies are markup-escaped (`&`, `<`, `>`)
because KDE renders a subset of HTML in them.

Images and voice notes are kept in memory only (byte-bounded LRUs: 48 MB
images, 16 MB decoded audio) and re-downloaded after a restart.

The session store is **never deleted automatically** except on a real
logout (user action or server-side unlink). Only `SQLITE_NOTADB` (wrong key)
or a missing/malformed keyring entry next to an existing session counts as
"unreadable": the login screen then explains it and offers "Discard saved
session and link again"; no new key is minted over an existing session. Any
other error (I/O, permissions, busy database, migration) is shown as an
error with "Try again" and nothing is touched. If no Secret Service is
reachable the login screen explains how to enable KWallet / GNOME Keyring
and "Try again" retries without restarting; there is no file fallback (an
opt-in passphrase-encrypted fallback is future work).

A single-instance lock (`ferrochat.lock` in the data directory, OS file
lock) prevents two Ferrochat processes from sharing one session store.

The SQLCipher key only lives in zeroized buffers (`secrecy`, `Zeroizing`).
Media limits: downloads ≤ 100 MB, at most 4 concurrent; images are
header-probed and refused above 16 Mpx when auto-downloaded and 40 Mpx when
opened; voice notes refuse to decode beyond their declared duration
(+50 % + 10 s) or 10 minutes, and beyond 90 000 packets (decompression
bombs). Recording stops at 30 minutes. Documents with executable or active
content types (`.exe`, `.bat`, `.ps1`, `.lnk`, `.jar`, `.msi`, `.desktop`,
`.sh`, `.AppImage`, `.html`, `.svg`, …) are saved but never launched; file
names are sanitised (separators, control characters, trailing dots/spaces,
reserved Windows names). Files are opened without a shell (`open` crate:
`xdg-open` / `ShellExecuteW`).

`--mock` mode uses an in-memory secret store and an in-memory cache and
writes nothing except an optional screenshot.

## 4. Sending

1. The app generates a WhatsApp-Web-shaped id (`3EB0` + 18 hex) and inserts
   an optimistic `Pending` bubble (clock icon).
2. `send_text` / `send_media` (upload then send) runs as a `Task`.
3. Success → `Sent` (one grey tick); error → `Failed` (red icon, click to
   retry text).
4. Receipts move it to `Delivered` (two grey ticks), `Read` (two blue
   ticks), `Played` (voice notes).

The receipt state machine (`ReceiptStatus::advance`) only moves forward
(`Pending → Sent → Delivered → Read → Played`), tolerates out-of-order
receipts, only allows `Failed` from `Pending`, and lets a later server
confirmation override `Failed`. It is applied identically in memory and in
the store, so a late history-sync copy can never downgrade ticks.

Outgoing photos get a 96 px JPEG thumbnail and real dimensions
(`image` crate); other files go as documents with their MIME type.
Voice notes are recorded with `cpal`, down-mixed/resampled to 48 kHz mono,
encoded with libopus (24 kbit/s VoIP mode) into Ogg with a correct
`OpusHead`/`OpusTags`, granule positions and pre-skip, plus a 64-bucket
waveform.

## 5. Read receipts

When a chat is opened (or the window regains focus with a chat open), the
last *unread* incoming messages are marked read with `mark_as_read`,
batched per participant in groups (the protocol needs the sender there).
Playing an incoming voice note sends a `played` receipt. Live messages in
the focused open chat are marked read immediately.

## 6. Voice note playback

Ogg demux with the pure-Rust `ogg` crate, Opus decode with `opus` (libopus
bindings; `symphonia` has no Opus decoder), output through `rodio` on a
dedicated audio thread. The UI polls position every 100 ms while playing and
draws the waveform bars as played / unplayed.

## 7. Known protocol gaps

* LID vs. phone-number addressing: LID ↔ PN pairs are learned from
  envelopes (`sender_alt` / `recipient_alt`), history sync
  (`phoneNumberToLidMappings`, conversation `pnJid`/`lidJid`) and persisted
  (`lid_map` table); messages, receipts, history chats and names are
  normalised to the PN JID. A LID seen before any mapping is learned still
  forms its own chat until the pair arrives (existing rows are not merged
  retroactively).
* Group "read" ticks turn blue on the first participant's read receipt
  (WhatsApp waits for all participants).
* No profile pictures, typing indicators, replies, reactions, edits or
  message search yet.

## UI reference

The palette, metrics and screen map that reproduce the official look are in
`docs/architecture.md` §11.
