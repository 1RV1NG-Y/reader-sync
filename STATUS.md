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
5. **Android background measurement — done, and it decides the next step.** Measured with the
   reader itself on the Galaxy S22 (Android 16), 2026-10-27, paired with the Linux app: while
   the app is in the foreground, sessions hold; when the screen goes off (power button) or the
   app is backgrounded, the session times out ~10 s later (8 s QUIC idle timeout) and the
   phone stops syncing. So the phone is unreachable in ordinary use (phone in pocket while
   reading on the PC) → per PLAN §5/§8, **Android background work is now part of I1**:
   keep the process (the Rust node runs in it, independent of the WebView) alive while
   needed, choosing the foreground-service type from the workload (keep a LAN listener
   alive; receive small updates and occasional files) against Android's documented types
   (`dataSync` is limited to 6 h/24 h for apps targeting Android 15+; `connectedDevice` has
   its own permission prerequisites; `specialUse` only if nothing else fits). Verify Doze
   behaviour (network access for a foreground service with the screen off) on the device.

## Windows CI fix and interrupted implementation

The first Windows run failed at the `only_copy` assertion after the interrupted download
had already resumed and finished. This was a holdings-propagation race in the test, not
an established Windows file-handle problem. The test now waits for Linux to learn Phone's
holding before checking both devices, and compares the completed file with the original.
Validation: all 35 core unit tests and 2 integration tests passed locally. A Windows run
with this fix is still required.

The implementation agents hit their usage limit. Uncommitted work is preserved for review:

- `app/src/library-core.mjs` and `app/src/positions.mjs`: partial checkpoint scheduling and
  handoff-generation changes. Regression tests and Reader/Handoff wiring are unfinished.
- `app/src-tauri/src/android.rs`, `src/lib.rs`, `Cargo.toml`, and the generated ACL manifest:
  partial Android service bridge. Kotlin service/plugin implementation is missing; this
  must not be treated as a working Android build or installed on the phone yet.
- No new phone installation or idle-mode test was performed. Existing phone data and pairing
  are unchanged. The user authorized those tests once a complete build is ready.

Use at most one narrowly scoped subagent at a time; parallel agents exhausted the user's
usage allowance. Review unfinished changes before continuing, and do not publish them as
completed features.

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
