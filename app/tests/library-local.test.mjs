import test from 'node:test';
import assert from 'node:assert/strict';
import { createLocalLibrary, detectFormat, memoryStore } from '../src/library.mjs';

// Just enough bytes for format detection; metadata extraction is stubbed (it needs a DOM).
const epubBytes = (tag = '') => {
  const head = new Uint8Array(30);
  head.set([0x50, 0x4b, 0x03, 0x04]);
  return new Blob([head, 'mimetypeapplication/epub+zip', `rest of the book ${tag}`]);
};
const file = (blob, name) => new File([blob], name);
const loc = (fraction, extra = {}) => ({ cfi: `epubcfi(/6/4!/4/2/1:${Math.round(fraction * 100)})`, section: 1, fraction, snippet: `at ${fraction}`, ...extra });

function setup(opts = {}) {
  const store = opts.store ?? memoryStore();
  let clock = 0;
  const library = createLocalLibrary({
    store,
    deviceName: 'Linux PC',
    now: () => new Date(Date.UTC(2026, 9, 27, 12, 0, clock++)).toISOString(),
    extractMetadata: opts.extractMetadata ?? (async f => ({ title: `Title of ${f.name}`, author: 'A. Author', cover: 'data:image/jpeg;base64,AAAA' })),
    timers: globalThis,
    onError: e => { throw e; },
  });
  return { store, library };
}

test('format detection reads magic bytes, not file names', async () => {
  assert.equal(detectFormat(await epubBytes().arrayBuffer()), 'epub');
  assert.equal(detectFormat(new TextEncoder().encode('%PDF-1.7\n')), 'pdf');
  assert.equal(detectFormat(new TextEncoder().encode('PK\x03\x04 not an ocf zip at all.................................')), 'zip');
  assert.equal(detectFormat(new TextEncoder().encode('<html>')), null);
});

test('import adds EPUB and PDF, skips duplicates and unsupported files, and notifies', async () => {
  const { library } = setup();
  const events = [];
  library.subscribe(e => events.push(e.type));
  const pdf = new Blob(['%PDF-1.4\n…']);
  const r = await library.importFiles([file(epubBytes(), 'one.epub'), file(pdf, 'two.pdf'), file(epubBytes(), 'copy-of-one.epub'), file(new Blob(['hello']), 'notes.txt')]);
  assert.deepEqual(r.added.map(b => [b.format, b.title]), [['epub', 'Title of one.epub'], ['pdf', 'Title of two.pdf']]);
  assert.deepEqual(r.skipped.map(s => s.reason), ['duplicate', 'unsupported']);
  assert.equal(r.skipped[0].id, r.added[0].id, 'a duplicate names the book it duplicates');
  assert.match(r.added[0].id, /^[0-9a-f]{64}$/);
  assert.deepEqual(events, ['books']);
  const again = await library.importFiles([file(epubBytes(), 'one.epub')]);
  assert.equal(again.added.length, 0);
  assert.equal((await library.listBooks()).length, 2);
});

test('book metadata and file round-trip; summaries are JSON-serialisable', async () => {
  const { library } = setup({ extractMetadata: async () => ({ title: '', author: '' }) });
  const { added: [book] } = await library.importFiles([file(epubBytes('x'), 'My Book.epub')]);
  assert.equal(book.title, 'My Book', 'falls back to the file name');
  const list = await library.listBooks();
  assert.deepEqual(JSON.parse(JSON.stringify(list)), list);
  const blob = await library.bookFile(book.id);
  assert.equal(blob.name, 'My Book.epub');
  assert.equal(blob.size, (await epubBytes('x').arrayBuffer()).byteLength);
  await assert.rejects(library.bookFile('nope'), /no longer in the library/);
});

test('a book that fails to parse is skipped with the reason', async () => {
  const { library } = setup({ extractMetadata: async () => { throw new Error('bad OPF'); } });
  const r = await library.importFiles([file(epubBytes(), 'broken.epub')]);
  assert.deepEqual(r.added, []);
  assert.equal(r.skipped[0].reason, 'error');
  assert.match(r.skipped[0].message, /bad OPF/);
});

test('positions: checkpoint is debounced, flush saves the live location, reopen restores it', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const { library, store } = setup();
  const { added: [book] } = await library.importFiles([file(epubBytes(), 'b.epub')]);
  let opened = await library.openBook(book.id);
  assert.deepEqual(opened, { position: null, decision: 'mine', heads: [], here: null });

  library.checkpoint(book.id, loc(0.1));
  library.checkpoint(book.id, loc(0.2));
  assert.equal((await store.getAll('positions')).length, 0, 'nothing written while scrolling');
  t.mock.timers.tick(2000);
  await new Promise(r => setImmediate(r));
  let [saved] = await store.getAll('positions');
  assert.equal(saved.locator.fraction, 0.2);

  library.checkpoint(book.id, loc(0.3)); // scroll, then close inside the debounce window
  await library.flushPosition(book.id, loc(0.35, { chapter: 'Two' }));
  t.mock.timers.tick(5000);
  await new Promise(r => setImmediate(r));
  [saved] = await store.getAll('positions');
  assert.equal(saved.locator.fraction, 0.35, 'the closing location wins, not the pending checkpoint');
  assert.equal(saved.locator.chapter, 'Two');

  opened = await library.openBook(book.id);
  assert.equal(opened.decision, 'mine');
  assert.equal(opened.position.fraction, 0.35);
  assert.equal(opened.heads[0].mine, true);
  assert.equal((await library.getBook(book.id)).progress, 0.35);
});

