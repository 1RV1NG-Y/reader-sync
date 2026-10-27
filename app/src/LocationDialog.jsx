import React, { useState } from 'react';
import { Check, ClipboardPaste, Copy } from 'lucide-react';
import { encodeLocation, parseGoTo } from './locator.mjs';
import { copyText, readClipboard } from './platform.mjs';
import { Dialog, percent } from './ui.jsx';

/**
 * "Copy location" / "Go to location". `current` = { bookId, locator } when a book is open.
 * onGo(parsed) navigates; it throws an Error with a user-readable message when it can't.
 */
export default function LocationDialog({ current, onGo, onClose, pdf = false }) {
  const token = current?.locator ? encodeLocation(current.bookId, current.locator) : null;
  const [copied, setCopied] = useState(false);
  const [input, setInput] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const copy = async () => {
    const ok = await copyText(token);
    setCopied(ok);
    // No clipboard access (e.g. a browser without permission): select it for Ctrl+C.
    if (!ok) document.getElementById('location-token')?.select();
  };
  const go = async e => {
    e?.preventDefault();
    setError('');
    let parsed;
    try { parsed = parseGoTo(input); } catch (err) { setError(err.message); return; }
    setBusy(true);
    try {
      await onGo(parsed);
      onClose();
    } catch (err) {
      setError(err.message ?? String(err));
    } finally {
      setBusy(false);
    }
  };
  const paste = async () => {
    const text = await readClipboard();
    if (text) setInput(text.trim());
    else document.getElementById('location-input')?.focus();
  };

  return (
    <Dialog title={current ? 'Location' : 'Go to location'} onClose={onClose} className="location-dialog">
      {token && (
        <section>
          <h3>This location</h3>
          <p className="location-summary">
            {[current.locator.chapter, 'page' in current.locator ? `page ${current.locator.page + 1}` : null, percent(current.locator.fraction)].filter(Boolean).join(' · ')}
            {current.locator.snippet && <q>{current.locator.snippet}</q>}
          </p>
          <div className="row">
            <input id="location-token" className="mono" readOnly value={token} onFocus={e => e.target.select()} aria-label="Location to copy" />
            <button type="button" className="primary" onClick={copy}>
              {copied ? <Check size={16} aria-hidden="true" /> : <Copy size={16} aria-hidden="true" />} {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
          <p className="hint">Paste it into “Go to location” on another device to open this spot — it doesn't depend on font size or screen.</p>
        </section>
      )}
      <form onSubmit={go}>
        <h3>Go to</h3>
        <div className="row">
          <input id="location-input" autoFocus={!token} value={input} onChange={e => { setInput(e.target.value); setError(''); }}
            placeholder={pdf ? 'Paste a location, or 42% or page 12' : 'Paste a location, or type 42%'} aria-label="Location to go to" spellCheck="false" autoComplete="off" />
          <button type="button" onClick={paste} title="Paste"><ClipboardPaste size={16} aria-hidden="true" /> Paste</button>
          <button type="submit" className="primary" disabled={busy || !input.trim()}>Go</button>
        </div>
        {error && <p className="error" role="alert">{error}</p>}
      </form>
    </Dialog>
  );
}
