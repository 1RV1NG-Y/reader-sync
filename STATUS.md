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
3. **Phone/Linux pairing is done.** Finish the I1 checks in PLAN.md §8 on real
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

## Codex handoff — stopping at the user's usage-budget request

### Completed and verified

- `bdee757`: corrected the Windows interrupted-transfer test's holdings-propagation race.
  The download itself had succeeded; the assertion ran before the peer learned the new
  holding. All 35 core unit tests and 2 integration tests passed locally. Windows run
  `40616459713` passed its core/app tests and is still compiling the app at handoff;
  check its final result and artifact before claiming Windows is ready.
- `6e58c02`: fixes pending checkpoints being discarded by an older idle commit, and prevents
  old save/handoff responses from showing a false “has your place” after newer movement.
  Four new backend regression tests pass, all frontend tests pass, and Linux builds.
  The two-instance UI test now asserts transferred CFI equality and the receiving reader's
  actual position; it passed (target 0.6, reopened 0.599655). Optional screenshots were removed
  from that test because WebKit's screenshot request hung under this compositor.

### Android background implementation

Native service/runtime/plugin implementation is now present (not the earlier partial bridge):
`connectedDevice` foreground service, persistent notification with Pause, CPU/Wi-Fi/multicast
locks, network-change callbacks, Devices → Background LAN sync controls, optional battery
exemption, and shared Rust node ownership with stop/reopen support. It uses the existing
`app.readersync` identity and library path. A signed release APK built successfully.

See the phone-test result below before treating background sync as verified. Remaining work
includes real Linux → phone → Windows handoff, Wi-Fi interruption and kill-mid-read tests,
position-conflict UI checks, and battery/endurance measurement. No additional implementation
should be inferred from a successful APK build.

### Phone test results and immediate next steps

- Latest signed APK is installed on the S22 using `adb install -r`; books and pairing
  preserved. The `connectedDevice` foreground service was observed running.
- Native node lifecycle regression passed three stop/reopen cycles with the same identity.
- Three isolated test books/positions reached the phone and were durably acknowledged,
  including over a fresh connection while the app was backgrounded.
- **Locked-screen and Doze reliability is NOT verified.** The display was later found awake
  during successful receipts. A confirmed screen-off/forced-IDLE state was reached, but no
  new receipt test completed in that state before the user's stop request.
- Next: verify receipt AND fresh connection with the screen confirmed off; test Doze with
  and without the optional battery exemption. Then exercise notification permission
  grant/denial, Pause/Resume/reopen, and sticky service recreation on the phone.
- Temporary forced idle and simulated battery state were reset; original battery exemption
  state is unchanged (not exempt). Notification permission was not granted by tests.
  The app/service remains running normally. Test helpers were stopped.
- Three test books remain in paired libraries; no real reading positions were changed.
  Local cleanup IDs: `/tmp/reader-phone-test-ids.jsonl`. Full native evidence and helper
  locations: `/tmp/reader-android-handoff.md`. Build log: `/tmp/reader-sync-android-build.log`.

The user requested a prompt handoff because over 60% of the five-hour usage allowance was
spent. Use narrowly scoped subagents sparingly; do not restart parallel implementation teams.

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
