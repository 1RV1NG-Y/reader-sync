// foliate-js glue: opens a book in a <foliate-view>, applies reading settings, and turns
// foliate-js `relocate` events into layout-independent locators (see ../locator.mjs).
//
// foliate-js is vendored unbundled (vendor/foliate-js, served at ./foliate-js/) and loaded at
// run time. Known engine behaviour, checked rather than assumed:
// - Scrolled mode renders ONE section (chapter) at a time, and foliate-js ignores touch in
//   scrolled mode, so a swipe can't leave a chapter (or a cover that doesn't scroll at all).
//   We add that: a swipe that starts at the end of a chapter and keeps going loads the next
//   one, and at the start the previous one. We don't stitch chapters.
// - In scrolled mode foliate-js reports scroll-driven relocations 250 ms after scrolling stops.
//   `liveLocator()` closes that gap: when a scroll happened after the last relocate, it reads
//   the location at the top of the viewport directly, so a close/background right after a
//   scroll saves where the reader actually is.
// - PDF (via PDF.js) uses foliate-js's fixed-layout renderer: one page at a time, no scrolled
//   mode, no reflow settings. The PDF locator is { page, offset: 0 }.
import { makeSnippet } from '../locator.mjs';
import { bookCSS, paginatorAttributes } from '../settings.mjs';

let foliate;
/** Loads foliate-js once; resolves to the module exports of view.js (View, makeBook, …). */
export function loadFoliate() {
  return foliate ??= import(/* @vite-ignore */ new URL('foliate-js/view.js', document.baseURI).href);
}

const labelOf = x => (typeof x === 'string' ? x : x && typeof x === 'object' ? Object.values(x)[0] : '') ?? '';

/** Book title/author from foliate-js metadata (strings, language maps, or contributor objects). */
export function bookTitle(metadata) {
  return String(labelOf(metadata?.title) || '').trim();
}
export function bookAuthor(metadata) {
  const one = c => typeof c === 'string' ? c : labelOf(c?.name);
  const list = Array.isArray(metadata?.author) ? metadata.author : metadata?.author ? [metadata.author] : [];
  return list.map(one).filter(Boolean).join(', ');
}

/**
 * Opens `file` inside `host` and returns a controller.
 *
 * @param host       element the <foliate-view> is appended to
 * @param file       File/Blob of the book
 * @param format     'epub' | 'pdf'
 * @param settings   reading settings (settings.mjs)
 * @param onRelocate (locator, info) on every location change; info = { fraction, chapter,
 *                   tocHref, section, sectionCount, reason }
 * @param onDocument (doc, index) for each loaded section document (key/tap listeners)
 */
