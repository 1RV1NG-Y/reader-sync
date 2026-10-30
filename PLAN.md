# reader-sync — plan

One e-book reader for Linux, Windows and Android that replaces Foliate, Thorium and Moon+ Reader,
with the **library (book files), reading positions and reading settings synced over the local
network**. No accounts, servers or cloud. Sync goes device to device; when two devices are never
on at the same time (the dual-boot PC), a third device — normally the phone — carries changes
between them. That's the only bridge: nothing in the design depends on one particular setup,
so the app works the same for anyone with a few devices on a home network.

Status: plan approved by Astra (2026-10-27), with final implementation notes folded in.
Nothing is built yet.

---

## 1. What we start from: the Bits sync core

`/home/yo/Projects/kb-bits/sync/core` (branch `sync-prototype`, commit `4afb7b9`) already
provides, with tests:

| Piece | Where | Reused as is? |
|---|---|---|
| Device identity (iroh key per install), QUIC/TLS-authenticated peers | `node.rs` | Yes |
| Pairing by QR / text code, single-use 10-min secret, join vs add codes | `node.rs`, `proto.rs` | Yes, with new identifiers |
| LAN discovery (mDNS), replicated address book, reconnect every 2 s with backoff | `node.rs` | Yes |
| Replicated document (Loro), version-vector sync, forwarding through a middle device | `node.rs` | Yes |
| Durable store: checksummed fsynced log, snapshot compaction, torn-tail recovery, exclusive lock | `store.rs` | Yes |
| Per-peer durable acknowledgements (the peer's acked version vector, kept in `peers.json`) | `node.rs` | Yes — the handoff status (§5) checks a specific revision against it |
| Notes model, merge rules, schema upgrades | `model.rs`, parts of `node.rs` | **No** — replaced by the books model |
| RPC surface, events | `rpc.rs`, parts of `node.rs` | Rewritten for books |
| Dual-boot handoff test (Linux → phone → Linux off → Windows) | `tests/sync.rs:155` | Adapted to positions and files |

**Decision: a focused copy, not a shared framework.** We copy the core into this repo, record
the source commit so fixes can be ported by hand, and replace the note-specific parts. Building a
generic sync library first would double the scope for a two-app user base.

**Separate identity from Bits**, so the two apps can never pair with each other or fight over a
port:

- ALPNs `reader/sync/1`, `reader/pair/1` (file transfers run on the sync connection, §2)
- Pairing-code prefix `READ1` (Bits uses `BITS2`)
- UDP port `47110` (Bits uses `47100`); firewall notes updated accordingly
- Own data directory, own `device.key`, own app identifier `app.readersync`
- Own library id namespace and schema version counter

## 2. Syncing book files

Book files must **not** go into the Loro document. Loro keeps full history (Bits' README lists
this as a known limitation), frames are capped at 128 MB, and every device would have to keep
every byte of every book forever. Instead:

- **Replicated document (Loro)** holds metadata, positions, and *which device holds which file*.
- **File store** holds the bytes, content-addressed, outside Loro.
- **File protocol** moves bytes between paired devices, as extra QUIC streams on the
  established sync connection.

### File store

- Files live at `books/<blake3-hash>.<ext>`. The hash of the **original bytes** is the file's
  identity. Editing title/author changes metadata only, never the identity.
- Receiving: write to `<hash>.part` → fsync → verify BLAKE3 of the whole file → rename → fsync
  the directory → **then** record "this device holds `<hash>`" in the replicated document (which
  is itself durable before it's acknowledged). So "the phone holds this book" is only ever
  claimed after the phone has the complete, verified, durable file.
- All file-state changes on a device (download finishing, remove download, purge, startup
  reconciliation) go through **one file manager task per device**, so they happen in a fixed
  order and never race each other. Lifecycle rules are in §6.

### File protocol

Request-driven:

```
Want { hash: [u8; 32], offset }   →   Have { size } + raw bytes from offset   |   Missing   |   Refused
```

Rules the implementation must enforce:

- **Membership.** File requests are only possible on a sync connection that has passed the
  `reader/sync/1` checks: the peer is listed in this library's `devices` and not removed (identity
  authenticated by QUIC/TLS), and its `Hello` named the same library. An unpaired or removed
  device never gets such a connection, so it can't ask at all. (Implemented this way rather than
  as a separate `reader/files/1` ALPN, which would have had to repeat the library check.)
