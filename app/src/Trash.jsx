import React, { useEffect, useState } from 'react';
import { RotateCcw, Trash2 } from 'lucide-react';
import { Confirm, Sheet } from './ui.jsx';

/** Books in Trash: restore them, or delete them forever (PLAN.md §6). */
export default function Trash({ library, synced, onClose, onMessage }) {
  const [books, setBooks] = useState(null);
  const [confirm, setConfirm] = useState(null); // books to delete forever

  useEffect(() => {
    const load = () => library.listTrash().then(setBooks, e => onMessage({ error: true, text: e.message }));
    load();
    return library.subscribe(e => { if (e.type === 'books') load(); });
  }, [library, onMessage]);

  const run = async (action, done) => {
    try { await action(); if (done) onMessage({ text: done }); }
    catch (e) { onMessage({ error: true, text: e?.message ?? String(e) }); }
  };
  const restore = b => run(() => library.setTrashed([b.id], false), `Restored “${b.title}”.`);
  const deleteForever = list => run(() => library.deleteForever(list.map(b => b.id)),
    list.length === 1 ? `Deleted “${list[0].title}” forever.` : `Deleted ${list.length} books forever.`);
  const everywhere = synced ? ' from all your devices' : '';

  return (
    <Sheet side="right" title="Trash" onClose={onClose} className="trash">
      <p className="hint">
        Books in Trash are hidden{synced ? ' on all your devices' : ''} but still take up space.
        Delete them forever to free it{everywhere ? ' everywhere' : ''}.
      </p>
      {books?.length > 0 && (
        <button type="button" className="danger-outline" onClick={() => setConfirm(books)}>
          <Trash2 size={15} aria-hidden="true" /> Empty Trash
        </button>
      )}
      {books?.length === 0 && <p className="hint">Trash is empty.</p>}
      <ul className="trash-list">
        {books?.map(b => (
          <li key={b.id} className="trash-row">
            {b.cover ? <img className="cover" src={b.cover} alt="" /> : <div className="cover cover-placeholder" aria-hidden="true" />}
            <div>
              <div className="book-title">{b.title}</div>
              <div className="book-author">{b.author}</div>
            </div>
            <div className="actions">
              <button type="button" onClick={() => restore(b)}><RotateCcw size={14} aria-hidden="true" /> Restore</button>
              <button type="button" className="danger" onClick={() => setConfirm([b])}>Delete forever</button>
            </div>
          </li>
        ))}
      </ul>
      {confirm && (
        <Confirm danger title={confirm.length === 1 ? 'Delete forever?' : `Delete ${confirm.length} books forever?`}
          confirmLabel="Delete forever"
          onCancel={() => setConfirm(null)}
          onConfirm={() => { deleteForever(confirm); setConfirm(null); }}>
          <p>
            {confirm.length === 1 ? <>“{confirm[0].title}”</> : 'These books'} will be removed{everywhere}, together with
            {confirm.length === 1 ? ' its' : ' their'} reading positions and files. This can’t be undone.
          </p>
          <p className="hint">To read {confirm.length === 1 ? 'it' : 'them'} again later, you’d import the file again and start from the beginning.</p>
        </Confirm>
      )}
    </Sheet>
  );
}
