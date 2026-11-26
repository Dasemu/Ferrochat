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

## Review round 1

* Never delete a session automatically. Only a real logout wipes; a wrong
  key is `SQLITE_NOTADB` and still needs the user's confirmation; I/O and
  migration errors are retryable errors. `StreamReplaced` is not a logout.
* Count unread by "the store did not have this id", not by delivery path:
  the offline drain after reconnect is exactly when users expect badges.
* Compressed media needs output caps: a few hundred kB of Opus packets can
  decode to gigabytes. Cap by declared duration and packet count.
* `pkg-config` will not link system libraries statically; build libopus from
  the bundled source (`LIBOPUS_NO_PKG=1`) to get a static link.
* Version sync must be a real merge of main into development (simulated in
  `.github/scripts/version.sh`), or the second release conflicts.

## Tray / background mode

* `ksni` 0.3 depends on zbus 5, which the tree already had (via `rfd`/`ashpd`
  and the keyring); `notify-rust` brings zbus 6. Two zbus versions are
  pre-existing, adding the tray did not add a third.
* `ksni::spawn` fails when no StatusNotifierWatcher is on the bus. That result
  is the only trustworthy "is there a tray" signal; the window mode must be
  decided from it, never assumed. Watch `watcher_offline` too and reopen the
  window if the panel dies while hidden.
* `iced::daemon` stays alive with zero windows and drops the compositor when
  the last window closes. Set `exit_on_close_request: false` on the window and
  route `close_requests()` through the state machine.
* After the window is closed glibc keeps the freed pages; `malloc_trim(0)`
  gives them back (a few MB). The font system and the app state dominate the
  remaining resident size.
* zbus proxies cache properties by default and ksni announces changes with
  `New*` signals, not `PropertiesChanged`: build the test proxy with
  `CacheProperties::No`.
* `xdotool windowclose` destroys the window, it does not send
  `WM_DELETE_WINDOW`; to test close requests under Xvfb send the client
  message yourself.

## Release automation

- Bot pushes made with `GITHUB_TOKEN` never trigger `push` workflows, so the release workflow chains off the version workflow with `workflow_run` (conclusion `success`, branch `main`, checkout `ref: main` for the bump commit).


## Round 4: status, communities, navigation

* **`status@broadcast` is not a chat.** v0.8 stored statuses as messages of a
  fake group. They are diverted before they reach `Event::Messages`, and a
  migration deletes the old rows. Never route a pseudo JID (`@broadcast`,
  `@newsletter`) through the chat code.
* **The library's read-receipt privacy gate does not cover statuses** (it
  only softens private DMs). Check the account's `readreceipts` privacy value
  yourself before sending a status view, and treat "cannot read it" as off.
* **Keep thumbnails out of the structures `view` clones.** Grouping the status
  list runs every frame; the in-memory status keeps only a decoded handle in
  a byte-budgeted LRU, the bytes live in the cache table.
* **Reuse the video viewer by id, not by message.** A status video sets
  `App::video` with the status id and the root view skips its full-window
  video layer while the status viewer is open; leaving the status viewer must
  also clear `App::video` (the helper process ends on drop), or the plain
  video viewer pops up.
* **A newer app-state collection needs a new resync marker.** Stars were never
  replayed for sessions synced before they were handled; bumping
  `RESYNC_VERSION` requests one more full snapshot.
* **Debug builds are slow in Xvfb**: screenshot scenes that wait for protocol
  data need a longer capture delay (`ScreenshotTarget::is_nav`).

## Round 3: flicker, names, lists, video

* **Hover flicker was a reflow, not a redraw.** The hover buttons (smiley,
  chevron) were inserted into the bubble's row only while the pointer was
  over it. They took about 70 px from the row, so a bubble that was already
  near the pane width re-wrapped its text, the row grew by a line or two, the
  content moved under the pointer and `on_exit` / `on_enter` fired again: the
  message "blinked" while the mouse crossed it. Image and SVG handles were
  *not* the cause here (handles are cached in the app state and icon handles
  are content-hashed). Fix: the buttons' slot is always laid out (same size
  hovered or not) and only its content appears. Rule: **hover must never
  change layout; reserve the space.**
* **A layout regression test without a window.** `iced::Renderer` implements
  `renderer::Headless` (backend `"tiny-skia"`), so a unit test can lay out a
  widget with `Widget::layout` and compare sizes. `ui::chat::layout_tests`
  lays out the same message with and without hover at three pane widths and
  fails on the old code (371.6 px vs 409.3 px high for a long bubble at
  700 px).