- **Only library files are served.** A request names a 32-byte hash, never a path. It's served
  only if that hash belongs to a non-purged book in the library and this device holds a verified
  copy; otherwise `Missing`.
- **Sizes and offsets are validated.** The book record's `size` is authoritative: `offset` must
  be ≤ `size`; `Have.size` must equal it; the receiver stops reading at `size` and treats extra
  bytes as an error. A `.part` file longer than `size` is discarded.
- **Resume.** An interrupted transfer restarts from the length of the existing `.part` file.
  Whole-file verification at the end catches corruption anywhere, including in resumed data.
- **Cancellation.** A transfer is cancelled (stream closed) when the book is purged, removed from
  this device, the peer is removed or disconnects, or the app shuts down. Whether the `.part` is
  kept depends on whether the book is still wanted (§6).
- **Bounded retries.** Per (book, peer): exponential backoff, capped. `Missing` from a peer
  means its holdings are stale; don't ask it again for that hash until its holdings change.
  After 3 verification failures from one peer, stop fetching that hash from it for this run and
  show the error on the book. No hot loops.
- **Scheduler.** Whenever a peer connects or the document changes, the file manager compares
  the books this device should have (§3) with what it holds, and requests missing ones from
  connected devices whose holdings list that hash. One or two transfers at a time.
- **Through the phone.** The phone fetches from Linux and then holds the file; Windows later
  fetches from the phone. This falls out of holdings; no special forwarding logic.
- **Position sync must stay responsive during transfers.** Files and sync use separate QUIC
  streams, but that alone doesn't guarantee it under bandwidth or disk saturation, so it's
  measured (B2 check, §8). If it fails the target, mitigations in order: QUIC stream priority
  for sync, rate-limiting file streams, smaller fsync batches for `.part` writes.

### Why our own protocol rather than iroh-blobs

The parts that make file sync dependable here are ours whichever library moves the bytes:
library membership, holdings tied to our durable document, cancellation on purge/remove,
out-of-space handling and startup reconciliation (§6). With `iroh-blobs` we'd still write all of
that, plus keep its separate store and its durability rules consistent with ours on Android,
and track an API that has changed between releases. What it would add — verified streaming
per chunk — matters most for large files from untrusted peers; ours are book-sized files from
authenticated paired devices, where whole-file verification costs at worst one re-download.
**Decision: our own protocol.** The files feature ships either way.

## 3. Data model (replicated document)

```
books/<bookId>            mergeable container; bookId = file hash + incarnation (starts at 1,
                          goes up only when a file is imported again after "delete forever",
                          §6), so two devices importing the same file get the same entry
  file      { hash, size, format, originalName }      set once
  title, author, series   editable registers (defaults read from the EPUB's OPF)
  addedAt, addedBy
  trashed, trashedAt      last-writer-wins (recoverable)
  purged                  tombstone after "delete forever"

positions/<bookId>/<deviceId>     each device writes ONLY its own key → never conflicts
  rev        this device's revision number (see §4); never reused
  seen       { deviceId: rev, … }  every position revision this reading has continued from
  locator    EPUB: cfi + section (spine index) · PDF: page + offset within page
  fraction   0..1 through the book (progress display and fallback)
  snippet    ~60 chars of text at the position (shown in prompts, sanity check on restore)
  at         the writing device's clock — for display only ("2 h ago"), never for decisions

holdings/<deviceId>/<hash>        { verifiedAt }  — written only by that device
removed/<deviceId>/<bookId>       "remove download from this device" — written only by that device
settings                          reading settings: font, size, line height, margins, theme, …
                                  one register per setting, last writer wins
devices, addresses                as in Bits
```

