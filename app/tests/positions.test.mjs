import test from 'node:test';
import assert from 'node:assert/strict';
import { computeHeads, createCheckpointer, decide, descendsFrom, mergeSeen, seenAfter } from '../src/positions.mjs';

const rec = (deviceId, rev, seen = {}) => ({ deviceId, rev, seen: { ...seen, [deviceId]: rev } });

test('descent compares revisions only, never clocks', () => {
  const linux = rec('linux', 5);
  const phone = rec('phone', 2, { linux: 5 });
  assert.ok(descendsFrom(phone, linux));
  assert.ok(!descendsFrom(linux, phone));
  assert.deepEqual(computeHeads([linux, phone]), [phone]);
});

test('decide: mine, continued, divergent', () => {
  assert.deepEqual(decide([], 'linux'), { decision: 'mine', heads: [] });
  const linux = rec('linux', 5);
  assert.equal(decide([linux], 'linux').decision, 'mine');
  const phone = rec('phone', 2, { linux: 5 });
  assert.equal(decide([linux, phone], 'linux').decision, 'continued');
  const offline = rec('phone', 3, { linux: 4 }); // continued from an older Linux revision
  const d = decide([linux, offline], 'linux');
  assert.equal(d.decision, 'divergent');
  assert.equal(d.heads[0].deviceId, 'linux', 'my head is listed first');
});

test('a choice that merges every shown head settles the divergence', () => {
  const linux = rec('linux', 5);
  const phone = rec('phone', 3, { linux: 4 });
  const rev = 6;
  const after = { deviceId: 'linux', rev, seen: seenAfter(linux, [linux, phone], 'linux', rev) };
  assert.deepEqual(after.seen, { linux: 6, phone: 3 });
  assert.equal(decide([after, phone], 'linux').decision, 'mine');
  assert.equal(decide([after, phone], 'phone').decision, 'continued');
  assert.deepEqual(mergeSeen({ a: 1, b: 5 }, { a: 3 }, undefined), { a: 3, b: 5 });
});

test('checkpoints are debounced: only the last location of a scroll burst is written', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const writes = [];
  const cp = createCheckpointer({ write: async (id, loc) => { writes.push([id, loc]); }, delay: 2000, timers: globalThis });
  cp.checkpoint('b', 1);
  t.mock.timers.tick(1500);
  cp.checkpoint('b', 2);
  t.mock.timers.tick(1500);
  cp.checkpoint('b', 3);
  assert.deepEqual(writes, []);
  t.mock.timers.tick(1999);
  assert.deepEqual(writes, []);
  t.mock.timers.tick(1);
  await cp.idle();
  assert.deepEqual(writes, [['b', 3]]);
  assert.equal(cp.pending('b'), undefined);
});

test('flush cancels the pending checkpoint and writes the live location immediately', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const writes = [];
  const cp = createCheckpointer({ write: async (id, loc) => { writes.push(loc); }, delay: 2000, timers: globalThis });
  cp.checkpoint('b', 'before-scroll');
  await cp.flush('b', 'live');
  assert.deepEqual(writes, ['live']);
  t.mock.timers.tick(5000);
  await cp.idle();
  assert.deepEqual(writes, ['live'], 'the cancelled checkpoint never lands');
  await cp.flush('b');
  assert.deepEqual(writes, ['live'], 'nothing pending, nothing written');
  cp.checkpoint('b', 'pending');
  await cp.flush('b');
  assert.deepEqual(writes, ['live', 'pending'], 'flush without a locator writes the pending one');
});

test('a flush issued while a debounced write is in flight lands after it', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const stored = [];
  let release;
  const gate = new Promise(r => { release = r; });
  const cp = createCheckpointer({
    write: async (id, loc) => { if (loc === 'slow') await gate; stored.push(loc); },
    delay: 2000, timers: globalThis,
  });
  cp.checkpoint('b', 'slow');
  t.mock.timers.tick(2000); // debounced write starts and blocks
  const flushed = cp.flush('b', 'closing');
  release();
  await flushed;
  assert.deepEqual(stored, ['slow', 'closing'], 'the closing location is the one stored last');
});

test('books are debounced independently and flushAll writes them all', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const writes = [];
  const cp = createCheckpointer({ write: async (id, loc) => { writes.push(`${id}:${loc}`); }, delay: 2000, timers: globalThis });
  cp.checkpoint('a', 1);
  cp.checkpoint('b', 2);
  await cp.flushAll();
  assert.deepEqual(writes.sort(), ['a:1', 'b:2']);
});

test('a failing debounced write is reported, and later writes still happen', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const errors = [];
  const writes = [];
  const cp = createCheckpointer({
    write: async (id, loc) => { if (loc === 'bad') throw new Error('disk full'); writes.push(loc); },
    delay: 10, timers: globalThis, onError: e => errors.push(e.message),
  });
  cp.checkpoint('b', 'bad');
  t.mock.timers.tick(10);
  await cp.idle();
  await cp.flush('b', 'good');
  assert.deepEqual(errors, ['disk full']);
  assert.deepEqual(writes, ['good']);
});
