# Lessons

Things learned while building Ferrochat that are worth knowing before the
next change.

## Protocol library

* **`whatsapp-rust` 0.7 is complete enough to skip the Go sidecar.** QR
  pairing, text, media upload/download (image, document, audio/PTT),
  receipts (read, played, read-self), history sync and logout are all native
  APIs. The evaluation took about an hour (read the crate source, build it,
  map events); see `docs/protocol.md` §1. A `whatsmeow` sidecar would have
  added a Go toolchain, IPC versioning and a second process for no gain.
* **Wrap it behind a trait from day one.** `net::WaClient` + `net::Event`
  means the whole app, the screenshots and the integration tests run against
  `net::fake::FakeClient`. No real WhatsApp account or network exists in CI.
* **Choose message ids client-side.** `SendOptions::with_message_id` lets the
  optimistic bubble, the store row and the later receipts share one id, so
  there is no "temporary id → server id" rewrite.
* `Event::Messages` is a batch and is at-least-once; the store insert is
  idempotent and the receipt merge monotonic, so redelivery is harmless.

## Encrypting the Signal store

* The library's SQLite store has a per-connection init hook intended for
  SQLCipher. Linking SQLCipher means: `whatsapp-rust` with
  `default-features = false` (drops its plain bundled SQLite),
  `whatsapp-rust-sqlite-storage` with `default-features = false`, and
  `libsqlite3-sys = { version = "0.37", features = ["bundled-sqlcipher"] }`.
  Only one crate may link `sqlite3`, so `rusqlite` must use the same
  `libsqlite3-sys` major (rusqlite 0.39 ↔ libsqlite3-sys 0.37).
* Use the raw-key form `PRAGMA key = "x'<64 hex>'"`: the key is already
  random, PBKDF2 would only slow every connection open.
* The r2d2 pool retries a failing connection for its whole timeout (30 s).
  Verify the key with a plain `rusqlite` connection first so a wrong key
  fails in milliseconds.
* Our own cache uses the same SQLCipher build without a key; unkeyed
  SQLCipher behaves like stock SQLite.

## iced 0.14

* Edition 2024 makes `impl Trait` return types capture every lifetime in
  scope. Style helpers like `fn bubble(t: &FerroTheme) -> impl Fn(&Theme) -> Style`
  need `+ use<>` or every view borrowing them fails with E0621.
* `scrollable().anchor_bottom()` is the right primitive for a chat, but it
  flips offsets: `RelativeOffset::START` is the newest message and
  `on_scroll`'s `relative_offset().y` approaches 1.0 at the *oldest* message.
  `snap_to_end` therefore scrolls to the top.
* `Subscription::run_with` takes a `fn` pointer, not a closure: put
  everything the stream needs (config, a restart generation, the secret store
  `Arc`) in a `Hash` key and hash only the parts that should restart it.
  Build the stream lazily (`stream::once(async { .. }).flatten()`) so
  `tokio::spawn` runs inside the executor.
* Bubble tails: a tiny `canvas` with one filled path is cheap and looks right.
* Big `Message` variants trip `clippy::large_enum_variant`; box task results.

## Audio

* `symphonia` has no Opus decoder. `ogg` (demux) + `opus` (libopus) is the
  smallest reliable combination; remember `OpusHead` pre-skip on decode and
  granule positions on encode.
* `rodio`'s output stream must live on its own thread; poll the position
  from the UI on a 100 ms tick only while playing.

## Environment

* No Secret Service in the container: the real-mode smoke test reaches the
  keyring and shows the explicit error on the login screen, which is the
  designed behaviour. WhatsApp's servers are not reachable from CI.
* A shared build host fills its disk quickly: debug test binaries with full
  debuginfo reached 16 GB. Remove `target/debug/incremental` when short on
  space.
