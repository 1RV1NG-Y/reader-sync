// The library: the one interface all UI code uses for books, positions and settings.
//
// Backends
//   local  (this file, milestone A1) — IndexedDB in the webview: book files as Blobs, metadata,
//          positions and settings. Works in the Tauri webviews (WebKitGTK, WebView2, Android
//          WebView) and in a plain browser. One device, no sync.
//   core   (integration, I1) — the Rust sync core behind one Tauri command `rpc(method, params)`,
//          with the SAME method names, parameters and results as documented here. Everything
//          is JSON except book bytes, which cross separately (importFiles in, bookFile out).
//
// Ids are opaque strings. The local backend uses the SHA-256 hex of the file's bytes; the core
// will use BLAKE3 + incarnation (PLAN.md §3). UI code must not parse or derive ids.
//
// ─── Shapes ─────────────────────────────────────────────────────────────────────────────────
//
// BookSummary
//   { id, title, author, format: 'epub' | 'pdf', size /*bytes*/, addedAt /*ISO*/,
//     originalName, progress /*0..1 — furthest along of any device, display only*/,
//     lastReadAt /*ISO | null — this device's last position save*/,
//     cover /*string | null — an image URL for <img src>: data: URL (local) / asset URL (core)*/ }
//
// Locator  — see locator.mjs. EPUB { cfi, section, fraction, snippet, chapter? },
//            PDF { page, offset, fraction, snippet, chapter? }. Treated as opaque JSON by the
//            backend except `fraction` (progress) — store it whole.
//
// Head  — a device's latest position that no other position descends from (PLAN.md §4)
//   { deviceId, deviceName, rev, locator, at /*ISO, writer's clock, display only*/, mine: bool }
//
// Settings — see settings.mjs: { fontFamily, fontSize, lineHeight, margins, maxWidth, theme,
//            justify, hyphenate, flow }. Each key is independent (one register per setting).
//
// ─── Methods (all async unless noted) ───────────────────────────────────────────────────────
//
// info() → { backend: 'local' | 'core', synced: bool, device: { id, name } }
// listBooks() → BookSummary[]                       (most recently read / added first)
// getBook(id) → BookSummary | null
// importFiles(files: File[]) → { added: BookSummary[], skipped: [{ name, reason, id?, message }] }
//      reason: 'duplicate' (same bytes already in the library; `id` names it) |
//              'unsupported' (not EPUB or PDF) | 'error' (`message` says why)
//      Title, author and cover come from the file (EPUB OPF / PDF info). Book bytes are the
//      exception to JSON: the core backend sends each file's bytes as a raw IPC body.
// bookFile(id) → Blob (a File named after the original file)
// openBook(id) → { position: Locator | null, decision: 'mine' | 'continued' | 'divergent',
//                  heads: Head[], here: Locator | null }
//      mine       open at `position` (null: never opened → start of book).
//      continued  another device's position descends from mine: open at `position` (theirs),
//                 show "Continued from <heads[0].deviceName>" with "Back to where I was here"
//                 (`here` = my own last position). The backend has ALREADY saved the
//                 continuation as a new revision (PLAN.md §4 rule 4).
//      divergent  several heads: `position` is null; the UI asks, listing each head's device,
//                 chapter, % and snippet, and reports the choice with resolvePosition.
// resolvePosition(id, choice) → Locator              — saved at once as a new revision
//      choice = { pick: { deviceId, rev }, shown: [{ deviceId, rev }] }  go to that head
//             | { stay: Locator, shown: [{ deviceId, rev }] }           stay at this location
//      `shown` lists every head the user was shown (prompt, note or banner); all of them are
//      merged into the new revision's `seen`, so the question doesn't come back anywhere.
//      Used by: the divergence prompt, "Back to where I was here", and the mid-session banner's
//      "Go there" / "Stay here".
// checkpoint(id, locator) → void (sync)             — call on EVERY relocate with the live
//      location; written after ~2 s without a newer call (scroll pause).
// flushPosition(id, locator) → void                 — cancel the pending checkpoint and save
//      `locator` (the live location) now. On close, back to library, blur, background, hand-off.
// getSettings() → Settings
// setSettings(patch) → Settings                     — validated, merged, saved, broadcast
// subscribe(listener) → unsubscribe (sync)          — listener(event) with event one of
//      { type: 'books' }                            library contents or metadata changed
//      { type: 'settings', settings }               settings changed (here or on another device)
//      { type: 'position', bookId, head }           another device's position for a book arrived
//                                                   → if that book is open, the UI shows a
//                                                   non-jumping banner "X is at ch. N — Go there /
//                                                   Stay here"
//
// local backend only, for checking the UI paths sync will use (dev builds expose it as
// window.readerSyncDebug):
//   debug.addDevicePosition(bookId, { deviceName, locator, continuesFromMine }) → Head
//      Writes a position as if another device had synced it. continuesFromMine=true makes it a
//      continuation of my latest position; false makes it divergent.