test('revisions are never reused, also after a restart on the same store', async () => {
  const store = memoryStore();
  const a = setup({ store }).library;
  const { added: [book] } = await a.importFiles([file(epubBytes(), 'b.epub')]);
  await a.flushPosition(book.id, loc(0.1));
  await a.flushPosition(book.id, loc(0.2));
  const before = (await store.getAll('positions'))[0];
  const b = setup({ store }).library; // "restart"
  assert.equal((await b.info()).device.id, (await a.info()).device.id, 'device identity persists');
  await b.flushPosition(book.id, loc(0.3));
  const after = (await store.getAll('positions'))[0];
  assert.ok(after.rev > before.rev);
  assert.equal(after.seen[after.deviceId], after.rev);
});

test('another device continuing from my position: opened there, saved at once, "back here" settles it', async () => {
  const { library } = setup();
  const { added: [book] } = await library.importFiles([file(epubBytes(), 'b.epub')]);
  await library.flushPosition(book.id, loc(0.2));
  const arrivals = [];
  library.subscribe(e => e.type === 'position' && arrivals.push(e));
  await library.debug.addDevicePosition(book.id, { deviceName: 'Phone', locator: loc(0.6), continuesFromMine: true });
  assert.equal(arrivals.length, 1);
  assert.equal(arrivals[0].head.deviceName, 'Phone');

  const opened = await library.openBook(book.id);
  assert.equal(opened.decision, 'continued');
  assert.equal(opened.position.fraction, 0.6);
  assert.equal(opened.here.fraction, 0.2);
  assert.equal(opened.heads[0].deviceName, 'Phone');
  // The continuation was saved, so reopening is now simply "mine" at the phone's place.
  assert.equal((await library.openBook(book.id)).decision, 'mine');

  // "Back to where I was here"
  const back = await library.resolvePosition(book.id, { stay: opened.here, shown: opened.heads });
  assert.equal(back.fraction, 0.2);
  const reopened = await library.openBook(book.id);
  assert.equal(reopened.decision, 'mine');
  assert.equal(reopened.position.fraction, 0.2);
});

test('divergent positions ask; choosing either head settles it for good', async () => {
  const { library } = setup();
  const { added: [book] } = await library.importFiles([file(epubBytes(), 'b.epub')]);
  await library.flushPosition(book.id, loc(0.2));
  await library.debug.addDevicePosition(book.id, { deviceName: 'Phone', locator: loc(0.7), continuesFromMine: false });
  const opened = await library.openBook(book.id);
  assert.equal(opened.decision, 'divergent');
  assert.equal(opened.position, null);
  assert.deepEqual(opened.heads.map(h => [h.deviceName, h.mine]), [['Linux PC', true], ['Phone', false]]);
  const phone = opened.heads[1];
  const chosen = await library.resolvePosition(book.id, { pick: { deviceId: phone.deviceId, rev: phone.rev }, shown: opened.heads });
  assert.equal(chosen.fraction, 0.7);
  const after = await library.openBook(book.id);
  assert.equal(after.decision, 'mine');
  assert.equal(after.position.fraction, 0.7);
});

test('"Stay here" on the mid-session banner settles the arrival without moving', async () => {
  const { library } = setup();
  const { added: [book] } = await library.importFiles([file(epubBytes(), 'b.epub')]);
  await library.flushPosition(book.id, loc(0.2));
  let head;
  library.subscribe(e => { if (e.type === 'position') head = e.head; });
  await library.debug.addDevicePosition(book.id, { deviceName: 'Phone', locator: loc(0.9), continuesFromMine: false });
  const stay = await library.resolvePosition(book.id, { stay: loc(0.25), shown: [head] });
  assert.equal(stay.fraction, 0.25);
  const after = await library.openBook(book.id);
  assert.equal(after.decision, 'mine');
  assert.equal(after.position.fraction, 0.25);
});

test('settings: defaults, validated patches, persistence and change events', async () => {
  const store = memoryStore();
  const { library } = setup({ store });
  const s0 = await library.getSettings();
  assert.equal(s0.flow, 'scrolled', 'scrolled mode is the default');
  const events = [];
  library.subscribe(e => events.push(e));
  const s1 = await library.setSettings({ fontSize: 99, theme: 'sepia', flow: 'sideways', bogus: 1, justify: false });
  assert.equal(s1.fontSize, 36, 'clamped');
  assert.equal(s1.theme, 'sepia');
  assert.equal(s1.flow, 'scrolled', 'invalid value ignored');
  assert.equal(s1.justify, false);
  assert.ok(!('bogus' in s1));
  assert.deepEqual(events, [{ type: 'settings', settings: s1 }]);
  assert.deepEqual(await setup({ store }).library.getSettings(), s1, 'persisted');
});
