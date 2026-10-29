import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const conf = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url)));

test('the app identifier is final (it is the Android package id)', () => {
  assert.equal(conf.identifier, 'app.readersync');
  assert.equal(conf.productName, 'reader-sync');
});

test('the CSP stays tight: no remote origins, no eval, book scripts cannot run', () => {
  const csp = Object.fromEntries(conf.app.security.csp.split(';').map(d => d.trim().split(/\s+/)).map(([k, ...v]) => [k, v]));
  assert.deepEqual(csp['script-src'], ["'self'"]);
  assert.deepEqual(csp['object-src'], ["'none'"]);
  for (const [directive, sources] of Object.entries(csp))
    for (const source of sources)
      assert.ok(!/^https?:\/\/(?!ipc\.localhost$)/.test(source) && source !== '*' && !source.includes('unsafe-eval'), `${directive} ${source}`);
});

test('foliate-js is vendored at a recorded commit', () => {
  const notes = readFileSync(new URL('../vendor/foliate-js/VENDORED.md', import.meta.url), 'utf8');
  assert.match(notes, /Pinned commit: `[0-9a-f]{40}`/);
  assert.ok(readFileSync(new URL('../vendor/foliate-js/LICENSE', import.meta.url), 'utf8').includes('MIT License'));
});

test('the local foliate-js patch is still applied (see VENDORED.md, "Local patches")', () => {
  const paginator = readFileSync(new URL('../vendor/foliate-js/paginator.js', import.meta.url), 'utf8');
  const turnPage = paginator.slice(paginator.indexOf('async #turnPage('), paginator.indexOf('prev(distance) {'));
  assert.match(turnPage, /finally \{\s*this\.#locked = false/, 'the navigation lock must be released in a finally');
  assert.match(turnPage, /if \(index != null\) await this\.#goTo/, 'no navigation past the first/last section');
  const fixedLayout = readFileSync(new URL('../vendor/foliate-js/fixed-layout.js', import.meta.url), 'utf8');
  assert.match(fixedLayout, /const \{ clientWidth: width, clientHeight: height \} = this/, 'PDF pages fit the width inside the scrollbar');
});
