// The position prompts sync will drive (PLAN.md §4). The local backend only ever reports
// 'mine'; the others are exercised with library.debug.addDevicePosition in dev builds.
import React from 'react';
import { Smartphone, X } from 'lucide-react';
import { Dialog, ago, percent } from '../ui.jsx';

const where = locator => [locator?.chapter, 'page' in (locator ?? {}) ? `page ${locator.page + 1}` : null, percent(locator?.fraction)].filter(Boolean).join(' · ');

/** "Continued from Phone · ch. 12" + "Back to where I was here". */
export function ContinuedNote({ head, onBack, onDismiss }) {
  return (
    <div className="notice" role="status">
      <Smartphone size={18} aria-hidden="true" />
      <span>Continued from <strong>{head.deviceName}</strong> · {where(head.locator)}</span>
      {onBack && <button type="button" onClick={onBack}>Back to where I was here</button>}
      <button type="button" className="icon-only" aria-label="Dismiss" onClick={onDismiss}><X size={16} /></button>
    </div>
  );
}

/** Mid-session arrival: never jumps by itself. */
export function ArrivalBanner({ head, onGo, onStay }) {
  return (
    <div className="notice" role="status">
      <Smartphone size={18} aria-hidden="true" />
      <span><strong>{head.deviceName}</strong> is at {where(head.locator)}</span>
      <button type="button" className="primary" onClick={onGo}>Go there</button>
      <button type="button" onClick={onStay}>Stay here</button>
    </div>
  );
}

/** Several heads: the reader chooses. Non-dismissable: the answer is saved everywhere. */
export function DivergentChooser({ heads, onChoose }) {
  return (
    <Dialog title="Where do you want to continue?" onClose={null} className="divergent-dialog">
      <p className="hint">You read this book on more than one device since they last synced.</p>
      <ul className="heads">
        {heads.map(h => (
          <li key={`${h.deviceId}:${h.rev}`}>
            <button type="button" className="head" onClick={() => onChoose(h)}>
              <span className="head-device">{h.mine ? `${h.deviceName} (this device)` : h.deviceName} <small>{ago(h.at)}</small></span>
              <span className="head-where">{where(h.locator)}</span>
              {h.locator?.snippet && <q>{h.locator.snippet}</q>}
            </button>
          </li>
        ))}
      </ul>
    </Dialog>
  );
}
