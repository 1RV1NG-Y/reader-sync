import React, { useEffect, useRef } from 'react';
import { Sheet } from '../ui.jsx';

const labelOf = x => (typeof x === 'string' ? x : x ? Object.values(x)[0] : '') || 'Untitled';

function Items({ items, current, onGo, depth = 0 }) {
  return (
    <ol className={`toc-list depth-${depth}`}>
      {items.map((item, i) => (
        <li key={`${item.href}-${i}`}>
          <button type="button" className={`toc-item ${item.href && item.href === current ? 'current' : ''}`}
            aria-current={item.href === current ? 'location' : undefined}
            disabled={!item.href} onClick={() => onGo(item.href)}>{labelOf(item.label)}</button>
          {item.subitems?.length ? <Items items={item.subitems} current={current} onGo={onGo} depth={depth + 1} /> : null}
        </li>
      ))}
    </ol>
  );
}

export default function TocPanel({ toc, current, onGo, onClose }) {
  const ref = useRef(null);
  useEffect(() => { ref.current?.querySelector('.current')?.scrollIntoView({ block: 'center' }); }, []);
  return (
    <Sheet side="left" title="Contents" onClose={onClose}>
      <nav ref={ref} aria-label="Table of contents">
        {toc.length ? <Items items={toc} current={current} onGo={onGo} /> : <p className="hint">This book has no table of contents.</p>}
      </nav>
    </Sheet>
  );
}
