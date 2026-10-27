import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { openLibrary } from './library.mjs';
import { DEFAULT_SETTINGS } from './settings.mjs';
import { isAndroid, onNativeFileDrop } from './platform.mjs';
import Library from './Library.jsx';
import Reader from './Reader.jsx';

export default function App() {
  const library = useMemo(openLibrary, []);
  const [settings, setSettings] = useState(null);
  const [screen, setScreen] = useState({ name: 'library' });
  const [error, setError] = useState(null);
  const pending = useRef(Promise.resolve());

  useEffect(() => {
    library.getSettings().then(setSettings, e => { setError(e); setSettings({ ...DEFAULT_SETTINGS }); });
    const off = library.subscribe(e => { if (e.type === 'settings') setSettings(e.settings); });
    if (import.meta.env.DEV) globalThis.readerSyncDebug = { ...library.debug, library };
    return off;
  }, [library]);

  useEffect(() => {
    if (!settings) return;
    document.documentElement.dataset.theme = settings.theme;
    document.documentElement.classList.toggle('android', isAndroid());
  }, [settings]);

  // Applied at once (sliders feel live), saved in order behind it.
  const changeSettings = useCallback(patch => {
    setSettings(s => ({ ...s, ...patch }));
    pending.current = pending.current.then(() => library.setSettings(patch)).catch(e => setError(e));
  }, [library]);

  // While reading, files dropped from the file manager are added to the library quietly.
  // (The library screen handles its own drops, with the drop overlay.)
  const [toast, setToast] = useState(null);
  const reading = screen.name === 'reader';
  useEffect(() => {
    if (!reading) return;
    return onNativeFileDrop(async e => {
      if (e.type !== 'drop') return;
      try {
        const { added, skipped } = e.files.length ? await library.importFiles(e.files) : { added: [], skipped: [] };
        const parts = added.map(b => `Added “${b.title}” to the library.`);
        for (const s of [...skipped, ...e.rejected]) parts.push(s.reason === 'duplicate' ? `“${s.name}” is already in the library.` : `${s.name}: ${s.message}`);
        setToast(parts.join(' ') || null);
      } catch (err) {
        setToast(err.message);
      }
    });
  }, [library, reading]);
  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), 5000);
    return () => clearTimeout(t);
  }, [toast]);

  const openBook = useCallback((bookId, target = null) => setScreen({ name: 'reader', bookId, target, key: Date.now() }), []);
  const closeBook = useCallback(() => setScreen({ name: 'library' }), []);

  if (!settings) return <div className="splash" />;
  return (
    <>
      {toast && <div className="toast app-toast" role="status" onClick={() => setToast(null)}>{toast}</div>}
      {error && <div className="app-error" role="alert">{String(error.message ?? error)} <button onClick={() => setError(null)}>Dismiss</button></div>}
      {screen.name === 'reader'
        ? <Reader key={screen.key} library={library} bookId={screen.bookId} target={screen.target} settings={settings}
            onSettings={changeSettings} onClose={closeBook} onOpenBook={openBook} />
        : <Library library={library} onOpenBook={openBook} />}
    </>
  );
}