* **Run `Task`s headlessly to test the whole app.** `iced_runtime::task::
  into_stream` (dev-dependency, same version iced uses) lets a test run a
  `Task`, collect its `Action::Output` messages and feed them back to
  `App::update`: `app::test_support::Harness` drives the real `App` and
  `Store` against `FakeClient`.
* **One name map was the whole "wrong names" bug.** Push names, history names
  and the address-book name shared one `HashMap`, so the last writer won and
  message push names beat saved names. Names now carry their source and the
  display order is a pure function (`ChatList::resolve`).
* **Address-book names only come through app-state sync** (`contact`
  mutations), and a session that was already synced never replays them: a
  one-time full snapshot request (`MajorSyncTask::AppStateSync { full_sync:
  true }`, marker `appstate_resync` in the cache) is needed for existing
  users.
* **Keep GStreamer out of the main binary.** A hard link against
  `libgstreamer` makes a machine without it fail at load time with a loader
  error instead of a message. A helper process (`ferrochat-player`) isolates
  the dependency, gives a precise failure ("GStreamer missing" vs "codec
  missing"), and freeing memory on close is just ending the process. The
  release workflow asserts with `ldd` that `ferrochat` has no `gst` link.
* `iced_video_player` renders through a wgpu shader only, so nothing shows on
  the tiny-skia fallback; one more reason for the helper design.
* `mouse_area` gives no window position for right clicks: track the cursor in
  an atomic from `listen_with` (returning `None`, so no message per move) and
  read it when the click arrives.
* `Task::perform` + `spawn_blocking` work on a plain tokio test runtime, which
  is all the harness needs.

- `keyring` 4 opens a *blocking* zbus connection: if any dependency turns on zbus `tokio` (ksni default features, `rfd`/`ashpd` `tokio`, a `zbus` dev-dep) it panics ("Cannot start a runtime from within a runtime") when called on a runtime worker. Keep zbus on `async-io` (`ksni` with `default-features = false, features = ["async-io"]`, `rfd` with `async-std`); `tests/real_keyring.rs` guards it against a real Secret Service.


- Status posting is switched off (`app::status::STATUS_POSTING_ENABLED = false`). `whatsapp-rust` posts to an explicit recipient list and does not expose the account's status privacy ("My contacts except..." / "Only share with..."), so posting to all saved contacts could leak a status to people the user excluded. "My status" is view-only; the composer code stays behind the constant. Re-enable only when the library exposes the privacy lists and we send to exactly that audience.

## Round 4: names, bubbles, unread, scrolling, video

* **A snapshot is not a resync.** wacore 0.7 drops an app-state snapshot
  whose version is not newer than the persisted one, so `full_sync: true` on
  a synced collection replays nothing (warn "Skipping stale snapshot", target
  `AppState`). Reset the version and MACs first. And never write a "done"
  marker from a call that returns `()`: measure the result (versions,
  contact events counted where the library dispatches them).
* **Library log targets are not crate paths.** whatsapp-rust logs app state
  under `Client/AppState` / `AppState`; `RUST_LOG=whatsapp_rust=info` hides
  them.
* **iced 0.14 fluid sizing.** A container or column with a Fill child
  becomes Fill. A bubble holding a right-aligned time therefore split its row
  50/50 with the spacer. Mark such widgets `width(Shrink)`; inside a Shrink
  (compressed) parent, Fill children get the widest non-Fill sibling's
  width, and a column whose children are all Fill collapses to zero width
  (text then wraps one glyph per line). Columns drop void (`Fixed(0)`)
  children, so a zero-height probe must report Shrink.
* **tiny-skia does not clip images and renders shadows per pixel.**
  `ContentFit::Cover` pictures spill over neighbours (the "dark rectangle"),
  and any quad `Shadow` costs an SDF evaluation per pixel per frame. Use
  Contain, and draw small shadows as plain quads.
* **Profile the raster, not just `view`.** `app::perf` drives the real view
  through iced's `UserInterface` and the headless renderer; view + layout
  were small, rasterization was the frame. Build the renderer crates with
  `--config profile.dev.package.<crate>.opt-level=3` for realistic numbers.
* **OpenH264 handles High profile with B-frames** only with
  `Flush::NoFlush`; the crate's default flush after every sample breaks the
  reorder buffer (9 of 90 frames decoded).
* **Count unread chats, not messages**, and keep `status@broadcast` and
  channels out of the chat list: status updates arrive as messages.
- **Quit can hang on the blocking pool.** iced ends `daemon::run` by dropping its tokio runtime, and
  `Runtime::drop` waits for every `spawn_blocking` task with no timeout. KDE Plasma keeps notifications in
  its history and sends `NotificationClosed` only on dismissal, so `wait_for_action` on the blocking pool
  froze Quit (found in Ferrocord v0.17.0 with gdb). Watch clicks on detached threads.

## Round 6: composer, emoji, mentions, viewer, info drawer

* **Shift+Enter sent the message** because the composer was a `text_input`:
  its `on_submit` fires on Enter whatever the modifiers and it cannot hold a
  newline. A `text_editor` with a `key_binding` (plain Enter ->
  `Binding::Custom(Send)`, Shift+Enter -> `Binding::Enter`) is the fix;
  Ferrocord had the same migration. Its `min/max_height` exclude padding.
* **Emoji**: the picker was a fixed 45-entry `grid` with no `scrollable`,
  drawn with whatever system font had the glyph (mono fallbacks, tofu,
  COLRv1 Noto drawn blank by swash). Bundling Twemoji as COLRv0 (2 MB,
  colour vector layers swash renders) and putting it FIRST in cosmic-text's
  common fallback list fixes every text widget at once; spans per grapheme
  cluster are still needed where Inter has the base glyph (keycaps, "#",
  (c)). The font is loaded zero-copy (`fontdb::Source::Binary`); debug-build
  idle RSS in mock mode with it is ~101 MB, first frame ~64 ms. A test shapes all ~1900 picker emoji
  and fails on any that is not exactly one glyph of the font.
* **Don't reflow a picker under the pointer**: updating "Recent" while the
  drawer is open moved the grid between two clicks; the shown list is a
  snapshot taken when the drawer opens.
* **"@123456789012345" previews** were the raw message text: the preview
  never resolved mentions and the bubble guessed PN vs LID from the digits.
  Keep `contextInfo.mentionedJid` and resolve through the LID <-> PN pairs;
  never show a LID's digits (they are not a phone number).
* **The old image viewer could not enlarge**: an `image` without explicit
  size is laid out at its natural size (Shrink), and without the full file
  it showed the 100 px inline thumbnail. A canvas viewer computes the fitted
  rectangle itself, so zoom around the cursor and pan are a few lines. Keep
  the full picture in the viewer's own slot when the shared LRU does not
  have it, and drop it on close.
* tiny-skia does not clip images (also inside a canvas frame): a zoomed
  picture paints over its siblings; draw the bar as a later stack layer.
* **"Click here for contact info" did nothing**: it was a fallback subtitle
  string; the header had no handler and no info panel existed.
* **Grey video placeholders**: the only preview source was the inline
  `jpegThumbnail`, which history-synced videos can lack. A poster frame from
  the helper (first `F` message, then drop the player), stored in the
  thumbnail column, gives every small video a preview once.
* `window::screenshot` captures do not show text inserted programmatically
  into a `text_editor`; xdotool with `windowfocus --sync` (Xvfb has no WM)
  drives the real flow.
* **Erratic unread in the open chat** had three causes: (1) a message that
  arrived while the window was unfocused raised the badge but was never
  queued for reading (`pending_read` only grew while focused), and focusing
  again cleared nothing, so the open chat stayed "unread"; (2) a history
  chunk replaced the chat's metadata wholesale, unread counter included,
  re-marking a chat the user was reading; (3) receipts went to the phone
  number a LID-addressed message had been normalised to, not to the JID it
  came from, so the phone could keep it unread. "Read" now means focused +
  newest page + scrolled to the bottom (`state::is_reading`); receipts use
  the wire JID (`App::wire`).

## In-app updater

* Run network work for the updater on a detached `std::thread` with a
  oneshot back to iced, not `spawn_blocking`: iced drops its tokio runtime
  on quit and waits for blocking tasks, so a stalled download would freeze
  Quit (same root cause as the notification quit hang).
* `ureq` 3 was already in the tree (whatsapp-rust, rustls + webpki roots);
  with `max_redirects(0)` every redirect hop can be checked against the host
  allowlist before it is followed.
* Renaming over a running ELF is fine on Linux, but `current_exe()` then
  points at a deleted file: detect the install method once at boot and
  restart into the known target path, not `current_exe()`.
* Release the single-instance lock before starting the new process, or the
  new one sees "already running" and hands off to the dying old one.
## Calls

* **Read the pinned crate before deciding something is impossible.** The
  brief assumed WhatsApp call media would need reverse engineering; the
  `whatsapp-rust` 0.7.0 already on disk ships a full VoIP media plane
  behind the `voip` feature (call key, relay, SRTP, MLOW/Opus). Grepping the
  registry source took minutes; the feature flag was the whole "port".
* **Keep the library's own objects for actions that need them.** Decline
  and answer take the `IncomingCall` offer (call key, relay, ringing
  generation), so the real client stores ringing offers; the app only sees
  call ids.
* **One terminate, two events.** With `voip` on, an unanswered ringing call
  produces both `IncomingCall(terminate)` and `MissedCall`; log by call id
  once, never per event.
* **No devices in tests.** Audio threads and the ringtone are skipped in
  `--mock`; the fake client stands in for the media engine.

## Stickers and GIFs

* **Check what the library dispatches before promising sync.** The phone's
  favourite stickers are app-state `favoriteSticker` mutations, and the
  schema is in `wacore-appstate`, but `whatsapp-rust` 0.7's
  `dispatch_app_state_mutation` silently drops them (no event, no read API).
  Favourites are therefore local, and nothing is written to that collection.
* **Port the self-playing animation widget, not a tick.** Ferrocord's
  `anim_image` asks for `request_redraw_at` only for its own next frame, on a
  shared 60 Hz grid, only while visible: animated stickers and GIFs cost
  nothing while the window is unfocused or closed, and `view` never runs
  per frame.
* **A GIF is an MP4, so decode it where the codecs are.** The helper already
  had OpenH264 + the `mp4` crate; a `--frames` mode (decode as fast as
  possible, keep evenly spaced frames within a byte budget, exit) and a
  `--gif-to-mp4` mode (OpenH264's encoder is in the same static library,
  the `mp4` crate has a writer) kept the main binary codec-free. OpenH264's
  encoder rejects `UsageType::CameraVideoNonRealTime` ("Invalid usage type
  = 2"); skipped frames must be off or the sample table gets holes.
* **`image` cannot write animated WebP**, so mock animated stickers wrap
  per-frame lossless VP8L bitstreams in `VP8X` / `ANIM` / `ANMF` chunks by
  hand (`mock::animated_webp`). It only encodes lossless WebP: stickers made
  from photos are posterized until they are a reasonable size.
* **Hash keys tie the views together.** Using the hex SHA-256 of the file as
  the shelf key makes it equal to the received copy's `cache_key`, so a sent
  sticker's bubble shows at once and a sticker seen in a chat is the same
  picker entry. The mock and fake clients now use real hashes too.
* **Don't borrow another app's API key.** WhatsApp's GIF search uses its own
  Tenor key; the honest option is the user's own GIPHY key in the keyring,
  with recent GIFs and GIF files working without one.
* The shared build host ran out of disk mid-build: `rm -rf
  target/debug/incremental` and `CARGO_INCREMENTAL=0` got it going again.

## Photo quality ("full image appears much later")

* **Five causes stacked up, none of them "the network is slow".** (1) An
  own sent photo never entered the picture cache and nothing asked for it
  after the upload: the bubble kept the 96 px thumbnail until an unrelated
  event (the next message, reopening the chat) ran the auto-download pass,
  which then downloaded our own upload back. (2) That pass only ever looked
  at the newest 30 messages: older photos scrolled into view stayed
  thumbnails forever (only the viewer fetched them). (3) A failed download
  only logged a warning; it was retried by accident on the next event and
  the user had no "Retry". (4) The library's ureq agent has no timeouts at
  all, so a dead pooled socket (suspend / network change) held one of the
  4 download slots until TCP gave up, minutes later. (5) Pictures were
  memory-only, so every restart started from thumbnails again.
* **`Handle::from_bytes` decodes in the renderer.** iced's tiny-skia
  raster cache decodes a full 12 Mpx JPEG on the UI thread when it is first
  drawn (~80 ms optimised, 2.4 s in a debug build, 45 MB RGBA) and again
  after every frame it was not drawn (the cache is trimmed per present).
  Decode + scale to the 660 px the bubble can show in a worker and hand iced
  RGBA; the LRU budget then measures real memory.
* **Find what is on screen with a widget operation**, the same way the
  popups find their row (`actions::Locate`): collect the rows' bounds and the
  scrollable's translation, debounced after scroll events. No message per
  scroll frame, no estimate from pixel offsets. The `Harness` drops widget
  actions, so test the operation against a `UserInterface` built from the
  real view and the flow by sending `Done::Visible`.
* A spinner does not need an app tick: a widget that requests its own next
  redraw only while visible (like `anim_image`), and that stays invisible
  for the first 0.7 s so fast downloads never flash it.