export async function openReader({ host, file, format, settings, onRelocate, onDocument, onMovement }) {
  const { makeBook } = await loadFoliate();
  const parsed = await makeBook(file);
  // PDFs: one page at a time, centred (foliate-js would otherwise pair pages into spreads).
  if (parsed.rendition?.layout === 'pre-paginated' && format === 'pdf') parsed.rendition.spread = 'none';
  const view = document.createElement('foliate-view');
  view.className = 'foliate-view';
  host.append(view);
  try {
    await view.open(parsed);
  } catch (e) {
    parsed.destroy?.();
    view.remove();
    throw e;
  }
  const { book } = view;
  const isPDF = format === 'pdf' || view.isFixedLayout;
  let last = null; // locator from the latest relocate
  let lastDetail = null;
  let scrolledSinceRelocate = false;

  // foliate-view's relocate detail: { fraction, section: { current, total }, location, tocItem,
  // pageItem, cfi (of the whole visible range), range }. The section index is section.current.
  const sectionOf = detail => detail.section?.current ?? detail.index ?? 0;
  const locatorFrom = detail => {
    const { fraction = 0, range, tocItem } = detail;
    const index = sectionOf(detail);
    const chapter = labelOf(tocItem?.label).trim() || undefined;
    if (isPDF) return { page: index, offset: 0, fraction: clamp01(fraction), snippet: '', chapter };
    let cfi;
    let snippet = '';
    if (range) {
      const start = range.cloneRange();
      start.collapse(true);
      cfi = view.getCFI(index, start);
      snippet = makeSnippet(range.toString().slice(0, 400));
    } else cfi = view.getCFI(index);
    return { cfi, section: index, fraction: clamp01(fraction), snippet, chapter };
  };

  const report = (locator, detail) => {
    scrolledSinceRelocate = false;
    last = locator;
    onRelocate?.(locator, {
      fraction: locator.fraction,
      chapter: locator.chapter,
      tocHref: detail.tocItem?.href,
      section: locator.section ?? locator.page,
      sectionCount: book.sections.length,
      reason: detail.reason,
    });
  };
  view.addEventListener('relocate', e => {
    lastDetail = e.detail;
    report(locatorFrom(e.detail), e.detail);
  });
  view.addEventListener('load', e => onDocument?.(e.detail.doc, e.detail.index));

  // Scrolled mode: swiping past the end (start) of a chapter goes to the next (previous) one.
  // Only a swipe that *starts* at the edge counts, so the momentum of a scroll that merely
  // reaches the end doesn't skip ahead.
  const SWIPE = 60; // px of finger travel beyond the edge
  let swipe = null;
  const atEdge = () => {
    const r = view.renderer;
    return { start: r.start <= 1, end: r.viewSize - r.end <= 2 };
  };
  const onTouchStart = e => {
    swipe = !isPDF && view.renderer.scrolled && e.touches.length === 1
      ? { y: e.touches[0].screenY, x: e.touches[0].screenX, ...atEdge() } : null;
  };
  const onTouchEnd = e => {
    const t = e.changedTouches[0];
    if (!swipe || !t) return;
    const dy = swipe.y - t.screenY;
    const dx = swipe.x - t.screenX;
    const { start, end } = swipe;
    swipe = null;
    if (Math.abs(dy) < SWIPE || Math.abs(dx) > Math.abs(dy)) return;
    const now = atEdge();
    if (dy > 0 && end && now.end) view.next().catch(console.error);
    else if (dy < 0 && start && now.start) view.prev().catch(console.error);
  };
  const watchTouches = target => {
    target.addEventListener('touchstart', onTouchStart, { passive: true });
    target.addEventListener('touchend', onTouchEnd, { passive: true });
  };
  watchTouches(view);
  view.addEventListener('load', e => watchTouches(e.detail.doc));
  // foliate-js ignores the first scroll after a programmatic jump (open, TOC, search, go-to),
  // even when that jump didn't move the scroll position; such a scroll is never reported, and
  // its internal anchor stays at the jump target, so a later re-layout (font size, window
  // resize) would snap back there. When a scroll settles without a relocate, make foliate-js
  // re-read the visible range with a zero-distance scroll (reports reason null, re-anchors).
  let settleTimer;
  view.renderer.addEventListener('scroll', () => {
    scrolledSinceRelocate = true;
    onMovement?.();
    clearTimeout(settleTimer);
    if (!isPDF) settleTimer = setTimeout(settle, 400);
  });
  const settle = () => {
    if (!scrolledSinceRelocate || !view.renderer.scrolled) return;
    if (view.renderer.start > 0) view.renderer.prev(0);
    else {
      const now = readTopOfViewport();
      if (now) report(now, { reason: 'scroll' });
    }
  };

  // Location at the top of the viewport right now (scrolled mode, after an unreported scroll).
  const readTopOfViewport = () => {
    const content = view.renderer.getContents?.()[0];
    const doc = content?.doc;
    const frame = doc?.defaultView?.frameElement;
    if (!doc || !frame || !doc.caretRangeFromPoint && !doc.caretPositionFromPoint) return null;
    const fr = frame.getBoundingClientRect();
    const vr = view.getBoundingClientRect();
    const x = Math.min(fr.width - 4, Math.max(4, vr.left + vr.width / 2 - fr.left));
    const y = Math.max(0, vr.top + 2 - fr.top); // scrolled mode: margin 0 (settings.mjs)
    let caret = doc.caretRangeFromPoint?.(x, y);
    if (!caret && doc.caretPositionFromPoint) {
      const p = doc.caretPositionFromPoint(x, y);
      if (p) { caret = doc.createRange(); caret.setStart(p.offsetNode, p.offset); }
    }
    if (!caret || !doc.body.contains(caret.startContainer)) return null;
    caret.collapse(true);
    const after = doc.createRange();
    after.setStart(caret.startContainer, caret.startOffset);
    after.setEndAfter(doc.body.lastChild ?? doc.body);
    const index = content.index ?? (lastDetail ? sectionOf(lastDetail) : 0);
    const tocItem = view.getProgressOf?.(index, caret)?.tocItem;
    return {
      cfi: view.getCFI(index, caret),
      section: index,
      fraction: last?.fraction ?? 0,
      snippet: makeSnippet(after.toString().slice(0, 400)),
      chapter: labelOf(tocItem?.label).trim() || last?.chapter,
    };
  };

  let currentSettings = null;
  const controller = {
    view,
    book,
    format: isPDF ? 'pdf' : 'epub',
    toc: book.toc ?? [],
    title: bookTitle(book.metadata),
    sectionFractions: () => view.getSectionFractions?.() ?? [],

    /** Starts at `locator` (or the beginning of the book). */
    async start(locator) {
      if (locator) {
        const ok = await controller.goToLocator(locator);
        if (ok) return;
      }
      await view.init({ showTextStart: false });
    },

    /** The live location: latest relocate, or the top of the viewport after a fresh scroll. */
    liveLocator() {
      if (scrolledSinceRelocate && !isPDF && view.renderer.scrolled) {
        try {
          const now = readTopOfViewport();
          if (now) return now;
        } catch (e) { console.warn('could not read the live location', e); }
      }
      return last;
    },

    async goToLocator(locator) {
      if (!locator) return false;
      if ('page' in locator) return !!(await view.goTo(locator.page));
      if (locator.cfi && await view.goTo(locator.cfi)) return true;
      if (Number.isInteger(locator.section) && await view.goTo(locator.section)) return true;
      await view.goToFraction(locator.fraction ?? 0);
      return true;
    },
    goTo: target => view.goTo(target),
    goToFraction: f => view.goToFraction(clamp01(f)),
    next: distance => view.next(distance),
    prev: distance => view.prev(distance),
    goLeft: () => view.goLeft(),
    goRight: () => view.goRight(),
    nextSection: () => view.renderer.nextSection?.(),
    prevSection: () => view.renderer.prevSection?.(),

    applySettings(s) {
      if (isPDF) {
        if (!view.renderer.hasAttribute('zoom')) view.renderer.setAttribute('zoom', 'fit-width');
        currentSettings = s;
        return;
      }
      const attrs = paginatorAttributes(s);
      for (const [name, value] of Object.entries(attrs))
        if (view.renderer.getAttribute(name) !== value) view.renderer.setAttribute(name, value);
      if (!currentSettings || bookCSS(currentSettings) !== bookCSS(s)) view.renderer.setStyles?.(bookCSS(s));
      currentSettings = s;
    },

    async *search(query) {
      yield* view.search({ query, matchCase: false, matchDiacritics: false, matchWholeWords: false });
    },
    clearSearch: () => view.clearSearch(),

    destroy() {
      clearTimeout(settleTimer);
      try { view.close(); } catch (e) { console.warn(e); }
      try { book.destroy?.(); } catch (e) { console.warn(e); }
      view.remove();
    },
  };
  controller.applySettings(settings);
  return controller;
}

const clamp01 = x => Math.min(1, Math.max(0, Number.isFinite(x) ? x : 0));
