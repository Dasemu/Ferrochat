# Ferrochat

## Overview
Native Rust WhatsApp desktop client (linked device). `iced` 0.14 UI that mimics
WhatsApp Desktop dark theme; protocol via pure-Rust `whatsapp-rust` 0.7 behind
the `net::WaClient` trait. Unofficial: third-party clients can get accounts banned.

## Commands
- `cargo build` / `cargo build --release` (use `CARGO_BUILD_JOBS=2` on shared hosts)
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- `cargo run -- --mock` (fake protocol + mock data, no network, no disk writes)
- Screenshots: `ICED_BACKEND=tiny-skia xvfb-run -a -s "-screen 0 1280x800x24" ./target/debug/ferrochat --screenshot <login|chats|chat> docs/screenshots/<name>.png`
- Regenerate fixtures: `cargo test --test gen_fixtures -- --ignored`

## Architecture
See `docs/architecture.md` (shared Ferro* skeleton + §11 app specifics) and
`docs/protocol.md` (library decision, event mapping, what lands on disk).
`ui -> app -> {net, store, auth, media}`; `net`/`store`/`auth` never import iced widgets.

## Conventions
- English everywhere; Conventional Commits; small commits.
- No `unwrap`/`expect` outside tests (crate lints deny them).
- Only `auth/secrets.rs` touches `keyring`. Never log bodies, JIDs, keys, QR codes:
  use `Redacted`, `MaskedJid`, redacting `Debug` impls.
- Pure logic goes in `app/state.rs`, `net/convert.rs`, `net/types.rs` with unit tests.
- Every new crate needs a one-line justification in the PR.

## Workflow quirks
- Work on `feat/*` branches, PR to `development`. Never push `main`/`development`.
- `version.yml` bumps `Cargo.toml` (PATCH on development, MINOR on dev→main merge);
  dev→main merges are detected via the commits/{sha}/pulls API; runs by
  github-actions[bot] are skipped. Protected branches need a bot bypass.

## Deployment
No packaging yet; ship `target/release/ferrochat`. Wayland `app_id` / desktop
entry id: `dev.ferro.ferrochat`.

## Gotchas
- Only ONE `libsqlite3-sys` may link: it is pinned to 0.37 with `bundled-sqlcipher`;
  `rusqlite` must stay on 0.39 and `whatsapp-rust` default features stay OFF.
- SQLCipher needs OpenSSL (`libssl-dev`); libopus is built from bundled source and
  linked statically (`.cargo/config.toml`, needs `cmake`).
- Never delete the session store except on a real logout; see `docs/protocol.md` §3.
- `scrollable().anchor_bottom()` reverses offsets: "bottom" is `RelativeOffset::START`.
- iced style fns returning `impl Fn` need `+ use<>` (edition 2024 captures).
- Keep iced default features on (x11/wayland backends).
- No Secret Service in CI: tests use `MemoryStore`.
