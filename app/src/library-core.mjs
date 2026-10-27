// The "core" library backend: the same interface as library.mjs's local backend (see the
// header there), served by the Rust sync core linked into the app (app/src-tauri/src/lib.rs,
// core/src/rpc.rs). JSON goes through the `rpc` command; book bytes through `import_book` (raw
// request body) and `book_file`; changes arrive as `core-event` events.
//
// Beyond the shared interface, this backend adds what sync needs in the UI:
//   status() → { deviceId, deviceName, devices: [{ id, name, platform, isSelf, removed,
//                connected, hasAllMyChanges, lastSeen, lastError }], ... }
//   handoff(bookId) → [{ device, name, connected, hasPlace, hasFile }]   (PLAN.md §5)
//   downloads() → { transfers: [{ book, from, received, size }], pausedForSpace, errors }
//   retryDownloads(), createInvite() → { ticket, qrSvg, expiresAt, kind },
//   join(ticket) → { device, joined }, removeDevice(id), networkChanged()
//   and the event { type: 'status' } (devices, acknowledgements, transfers changed).
//
// Covers are a local cache (IndexedDB), made on each device from its own copy of the file;
// they are never synced. Book summaries also carry `available` (the file is on this device)
// and `heldBy` (names of devices that hold it).

import { detectFormat } from './library.mjs';
import { cleanLocator } from './locator.mjs';
import { createCheckpointer } from './positions.mjs';
import { cleanSettingsPatch, withDefaults } from './settings.mjs';

/** Replicate a checkpoint at most this often while reading (PLAN.md §4). */
const COMMIT_EVERY_MS = 30_000;
/** …and once reading pauses this long, so "✓ <device> has your place" can appear. */
const COMMIT_WHEN_IDLE_MS = 8_000;

const stripExtension = name => String(name ?? '').replace(/\.(epub|pdf)$/i, '').trim();

/** A Locator (locator.mjs) as the core's Location: the locator is stored whole. */
const toLocation = locator => {
  const clean = cleanLocator(locator);
  return { locator: clean, fraction: Math.min(1, Math.max(0, Number(clean.fraction) || 0)), snippet: String(clean.snippet ?? '').slice(0, 200) };
};

