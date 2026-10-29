// Small shared UI pieces.
import React, { useEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';

export function IconButton({ icon: Icon, label, onClick, active, disabled, className = '', ...rest }) {
  return (
    <button type="button" className={`icon-button ${active ? 'active' : ''} ${className}`} onClick={onClick}
      aria-label={label} title={label} aria-pressed={active ?? undefined} disabled={disabled} {...rest}>
      <Icon size={20} strokeWidth={1.75} aria-hidden="true" />
    </button>
  );
}

/** Side sheet (left or right) with a scrim; full width on phones. */
export function Sheet({ side = 'left', title, onClose, children, className = '' }) {
  const ref = useRef(null);
  useEffect(() => { ref.current?.querySelector('[autofocus], input, button')?.focus(); }, []);
  return (
    <div className="sheet-layer" onKeyDown={e => { if (e.key === 'Escape') { e.stopPropagation(); onClose(); } }}>
      <div className="scrim" onClick={onClose} />
      <aside ref={ref} className={`sheet sheet-${side} ${className}`} role="dialog" aria-label={title}>
        <header className="sheet-header">
          <h2>{title}</h2>
          <IconButton icon={X} label="Close" onClick={onClose} />
        </header>
        <div className="sheet-body">{children}</div>
      </aside>
    </div>
  );
}

/** Centered modal dialog. `onClose` null makes it non-dismissable (a choice is required). */
export function Dialog({ title, onClose, children, className = '' }) {
  const ref = useRef(null);
  useEffect(() => { ref.current?.querySelector('[autofocus], input, button.primary, button')?.focus(); }, []);
  // At the page root, so a dialog opened from a sheet isn't clipped or covered by it.
  return createPortal(
    <div className="dialog-layer" onKeyDown={e => { if (e.key === 'Escape' && onClose) { e.stopPropagation(); onClose(); } }}>
      <div className="scrim" onClick={onClose ?? undefined} />
      <div ref={ref} className={`dialog ${className}`} role="dialog" aria-modal="true" aria-label={title}>
        <header className="dialog-header">
          <h2>{title}</h2>
          {onClose && <IconButton icon={X} label="Close" onClick={onClose} />}
        </header>
        {children}
      </div>
    </div>,
    document.body,
  );
}

export const percent = f => `${Math.floor((f ?? 0) * 100)}%`;

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
export function ago(iso) {
  if (!iso) return '';
  const s = (Date.parse(iso) - Date.now()) / 1000;
  const abs = Math.abs(s);
  if (abs < 60) return 'just now';
  if (abs < 3600) return rtf.format(Math.round(s / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(s / 3600), 'hour');
  return rtf.format(Math.round(s / 86400), 'day');
}

export const formatSize = bytes => bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} kB`;

/** Asks before something that can't be undone or loses something. */
export function Confirm({ title, children, confirmLabel, danger, onConfirm, onCancel }) {
  return (
    <Dialog title={title} onClose={onCancel} className="confirm-dialog">
      <div className="confirm-body">{children}</div>
      <div className="confirm-actions">
        <button type="button" onClick={onCancel}>Cancel</button>
        <button type="button" className={`primary ${danger ? 'danger' : ''}`} autoFocus onClick={onConfirm}>{confirmLabel}</button>
      </div>
    </Dialog>
  );
}

/**
 * A small menu at a point (a "more" button or a right-click / long-press). `items` are
 * `{ label, icon, onSelect, danger }`; it closes on choosing, Escape, or a click elsewhere.
 */
export function Menu({ at, items, onClose }) {
  const ref = useRef(null);
  useEffect(() => {
    ref.current?.querySelector('button')?.focus();
    // The button that opened it toggles it itself.
    const away = e => { if (!ref.current?.contains(e.target) && !e.target.closest?.('[aria-haspopup="menu"][aria-expanded="true"]')) onClose(); };
    document.addEventListener('pointerdown', away, true);
    window.addEventListener('resize', onClose);
    return () => { document.removeEventListener('pointerdown', away, true); window.removeEventListener('resize', onClose); };
  }, [onClose]);
  // Keep it on screen: open left/up from the point when there's no room right/below.
  const style = {
    left: Math.max(8, Math.min(at.x, innerWidth - 240)),
    top: at.y + 44 * items.length + 16 > innerHeight ? undefined : at.y,
    bottom: at.y + 44 * items.length + 16 > innerHeight ? Math.max(8, innerHeight - at.y) : undefined,
  };
  const move = (e, step) => {
    const buttons = [...ref.current.querySelectorAll('button')];
    buttons[(buttons.indexOf(document.activeElement) + step + buttons.length) % buttons.length]?.focus();
    e.preventDefault();
  };
  return (
    <div ref={ref} className="menu" role="menu" style={style}
      onKeyDown={e => {
        if (e.key === 'Escape') { e.stopPropagation(); onClose(); }
        else if (e.key === 'ArrowDown') move(e, 1);
        else if (e.key === 'ArrowUp') move(e, -1);
      }}>
      {items.map(({ label, icon: Icon, onSelect, danger }) => (
        <button key={label} type="button" role="menuitem" className={danger ? 'danger' : ''} onClick={() => { onClose(); onSelect(); }}>
          {Icon && <Icon size={16} strokeWidth={1.75} aria-hidden="true" />} {label}
        </button>
      ))}
    </div>
  );
}