**Which books a device downloads:** every book that isn't trashed, purged, or removed from that
device. There's no app-side storage limit; the only limit is the device's free space (out of
space: §6). A library under 1 GB is small for any of these devices.

**Reading settings are synced**: change the font on one device and the others follow. Positions
don't depend on layout, so this never moves your place. If one setting turns out to need a
different value per device (font size on phone vs. monitor is the likely one), it can be made
per device later without changing anything else.

## 4. Positions

Goals: never lose your place, never jump somewhere you didn't want, and decide without trusting
any device's clock.

### Revisions and ancestry

- Each device keeps one **revision counter**, persisted, never reused. Every position it saves
  (local checkpoint or replicated) gets the next number. A revision is identified by
  `(deviceId, rev)`.
- Each position carries `seen`: for every device, the highest revision of that device this
  reading has continued from (a small version vector, one entry per device). Overwriting a
  device's record with a newer position keeps `seen`, so ancestry survives even though
  intermediate positions are gone.
- **A descends from B** iff `A.seen[B.device] ≥ B.rev`. That's the only comparison used.
- The **heads** for a book are the latest positions of all devices that no other position
  descends from.

### Starting a reading session (opening a book)

1. **Recover first.** If this device's local checkpoint is newer than its replicated position
   (e.g. after a crash or power loss), commit it to the replicated document before anything
   else. Selection always sees your real last position.
2. Compute the heads:
   - **One head, and it's mine** → open there.
   - **One head, another device's** (it descends from my last position) → it's a
     **continuation**: open there automatically, with a small "Continued from Phone · ch. 12"
     note and a one-tap "Back to where I was here".
   - **Several heads** → **divergent** (e.g. you reread a chapter on the PC offline while going
     ahead on the phone): ask, showing each head's device, chapter, % and snippet.
3. The new session's `seen` = merge of my previous `seen` and the `seen` of **every head that
   was shown** (and the heads themselves). So choosing in the divergence prompt — either way —
   settles it everywhere, and the prompt doesn't come back. Going "back to where I was here"
   after a continuation also counts as having seen the newer position.
4. **Every decision is saved at once.** An automatic continuation, a choice in the divergence
   prompt, "Back to where I was here", and either button on the mid-session banner (below) each
   immediately write a new revision at the resulting location with the merged `seen`:
   checkpointed to disk and replicated, without waiting for you to scroll. Closing the app or
   crashing right after choosing can't leave the divergence unresolved.

### While reading

- **Live location.** The reader tracks the current location on every move (foliate-js
  `relocate` event), with no delay. The debounce below only limits disk writes; it never
  decides what "current" means.
- **Never jump.** If a new head arrives mid-session, show a banner: "Phone is at ch. 14 —
  Go there / Stay here". Either choice merges it into `seen` and is saved at once (rule 4).
  Ignoring it doesn't, so if both devices keep reading, that's a real divergence and will be
  asked on the next open.
- **Checkpoint locally** on every scroll pause (~2 s debounce): next revision, written to a
  small local file atomically (temp file → fsync → rename). This is what crash recovery uses.
- **Replicate** (commit to the Loro document) when the window/app loses focus or goes to
  background, the book is closed, on section change, at most every ~30 s while reading, and
  whenever a handoff is requested (§5). Not every checkpoint is replicated, because the
  document keeps full history and thousands of commits a day would bloat it.
- **Closing, backgrounding or handing off** always starts from the live location: capture it,
  save it as a new revision (cancelling any pending debounce), then replicate it. A checkpoint
  from before your last scroll is never what gets handed off. On Android this runs on the
  page's `visibilitychange`/pause, before the webview is suspended.
- "Furthest along" never picks a position (you may be rereading or peeking at the ending); it's
  only used for the progress % in the library.

### Tests (automated, B1)

- Linux → phone → Windows → Linux, each opening where the previous one stopped, automatically.
- Two devices reading offline from the same start → prompt on next open; after choosing on one
  device, no prompt anywhere.
