# Status — where the work stands

Read `PLAN.md` first (the agreed design; §8 is the work plan). This file says what is done,
what is verified, and exactly what is left, so work can resume from here.

Last updated: 2026-10-27, during milestone I1 (integration).

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
| Desktop migration of the A1 library | (no code change) | ran on a copy of the user's real data: 2 books + positions + theme |

## I1 — what's left

1. ~~Android build with the core~~ done (see table). Debugging the phone's WebView:
   `npx tauri android build --debug --apk --target aarch64` (env: `ANDROID_HOME=~/Android/Sdk
   NDK_HOME=~/Android/Sdk/ndk/<ver> JAVA_HOME=/usr/lib/jvm/java-21-openjdk`), install
   `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`,
   `adb forward tcp:9333 localabstract:webview_devtools_remote_<pid>`, use the DevTools
   protocol; reinstall the release APK (`npm run android:build`) afterwards.
2. ~~Desktop migration~~ verified on a copy; the real data migrates on the user's first launch.
   **Firewall:** the user's Linux has ufw active with rules only for Bits (47100, 5353). The
   reader needs UDP 47110 from the LAN: `sudo ufw allow from 192.168.100.0/24 to any port 47110 proto udp`.
3. **Pair the phone and the Linux app for real**, then the I1 checks in PLAN.md §8 on real
   devices (the user does the Windows boot): import on Linux → ✓ phone → shut down → Windows
   resumes at the passage; scroll-then-close; Wi-Fi off mid-transfer; kill mid-reading;
   divergence prompt; screen-off handoff with the phone locked.
4. **Windows CI app build** — public repo https://github.com/1RV1NG-Y/reader-sync; the
   `Windows` workflow runs core + app tests, builds the app and uploads
   `reader-sync-windows-x64` (the exe). Download: `gh run download <run id> -n reader-sync-windows-x64`.
   Check the latest run with `gh run list --limit 1`.
5. **Android background measurement** (PLAN §5, §8 "Right away") — not done. Decides whether
   background work comes before milestone D.

## Known gaps / notes for whoever continues

- The local IndexedDB backend (`app/src/library.mjs`, `createLocalLibrary`) remains for the
  browser preview (`npm run dev`) and its tests; the app inside Tauri always uses the core.
- `app/vendor/foliate-js/paginator.js` carries one local patch (see `VENDORED.md`); a test in
  `app/tests/config.test.mjs` fails if it's lost.
- Book covers are not synced: each device makes its own from its copy of the file.
- Trash / remove download / delete forever exist in the core (RPC `setTrashed`,
  `removeDownload`, `onlyCopy`, `deleteForever`) but have no UI yet (milestone D).
- User preferences: no title bars on Linux apps (window controls in the app's own style);
  don't re-propose settled decisions (e.g. the phone is the bridge); the app must work for
  ordinary setups, not only the user's dual boot.
