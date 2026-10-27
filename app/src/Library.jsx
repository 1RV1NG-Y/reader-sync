import React, { useCallback, useEffect, useRef, useState } from 'react';
import { BookOpen, LayoutGrid, List, MonitorSmartphone, Navigation, Plus } from 'lucide-react';
import Devices from './Devices.jsx';
import LocationDialog from './LocationDialog.jsx';
import { IconButton, ago, formatSize, percent } from './ui.jsx';
import WindowControls from './WindowControls.jsx';
import { onNativeFileDrop } from './platform.mjs';

const ACCEPT = '.epub,.pdf,application/epub+zip,application/pdf';
const loadMode = () => { try { return localStorage.getItem('reader-sync.libraryView') || 'grid'; } catch { return 'grid'; } };

function Cover({ book }) {
  if (book.cover) return <img className="cover" src={book.cover} alt="" draggable="false" />;
  return (
    <div className="cover cover-placeholder" aria-hidden="true">
      <span>{book.title}</span>
      {book.author && <small>{book.author}</small>}
    </div>
  );
}

const progressLabel = b => b.progress > 0 || b.lastReadAt ? percent(b.progress) : 'New';

/** For a book whose file isn't on this device yet: what's happening to it (null when it's here). */
function arrival(book, downloads) {
  if (book.available !== false) return null;
  const t = downloads?.transfers.find(x => x.book === book.id);
  if (t) return `Downloading ${percent(t.received / t.size)}`;
  if (downloads?.errors[book.id]) return downloads.errors[book.id];
  if (downloads?.pausedForSpace) return 'Waiting for free space';
  return book.heldBy?.length ? `Waiting for ${book.heldBy[0]}` : 'Not on any connected device';
}