- Same scenarios with device clocks hours apart and running backwards: identical decisions.
- Crash with an unreplicated checkpoint ahead of the replicated position → recovered and
  selected on restart, never an older position.
- Scroll, then close immediately (inside the debounce window) → the closing location is the one
  saved and replicated.
- Choose in the divergence prompt (and, separately, "Stay here" on the banner), then close or
  crash without moving → no prompt on any device afterwards.
- Counter survives restart and crash without reusing a revision.

## 5. "Ready to continue on another device"

The failure mode device-to-device sync can't remove: if Linux shuts down before another device
has your latest position (or a newly imported book), Windows can't get it until Linux boots
again. So the status must be exact, and the safe path easy. Statuses name the paired device
("Phone", "Laptop", …); a book shows ✓ if any other device qualifies.

**✓ Phone has your place** for a book means all of:

1. Your live location (§4) is the one last committed to the replicated document. **Moving
   at all turns the ✓ off immediately** — on the location change itself, not when the delayed
   checkpoint is written.
2. The Phone's durable acknowledgement covers that commit: its acked version vector includes the
   commit's operation id. (Not "Phone has everything", which other changes could hold up, and
   not "Phone acked something earlier".)
3. The Phone's holdings include this book's file hash (it has the complete, verified file).

Otherwise the status is **⟳ Sending to Phone…** (with % while a file transfers) when a device
is connected, or **! No other device has your place yet** when none is.

**Handoff** — on closing the app or book, going to background, or the explicit "Hand off" button:
capture the live location, save it as a new revision and commit it to the replicated document
(§4), then wait for the acknowledgement of that specific commit (and, for a book not yet held elsewhere, its file). On close this is a short
"Handing off to Phone…" wait (a few seconds) with "Close anyway".

**Android background.** The phone can only bridge if it's listening when a desktop is about to
shut down. How long it keeps sync running after being backgrounded / screen off has never been
measured (Bits README). It's measured at the start (§8) with the existing Bits APK, which uses
the same core and network stack. **If ordinary use leaves the phone unreachable — e.g. phone in
your pocket with the screen off while you read on the PC — background work moves ahead of the
daily-driver milestone.** The mechanism is chosen from the actual workload (keep a LAN listener
alive and receive small updates plus occasional book files while a desktop is connected),
against Android's documented foreground-service types: `dataSync` is limited to 6 h per 24 h for
apps targeting Android 15+; `specialUse` is meant only for cases the other types don't cover,
so being sideloaded doesn't by itself make it appropriate.

**Desktop background** (added during I1, at the user's request). Closing the window doesn't stop
sync: the app stays in the system tray (status line such as "Sending 2 books to Phone", Open,
Quit), so books just imported and positions just saved still reach the other devices. Quit
first finishes giving the *connected* devices what they lack (`Node::outgoing`), with "Quit now"
to skip that; nothing connected → quits at once. Where there's no tray (Linux without a
StatusNotifierItem host, e.g. GNOME without the AppIndicator extension) closing the window does
that same finish-then-quit, invisibly, rather than leave an app nobody can see. A second launch
shows the running app's window. "Start when you log in" (Devices, per device, off by default)
starts it in the tray. With the app in the tray the reader's close no longer waits for the
handoff; the background does.

## 6. File lifecycle

### Deleting and freeing space

| Action | Effect | Frees space? | Undo |
|---|---|---|---|
| Remove download from this device | Records `removed/<me>/<bookId>`, deletes the local file and this device's holding. Book stays in the library everywhere. | Yes, on this device | Open the book (re-downloads if another device has it) |
| Move to Trash | Replicated; hidden from library; **files kept**; devices that don't have it yet won't download it | No | Restore from Trash |
| Delete forever (from Trash) | Replicated `purged` tombstone: the book, its positions and its files are gone from every device | Yes, everywhere | None |

Trash is for recoverability, not for space: only "remove download" and "delete forever" free
space.