import { cleanLocator } from './locator.mjs';
import { createCheckpointer, decide, seenAfter } from './positions.mjs';
import { cleanSettingsPatch, withDefaults } from './settings.mjs';

export const CHECKPOINT_DELAY_MS = 2000;

// ─── Format detection (magic bytes) ─────────────────────────────────────────────────────────

/** 'epub' | 'pdf' | null from the first bytes of a file (at least 58 bytes for EPUB). */
export function detectFormat(head) {
  const b = head instanceof Uint8Array ? head : new Uint8Array(head);
  if (b[0] === 0x25 && b[1] === 0x50 && b[2] === 0x44 && b[3] === 0x46 && b[4] === 0x2d) return 'pdf';
  if (b[0] === 0x50 && b[1] === 0x4b && b[2] === 0x03 && b[3] === 0x04) {
    // OCF: first entry is the stored "mimetype" file containing "application/epub+zip".
    const text = new TextDecoder('latin1').decode(b.subarray(30, 30 + 8 + 20));
    if (text === 'mimetypeapplication/epub+zip') return 'epub';
    // Some tools don't follow the OCF ordering rule; foliate-js still opens these as EPUB.
    return 'zip';
  }
  return null;
}

export async function sha256Hex(bytes) {
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
  return Array.from(digest, b => b.toString(16).padStart(2, '0')).join('');
}

const stripExtension = name => String(name ?? '').replace(/\.(epub|pdf)$/i, '').trim();
const positionKey = (bookId, deviceId) => `${bookId}\u0000${deviceId}`;

// ─── Storage adapters ───────────────────────────────────────────────────────────────────────
// Tables: books (id → record), files (id → Blob), positions (bookId\0deviceId → record),
// meta ('device' | 'rev' | 'settings' | 'devices' → value).
// Adapter: get(table, key), getAll(table) → values, write(ops) atomically where
// ops = [{ table, key, value }] (value undefined = delete).

const TABLES = ['books', 'files', 'positions', 'meta'];

export function memoryStore() {
  const tables = Object.fromEntries(TABLES.map(t => [t, new Map()]));
  const copy = v => (v && typeof v === 'object' && !(v instanceof Blob)) ? structuredClone(v) : v;
  return {
    async get(table, key) { return copy(tables[table].get(key)); },
    async getAll(table) { return [...tables[table].values()].map(copy); },
    async write(ops) {
      for (const { table, key, value } of ops)
        value === undefined ? tables[table].delete(key) : tables[table].set(key, copy(value));
    },
  };
}

