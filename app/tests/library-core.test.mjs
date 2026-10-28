import test from 'node:test';
import assert from 'node:assert/strict';
import { createCoreLibrary } from '../src/library-core.mjs';

const loc = fraction => ({ cfi: `epubcfi(/6/4!/4/2/1:${Math.round(fraction * 100)})`, section: 1, fraction, snippet: '' });
const drain = () => new Promise(resolve => setImmediate(resolve));

function setup(t, hook = () => {}) {
  const original = { setTimeout, clearTimeout, now: Date.now };
  let now = 0, serial = 0;
  const tasks = new Map(), calls = [], events = [];
  globalThis.setTimeout = (fn, delay = 0) => {
    const id = ++serial;
    tasks.set(id, { at: now + delay, fn });
    return id;
  };
  globalThis.clearTimeout = id => tasks.delete(id);
  Date.now = () => now;
  t.after(() => {
    globalThis.setTimeout = original.setTimeout;
    globalThis.clearTimeout = original.clearTimeout;
    Date.now = original.now;
  });
  const library = createCoreLibrary({
    invoke: async (_, { method, params }) => {
      calls.push({ method, params });
      const result = hook(method, params);
      if (result !== undefined) return result;
      if (method === 'status') return { deviceId: 'me', devices: [] };
      if (method === 'books') return [];
      if (method === 'openBook') return { heads: [], decision: 'mine', position: { locator: loc(0) } };
      if (method === 'handoff') return [{ device: 'phone', hasPlace: true, hasFile: true }];
      return null;
    },
    listen: async () => () => {}, covers: { peek: () => null },
    extractMetadata: async () => ({}), onError: error => { throw error; },
  });
  library.subscribe(e => events.push(e));
  async function advance(to) {
    while (true) {
      const next = [...tasks].filter(([, task]) => task.at <= to).sort((a, b) => a[1].at - b[1].at)[0];
      if (!next) break;
      now = next[1].at;
      tasks.delete(next[0]);
      next[1].fn();
      await drain();
    }
    now = to;
    await drain();
  }
  return { library, calls, events, advance };
}

test('resuming reading before an idle commit preserves the newer checkpoint', async t => {
  const { library, calls, advance } = setup(t);
  await library.openBook('book');
  library.checkpoint('book', loc(.1));
  await advance(2000);
  await advance(9500);
  library.checkpoint('book', loc(.2));
  await advance(13000);
  assert.deepEqual(calls.filter(c => c.method === 'checkpoint').map(c => c.params.location.fraction), [.1, .2]);
  assert.equal(calls.filter(c => c.method === 'commitPosition').length, 0);
  await advance(19500);
  assert.equal(calls.filter(c => c.method === 'commitPosition').length, 1);
});

test('live flush follows an in-flight checkpoint without clearing newer movement', async t => {
  let release;
  const { library, calls, events, advance } = setup(t, method => method === 'checkpoint' ? new Promise(r => { release = r; }) : undefined);
  await library.openBook('book');
  library.checkpoint('book', loc(.1), 1);
  await advance(2000);
  const flush = library.flushPosition('book', loc(.3), 2);
  await drain();
  assert.equal(calls.some(c => c.method === 'flushPosition'), false);
  release();
  await flush;
  assert.deepEqual(events.filter(e => e.type === 'saved').map(e => [e.token, e.current]), [[1, false], [2, true]]);
  assert.equal(calls.find(c => c.method === 'flushPosition').params.location.fraction, .3);
  await advance(20000);
  assert.equal(calls.filter(c => c.method === 'commitPosition').length, 0);
});

test('handoff refuses a stale response and pending unsaved movement', async t => {
  let reply;
  const { library, advance } = setup(t, method => method === 'handoff' ? new Promise(r => { reply = r; }) : undefined);
  await library.openBook('book');
  const old = library.handoff('book');
  await drain();
  library.checkpoint('book', loc(.2));
  await advance(2000);
  reply([{ hasPlace: true, hasFile: true }]);
  assert.equal((await old)[0].hasPlace, false);
  library.checkpoint('book', loc(.4));
  const pending = library.handoff('book');
  await drain();
  reply([{ hasPlace: true, hasFile: true }]);
  assert.equal((await pending)[0].hasPlace, false);
});

test('flush without a locator persists pending checkpoint before committing', async t => {
  const { library, calls } = setup(t);
  await library.openBook('book');
  library.checkpoint('book', loc(.6));
  await library.flushPosition('book');
  assert.deepEqual(calls.filter(c => ['checkpoint', 'commitPosition'].includes(c.method)).map(c => c.method), ['checkpoint', 'commitPosition']);
});