export default function Library({ library, onOpenBook }) {
  const [books, setBooks] = useState(null);
  const [mode, setMode] = useState(loadMode);
  const [dragging, setDragging] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState(null);
  const [goTo, setGoTo] = useState(false);
  const [synced, setSynced] = useState(false);
  const [devices, setDevices] = useState(false);
  const [downloads, setDownloads] = useState(null);
  const input = useRef(null);
  const dragDepth = useRef(0);

  const refresh = useCallback(() => library.listBooks().then(setBooks, e => setMessage({ error: true, text: e.message })), [library]);
  useEffect(() => {
    refresh();
    return library.subscribe(e => { if (e.type === 'books') refresh(); });
  }, [library, refresh]);
  useEffect(() => { try { localStorage.setItem('reader-sync.libraryView', mode); } catch { /* preview */ } }, [mode]);
  // Synced library: download progress, refreshed (at most every half second) as transfers move.
  useEffect(() => {
    let alive = true;
    let timer = null;
    library.info().then(info => {
      if (!alive || !info.synced) return;
      setSynced(true);
      const load = () => library.downloads().then(d => { if (alive) setDownloads(d); }, () => {});
      load();
      const off = library.subscribe(e => {
        if ((e.type === 'status' || e.type === 'books') && !timer) timer = setTimeout(() => { timer = null; load(); }, 500);
      });
      cleanup = off;
    });
    let cleanup = null;
    return () => { alive = false; clearTimeout(timer); cleanup?.(); };
  }, [library]);
  const open = b => {
    const why = arrival(b, downloads);
    if (why) setMessage({ text: `“${b.title}” isn’t on this device yet: ${why.charAt(0).toLowerCase()}${why.slice(1)}.` });
    else onOpenBook(b.id);
  };
  useEffect(() => {
    if (!message || message.error) return;
    const t = setTimeout(() => setMessage(null), 6000);
    return () => clearTimeout(t);
  }, [message]);

  const importFiles = async (files, rejected = []) => {
    if (!files.length && !rejected.length) return;
    if (!files.length) {
      setMessage({ error: true, text: rejected.map(r => `${r.name}: ${r.message}`).join(' ') });
      return;
    }
    setBusy(true);
    setMessage({ text: files.length === 1 ? `Adding ${files[0].name}…` : `Adding ${files.length} files…` });
    try {
      const { added, skipped } = await library.importFiles(files);
      const parts = [];
      if (added.length) parts.push(added.length === 1 ? `Added “${added[0].title}”.` : `Added ${added.length} books.`);
      const dup = skipped.filter(s => s.reason === 'duplicate');
      if (dup.length) parts.push(dup.length === 1 ? `“${dup[0].name}” is already in the library.` : `${dup.length} were already in the library.`);
      for (const s of [...skipped.filter(s => s.reason !== 'duplicate'), ...rejected]) parts.push(`${s.name}: ${s.message}`);
      setMessage({ text: parts.join(' ') || 'Nothing added.', error: added.length === 0 && skipped.some(s => s.reason !== 'duplicate') });
    } catch (e) {
      setMessage({ error: true, text: e.message });
    } finally {
      setBusy(false);
    }
  };

  const goToLocation = async parsed => {
    if (parsed.kind !== 'token') throw new Error('Open a book first to go to a percentage or CFI. Here, paste a copied location.');
    const book = await library.getBook(parsed.bookId);
    if (!book) throw new Error('That location is for a book that isn’t in this library yet. Import the same file first.');
    onOpenBook(book.id, parsed);
  };

  // Files dropped from the file manager (desktop app).
  const importRef = useRef(importFiles);
  importRef.current = importFiles;
  useEffect(() => onNativeFileDrop(e => {
    if (e.type === 'enter') setDragging(true);
    else if (e.type === 'leave') setDragging(false);
    else importRef.current(e.files, e.rejected);
  }), []);

  const onDrop = e => {
    e.preventDefault();
    dragDepth.current = 0;
    setDragging(false);
    importFiles([...(e.dataTransfer?.files ?? [])]);
  };
  const hasFiles = e => [...(e.dataTransfer?.types ?? [])].includes('Files');

  return (
    <div className={`library ${dragging ? 'dragging' : ''}`}
      onDragEnter={e => { if (!hasFiles(e)) return; e.preventDefault(); dragDepth.current++; setDragging(true); }}
      onDragOver={e => { if (hasFiles(e)) e.preventDefault(); }}
      onDragLeave={() => { if (--dragDepth.current <= 0) { dragDepth.current = 0; setDragging(false); } }}
      onDrop={onDrop}>
      <header className="toolbar library-toolbar" data-tauri-drag-region>
        <div className="toolbar-group" data-tauri-drag-region>
          <BookOpen className="brand-icon" size={22} aria-hidden="true" data-tauri-drag-region />
          <h1 data-tauri-drag-region>Library</h1>
        </div>
        <div className="toolbar-spacer" data-tauri-drag-region />
        <div className="toolbar-group" data-tauri-drag-region>
          {synced && <IconButton icon={MonitorSmartphone} label="Devices" onClick={() => setDevices(true)} />}
          <IconButton icon={Navigation} label="Go to location" onClick={() => setGoTo(true)} />
          <IconButton icon={mode === 'grid' ? List : LayoutGrid} label={mode === 'grid' ? 'Show as list' : 'Show as grid'} onClick={() => setMode(m => m === 'grid' ? 'list' : 'grid')} />
          <button type="button" className="primary import-button" onClick={() => input.current.click()} disabled={busy}>
            <Plus size={18} aria-hidden="true" /> <span>Import</span>
          </button>
          <input ref={input} type="file" accept={ACCEPT} multiple hidden onChange={e => { importFiles([...e.target.files]); e.target.value = ''; }} />
          <WindowControls />
        </div>
      </header>

      {message && <div className={`toast ${message.error ? 'error' : ''}`} role="status" onClick={() => setMessage(null)}>{message.text}</div>}

      <main className="library-main">
        {books === null ? null : books.length === 0 ? (
          <div className="empty">
            <BookOpen size={48} strokeWidth={1.25} aria-hidden="true" />
            <h2>Your library is empty</h2>
            <p>Import EPUB or PDF files, or drop them here.</p>
            <button type="button" className="primary" onClick={() => input.current.click()}><Plus size={18} aria-hidden="true" /> Import books</button>
          </div>
        ) : mode === 'grid' ? (
          <ul className="book-grid">
            {books.map(b => (
              <li key={b.id}>
                <button type="button" className={`book-card ${arrival(b, downloads) ? 'arriving' : ''}`} onClick={() => open(b)} title={`${b.title}${b.author ? ` — ${b.author}` : ''}`}>
                  <Cover book={b} />
                  {arrival(b, downloads) && <span className="arrival">{arrival(b, downloads)}</span>}
                  <span className="book-title">{b.title}</span>
                  <span className="book-author">{b.author || ' '}</span>
                  <span className="book-progress">
                    <span className="bar"><span style={{ width: percent(b.progress) }} /></span>
                    <span>{progressLabel(b)}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <ul className="book-list">
            {books.map(b => (
              <li key={b.id}>
                <button type="button" className={`book-row ${arrival(b, downloads) ? 'arriving' : ''}`} onClick={() => open(b)}>
                  <Cover book={b} />
                  <span className="book-row-text">
                    <span className="book-title">{b.title}</span>
                    <span className="book-author">{b.author}</span>
                    <span className="book-meta">{arrival(b, downloads) ?? <>{b.format.toUpperCase()} · {formatSize(b.size)}{b.lastReadAt ? ` · read ${ago(b.lastReadAt)}` : ''}</>}</span>
                  </span>
                  <span className="book-progress"><span className="bar"><span style={{ width: percent(b.progress) }} /></span><span>{progressLabel(b)}</span></span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </main>
      {downloads?.pausedForSpace && (
        <div className="toast error space" role="alert">
          Not enough space to download “{downloads.pausedForSpace.title}” (needs {formatSize(downloads.pausedForSpace.needed)}, {formatSize(downloads.pausedForSpace.available)} free). Free up space, then
          <button type="button" onClick={() => library.retryDownloads()}>Retry</button>
        </div>
      )}
      {devices && <Devices library={library} onClose={() => setDevices(false)} />}
      {dragging && <div className="drop-overlay" aria-hidden="true"><div>Drop EPUB or PDF files to add them</div></div>}
      {goTo && <LocationDialog onGo={goToLocation} onClose={() => setGoTo(false)} />}
    </div>
  );
}