### Removing the only known copy (remove download)

Before "remove download", check holdings of other non-removed devices. If **no other device is
known to have the file**, say so plainly: "No other device has a copy of this book. If you
remove it, it can't be reopened until you import the file again." The book stays in the library
marked "No copy available"; importing the same file again (same hash → same book, which isn't
purged) restores the file, and its positions were never lost. ("Known" because another device's
holdings can be stale; the wording says what we know, nothing more.)

### Deleting forever, and importing again afterwards

"Delete forever" is permanent and its confirmation says so; there's no restore path for that
book entry. `purged` is a one-way flag, separate from `trashed`: once set, nothing a device
does to that entry — restoring from Trash, saving a position, recording a holding, finishing a
download — un-purges it, so a stale device that hasn't seen the purge can't bring the book back
when it reconnects.

Importing the same file **deliberately** after the purge is allowed and creates a **new** book
entry: same hash, next incarnation (§3), fresh metadata, no old positions. Two devices that both
re-import after seeing the purge pick the same incarnation and merge. A device that imports the
file *before* it has seen the purge imports into the old entry; when the purge arrives, that
device shows "*Title* was deleted forever on Phone — import it again?", and only a yes creates
the new incarnation. A file is deleted from disk only when no non-purged entry uses its hash.

### Running out of space

- Before starting a download, check free space against the file's size plus a margin. If it
  doesn't fit, don't start.
- If a write fails with "disk full" mid-transfer, keep the `.part` (it's valid data and resumes
  later) and **pause all downloads on this device**, since the next ones would fail too.
- Show one actionable message: "Not enough space to download *Title* (needs 12 MB, 3 MB free).
  Remove downloads or free up space." with buttons for "Manage downloads" and "Retry".
- Retry only on "Retry", on app start, or when a periodic free-space check (every few minutes
  while paused) shows enough room. No automatic retry loop.
- The replicated document hitting a full disk is already handled by the Bits store: the change
  is rolled back and nothing unsaved is sent.

### Purge or remove while a download is finishing

Purge and remove cancel any in-flight transfer for that book. Since completion runs in the same
file manager task, the last step before rename + holding record re-checks that the book is still
wanted (not purged, not removed from this device). If it isn't, the `.part` is deleted and no
holding is recorded — a finishing download can never bring a book back.

### Crash safety and startup reconciliation

Operations are ordered so a crash at any point leaves something reconciliation can fix:

- Adding: file renamed into place **before** the holding is recorded.
- Removing: holding (and `removed` record) written **before** the file is deleted.

On startup, before the scheduler runs, the file manager compares `books/` with this device's
holdings:

| Found | Action |
|---|---|
| File, no holding, book wanted | Verify hash → record holding (else delete) |
| File, book purged or removed from this device | Delete the file, remove any holding |
| Holding, but file missing or wrong size | Remove holding; re-download if wanted |
| `.part` for a book no longer wanted | Delete it |
| `.part` for a wanted book | Keep; resume when a holder connects |

Files with matching holdings are size-checked on every start; full hash verification only for
mismatches, so startup stays fast with a large library.

### Tests (automated, B2)

Out of space mid-download (pauses, keeps `.part`, no retry loop, resumes after space frees);
purge and remove during the last chunk (no holding, no file); `kill -9` at each step of add and
remove followed by restart (reconciled correctly); only-copy warning shown exactly when no
other device's holdings list the file; a stale device restoring from Trash, saving a position or
finishing a download for a purged book doesn't resurrect it; deliberate re-import after purge
creates a new incarnation, and two devices re-importing converge on the same one; refused requests from an unpaired / removed device;
out-of-range offset and oversize stream rejected.

## 7. App and reader engine

- **Shell: Tauri v2 on all three platforms**, linking the core directly (no helper process). Bits
  uses Electron + a helper on desktop and Tauri on Android; for this app one shell everywhere is
  simpler and the Android side already exists as a template. On Linux the webview is WebKitGTK,
  the same engine Foliate itself uses.
