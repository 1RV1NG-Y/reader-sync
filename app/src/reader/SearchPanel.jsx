import React, { useEffect, useRef, useState } from 'react';
import { Sheet } from '../ui.jsx';

/** Search in book (foliate-js search: all sections, diacritics-insensitive). */
export default function SearchPanel({ reader, state, setState, onGo, onClose }) {
  const [query, setQuery] = useState(state.query);
  const run = useRef(0);
  useEffect(() => () => { run.current++; }, []);

  const search = async e => {
    e.preventDefault();
    const q = query.trim();
    const id = ++run.current;
    reader.clearSearch();
    if (!q) { setState({ query: '', groups: [], progress: null }); return; }
    setState({ query: q, groups: [], progress: 0 });
    try {
      for await (const result of reader.search(q)) {
        if (id !== run.current) return;
        if (result === 'done') setState(s => ({ ...s, progress: null }));
        else if (result.progress != null) setState(s => ({ ...s, progress: result.progress }));
        else if (result.subitems) setState(s => ({ ...s, groups: [...s.groups, result] }));
      }
    } catch (err) {
      if (id === run.current) setState(s => ({ ...s, progress: null, error: err.message }));
    }
  };
  const count = state.groups.reduce((n, g) => n + g.subitems.length, 0);

  return (
    <Sheet side="left" title="Search in book" onClose={onClose}>
      <form className="row" onSubmit={search} role="search">
        <input type="search" autoFocus value={query} onChange={e => setQuery(e.target.value)} placeholder="Search" aria-label="Search in book" />
        <button type="submit" className="primary">Search</button>
      </form>
      {state.query && (
        <p className="hint" role="status">
          {state.progress != null ? `Searching… ${Math.round(state.progress * 100)}%` : `${count} ${count === 1 ? 'result' : 'results'}`}
        </p>
      )}
      {state.error && <p className="error">{state.error}</p>}
      <div className="search-results">
        {state.groups.map((g, i) => (
          <section key={i}>
            <h3>{g.label || 'Untitled section'}</h3>
            <ul>
              {g.subitems.map(r => (
                <li key={r.cfi}>
                  <button type="button" onClick={() => onGo(r.cfi)}>
                    {r.excerpt.pre}<mark>{r.excerpt.match}</mark>{r.excerpt.post}
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Sheet>
  );
}
