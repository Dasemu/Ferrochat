# Ferrochat

A lightweight, native WhatsApp desktop client written in Rust. It links to
your phone as a companion device (like WhatsApp Web / Desktop), using the
pure-Rust [`whatsapp-rust`](https://github.com/jlucaso1/whatsapp-rust)
implementation of the multi-device protocol, and draws its UI with
[`iced`](https://iced.rs) in a look that closely follows WhatsApp Desktop's
dark theme.

> [!WARNING]
> **Unofficial client.** Ferrochat is not affiliated with, endorsed by or
> supported by WhatsApp or Meta. **Third-party clients break WhatsApp's
> terms of service and can get your account temporarily or permanently
> banned.** Use it at your own risk, ideally with an account you can afford
> to lose. No WhatsApp logos, icons or fonts are shipped; the look is
> recreated with Inter (OFL) and Lucide icons (ISC).

## Install / update

Linux x86_64, no root needed. One command installs and updates (it downloads
the latest [release](https://github.com/Dasemu/Ferrochat/releases/latest), checks
its SHA-256 and puts the binary, menu entry and icon under `~/.local`):

```sh
curl -fsSL https://raw.githubusercontent.com/Dasemu/Ferrochat/main/install.sh | sh
```

Uninstall (removes only what the installer added, never your account data):

```sh
curl -fsSL https://raw.githubusercontent.com/Dasemu/Ferrochat/main/install.sh | sh -s -- --uninstall
```

Manual alternative: download `ferrochat-vX.Y.Z-linux-x86_64.tar.gz` and its
`.sha256` from the Releases page, then:

```sh
sha256sum -c ferrochat-*-linux-x86_64.tar.gz.sha256
tar xzf ferrochat-*-linux-x86_64.tar.gz
./ferrochat-*-linux-x86_64/ferrochat
```

The tarball also has `packaging/dev.ferro.ferrochat.desktop` and `.svg` if you want
a menu entry. Releases are built on Ubuntu 22.04 (glibc 2.35).

![Chat](docs/screenshots/chat.png)

## Features

* QR-code pairing as a linked device (codes refresh automatically, reload
  when they run out), logout / unlink.
* Chat list with search, filter chips (All / Unread / Favourites / Groups),
  unread badges, pinned and muted markers, last-message preview with ticks.
* History from WhatsApp's history sync, cached in SQLite; older pages load
  when scrolling up.
* Send and receive text, photos, documents and voice notes (record with the
  mic button, play back with a waveform).
* Read receipts, both ways: grey / blue ticks on your messages, read and
  played receipts sent when you open a chat or play a voice note.
* Desktop notifications (sender + preview, or sender only).

See [docs/protocol.md](docs/protocol.md) for what works, what is stored on
disk and the known gaps.

## Build

Rust stable (edition 2024, ≥ 1.94) and, on Debian/Ubuntu:

```sh
sudo apt-get install -y pkg-config cmake libdbus-1-dev libasound2-dev \
  libxkbcommon-dev libwayland-dev libssl-dev libfontconfig1-dev
cargo build --release
```

`libssl-dev` is needed for SQLCipher (the encrypted session store);
`cmake` builds the bundled libopus, which is linked statically
(`.cargo/config.toml` sets `LIBOPUS_NO_PKG=1`), so the binary needs no
libopus package at runtime. Runtime packages: OpenSSL's `libcrypto`, ALSA /
PipeWire, D-Bus, a Secret Service (KWallet or GNOME Keyring) and
`xdg-open`. Linux (KDE Plasma / Wayland) is the primary
target; Windows and macOS should build but are not tested.

## Run

```sh
./target/release/ferrochat            # real account: scan the QR code with your phone
./target/release/ferrochat --mock     # demo data, no network, nothing written to disk
./target/release/ferrochat --screenshot chat out.png   # render a mock screen and exit
```

A Secret Service (KWallet or GNOME Keyring) is required: Ferrochat stores
the key of its encrypted session database there and refuses to fall back to
a plain file.

Environment: `RUST_LOG` (log filter), `FERRO_THEME=light`,
`FERRO_TRACE_STARTUP=1` (prints `first_frame_ms`), `FERROCHAT_NOTIFY=sender`
(notifications without message preview), `FERROCHAT_DATA_DIR`.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Architecture: [docs/architecture.md](docs/architecture.md). Lessons learned:
[LESSONS.md](LESSONS.md).

## Licence

MIT. Inter is under the SIL Open Font License 1.1, Lucide icons under ISC
(see `assets/`).