export function createCoreLibrary({ invoke, listen, covers, extractMetadata, checkpointDelay = 2000, onError = console.error }) {
  const call = (method, params) => invoke('rpc', { method, params: params ?? null });
  const listeners = new Set();
  const emit = event => { for (const l of listeners) try { l(event); } catch (e) { onError(e); } };

  let status = null;
  const refreshStatus = async () => (status = await call('status'));
  const me = () => status?.deviceId;
  const nameOf = id => status?.devices.find(d => d.id === id)?.name ?? 'Another device';

  const headView = placed => ({
    deviceId: placed.device,
    deviceName: nameOf(placed.device),
    rev: placed.rev,
    locator: placed.locator,
    at: placed.at,
    mine: placed.device === me(),
  });

  let books = new Map(); // id → BookView from the core
  const summary = view => ({
    id: view.id,
    title: view.title || stripExtension(view.originalName) || 'Untitled',
    author: view.author,
    format: view.format,
    size: view.size,
    addedAt: view.addedAt,
    originalName: view.originalName,
    progress: view.progress ?? 0,
    lastReadAt: view.lastReadAt ?? null,
    cover: covers.peek(view.id),
    available: view.heldBy.includes(me()),
    heldBy: view.heldBy.filter(d => d !== me()).map(nameOf),
    trashed: view.trashed,
  });
  const refreshBooks = async () => {
    books = new Map((await call('books')).map(b => [b.id, b]));
    makeMissingCovers();
    return books;
  };

  // Covers for books whose file is here but whose cover isn't cached yet (e.g. arrived from
  // another device). One at a time; each book is tried once per session.
  const coverTried = new Set();
  let makingCovers = false;
  const makeMissingCovers = async () => {
    if (makingCovers) return;
    makingCovers = true;
    try {
      for (const view of books.values()) {
        if (coverTried.has(view.id) || covers.peek(view.id) || !view.heldBy.includes(me())) continue;
        coverTried.add(view.id);
        try {
          const meta = await extractMetadata(await library.bookFile(view.id), view.format);
          if (typeof meta?.cover === 'string') {
            await covers.set(view.id, meta.cover);
            emit({ type: 'books' });
          }
        } catch (e) { onError(e); }
      }
    } finally {
      makingCovers = false;
    }
  };

  // Positions: debounced local checkpoints in the core; replicated at least every 30 s and on
  // section change. Flushes and decisions go through `exclusive`, after any pending write.
  const lastCommit = new Map(); // bookId → { at, section }
  const idleCommits = new Map(); // bookId → timer
  const commit = async (bookId, section) => {
    clearTimeout(idleCommits.get(bookId));
    await call('commitPosition', { id: bookId });
    lastCommit.set(bookId, { at: Date.now(), section });
  };
  const checkpointer = createCheckpointer({
    delay: checkpointDelay,
    onError,
    write: async (bookId, locator) => {
      await call('checkpoint', { id: bookId, location: toLocation(locator) });
      const last = lastCommit.get(bookId);
      if (!last || Date.now() - last.at > COMMIT_EVERY_MS || last.section !== locator.section) await commit(bookId, locator.section);
      else {
        clearTimeout(idleCommits.get(bookId));
        idleCommits.set(bookId, setTimeout(() => checkpointer.exclusive(bookId, () => commit(bookId, locator.section)).catch(onError), COMMIT_WHEN_IDLE_MS));
      }
      emit({ type: 'saved', bookId });
    },
  });

  // Mid-session banner: heads from other devices that arrive while a book is open.
  const watched = new Map(); // bookId → Set of "device:rev" already reported
  const watch = (bookId, heads) => watched.set(bookId, new Set(heads.map(h => `${h.device}:${h.rev}`)));
  const checkArrivals = async () => {
    for (const [bookId, known] of watched) {
      try {
        const sel = await call('peekPosition', { id: bookId });
        if (sel.decision === 'mine') continue;
        for (const head of sel.heads) {
          const key = `${head.device}:${head.rev}`;
          if (head.device === me() || known.has(key)) continue;
          known.add(key);
          emit({ type: 'position', bookId, head: headView(head) });
        }
      } catch (e) { onError(e); }
    }
  };

  let settingsJson = null;
  const checkSettings = async () => {
    const settings = withDefaults(await call('settings'));
    const json = JSON.stringify(settings);
    if (settingsJson !== null && json !== settingsJson) emit({ type: 'settings', settings });
    settingsJson = json;
    return settings;
  };

  const started = (async () => {
    await refreshStatus();
    await refreshBooks();
    await listen('core-event', async ({ payload }) => {
      try {
        if (payload === 'libraryChanged') {
          await refreshBooks();
          emit({ type: 'books' });
          await checkSettings();
          await checkArrivals();
        } else if (payload === 'statusChanged') {
          await refreshStatus();
          emit({ type: 'status' });
        }
      } catch (e) { onError(e); }
    });
  })();
  const ready = () => started;

  const library = {
    async info() {
      await ready();
      return { backend: 'core', synced: true, device: { id: status.deviceId, name: status.deviceName } };
    },

    async listBooks() {
      await ready();
      return [...books.values()]
        .filter(b => !b.trashed)
        .map(summary)
        .sort((a, b) => (b.lastReadAt ?? b.addedAt).localeCompare(a.lastReadAt ?? a.addedAt));
    },

    async getBook(id) {
      await ready();
      const view = books.get(id) ?? (await refreshBooks()).get(id);
      return view ? summary(view) : null;
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
          let meta = {};
          try { meta = (await extractMetadata(file, kind)) ?? {}; }
          catch (e) {
            skipped.push({ name, reason: 'error', message: `Can't open this book: ${e?.message ?? e}` });
            continue;
          }
          const details = { format: kind, originalName: name, title: String(meta.title ?? '').trim(), author: String(meta.author ?? '').trim() };
          const bytes = new Uint8Array(await file.arrayBuffer());
          const result = await invoke('import_book', bytes, { headers: { 'x-import-meta': encodeURIComponent(JSON.stringify(details)) } });
          if (typeof meta.cover === 'string') await covers.set(result.book.id, meta.cover);
          await refreshBooks();
          const view = books.get(result.book.id);
          if (result.added) added.push(summary(view));
          else skipped.push({ name, reason: 'duplicate', id: result.book.id, message: 'Already in the library.' });
        } catch (e) {
          skipped.push({ name, reason: 'error', message: String(e?.message ?? e) });
        }
      }
      if (added.length) emit({ type: 'books' });
      return { added, skipped };
    },

    async bookFile(id) {
      await ready();
      const view = books.get(id);
      const bytes = await invoke('book_file', { id });
      const type = view?.format === 'pdf' ? 'application/pdf' : 'application/epub+zip';
      return new File([bytes], view?.originalName || `${id}.${view?.format ?? 'epub'}`, { type });
    },

    async openBook(id) {
      await ready();
      await checkpointer.flush(id);
      const sel = await checkpointer.exclusive(id, () => call('openBook', { id }));
      watch(id, sel.heads);
      lastCommit.set(id, { at: Date.now(), section: sel.position?.locator?.section });
      return {
        position: sel.position?.locator ?? null,
        decision: sel.decision,
        heads: sel.heads.map(headView),
        here: sel.mine?.locator ?? null,
      };
    },

    async resolvePosition(id, choice) {
      await ready();
      const shown = (choice?.shown ?? []).map(h => ({ device: h.deviceId, rev: h.rev }));
      let core;
      if (choice?.pick) core = { head: { device: choice.pick.deviceId, rev: choice.pick.rev } };
      else if (choice?.stay) core = { here: toLocation(choice.stay) };
      else throw new Error('resolvePosition needs { pick } or { stay }.');
      const placed = await checkpointer.exclusive(id, () => call('resolvePosition', { id, shown, choice: core }));
      for (const h of shown) watched.get(id)?.add(`${h.device}:${h.rev}`);
      return placed.locator;
    },

    checkpoint(id, locator) {
      checkpointer.checkpoint(id, cleanLocator(locator));
    },

    async flushPosition(id, locator) {
      if (!locator) {
        await checkpointer.flush(id);
        return checkpointer.exclusive(id, () => call('commitPosition', { id }));
      }
      clearTimeout(idleCommits.get(id));
      await checkpointer.exclusive(id, () => call('flushPosition', { id, location: toLocation(locator) }));
      lastCommit.set(id, { at: Date.now(), section: locator.section });
      emit({ type: 'saved', bookId: id });
    },

    async getSettings() {
      await ready();
      return checkSettings();
    },

    async setSettings(patch) {
      const settings = withDefaults(await call('setSettings', { patch: cleanSettingsPatch(patch) }));
      settingsJson = JSON.stringify(settings);
      emit({ type: 'settings', settings });
      return settings;
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    // ── sync ──
    async status() { await ready(); return refreshStatus(); },
    handoff: id => call('handoff', { id }),
    downloads: () => call('downloads'),
    retryDownloads: () => call('retryDownloads'),
    createInvite: () => call('createInvite'),
    join: ticket => call('join', { ticket }),
    removeDevice: id => call('removeDevice', { id }),
    networkChanged: () => call('networkChanged'),
  };
  return library;
}

/** Cover cache: book id → image URL string, in IndexedDB, with a synchronous in-memory view. */
export function coverCache(name = 'reader-sync-covers') {
  const memory = new Map();
  const db = new Promise((resolve, reject) => {
    const req = indexedDB.open(name, 1);
    req.onupgradeneeded = () => req.result.createObjectStore('covers');
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  const loaded = db.then(d => new Promise((resolve, reject) => {
    const req = d.transaction('covers').objectStore('covers').openCursor();
    req.onsuccess = () => {
      const cursor = req.result;
      if (!cursor) return resolve();
      memory.set(cursor.key, cursor.value);
      cursor.continue();
    };
    req.onerror = () => reject(req.error);
  })).catch(() => {});
  return {
    loaded,
    peek: id => memory.get(id) ?? null,
    async set(id, url) {
      memory.set(id, url);
      const d = await db;
      await new Promise((resolve, reject) => {
        const t = d.transaction('covers', 'readwrite');
        t.objectStore('covers').put(url, id);
        t.oncomplete = resolve;
        t.onerror = () => reject(t.error);
      });
    },
  };
}
