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

// The app defines its own permissions, so Tauri rejects any app command no capability allows,
// before the handler runs; the page only sees a rejected promise.
test('every app command is allowed by a permission that a capability enables', () => {
  const rs = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');
  const handler = rs.match(/generate_handler!\[([\s\S]*?)\]/)[1];
  const commands = handler.split('\n').map(l => l.trim().replace(/,$/, '')).filter(l => l && !l.startsWith('#'))
    .map(c => c.split('::').pop());
  const toml = readFileSync(new URL('../src-tauri/permissions/read-dropped-file.toml', import.meta.url), 'utf8');
  const permissions = {};
  for (const block of toml.split('[[permission]]').slice(1))
    permissions[block.match(/identifier = "(.+)"/)[1]] = JSON.parse(block.match(/commands\.allow = (\[.*\])/)[1]);
  const enabled = ['default', 'mobile'].flatMap(name =>
    JSON.parse(readFileSync(new URL(`../src-tauri/capabilities/${name}.json`, import.meta.url))).permissions);
  const allowed = new Set(Object.entries(permissions).filter(([id]) => enabled.includes(id)).flatMap(([, cmds]) => cmds));
  assert.ok(commands.length > 5, 'found the command list');
  assert.deepEqual(commands.filter(c => !allowed.has(c)), []);
  const mobile = JSON.parse(readFileSync(new URL('../src-tauri/capabilities/mobile.json', import.meta.url))).permissions;
  for (const c of commands.filter(c => c.startsWith('android_')))
    assert.ok(Object.entries(permissions).some(([id, cmds]) => cmds.includes(c) && mobile.includes(id)), `${c} is allowed on Android`);
});
