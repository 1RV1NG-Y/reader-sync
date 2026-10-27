import React, { useCallback, useEffect, useRef, useState } from 'react';
import { ALargeSmall, ArrowLeft, ChevronLeft, ChevronRight, Maximize, Minimize, Navigation, Search, TableOfContents } from 'lucide-react';
import { openReader } from './reader/engine.mjs';
import SettingsPanel from './reader/SettingsPanel.jsx';
import TocPanel from './reader/TocPanel.jsx';
import SearchPanel from './reader/SearchPanel.jsx';
import { ArrivalBanner, ContinuedNote, DivergentChooser } from './reader/PositionNotices.jsx';
import LocationDialog from './LocationDialog.jsx';
import { canFullscreen, isAndroid, onBackButton, onWindowClose, toggleFullscreen } from './platform.mjs';
import { IconButton, percent } from './ui.jsx';
import WindowControls from './WindowControls.jsx';
import Handoff, { waitForHandoff } from './Handoff.jsx';

const COMPACT_QUERY = '(max-width: 700px), (pointer: coarse) and (max-width: 1000px)';
function useCompact() {
  const [compact, setCompact] = useState(() => isAndroid() || matchMedia(COMPACT_QUERY).matches);
  useEffect(() => {
    const mq = matchMedia(COMPACT_QUERY);
    const on = () => setCompact(isAndroid() || mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return compact;
}

const isTyping = e => e.target?.closest?.('input, textarea, select, [contenteditable="true"]');

export default function Reader({ library, bookId, target, settings, onSettings, onClose, onOpenBook }) {
  const host = useRef(null);
  const reader = useRef(null); // engine controller
  const live = useRef(null); // last relocate locator
  const settingsRef = useRef(settings);
  const [book, setBook] = useState(null);
  const [status, setStatus] = useState('loading'); // 'loading' | 'ready' | { error }
  const [where, setWhere] = useState({ fraction: 0, chapter: '', tocHref: null });
  const [panel, setPanel] = useState(null); // 'toc' | 'search' | 'settings' | 'location'
  const [notice, setNotice] = useState(null); // { kind: 'continued' | 'arrival' | 'divergent', … }
  const [chrome, setChrome] = useState(true);
  // The reader moved since the position was last saved to the library: "has your place" is off.
  const [moved, setMoved] = useState(false);
  const [handingOff, setHandingOff] = useState(null); // device name while the close waits
  const closeAnyway = useRef(null);
  const [fullscreen, setFullscreen] = useState(false);
  const [search, setSearch] = useState({ query: '', groups: [], progress: null });
  const compact = useCompact();
  const compactRef = useRef(compact);
  compactRef.current = compact;
  const panelRef = useRef(panel);
  panelRef.current = panel;
  const readyRef = useRef(false);
  const fullscreenRef = useRef(false);
  fullscreenRef.current = fullscreen;

  // The live location: never older than what's on screen (see engine.liveLocator).
  const liveLocator = useCallback(() => reader.current?.liveLocator() ?? live.current, []);
  const flush = useCallback(() => {
    const locator = liveLocator();
    return locator ? library.flushPosition(bookId, locator) : Promise.resolve();
  }, [library, bookId, liveLocator]);

  const close = useCallback(async () => {
    try { await flush(); } catch (e) { console.error(e); }
    onClose();
  }, [flush, onClose]);

  // ── open ──────────────────────────────────────────────────────────────────────────────────
  useEffect(() => {
    let cancelled = false;
    let controller = null;
    (async () => {
      const summary = await library.getBook(bookId);
      if (!summary) throw new Error('This book is no longer in the library.');
      setBook(summary);
      const opened = await library.openBook(bookId);
      const file = await library.bookFile(bookId);
      if (cancelled) return;
      controller = await openReader({
        host: host.current,
        file,
        format: summary.format,
        settings: settingsRef.current,
        onRelocate: (locator, info) => {
          live.current = locator;
          setWhere(info);
          setMoved(true);
          library.checkpoint(bookId, locator);
        },
        onDocument: doc => {
          doc.addEventListener('keydown', onKey);
          doc.addEventListener('click', e => onTap(e, doc));
          // Phone: the bars get out of the way when the reader scrolls the book with a finger.
          // Not on relocate: those also come from the arrows and the progress bar (the bars
          // would vanish between two taps on an arrow) and arrive up to ~0.5 s after a scroll
          // (they would hide bars the reader had just tapped to show).
          doc.addEventListener('touchstart', noteTouchStart, { passive: true });
          doc.addEventListener('touchmove', hideBarsWhileReading, { passive: true });
        },
      });
      if (cancelled) { controller.destroy(); return; }
      reader.current = controller;
      const explicit = target?.kind === 'token' && target.bookId === bookId ? target.locator : null;
      await controller.start(explicit ?? opened.position ?? (opened.decision === 'divergent' ? opened.here : null));
      if (target && !explicit) await goToParsed(target).catch(e => console.warn(e));
      if (!explicit && !target) {
        if (opened.decision === 'continued') setNotice({ kind: 'continued', head: opened.heads[0], here: opened.here, heads: opened.heads });
        if (opened.decision === 'divergent') setNotice({ kind: 'divergent', heads: opened.heads });
      }
      setStatus('ready');
      setTimeout(() => { readyRef.current = true; }, 500);
      if (compactRef.current) setTimeout(() => { if (!panelRef.current) setChrome(false); }, 1500);
    })().catch(e => { if (!cancelled) { console.error(e); setStatus({ error: e.message ?? String(e) }); } });

    return () => {
      cancelled = true;
      const locator = reader.current?.liveLocator() ?? live.current;
      if (locator) library.flushPosition(bookId, locator).catch(console.error);
      reader.current = null;
      controller?.destroy();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [library, bookId]);

  const touchStartY = useRef(null);
  function noteTouchStart(e) { touchStartY.current = e.touches[0]?.screenY ?? null; }
  function hideBarsWhileReading(e) {
    const y = e.touches[0]?.screenY;
    if (touchStartY.current == null || y == null || Math.abs(y - touchStartY.current) < 12) return; // a tap's jitter
    if (compactRef.current && !panelRef.current && readyRef.current) setChrome(false);
  }

  // ── settings applied live ─────────────────────────────────────────────────────────────────
  useEffect(() => {
    settingsRef.current = settings;
    reader.current?.applySettings(settings);
  }, [settings]);

  // ── save the live location whenever the app may go away ───────────────────────────────────
  useEffect(() => {
    const onVisibility = () => { if (document.visibilityState === 'hidden') flush().catch(console.error); };
    // Focus moving into the book's own iframe also blurs this window; only flush on a real blur.
    const onBlur = () => setTimeout(() => { if (!document.hasFocus()) flush().catch(console.error); }, 0);
    const onPageHide = () => { flush().catch(console.error); };
    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('blur', onBlur);
    window.addEventListener('pagehide', onPageHide);
    // Closing the window: save the live location, then give a connected device a few seconds
    // to confirm it has it (PLAN.md §5), unless the reader chooses "Close anyway".
    const offClose = onWindowClose(async () => {
      await flush();
      const devices = await library.handoff?.(bookId).catch(() => []) ?? [];
      const target = devices.find(d => d.connected);
      if (!target || devices.some(d => d.hasPlace && d.hasFile)) return;
      setHandingOff(target.name);
      await Promise.race([waitForHandoff(library, bookId), new Promise(r => { closeAnyway.current = r; })]);
    });
    const offBack = onBackButton(async () => {
      if (panelRef.current) { setPanel(null); return true; }
      await close();
      return true;
    });
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('blur', onBlur);
      window.removeEventListener('pagehide', onPageHide);
      offClose();
      offBack();
    };
  }, [flush, close, library, bookId]);

  useEffect(() => library.subscribe(e => { if (e.type === 'saved' && e.bookId === bookId) setMoved(false); }), [library, bookId]);

  // ── another device's position arrives while reading → banner, never a jump ────────────────
  useEffect(() => library.subscribe(e => {
    if (e.type === 'position' && e.bookId === bookId && !e.head.mine) setNotice({ kind: 'arrival', head: e.head });
  }), [library, bookId]);

  // ── navigation ────────────────────────────────────────────────────────────────────────────
  const goToParsed = async parsed => {
    const r = reader.current;
    if (!r) throw new Error('The book is still opening.');
    switch (parsed.kind) {
      case 'token':
        if (parsed.bookId !== bookId) {
          const other = await library.getBook(parsed.bookId);
          if (!other) throw new Error('That location is for a book that isn’t in this library.');
          await flush();
          onOpenBook(other.id, parsed);
          return;
        }
        await r.goToLocator(parsed.locator);
        return;
      case 'cfi':
        if (r.format === 'pdf') throw new Error('PDFs have no CFI locations. Type a page, such as page 12.');
        if (!(await r.goTo(parsed.cfi))) throw new Error('That CFI doesn’t point into this book.');
        return;
      case 'fraction':
        await r.goToFraction(parsed.fraction);
        return;
      case 'page':
        if (r.format !== 'pdf') throw new Error('Pages are for PDFs. For this book, type a percentage such as 42%.');
        if (!(await r.goTo(parsed.page))) throw new Error('This PDF doesn’t have that page.');
        return;
    }
  };

  const onKey = e => {
    if (e.defaultPrevented || isTyping(e)) return;
    if (panelRef.current) {
      if (e.key === 'Escape') setPanel(null);
      return;
    }
    const r = reader.current;
    if (!r) return;
    const scrolled = r.format !== 'pdf' && settingsRef.current.flow === 'scrolled';
    const mod = e.ctrlKey || e.metaKey;
    const handled = () => e.preventDefault();
    if (mod && e.key.toLowerCase() === 'f') { handled(); setPanel('search'); return; }
    if (mod && e.key.toLowerCase() === 'g') { handled(); setPanel('location'); return; }
    if (mod || e.altKey) return;
    switch (e.key) {
      case 'ArrowRight': handled(); scrolled ? r.nextSection() : r.goRight(); break;
      case 'ArrowLeft': handled(); scrolled ? r.prevSection() : r.goLeft(); break;
      case 'ArrowDown': handled(); scrolled ? r.next(80) : r.next(); break;
      case 'ArrowUp': handled(); scrolled ? r.prev(80) : r.prev(); break;
      case 'PageDown': case ' ': handled(); e.shiftKey ? r.prev() : r.next(); break;
      case 'PageUp': handled(); r.prev(); break;
      case 'F11': handled(); fullscreenToggle(); break;
      case 'Escape': if (fullscreenRef.current) { handled(); fullscreenToggle(); } break;
      default:
    }
  };

  // Phone: tap the middle to show/hide the bars; tap the top/bottom quarter to page back/forward.
  const onTap = (e, doc) => {
    if (e.defaultPrevented || e.target.closest?.('a[href]')) return;
    if (!doc.getSelection()?.isCollapsed) return;
    if (!compactRef.current) return;
    const frame = doc.defaultView.frameElement;
    const y = (frame?.getBoundingClientRect().top ?? 0) + e.clientY;
    const h = window.innerHeight;
    if (y < h * 0.25) reader.current?.prev();
    else if (y > h * 0.75) reader.current?.next();
    else setChrome(c => !c);
  };

  useEffect(() => {
    const keydown = e => onKey(e);
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  });

  const fullscreenToggle = () => toggleFullscreen().then(setFullscreen, console.error);
  useEffect(() => {
    const on = () => setFullscreen(!!document.fullscreenElement);
    document.addEventListener('fullscreenchange', on);
    return () => document.removeEventListener('fullscreenchange', on);
  }, []);

  // ── position decisions (PLAN.md §4) ───────────────────────────────────────────────────────
  const resolve = async choice => {
    const locator = await library.resolvePosition(bookId, choice);
    await reader.current?.goToLocator(locator);
  };
  const shownOf = heads => heads.map(h => ({ deviceId: h.deviceId, rev: h.rev }));

  // The progress bar jumps when it's let go: the native `change` event, which fires for mouse,
  // keyboard and touch alike. (Android often ends a touch drag with pointercancel, not
  // pointerup, so a pointerup handler left the bar moved but the book where it was.)
  const slider = useRef(null);
  useEffect(() => {
    const el = slider.current;
    if (!el) return;
    const go = () => reader.current?.goToFraction(Number(el.value) / 1000).catch(console.error);
    el.addEventListener('change', go);
    return () => el.removeEventListener('change', go);
  }, [status]);

  const openPanel = name => { setPanel(p => p === name ? null : name); setChrome(true); };
  const closePanel = () => {
    if (panel === 'search') { reader.current?.clearSearch(); setSearch({ query: '', groups: [], progress: null }); }
    setPanel(null);
  };
  const goAndClose = target => {
    reader.current?.goTo(target);
    if (compact || panel === 'toc') setPanel(null);
  };

  const r = reader.current;
  const pdf = (book?.format ?? r?.format) === 'pdf';
  const scrolled = !pdf && settings.flow === 'scrolled';
  const title = book?.title ?? '';

  return (
    <div className={`reader ${compact ? 'compact' : ''} ${chrome ? 'chrome-shown' : 'chrome-hidden'} ${fullscreen ? 'fullscreen' : ''}`}>
      <header className="toolbar reader-toolbar" data-tauri-drag-region>
        <div className="toolbar-group" data-tauri-drag-region>
          <IconButton icon={ArrowLeft} label="Back to library" onClick={close} />
          <IconButton icon={TableOfContents} label="Contents" active={panel === 'toc'} onClick={() => openPanel('toc')} disabled={status !== 'ready'} />
          <IconButton icon={Search} label="Search in book" active={panel === 'search'} onClick={() => openPanel('search')} disabled={status !== 'ready'} />
          <IconButton icon={Navigation} label="Location: copy or go to" active={panel === 'location'} onClick={() => openPanel('location')} disabled={status !== 'ready'} />
        </div>
        <div className="toolbar-title" title={title} data-tauri-drag-region>
          <span className="title" data-tauri-drag-region>{title}</span>
          {!compact && where.chapter && <span className="chapter" data-tauri-drag-region>{where.chapter}</span>}
        </div>
        <div className="toolbar-group" data-tauri-drag-region>
          <Handoff library={library} bookId={bookId} moved={moved} compact={compact} onHandOff={() => flush().catch(console.error)} />
          <IconButton icon={ALargeSmall} label="Reading settings" active={panel === 'settings'} onClick={() => openPanel('settings')} />
          {canFullscreen() && <IconButton icon={fullscreen ? Minimize : Maximize} label={fullscreen ? 'Leave full screen' : 'Full screen'} onClick={fullscreenToggle} />}
          {!fullscreen && <WindowControls />}
        </div>
      </header>

      <main className="reader-main">
        <div ref={host} className="reader-host" data-theme={settings.theme}
          onTouchStart={e => noteTouchStart(e.nativeEvent)} onTouchMove={e => hideBarsWhileReading(e.nativeEvent)} />
        {status === 'loading' && <div className="reader-status">Opening…</div>}
        {status?.error && (
          <div className="reader-status error" role="alert">
            <p>Can't open this book.</p><p className="hint">{status.error}</p>
            <button type="button" onClick={onClose}>Back to library</button>
          </div>
        )}
        {notice?.kind === 'continued' && (
          <ContinuedNote head={notice.head}
            onBack={notice.here ? () => { setNotice(null); resolve({ stay: notice.here, shown: shownOf(notice.heads) }).catch(console.error); } : null}
            onDismiss={() => setNotice(null)} />
        )}
        {notice?.kind === 'arrival' && (
          <ArrivalBanner head={notice.head}
            onGo={() => { setNotice(null); resolve({ pick: { deviceId: notice.head.deviceId, rev: notice.head.rev }, shown: shownOf([notice.head]) }).catch(console.error); }}
            onStay={() => {
              setNotice(null);
              const here = liveLocator();
              if (here) library.resolvePosition(bookId, { stay: here, shown: shownOf([notice.head]) }).catch(console.error);
            }} />
        )}
      </main>

      {handingOff && (
        <div className="handing-off" role="alertdialog" aria-label="Handing off">
          <div>
            <span>Handing off your place to {handingOff}…</span>
            <button type="button" onClick={() => closeAnyway.current?.()}>Close anyway</button>
          </div>
        </div>
      )}

      <footer className="reader-footer">
        <IconButton icon={ChevronLeft} label={scrolled ? 'Previous chapter' : 'Previous page'} disabled={status !== 'ready'}
          onClick={() => scrolled ? r?.prevSection() : r?.goLeft()} />
        <div className="progress">
          <input ref={slider} type="range" min="0" max="1000" step="1" aria-label="Position in book" disabled={status !== 'ready'}
            value={Math.round((where.fraction ?? 0) * 1000)}
            onChange={e => setWhere(w => ({ ...w, fraction: Number(e.target.value) / 1000 }))} />
          <span className="progress-label">{[
            compact ? where.chapter : null,
            pdf && where.sectionCount ? `p. ${(where.section ?? 0) + 1} / ${where.sectionCount}` : null,
            percent(where.fraction),
          ].filter(Boolean).join(' · ')}</span>
        </div>
        <IconButton icon={ChevronRight} label={scrolled ? 'Next chapter' : 'Next page'} disabled={status !== 'ready'}
          onClick={() => scrolled ? r?.nextSection() : r?.goRight()} />
      </footer>

      {panel === 'toc' && r && <TocPanel toc={r.toc} current={where.tocHref} onGo={goAndClose} onClose={closePanel} />}
      {panel === 'search' && r && <SearchPanel reader={r} state={search} setState={setSearch} onGo={goAndClose} onClose={closePanel} />}
      {panel === 'settings' && <SettingsPanel settings={settings} onChange={onSettings} onClose={closePanel} pdf={pdf} />}
      {panel === 'location' && (
        <LocationDialog pdf={pdf} current={{ bookId, locator: liveLocator() }} onGo={goToParsed} onClose={closePanel} />
      )}
      {notice?.kind === 'divergent' && (
        <DivergentChooser heads={notice.heads} onChoose={h => {
          setNotice(null);
          resolve({ pick: { deviceId: h.deviceId, rev: h.rev }, shown: shownOf(notice.heads) }).catch(console.error);
        }} />
      )}
    </div>
  );
}
