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
| `Disconnected`, `ConnectFailure`, `StreamError` | `Connection(Reconnecting { attempt })` | banner; the library reconnects with backoff |
| `LoggedOut`, `StreamReplaced` | `LoggedOut { reason }` | session store and key deleted, cache wiped, back to QR |
| `TemporaryBan`, `ClientOutdated` | `Fatal` | shown on the login screen |
| `Messages` | `Messages { live }` | stored, reducers, notifications (live only), read receipts |
| `Receipt` | `Receipt` or `ChatRead` (read-self) | tick state machine |
| `HistorySync` | `History` + `Messages { live: false }` | chats, names, cached messages |
| `PushNameUpdate`, `ContactUpdate` | `ContactName` | display names |
| `MarkChatAsReadUpdate` | `ChatRead` | badge cleared |

Message bodies handled: `conversation`, `extendedTextMessage`,
`imageMessage`, `documentMessage` (also wrapped in
`documentWithCaptionMessage`), `audioMessage` (PTT and audio files),
`protocolMessage{REVOKE}` (shown as "This message was deleted").
Ephemeral / view-once wrappers are unwrapped. Video, stickers, locations,
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

Images and voice notes are kept in memory only (byte-bounded LRUs: 48 MB
images, 16 MB decoded audio) and re-downloaded after a restart.

If the keyring has no key but a session file exists (keyring wiped), the
file is undecryptable and is deleted; the user pairs again. If no Secret
Service is reachable the login screen shows an explicit error instead of
falling back to a file.

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

* LID vs. phone-number addressing: chats are keyed by whatever JID the
  server uses (`…@lid` or `…@s.whatsapp.net`); the same contact can appear
  twice if the server switches addressing mid-history.
* Group "read" ticks turn blue on the first participant's read receipt
  (WhatsApp waits for all participants).
* No profile pictures, typing indicators, replies, reactions, edits or
  message search yet.

## UI reference

The palette, metrics and screen map that reproduce the official look are in
`docs/architecture.md` §11.
