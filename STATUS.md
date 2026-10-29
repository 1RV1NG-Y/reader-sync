# Status — where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan). This file says what is done,
what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-29. Milestone I1 (integration) is implemented; what remains of it is
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
| Desktop background: tray, quit after sync, no-tray fallback, single instance, start at login (PLAN §5) | "Desktop: keep syncing in the tray" | core test (`outgoing`); Linux, isolated instances: close keeps syncing; second launch reopens the window; Quit waited until a 150 MB book reached the peer (3.4 s), then exited; same without a tray (private D-Bus); login entry written/removed; `--background` starts tray-only. Windows: CI build only, not tried |

## Current state of the user's devices

- **Linux:** release build (with the tray) installed at `~/.local/share/reader-sync/reader-sync`, menu entry
  "reader-sync". Update after changes with `npm run install:linux` in `app/`. Library data is in
  the Tauri app-data dir (`~/.local/share/app.readersync/library`), shared with dev builds.
- **Phone (Galaxy S22, Android 16):** release APK from `a2f3a94` (library, publisher styles, PDF
  recolouring) installed 2026-10-29 with `adb install -r`, not opened since — another agent was
  using the phone; the sync service restarts when the app is next opened. Not yet tested on the
  phone. Wireless adb's port changes when debugging restarts (2026-10-29: 35417) — find it with
  `avahi-browse -rpt _adb-tls-connect._tcp`), paired with Linux,
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
7. **Desktop background on Windows (user):** close the window → tray icon stays, its menu says
   what's being sent; Quit; Devices → "Start when you log in".
8. Android service, not yet exercised: notification permission denied, Pause/Resume from the
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

## Milestone D — daily driver (in progress)

Done (2026-10-29), checked in the Linux app (isolated instances over WebDriver) and the browser
preview; installed on the phone but **not yet tried there or on Windows** (CI build):
- **Library management:** each book has a menu (⋯ button, right-click, long-press): Remove from
  this device (only-copy warning), Download again, Move to Trash (with Undo). Trash sheet:
  Restore, Delete forever, Empty Trash (dialogs render through a portal so they sit above
  sheets). Core: `BookView.downloadRemoved`, RPC `restoreDownload`.
- **Search and sort:** bar under the toolbar; title/author/file name, accent-insensitive; sort by
  recently read, title (ignoring articles), author surname, recently added (`library-view.mjs`).
- **Publisher styles** switch (synced setting `publisherStyles`): off drops the book's fixed
  sizes, floats, positioning and spacing (`settings.mjs` `PLAIN_LAYOUT`). Fixes Accelerando's
  chapter headings.
- **PDFs:** pages recoloured for sepia/dark by mapping each pixel's luminance onto the theme
  (`pdfPalette`, `recolorPixels`; done on the canvas because WebKitGTK ignores SVG filters on
  HTML); per-book "Original colours" (core `Book.originalColors`, via `updateBook`); "Page: Fit
  width / Whole page" (setting `pdfZoom`). Second foliate-js patch: fit-width measures inside
  the scrollbar (VENDORED.md); the engine re-fits once after a page turn when needed.

Left in D: metadata editing, covers (sync or better generation). The user's daily-use check.

Backlog (user's decision): import from Foliate / Thorium / Moon+; Android "Open with" + share
target; desktop "Open with" (handle a file passed at launch, then add
`MimeType=application/epub+zip;application/pdf;` back to the desktop entry in
`app/scripts/install-linux.mjs`); device rename.

## Testing tools

- Linux app under WebDriver: `TAURI_WEBVIEW_AUTOMATION=true` + `WebKitWebDriver`;
  `app/tests/e2e/two-instances.py` runs two instances (`READER_SYNC_DEVICE_NAME` sets names,
  `READER_SYNC_MULTI_INSTANCE=1` turns off single-instance so they can run side by side).
- Tray menus can be read and clicked over D-Bus (`busctl --user`: the item's `Menu` property,
  then `com.canonical.dbusmenu` `GetLayout` / `Event … clicked`). The autostart library writes
  to the real `~/.config/autostart` even when `XDG_CONFIG_HOME` is redirected.
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
