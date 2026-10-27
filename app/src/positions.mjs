// Reading-position logic shared by library backends (PLAN.md §4). Pure: no DOM, no storage.
//
// A position record (one per book per device; each device writes only its own):
//   { bookId, deviceId, rev, seen: { [deviceId]: rev }, locator, at }
// `rev` is the writing device's revision number (never reused); `seen` holds, per device, the
// highest revision this reading has continued from. A descends from B iff
// A.seen[B.deviceId] >= B.rev. Clocks (`at`) are for display only and never decide anything.

/** True when record `a` descends from record `b` (a has continued from b, or from later). */
export const descendsFrom = (a, b) => a !== b && (a.seen?.[b.deviceId] ?? 0) >= b.rev;

/** Latest records that no other record descends from. */
export function computeHeads(records) {
  return records.filter(r => !records.some(other => descendsFrom(other, r)));
}

/** Pointwise maximum of `seen` maps. */
export function mergeSeen(...maps) {
  const out = {};
  for (const map of maps) for (const [device, rev] of Object.entries(map ?? {}))
    if (!(out[device] >= rev)) out[device] = rev;
  return out;
}

/**
 * What to do when `me` opens a book whose position records are `records`:
 *   { decision: 'mine',      heads: [mine] | [] }   — open at my own position (or the start)
 *   { decision: 'continued', heads: [theirs] }      — another device's position descends from mine
 *   { decision: 'divergent', heads: [a, b, …] }     — several heads: ask
 */
export function decide(records, me) {
  const heads = computeHeads(records);
  if (heads.length === 0) return { decision: 'mine', heads: [] };
  if (heads.length === 1) return { decision: heads[0].deviceId === me ? 'mine' : 'continued', heads };
  // Stable order: mine first, then by device id (never by clock).
  heads.sort((a, b) => (b.deviceId === me) - (a.deviceId === me) || a.deviceId.localeCompare(b.deviceId));
  return { decision: 'divergent', heads };
}

/** The `seen` of a new revision that continues from `previous` (mine) and every head shown. */
export function seenAfter(previous, shownHeads, me, rev) {
  return mergeSeen(
    previous?.seen,
    ...shownHeads.map(h => mergeSeen(h.seen, { [h.deviceId]: h.rev })),
    { [me]: rev },
  );
}

/**
 * Debounced position checkpoints with an immediate flush.
 *
 *   checkpoint(bookId, locator)  remember the live location; write it after `delay` ms without a
 *                                newer call for that book (a scroll pause).
 *   flush(bookId, locator?)      cancel the pending write and write `locator` (or the pending
 *                                one) now. Resolves when written. Used on close, back to library,
 *                                blur and backgrounding, always with the live location.
 *   flushAll()                   flush every book with a pending write.
 *   exclusive(bookId, fn)        cancel the pending write and run `fn` (a decision write) in the
 *                                book's write order instead. Resolves with fn's result.
 *   pending(bookId)              the locator waiting to be written, if any.
 *
 * Writes for one book are serialised: a flush issued while a debounced write is in flight lands
 * after it, so the newest location is always the one stored last.
 */
export function createCheckpointer({ write, delay = 2000, timers = globalThis, onError = console.error }) {
  const waiting = new Map(); // bookId → { locator, timer }
  const chains = new Map(); // bookId → promise of the last write

  const enqueue = (bookId, run) => {
    const previous = chains.get(bookId) ?? Promise.resolve();
    const next = previous.catch(() => {}).then(run);
    chains.set(bookId, next);
    next.catch(() => {}).then(() => { if (chains.get(bookId) === next) chains.delete(bookId); });
    return next;
  };

  return {
    checkpoint(bookId, locator) {
      const entry = waiting.get(bookId);
      if (entry) timers.clearTimeout(entry.timer);
      const timer = timers.setTimeout(() => {
        waiting.delete(bookId);
        enqueue(bookId, () => write(bookId, locator)).catch(onError);
      }, delay);
      waiting.set(bookId, { locator, timer });
    },
    flush(bookId, locator) {
      const entry = waiting.get(bookId);
      if (entry) timers.clearTimeout(entry.timer);
      waiting.delete(bookId);
      const target = locator ?? entry?.locator;
      if (!target) return chains.get(bookId)?.catch(() => {}) ?? Promise.resolve();
      return enqueue(bookId, () => write(bookId, target));
    },
    exclusive(bookId, fn) {
      const entry = waiting.get(bookId);
      if (entry) timers.clearTimeout(entry.timer);
      waiting.delete(bookId);
      return enqueue(bookId, fn);
    },
    flushAll() {
      return Promise.all([...waiting.keys()].map(id => this.flush(id)));
    },
    pending: bookId => waiting.get(bookId)?.locator,
    /** Resolves when every write started so far has finished. */
    idle: () => Promise.all([...chains.values()].map(p => p.catch(() => {}))),
  };
}