export function indexedDbStore(name = 'reader-sync') {
  const opened = new Promise((resolve, reject) => {
    const req = indexedDB.open(name, 1);
    req.onupgradeneeded = () => {
      for (const t of TABLES) if (!req.result.objectStoreNames.contains(t)) req.result.createObjectStore(t);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
    req.onblocked = () => reject(new Error('The library is open in another window with an older version.'));
  });
  const tx = (tables, mode, fn) => opened.then(db => new Promise((resolve, reject) => {
    const t = db.transaction(tables, mode);
    const result = fn(t);
    t.oncomplete = () => resolve(result?.result);
    t.onerror = () => reject(t.error);
    t.onabort = () => reject(t.error ?? new Error('Saving to the library was aborted.'));
  }));
  return {
    get: (table, key) => tx([table], 'readonly', t => t.objectStore(table).get(key)),
    getAll: table => tx([table], 'readonly', t => t.objectStore(table).getAll()),
    write: ops => tx([...new Set(ops.map(o => o.table))], 'readwrite', t => {
      for (const { table, key, value } of ops)
        value === undefined ? t.objectStore(table).delete(key) : t.objectStore(table).put(value, key);
    }),
  };
}

// ─── Local backend ──────────────────────────────────────────────────────────────────────────

/**
 * @param store            storage adapter (indexedDbStore() in the app, memoryStore() in tests)
 * @param extractMetadata  async (file, format) → { title?, author?, cover? } (DOM + foliate-js)
 * @param deviceName       name of this device, shown in prompts on other devices later
 */
export function createLocalLibrary({
  store,
  extractMetadata = async () => ({}),
  deviceName = 'This device',
  now = () => new Date().toISOString(),
  hash = sha256Hex,
  timers = globalThis,
  checkpointDelay = CHECKPOINT_DELAY_MS,
  onError = console.error,
} = {}) {
  const listeners = new Set();
  const emit = event => {
    for (const listener of listeners) {
      try { listener(event); } catch (e) { onError(e); }
    }
  };

  // Device identity and the persisted revision counter. The counter is bumped in memory
  // synchronously and stored in the same transaction as the position it numbers, so a
  // revision is never reused, even across a crash.
  let state;
  const ready = () => state ??= (async () => {
    let device = await store.get('meta', 'device');
    if (!device) {
      device = { id: crypto.randomUUID(), name: deviceName };
      await store.write([{ table: 'meta', key: 'device', value: device }]);
    }
    const rev = (await store.get('meta', 'rev')) ?? 0;
    const devices = (await store.get('meta', 'devices')) ?? {};
    return { device, rev, devices: { ...devices, [device.id]: device.name } };
  })();

  const positionsOf = async bookId => (await store.getAll('positions')).filter(p => p.bookId === bookId);

  const headView = (s, r) => ({
    deviceId: r.deviceId,
    deviceName: s.devices[r.deviceId] ?? 'Another device',
    rev: r.rev,
    locator: r.locator,
    at: r.at,
    mine: r.deviceId === s.device.id,
  });

  // Saves a new revision of this device's position. `shown` = heads (records) the user saw.
  const savePosition = async (bookId, locator, shown = []) => {
    const s = await ready();
    const me = s.device.id;
    const previous = await store.get('positions', positionKey(bookId, me));
    const rev = ++s.rev;
    const record = { bookId, deviceId: me, rev, seen: seenAfter(previous, shown, me, rev), locator: cleanLocator(locator), at: now() };
    await store.write([
      { table: 'positions', key: positionKey(bookId, me), value: record },
      { table: 'meta', key: 'rev', value: rev },
    ]);
    return record;
  };

  const checkpointer = createCheckpointer({
    write: (bookId, locator) => savePosition(bookId, locator),
    delay: checkpointDelay,
    timers,
    onError,
  });

  const summary = (book, positions, me) => {
    const mine = positions.find(p => p.deviceId === me);
    const progress = positions.reduce((max, p) => Math.max(max, p.locator?.fraction ?? 0), 0);
    return {
      id: book.id, title: book.title, author: book.author, format: book.format, size: book.size,
      addedAt: book.addedAt, originalName: book.originalName, cover: book.cover ?? null,
      progress, lastReadAt: mine?.at ?? null,
    };
  };

  const requireBook = async id => {
    const book = await store.get('books', id);
    if (!book) throw new Error('This book is no longer in the library.');
    return book;
  };

  const library = {
    async info() {
      const s = await ready();
      return { backend: 'local', synced: false, device: { ...s.device } };
    },

    async listBooks() {
      const s = await ready();
      const [books, positions] = await Promise.all([store.getAll('books'), store.getAll('positions')]);
      const byBook = Map.groupBy(positions, p => p.bookId);
      return books
        .map(b => summary(b, byBook.get(b.id) ?? [], s.device.id))
        .sort((a, b) => (b.lastReadAt ?? b.addedAt).localeCompare(a.lastReadAt ?? a.addedAt));
    },

    async getBook(id) {
      const s = await ready();
      const book = await store.get('books', id);
      return book ? summary(book, await positionsOf(id), s.device.id) : null;
    },

    async importFiles(files) {
      await ready();
      const added = [];
      const skipped = [];
      for (const file of files) {
        const name = file.name ?? 'Untitled';
        try {
          const format = detectFormat(await file.slice(0, 64).arrayBuffer());
          if (format !== 'epub' && format !== 'pdf' && !(format === 'zip' && /\.epub$/i.test(name))) {
            skipped.push({ name, reason: 'unsupported', message: 'Only EPUB and PDF books can be added.' });
            continue;
          }
          const kind = format === 'pdf' ? 'pdf' : 'epub';
          const bytes = await file.arrayBuffer();
          const id = await hash(bytes);
          if (await store.get('books', id) || added.some(b => b.id === id)) {
            skipped.push({ name, reason: 'duplicate', id, message: 'Already in the library.' });
            continue;
          }
          let meta = {};
          try { meta = (await extractMetadata(file, kind)) ?? {}; }
          catch (e) {
            skipped.push({ name, reason: 'error', message: `Can't open this book: ${e?.message ?? e}` });
            continue;
          }
          const book = {
            id, format: kind, size: bytes.byteLength, originalName: name, addedAt: now(),
            title: String(meta.title || '').trim() || stripExtension(name) || 'Untitled',
            author: String(meta.author || '').trim(),
            cover: typeof meta.cover === 'string' ? meta.cover : null,
          };
          const blob = new Blob([bytes], { type: kind === 'pdf' ? 'application/pdf' : 'application/epub+zip' });
          await store.write([
            { table: 'files', key: id, value: blob },
            { table: 'books', key: id, value: book },
          ]);
          added.push(summary(book, [], null));
        } catch (e) {
          skipped.push({ name, reason: 'error', message: e?.message ?? String(e) });
        }
      }
      if (added.length) emit({ type: 'books' });
      return { added, skipped };
    },

    async bookFile(id) {
      const book = await requireBook(id);
      const blob = await store.get('files', id);
      if (!blob) throw new Error('The file for this book is missing on this device.');
      return new File([blob], book.originalName || `${id}.${book.format}`, { type: blob.type });
    },

    async openBook(id) {
      await requireBook(id);
      await checkpointer.flush(id); // never decide on a stale record
      const s = await ready();
      const me = s.device.id;
      const records = await positionsOf(id);
      const mine = records.find(r => r.deviceId === me) ?? null;
      const { decision, heads } = decide(records, me);
      const here = mine?.locator ?? null;
      let position = null;
      if (decision === 'mine') position = heads[0]?.locator ?? null;
      else if (decision === 'continued') {
        position = heads[0].locator;
        // Rule 4: an automatic continuation is saved at once.
        await checkpointer.exclusive(id, () => savePosition(id, position, heads));
      }
      return { position, decision, heads: heads.map(r => headView(s, r)), here };
    },

    async resolvePosition(id, choice) {
      await requireBook(id);
      const records = await positionsOf(id);
      const find = ({ deviceId, rev }) => records.find(r => r.deviceId === deviceId && r.rev >= rev);
      const shown = (choice?.shown ?? []).map(h => find(h) ?? { deviceId: h.deviceId, rev: h.rev, seen: {} });
      let locator;
      if (choice?.pick) {
        const picked = find(choice.pick);
        if (!picked) throw new Error('That position is no longer available.');
        locator = picked.locator;
        if (!shown.includes(picked)) shown.push(picked);
      } else if (choice?.stay) {
        locator = cleanLocator(choice.stay);
      } else throw new Error('resolvePosition needs { pick } or { stay }.');
      await checkpointer.exclusive(id, () => savePosition(id, locator, shown));
      return locator;
    },

    checkpoint(id, locator) {
      checkpointer.checkpoint(id, cleanLocator(locator));
    },

    async flushPosition(id, locator) {
      await checkpointer.flush(id, locator ? cleanLocator(locator) : undefined);
    },

    async getSettings() {
      return withDefaults(await store.get('meta', 'settings'));
    },

    async setSettings(patch) {
      const clean = cleanSettingsPatch(patch);
      const settings = withDefaults({ ...(await store.get('meta', 'settings')), ...clean });
      await store.write([{ table: 'meta', key: 'settings', value: settings }]);
      emit({ type: 'settings', settings });
      return settings;
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    debug: {
      async addDevicePosition(bookId, { deviceName = 'Phone', locator, continuesFromMine = true } = {}) {
        const s = await ready();
        const me = s.device.id;
        const deviceId = `simulated-${deviceName.toLowerCase().replace(/[^a-z0-9]+/g, '-')}`;
        await checkpointer.flush(bookId);
        const records = await positionsOf(bookId);
        const mine = records.find(r => r.deviceId === me);
        const theirs = records.find(r => r.deviceId === deviceId);
        const rev = (theirs?.rev ?? 0) + 1;
        const seen = { ...(theirs?.seen ?? {}), ...(continuesFromMine && mine ? { ...mine.seen, [me]: mine.rev } : {}), [deviceId]: rev };
        const record = { bookId, deviceId, rev, seen, locator: cleanLocator(locator), at: now() };
        s.devices[deviceId] = deviceName;
        await store.write([
          { table: 'positions', key: positionKey(bookId, deviceId), value: record },
          { table: 'meta', key: 'devices', value: s.devices },
        ]);
        const head = headView(s, record);
        emit({ type: 'position', bookId, head });
        emit({ type: 'books' });
        return head;
      },
    },
  };
  return library;
}

// ─── Entry point ────────────────────────────────────────────────────────────────────────────

const platformName = () => {
  const ua = globalThis.navigator?.userAgent ?? '';
  return /Android/i.test(ua) ? 'Android phone' : /Windows/i.test(ua) ? 'Windows PC' : /Linux/i.test(ua) ? 'Linux PC' : 'This device';
};

let instance;
/** The app's library (one per page): the sync core inside the app, the local backend in a browser. */
export function openLibrary() {
  if (instance) return instance;
  const extractMetadata = (file, format) => import('./reader/metadata.mjs').then(m => m.extractMetadata(file, format));
  if (!globalThis.__TAURI_INTERNALS__) {
    return instance = createLocalLibrary({ store: indexedDbStore(), deviceName: platformName(), extractMetadata });
  }
  const ready = import('./library-core.mjs').then(async ({ createCoreLibrary, coverCache }) => {
    const covers = coverCache();
    await covers.loaded;
    const core = createCoreLibrary({
      invoke: async (...args) => (await import('@tauri-apps/api/core')).invoke(...args),
      listen: async (...args) => (await import('@tauri-apps/api/event')).listen(...args),
      covers,
      extractMetadata,
      rawIpc: !/Android/i.test(navigator.userAgent),
    });
    // Phone back from the background, or a laptop back from sleep: retry peers right away.
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible') core.networkChanged().catch(() => {});
    });
    migrateLocalLibrary(core).catch(console.error);
    return core;
  });
  // The UI gets the library synchronously; every method waits for the core module to load.
  return instance = new Proxy({}, {
    get: (_, name) => name === 'subscribe'
      ? listener => { let off = null, gone = false; ready.then(lib => { if (!gone) off = lib.subscribe(listener); }); return () => { gone = true; off?.(); }; }
      : name === 'checkpoint'
        ? (...args) => { ready.then(lib => lib.checkpoint(...args)); }
        : name === 'debug' ? undefined
          : async (...args) => (await ready)[name](...args),
  });
}

/**
 * Moves what the local backend held before sync existed (milestone A1) into the core once:
 * every book file with its title and cover, this device's last position, and the settings.
 * The old data is left in place as a backup; a flag stops it running again.
 */
export async function migrateLocalLibrary(core, store = indexedDbStore()) {
  if (await store.get('meta', 'migratedToCore')) return;
  const device = await store.get('meta', 'device');
  const books = await store.getAll('books');
  const positions = await store.getAll('positions');
  for (const book of books) {
    const blob = await store.get('files', book.id);
    if (!blob) continue;
    const file = new File([blob], book.originalName || `${book.id}.${book.format}`);
    const { added, skipped } = await core.importFiles([file]);
    const id = added[0]?.id ?? skipped.find(s => s.reason === 'duplicate')?.id;
    if (!id) throw new Error(`Couldn't move “${book.title}” into the synced library: ${skipped[0]?.message}`);
    const mine = positions.find(p => p.bookId === book.id && p.deviceId === device?.id);
    if (mine?.locator) await core.flushPosition(id, mine.locator);
  }
  const settings = await store.get('meta', 'settings');
  if (settings) await core.setSettings(settings);
  await store.write([{ table: 'meta', key: 'migratedToCore', value: new Date().toISOString() }]);
}
