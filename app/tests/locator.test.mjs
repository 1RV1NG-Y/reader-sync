import test from 'node:test';
import assert from 'node:assert/strict';
import { TOKEN_PREFIX, cleanLocator, decodeLocation, encodeLocation, isLocator, makeSnippet, parseGoTo } from '../src/locator.mjs';

const epub = { cfi: 'epubcfi(/6/8!/4/2[c2]/6/1:17)', section: 3, fraction: 0.4213, snippet: 'It was a bright cold day in April, and the clocks…', chapter: 'Chapter 2 — “Ünïcode”' };
const pdf = { page: 41, offset: 0, fraction: 0.2, snippet: '' };

test('a location token round-trips EPUB and PDF locators and the book id', () => {
  for (const locator of [epub, pdf]) {
    const token = encodeLocation('ab12cd', locator);
    assert.ok(token.startsWith(TOKEN_PREFIX));
    assert.match(token, /^[\x21-\x7e]+$/, 'plain printable ASCII, no spaces');
    assert.deepEqual(decodeLocation(token), { bookId: 'ab12cd', locator: cleanLocator(locator) });
  }
});

test('tokens survive whitespace and line breaks added by chat apps', () => {
  const token = encodeLocation('book', epub);
  const mangled = `  ${token.slice(0, 20)}\n${token.slice(20, 50)} ${token.slice(50)}\n`;
  assert.deepEqual(decodeLocation(mangled).locator, cleanLocator(epub));
});

test('encoding drops unknown fields and rejects malformed locators', () => {
  const token = encodeLocation('b', { ...epub, range: { huge: true }, extra: 1 });
  assert.deepEqual(Object.keys(decodeLocation(token).locator).sort(), ['cfi', 'chapter', 'fraction', 'section', 'snippet']);
  assert.throws(() => encodeLocation('b', { cfi: 'not a cfi', section: 0, fraction: 0 }));
  assert.throws(() => encodeLocation('b', { cfi: 'epubcfi(/6/2!)', section: -1, fraction: 0 }));
  assert.throws(() => encodeLocation('b', { page: 1, offset: 0, fraction: 1.5 }));
  assert.throws(() => encodeLocation('', epub));
});

test('decoding rejects damaged, truncated and foreign input with readable errors', () => {
  const token = encodeLocation('b', epub);
  assert.throws(() => decodeLocation(token.slice(0, -12)), /damaged|incomplete/);
  assert.throws(() => decodeLocation('hello'), /not a reader-sync location/);
  assert.throws(() => decodeLocation(TOKEN_PREFIX + '!!!'), /damaged/);
  const b64 = x => Buffer.from(JSON.stringify(x)).toString('base64url');
  const future = TOKEN_PREFIX + b64({ v: 2, b: 'b', l: epub });
  assert.throws(() => decodeLocation(future), /newer version/);
  const bad = TOKEN_PREFIX + b64({ v: 1, b: 'b', l: { cfi: 'x' } });
  assert.throws(() => decodeLocation(bad), /damaged/);
});

test('go-to accepts tokens, CFIs, percentages and PDF pages', () => {
  const token = encodeLocation('b', epub);
  assert.equal(parseGoTo(token).kind, 'token');
  assert.deepEqual(parseGoTo(' epubcfi(/6/4!/2) '), { kind: 'cfi', cfi: 'epubcfi(/6/4!/2)' });
  assert.deepEqual(parseGoTo('42%'), { kind: 'fraction', fraction: 0.42 });
  assert.deepEqual(parseGoTo('12,5 %'), { kind: 'fraction', fraction: 0.125 });
  assert.deepEqual(parseGoTo('page 3'), { kind: 'page', page: 2 });
  assert.throws(() => parseGoTo('120%'), /0% to 100%/);
  assert.throws(() => parseGoTo(''), /Paste a location/);
  assert.throws(() => parseGoTo('chapter four'), /Not a location/);
});

test('snippets are whitespace-normalised and cut near 60 characters at a word boundary', () => {
  assert.equal(makeSnippet('  short\n\ttext '), 'short text');
  const long = 'word '.repeat(40);
  const s = makeSnippet(long);
  assert.ok(s.length <= 61 && s.endsWith('…') && !s.includes('  '));
  assert.equal(makeSnippet('x'.repeat(100)).length, 61);
});

test('isLocator distinguishes the EPUB and PDF shapes', () => {
  assert.ok(isLocator(epub) && isLocator(pdf));
  assert.ok(!isLocator(null) && !isLocator([]) && !isLocator({ fraction: 0.1 }));
});
