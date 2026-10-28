# reader-sync

An e-book reader for Linux, Windows and Android that keeps your **library (the book files
themselves), reading positions and reading settings in sync over your local network** —
directly between your devices, with no account, server or cloud service.

Devices that are never on at the same time (say, the two systems of a dual-boot PC) stay in
sync through a third device, normally your phone: it carries books and positions between
them. The reader always tells you whether another device has your latest place
("✓ Phone has your place") before you shut down.

Status: under active development — see [STATUS.md](STATUS.md) for what works today and
[PLAN.md](PLAN.md) for the design.

## Features

- EPUB reading with [foliate-js](https://github.com/johnfactotum/foliate-js), scrolled or
  paginated, with font, size, spacing, margins and themes. PDFs currently use a single-page
  viewer, without EPUB's scrolling and text settings.
- Library sync: import a book on one device and it downloads on the others, verified by its
  BLAKE3 hash; interrupted transfers resume.
- Reading positions per device, chosen without trusting clocks: continuing on another device
  opens where you stopped; reading apart on two devices asks which place to keep.
- Pairing by QR code or a text code; discovery over mDNS; sync over QUIC
  ([iroh](https://github.com/n0-computer/iroh)), replicated with [Loro](https://loro.dev).

## Layout

| Path | What it is |
|---|---|
| `core/` | `readersync-core`: replicated library (books, positions, holdings, settings), durable storage, file store and transfers, pairing and sync. Forked from the sync core of [bits](https://github.com/1RV1NG-Y/bits). |
| `app/` | The app: React + Vite UI, Tauri v2 shell for Linux, Windows and Android (`app/src-tauri`), vendored foliate-js (`app/vendor`). |

## Build and test

```sh
cargo test -p readersync-core          # core: storage, positions, files, multi-device sync
cd app && npm ci && npm test           # UI logic
cd app && npx tauri build --debug --no-bundle    # desktop app → target/debug/reader-sync
cd app && npm run android:build        # debug-signed arm64 APK → app/android-dist/ (Android SDK + NDK)
```

Sync uses UDP port **47110** and mDNS (UDP 5353) on your local network; allow them in your
firewall (e.g. `sudo ufw allow from 192.168.1.0/24 to any port 47110 proto udp`).

## License

MIT — see [LICENSE](LICENSE). foliate-js and its bundled libraries keep their own licenses
(`app/vendor/foliate-js/VENDORED.md`).