- **UI:** React + Vite, as in Bits. Screens: Library, Reader, Devices (port of Bits'
  `Devices.jsx`), Trash, Settings. The UI talks to the core through one RPC interface, as in
  Bits' `library.mjs`.
- **Desktop layout follows Thorium**: its library view and the reader's toolbar, navigation
  (table of contents, bookmarks, search, go-to position) and settings panel are the model.
  Android follows the same structure adapted to a phone, with the Moon+ conveniences you use
  (scrolled reading first). Thorium has Linux builds, so it can be installed here and compared
  side by side during A1.
- **Reader engine: foliate-js**, in **scrolled mode**. Known limitations to check rather than
  assume: scrolled mode renders one section (chapter) at a time rather than one endless scroll;
  the API is unstable, so we pin a commit and vendor it.
- **Formats:** EPUB first, then PDF (for books with no EPUB). foliate-js's PDF support (via
  pdf.js) is marked experimental, so it's checked in A1. PDF has no CFI; its locator is page +
  offset within the page. Other formats foliate-js handles (MOBI/AZW3, FB2, CBZ) are low priority.
- **Import:** desktop file picker and drag-and-drop; Android "Open with" / share target in D.

## 8. Work plan

All code is the real app — no throwaway prototypes. Two tracks start together, because the sync
core doesn't depend on the reader's look and feel, and the reader doesn't need sync to be judged.
They join in the integration milestone.

### Right away: Android background measurement

Using the existing Bits APK (same core and network stack), measure on the Galaxy S22 how long the
phone keeps syncing after the app is backgrounded and the screen turns off, in ordinary use.
The result decides where Android background work goes (§5).

### Track A — app and reader

**A1 — app shell and reader.** The Tauri project for Linux and Android, Windows via CI; Library
and Reader screens with Thorium-style layout; foliate-js in scrolled mode; reading settings;
local import; local position checkpointing; "copy location" / "go to location" (a real feature,
also used to test positions before sync exists). Links the core as soon as B1's book model is
in place (B1 is mostly a copy, so early).
**Checks, in the real app on all three platforms:**
- You read a chapter of your own EPUBs on each and judge it against Foliate / Thorium / Moon+;
  layout compared side by side with Thorium.
- A location copied on Linux at one font size opens at the same paragraph on the phone at a
  different font size, and on Windows.
- One of your PDFs: is foliate-js's PDF mode usable, or does PDF need its own viewer?
The first install on your phone already uses the final app identifier, `app.readersync` (§9).

### Track B — sync core

**B1 — core fork and model.** Copy the core, rename identifiers (§1), replace the notes model
with books / positions (revisions, `seen`, head selection) / holdings / removed / settings (§3,
§4), new RPC surface. Keep and adapt the existing tests (store, pairing, forwarding, reconnect,
dual-boot handoff). **Check:** `cargo test` green on Linux and Windows CI, including §4's tests
and the dual-boot test rewritten for positions.

**B2 — book file sync.** File store, file manager task, `reader/files/1`, scheduler, lifecycle
(§2, §6). **Check (automated):**
- Linux imports → phone receives and verifies → Linux off → Windows gets the file from the phone.
- Interrupted transfer resumes from the partial file; corrupted bytes rejected and refetched with
  bounded retries.
- All of §6's tests.
- **Measured:** while a large file (≥ 200 MB test fixture) transfers at full speed, and again
  over a throttled link with slow disk writes, a position commit is acknowledged by the other
  device within 2 s. Below target → mitigations in §2, re-measure.

### Integration — I1: the vertical slice

Sync wired into the app: Devices and pairing, handoff status with the exact semantics of §5,
handoff on close/background, crash recovery before selection, continuation and divergence prompt
(§4), file transfers with progress and the out-of-space message. Windows via CI.
If the background measurement showed the phone unreachable in ordinary use, the Android
background work is done here.
**Check on real devices:** import an EPUB on Linux, read in scrolled mode, change font size, see
"✓ Phone has your place", shut Linux down, boot Windows, resume at the same passage. Then:
- Scroll on, and confirm the ✓ turns off at once and comes back only after the phone acks.
- Scroll and close the app immediately → Windows resumes at the closing location, not the one
  before the scroll.
