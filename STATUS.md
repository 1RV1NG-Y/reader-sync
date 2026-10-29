# Status — where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan). This file says what is done,
what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-28. Milestone I1 (integration) is implemented; what remains of it is
testing on real devices; the dual-boot test passed.

## Done

| Milestone | Commit(s) | Verified by |
|---|---|---|
| B1 core: books, positions (revisions + `seen`), handoff status, settings | `854d73c` | `cargo test -p readersync-core` |
| B2 book file sync: store, transfer on sync connection, lifecycle | `8a754e5` | same; incl. 200 MB transfer measurement |
| A1 app shell + reader (Tauri, React, foliate-js scrolled) | `eb34ba2` | `npm test` in `app/`; Linux app driven via WebKitWebDriver |
| Linux: no title bar, controls in the toolbar; native file drop | `524a451` | WebDriver screenshots; user tried drop |
| Android reader fixes (foliate-js lock patch, slider, edge swipes, bars) | `d4c1751` | real touch over adb on Galaxy S22 |
| I1 part 1: app on the core, Devices, handoff indicator, arrivals | `9d1dfe9` | `app/tests/e2e/two-instances.py` (two desktop instances) |
| Android on the core (base64 book bytes over Android IPC) | `77f9f4b` | phone: A1 library migrated (book, position, theme), book opens |
| Desktop migration of the A1 library | (no code change) | user's real Linux data migrated on first launch |
| Public repo, README, MIT license, Windows CI that builds the app | `7bde440` | CI green on every push since `bdee757` |
| Pending checkpoints kept across idle commits; stale "has your place" rejected | `6e58c02` | 4 backend regression tests; two-instance test checks the receiving reader's position |
| Android background sync: `connectedDevice` foreground service | `d8159fc` | S22: screen off 60 s, forced deep Doze, and a fresh connection in Doze — each position durably acked in ~0.5 s (details below) |
| Linux install with a menu entry (`npm run install:linux`) | `79def67` | installed on the user's PC, launched from the menu |

## Current state of the user's devices

- **Linux:** release build installed at `~/.local/share/reader-sync/reader-sync`, menu entry
  "reader-sync". Update after changes with `npm run install:linux` in `app/`. Library data is in
  the Tauri app-data dir (`~/.local/share/app.readersync/library`), shared with dev builds.
- **Phone (Galaxy S22, Android 16):** release APK from `d8159fc` installed, paired with Linux,
  background service running. Rebuild with `npm run android:build` (output
  `app/android-dist/reader-sync.apk`, git-ignored), then `adb install -r`.
- **Windows:** installed by the user from the CI artifact and paired. New builds come from CI: `gh run list --limit 1`, then
  `gh run download <run id> -n reader-sync-windows-x64` (latest green run: `40619126843`).
- **Library:** Accelerando, A Canticle for Leibowitz, Stories of Your Life and Others. Astra's
  three test books were trashed and deleted forever on 2026-10-27; the phone acked it.
