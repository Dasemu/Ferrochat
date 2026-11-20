# Ferrochat

## Overview
Native Rust WhatsApp desktop client (linked device). `iced` 0.14 UI that mimics
WhatsApp Desktop dark theme; protocol via pure-Rust `whatsapp-rust` 0.7 behind
the `net::WaClient` trait. Unofficial: third-party clients can get accounts banned.

## Commands
- `cargo build` / `cargo build --release` (use `CARGO_BUILD_JOBS=2` on shared hosts)
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- `cargo run -- --mock` (fake protocol + mock data, no network, no disk writes)
- Screenshots: `ICED_BACKEND=tiny-skia xvfb-run -a -s "-screen 0 1280x800x24" ./target/debug/ferrochat --screenshot <login|chats|chat|reply|reactions|chat-search|list-search|delete|settings> docs/screenshots/<name>.png`
- Tray test needs a session bus: `dbus-run-session -- cargo test` (the SNI test is skipped without one).
- Regenerate fixtures: `cargo test --test gen_fixtures -- --ignored`

- Video: `target/debug/ferrochat-player` must exist next to `ferrochat`
  (`cargo build` builds both). Needs `libgstreamer1.0-dev
  libgstreamer-plugins-base1.0-dev` to build and `gstreamer1.0-plugins-good`
  to run the tests. `--no-default-features` builds without the helper.

## Architecture
See `docs/architecture.md` (shared Ferro* skeleton + §11 app specifics) and
`docs/protocol.md` (library decision, event mapping, what lands on disk).
`ui -> app -> {net, store, auth, media}`; `net`/`store`/`auth` never import iced widgets.

## Conventions
- English everywhere; Conventional Commits; small commits.
- No `unwrap`/`expect` outside tests (crate lints deny them).
- Only `auth/secrets.rs` touches `keyring`. Never log bodies, JIDs, keys, QR codes:
  use `Redacted`, `MaskedJid`, redacting `Debug` impls.
- Pure logic goes in `app/state.rs`, `app/live.rs`, `net/convert.rs`, `net/types.rs`
  with unit tests; user actions live in `app/actions.rs`.
- Every new crate needs a one-line justification in the PR.

## Workflow quirks
- Work on `feat/*` branches, PR to `development`. Never push `main`/`development`.
- `version.yml` bumps `Cargo.toml` (PATCH on development, MINOR on dev→main merge);
  dev→main merges are detected via the commits/{sha}/pulls API; runs by
  github-actions[bot] are skipped. Protected branches need a bot bypass.

## Deployment
Releases are automatic: `.github/workflows/release.yml` chains off the "Version bump" workflow
via `workflow_run` (GITHUB_TOKEN bot pushes don't trigger `push` workflows), builds on
ubuntu-22.04, and creates GitHub Release `v<Cargo version>` with `ferrochat-v<ver>-linux-x86_64.tar.gz`
+ `.sha256` (skips if the release exists; manual re-run: workflow_dispatch). Users run
`install.sh` (`curl ... | sh`; `-s -- --uninstall`); test it with `FERRO_RELEASE_API=file://...`.
Wayland `app_id` / desktop entry id: `dev.ferro.ferrochat` (`packaging/`).

## Gotchas
- Only ONE `libsqlite3-sys` may link: it is pinned to 0.37 with `bundled-sqlcipher`;
  `rusqlite` must stay on 0.39 and `whatsapp-rust` default features stay OFF.
- SQLCipher needs OpenSSL (`libssl-dev`); libopus is built from bundled source and
  linked statically (`.cargo/config.toml`, needs `cmake`).
- Never delete the session store except on a real logout; see `docs/protocol.md` §3.
- `scrollable().anchor_bottom()` reverses offsets: "bottom" is `RelativeOffset::START`.
- The default `tiny-skia` renderer ignores image corner radii: round avatars are
  masked into the RGBA tile (`media::avatar::circle_mask`).
- Message popups (reactions / menu) float in a stack layer placed from the row's
  on-screen bounds (`actions::Locate`); a scroll closes them. Jumping to a message
  uses the same operation plus `scroll_to` (offsets count from the newest end).
- Reactions/edits wait for pending message inserts (`App::deferred`), otherwise
  they race the cache write and are lost.
- Group ticks: per-participant receipts are stored; blue only when all read.
- iced style fns returning `impl Fn` need `+ use<>` (edition 2024 captures).
- The app is an `iced::daemon`: the main window is opened by `App::boot` / `background.rs`,
  hiding to the tray CLOSES it (frees the renderer). Never leave the process running with no
  window and no tray (`app/window.rs` invariant). `ksni` must stay on zbus 5 (shared with ashpd).
- Keep iced default features on (x11/wayland backends).
- No Secret Service in CI: tests use `MemoryStore`.
- Hover must never change layout: the message hover buttons live in a slot
  that is always laid out (`chat::action_slot`); `ui::chat::layout_tests`
  fails if a hovered row changes size.
- Names have sources (saved > business > push "~ Name" > number): never write
  names into one map; use `ChatList::resolve` / `set_name`. Display names of
  senders must not prefer `Message::sender_name` (a push name).
- `ferrochat` must not link GStreamer: only `src/bin/ferrochat-player.rs` may
  `use gstreamer`. Video frames are `image::Handle`s made in `update`, never
  in `view`; the video viewer replaces the whole view while open.
- Chat flags / lists are optimistic: apply, persist, send, revert on failure
  (`app/lists.rs`). Never send the `favorites` app-state action (it carries
  the whole list and would overwrite the phone's).
- App-level tests: `app::test_support::Harness` (real `App` + fake client).
- Statuses are not chats (`net::social`, `app/status.rs`): diverted in
  `net/whatsapp.rs::translate`, own cache tables, 24 h + 1500-row bound. View
  receipts go through `WaClient::send_status_view`, which respects the
  `readreceipts` privacy setting. All Status / panel / star / logout actions
  are `Ui::Nav(NavMsg)` handled in `app/nav.rs`; leaving the status viewer
  must clear `App::video`.
