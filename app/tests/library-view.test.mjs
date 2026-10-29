import test from 'node:test';
import assert from 'node:assert/strict';
import { filterBooks, sortBooks } from '../src/library-view.mjs';

const books = [
  { id: 1, title: 'The Left Hand of Darkness', author: 'Ursula K. Le Guin', addedAt: '2026-01-03', lastReadAt: null },
  { id: 2, title: 'Accelerando', author: 'Charles Stross', addedAt: '2026-01-01', lastReadAt: '2026-02-01' },
  { id: 3, title: 'A Canticle for Leibowitz', author: 'Walter M. Miller Jr.', addedAt: '2026-01-02', lastReadAt: null },
  { id: 4, title: 'Émile', author: '', addedAt: '2026-01-04', lastReadAt: null, originalName: 'rousseau-emile.epub' },
];
const ids = list => list.map(b => b.id);

test('search matches every word in title, author or file name, ignoring case and accents', () => {
  assert.deepEqual(ids(filterBooks(books, 'le guin')), [1]);
  assert.deepEqual(ids(filterBooks(books, 'EMILE')), [4]);
  assert.deepEqual(ids(filterBooks(books, 'rousseau')), [4]);
  assert.deepEqual(ids(filterBooks(books, 'stross accel')), [2]);
  assert.deepEqual(ids(filterBooks(books, '  ')), [1, 2, 3, 4]);
  assert.deepEqual(ids(filterBooks(books, 'nothing')), []);
});

test('sorts by recent reading, title (ignoring articles), author surname, and date added', () => {
  assert.deepEqual(ids(sortBooks(books, 'recent')), [2, 4, 1, 3]);
  assert.deepEqual(ids(sortBooks(books, 'title')), [2, 3, 4, 1]); // Accelerando, Canticle, Émile, Left Hand
  assert.deepEqual(ids(sortBooks(books, 'author')), [1, 3, 2, 4]); // Le Guin, Miller, Stross, (none)
  assert.deepEqual(ids(sortBooks(books, 'added')), [4, 1, 3, 2]);
});

test('an author written surname-first sorts by that surname', () => {
  const list = [
    { id: 1, title: 'x', author: 'Le Guin, Ursula K.', addedAt: '' },
    { id: 2, title: 'y', author: 'Mary Lamb and Charles Lamb', addedAt: '' },
    { id: 3, title: 'z', author: 'Iain M. Banks & Ken MacLeod', addedAt: '' },
  ];
  assert.deepEqual(ids(sortBooks(list, 'author')), [3, 2, 1]); // Banks, Lamb, Le Guin
});