- **Firewall:** the user's Linux runs ufw. The reader needs UDP 47110 from the LAN:
  `sudo ufw allow from 192.168.100.0/24 to any port 47110 proto udp` (check it's in place if
  the phone can't connect).

## I1 — what's left (real-device checks, PLAN §8)

1. ~~The dual-boot test~~ **passed 2026-10-28 (user):** Linux → phone → Windows resumed
   correctly; the user also added books on Windows.
2. Scroll on, and confirm the ✓ turns off at once and returns only after the phone acks.
3. Scroll and close immediately → the other device resumes at the closing location.
4. Phone Wi-Fi off halfway through a book's transfer, back on → transfer resumes.
5. Kill the app mid-reading → reopens at the last checkpoint.
6. Read offline on two devices → divergence prompt; choose → no prompt elsewhere.
7. Android service, not yet exercised: notification permission denied, Pause/Resume from the
   notification, service recreated after the process is killed, battery cost over hours, and
   Doze with the optional battery exemption on.

## Android background sync — evidence

Measured 2026-10-27 with the reader itself, real Linux app (driven over WebDriver) ↔ S22, no
battery exemption, using only test books:

1. Screen off (`mWakefulness=Dozing`, display `DOZE`, confirmed before and after): link still up
   after 60 s; a flushed position durably acked by the phone in 509 ms.
2. Deep Doze (`dumpsys battery unplug` + `deviceidle force-idle`, state `IDLE` before and after):
   link up after 45 s; acked in 510 ms.
3. Fresh connection in deep Doze (Linux app closed ~35 s, restarted): reconnected at once, acked
   in 503 ms.

Restored afterwards (`deviceidle unforce`, `battery reset`). Before the service existed, the
phone stopped syncing ~10 s after the screen went off (8 s QUIC idle timeout).

Implementation (Astra): `connectedDevice` foreground service with a persistent notification and
Pause, CPU/Wi-Fi/multicast locks, network-change callbacks, Devices → "Background LAN sync"
controls, optional battery-exemption flow (implemented, not tested). The Rust node is owned by
the Android runtime (`app/src-tauri/src/android.rs`, `node_slot.rs`; Kotlin in
`gen/android/.../LanSyncService.kt`, `LanSyncRuntime.kt`, `LanSyncPlugin.kt`) so it survives
the activity being recreated. `cargo test -p reader-sync --lib node_slot` covers stop/reopen.

## Next milestone: D — daily driver (not started)

From PLAN §8, plus what came up since:
- Trash / remove download / delete forever UI (the core has `setTrashed`, `removeDownload`,
  `onlyCopy`, `deleteForever`; no UI yet).
- Metadata editing, covers, library sorting/search, device rename.
- "Publisher styles: on/off" reading setting (e.g. Accelerando's fixed-height headings).
- PDF support. **Dark/sepia for PDFs:** recolour the rendered page so paper → theme background
  and ink → theme text (a CSS/SVG colour filter, not just `invert`), plus a per-book
  "Original colours" switch for comics and photo-heavy PDFs (images get recoloured too).
- Android "Open with" + share target; desktop "Open with" (handle a file passed at launch,
  then add `MimeType=application/epub+zip;application/pdf;` back to the desktop entry in
  `app/scripts/install-linux.mjs`).
- Import from Foliate / Thorium / Moon+ folders (positions where readable).

## Testing tools

- Linux app under WebDriver: `TAURI_WEBVIEW_AUTOMATION=true` + `WebKitWebDriver`;
  `app/tests/e2e/two-instances.py` runs two instances (`READER_SYNC_DEVICE_NAME` sets names).
- Android WebView: build `npx tauri android build --debug --apk --target aarch64` (env
  `ANDROID_HOME=~/Android/Sdk NDK_HOME=~/Android/Sdk/ndk/<ver>
  JAVA_HOME=/usr/lib/jvm/java-21-openjdk`), install the universal debug APK,
  `adb forward tcp:9333 localabstract:webview_devtools_remote_<pid>`, drive it over the
  DevTools protocol; reinstall the release APK afterwards. Real touch: `adb shell input`.
- The phone is on wireless adb; in fish/zsh use `set -x ANDROID_SERIAL <ip:port>` /
  `export ANDROID_SERIAL=...` rather than an `adb -s` alias variable.

## Known gaps / notes for whoever continues

- The local IndexedDB backend (`app/src/library.mjs`, `createLocalLibrary`) remains for the
  browser preview (`npm run dev`) and its tests; the app inside Tauri always uses the core.
- `app/vendor/foliate-js/paginator.js` carries one local patch (see `VENDORED.md`); a test in
  `app/tests/config.test.mjs` fails if it's lost.
- Book covers are not synced: each device makes its own from its copy of the file.
- WebKit's screenshot request can hang under the user's compositor; the e2e test doesn't use it.
- User preferences: no title bars on Linux apps (window controls in the app's own style);
  don't re-propose settled decisions (e.g. the phone is the bridge); the app must work for
  ordinary setups, not only the user's dual boot. The user may hit usage limits — keep this
  file current after each step.