- **Screen-off handoff with the finished reader:** phone locked in your pocket, app in the
  background; on Linux import a new book and read; shut down; boot Windows → book and position
  are there. The early Bits measurement is only preliminary evidence; this is the test that
  counts, because the reader adds file transfers and its own app lifecycle.
- Turn the phone's Wi-Fi off halfway through a book's transfer, back on → transfer resumes.
- Kill the app mid-reading (no clean close) → reopens at the last checkpoint.
- Read offline on two devices → divergence prompt; choose → no prompt elsewhere.

### D — daily driver

Trash / remove download / delete forever with the only-copy warning, metadata editing, covers,
library sorting/search, a "Publisher styles: on/off" reading setting (some books' own CSS breaks at larger font sizes, e.g. fixed-height chapter headings), PDF support (pages
recoloured for sepia/dark — paper → theme background, ink → theme text — with a per-book
"Original colours" switch; fit width / whole page), and on phones the status bar hidden while
a book is open (a reading setting, on by default; a swipe from the top shows it for a moment).
**Check:** you use it instead of the three apps for a week.

**Backlog** (the user's call, 2026-10-29: after the above): import existing books from your
Foliate / Thorium / Moon+ folders (optionally their positions, where readable); Android "Open
with" + share target and desktop "Open with"; renaming devices; the Android app in the "Latest
build" release, for sharing (once the whole system is mostly polished): CI builds the APK and signs
it with a key made for releases, kept in GitHub secrets and backed up outside GitHub — every
release needs the same key so people can update. The user's own phone keeps updating over adb
with the local key; a release APK can't install over that, which is fine.

### Later, as needed

Bookmarks/highlights/notes sync (per-item records, no conflicts by design), rarer formats, Loro
history compaction, signed builds / installer.

## 9. Decisions and what's still open

Decided:

- Sync is device to device, with another device (normally the phone) as the bridge between
  devices that are never on together. No setup-specific shortcuts.
- Every device downloads every non-trashed book; space is freed with "remove download" or
  "delete forever".
- Formats: EPUB, then PDF. Others only if ever needed.
- Desktop layout modelled on Thorium; phone keeps the Moon+ conveniences, scrolled view first.
- Reading settings sync across devices.
- Tauri on all three platforms.
- Our own file transfer protocol (§2).
- App identifier (Android package id and Tauri identifier): **`app.readersync`**. Fixed from the
  first install; changing it later means a fresh install, losing the device's identity and
  local data. The **display name** can change any time; working name `reader-sync`.

Nothing open. Next: implementation, starting tracks A and B together (§8).

## 10. Risks

| Risk | Mitigation |
|---|---|
| foliate-js scrolled mode doesn't feel right on Windows/Android | A1 check in the real app; fallback is custom chapter stitching, only if needed. Track B continues regardless |
| Phone unreachable when a desktop shuts down | Exact status + handoff wait (§5); background measurement first, background work before daily driver if needed |
| Status claims a handoff that didn't happen | §5 checks the specific commit against the peer's durable ack; newer local checkpoints turn ✓ off; tested in I1 |
| Wrong device clock picks the wrong position | Decisions use revisions and `seen`, never timestamps; tested with skewed clocks |
| Replicated document grows with every position save | Local checkpoints, coarse replication (§4); compaction later |
| A finishing download or crash leaves files and holdings out of step | Single file manager task, ordered writes, startup reconciliation (§6) |
| Large transfers starve position sync | Measured in B2 against a 2 s target; stream priority / rate limits if needed |
| Copied core drifts from Bits fixes | Source commit recorded; port fixes by hand when Bits changes `store.rs`/`node.rs` |
| WebKitGTK vs WebView2 vs Android WebView rendering differences | A1 runs on all three; CFI makes positions independent of layout |
