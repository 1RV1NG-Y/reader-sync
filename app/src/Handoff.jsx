import React, { useEffect, useState } from 'react';
import { CircleAlert, CircleCheck, RefreshCw } from 'lucide-react';

/**
 * "Ready to continue on another device" (PLAN.md §5) for the open book: ✓ when another device
 * durably holds this device's latest position and the book file; it turns off the moment the
 * reader moves (`moved`) until that newer position has been saved and acknowledged.
 * Clicking it hands off now (saves and sends the live location).
 */
export function summarizeHandoff(devices, moved) {
  const ready = !moved && devices.find(d => d.hasPlace && d.hasFile);
  if (ready) return { state: 'ready', text: `${ready.name} has your place`, short: ready.name };
  const connected = devices.find(d => d.connected);
  if (connected) return { state: 'sending', text: `Sending your place to ${connected.name}…`, short: connected.name };
  return { state: 'none', text: 'No other device has your place yet — open the app on another device', short: 'Not sent' };
}

export default function Handoff({ library, bookId, moved, onHandOff, compact }) {
  const [devices, setDevices] = useState(null);
  // A save makes the last answer stale: never show ✓ for an older position.
  const [stale, setStale] = useState(false);
  useEffect(() => {
    if (!library.handoff) return;
    let alive = true;
    let timer = null;
    let epoch = 0;
    const load = () => {
      const request = ++epoch;
      return library.handoff(bookId).then(d => {
        if (alive && request === epoch) { setDevices(d); setStale(false); }
      }, () => {});
    };
    load();
    const off = library.subscribe(e => {
      if ((e.type === 'saved' || e.type === 'saving') && e.bookId === bookId) { epoch++; setStale(true); }
      if ((e.type === 'status' || e.type === 'saving' || e.type === 'saved' || e.type === 'books') && !timer) timer = setTimeout(() => { timer = null; load(); }, 300);
    });
    return () => { alive = false; clearTimeout(timer); off(); };
  }, [library, bookId]);
  if (!devices || !devices.length) return null;
  const { state, text, short } = summarizeHandoff(devices, moved || stale);
  const Icon = state === 'ready' ? CircleCheck : state === 'sending' ? RefreshCw : CircleAlert;
  return (
    <button type="button" className={`handoff handoff-${state}`} title={`${text}. Click to hand off now.`} aria-label={text} onClick={onHandOff}>
      <Icon size={16} strokeWidth={2} aria-hidden="true" className={state === 'sending' ? 'spin' : ''} />
      {!compact && <span>{short}</span>}
    </button>
  );
}

/** Waits (up to `ms`) until a connected device has the place and the file, or none is connected. */
export async function waitForHandoff(library, bookId, ms = 6000) {
  if (!library.handoff) return true;
  const until = Date.now() + ms;
  while (Date.now() < until) {
    const devices = await library.handoff(bookId).catch(() => []);
    if (devices.some(d => d.hasPlace && d.hasFile)) return true;
    if (!devices.some(d => d.connected)) return false;
    await new Promise(r => setTimeout(r, 300));
  }
  return false;
}
