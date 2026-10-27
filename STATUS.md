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

## I1 — what's left

1. **Android build with the core** — not built yet since linking the core. Build with
   `cd app && npm run android:build` (debug-signed release; SDK at `~/Android/Sdk`, see
   `scripts/build-apk.mjs`), install with `adb install -r app/android-dist/reader-sync.apk`
   (phone: `adb mdns services` shows its wireless-debugging address; `adb connect` it).
   Check: the app starts (no "library could not start" error), the A1 IndexedDB library is
   migrated (Ted Chiang book stays, with position), Devices → Scan code opens the camera.
   For debugging the WebView: `npx tauri android build --debug --apk --target aarch64` (needs
   `ANDROID_HOME=~/Android/Sdk NDK_HOME=~/Android/Sdk/ndk/<ver> JAVA_HOME=/usr/lib/jvm/java-21-openjdk`),
   install `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`,
   then `adb forward tcp:9333 localabstract:webview_devtools_remote_<pid>` and use the
   DevTools protocol. Reinstall the release APK afterwards.
2. **Migration check on desktop** — the user's real Linux library (A1, IndexedDB under
   `~/.local/share/app.readersync`) migrates on first launch of this build. Not yet observed;
   test on a copy first (`XDG_DATA_HOME=<copy>`), then let the user launch the real app.
3. **Pair the phone and the Linux app for real**, then the I1 checks in PLAN.md §8 on real
   devices (the user does the Windows boot): import on Linux → ✓ phone → shut down → Windows
   resumes at the passage; scroll-then-close; Wi-Fi off mid-transfer; kill mid-reading;
   divergence prompt; screen-off handoff with the phone locked.
4. **Windows CI app build** — `.github/workflows/windows.yml` only runs core tests. Add: Node
   setup, `npm ci` in `app/`, `npx tauri build --no-bundle` (or a zip of the exe), upload the
   artifact. The repo has no GitHub remote yet; ask the user before creating one.
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
