// Locators: where the reader is in a book, independent of layout (font, size, window, device).
//
//   EPUB  { cfi, section, fraction, snippet, chapter? }
//   PDF   { page, offset, fraction, snippet, chapter? }
//
//   cfi       collapsed EPUB CFI of the first visible text, e.g. "epubcfi(/6/8!/4/2/1:12)"
//   section   spine index (0-based) — redundant with the CFI, used as a fallback and for display
//   page      0-based page index (PDF)
//   offset    0..1 position within the page (PDF; foliate-js shows whole pages, so 0 for now)
//   fraction  0..1 through the whole book (progress display; last-resort fallback)
//   snippet   ~60 characters of text at the location (shown in prompts, sanity check)
//   chapter   table-of-contents label at the location, display only (may be missing)
//
// A location token ("Copy location" / "Go to location") carries the book id and a locator, so a
// position can be moved by hand between devices before sync exists, and checked for
// portability: `rsloc1.` + base64url(JSON {v:1, b:bookId, l:locator}). Tokens are plain ASCII
// so they survive chat apps, e-mail and terminals.
//
// Pure module: no DOM, runs under node --test.

export const TOKEN_PREFIX = 'rsloc1.';
const SNIPPET_LENGTH = 60;
const CFI = /^epubcfi\(.+\)$/;

const isFraction = x => typeof x === 'number' && Number.isFinite(x) && x >= 0 && x <= 1;
const isIndex = x => Number.isInteger(x) && x >= 0;
const optionalString = (x, max) => x === undefined || x === null || (typeof x === 'string' && x.length <= max);

/** True when `x` is a well-formed locator (EPUB or PDF shape). */
export function isLocator(x) {
  if (!x || typeof x !== 'object' || Array.isArray(x)) return false;
  if (!isFraction(x.fraction) || !optionalString(x.snippet, 400) || !optionalString(x.chapter, 400)) return false;
  if ('cfi' in x) return typeof x.cfi === 'string' && CFI.test(x.cfi) && x.cfi.length <= 4000 && isIndex(x.section);
  if ('page' in x) return isIndex(x.page) && isFraction(x.offset);
  return false;
}

/** Keeps only the documented fields, so locators stay small and JSON-clean. */
export function cleanLocator(x) {
  if (!isLocator(x)) throw new Error('Not a valid location.');
  const base = 'cfi' in x ? { cfi: x.cfi, section: x.section } : { page: x.page, offset: x.offset };
  const out = { ...base, fraction: x.fraction, snippet: x.snippet ?? '' };
  if (x.chapter) out.chapter = x.chapter;
  return out;
}

/** Whitespace-normalised text cut to about 60 characters, at a word boundary when one is near. */
export function makeSnippet(text, length = SNIPPET_LENGTH) {
  const flat = String(text ?? '').replace(/\s+/g, ' ').trim();
  if (flat.length <= length) return flat;
  const cut = flat.slice(0, length + 1);
  const space = cut.lastIndexOf(' ');
  return (space > length * 0.6 ? cut.slice(0, space) : cut.slice(0, length)).trimEnd() + '…';
}

const toBase64Url = text => {
  const bytes = new TextEncoder().encode(text);
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
};

const fromBase64Url = text => {
  if (!/^[A-Za-z0-9_-]+$/.test(text)) throw new Error('This is not a reader-sync location.');
  const padded = text.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - text.length % 4) % 4);
  const binary = atob(padded);
  return new TextDecoder('utf-8', { fatal: true }).decode(Uint8Array.from(binary, c => c.charCodeAt(0)));
};

/** Location token for "Copy location". */
export function encodeLocation(bookId, locator) {
  if (typeof bookId !== 'string' || !bookId) throw new Error('A location needs a book.');
  return TOKEN_PREFIX + toBase64Url(JSON.stringify({ v: 1, b: bookId, l: cleanLocator(locator) }));
}

/** Inverse of encodeLocation. Throws a user-readable Error for anything else. */
export function decodeLocation(token) {
  const text = String(token ?? '').replace(/\s+/g, '');
  if (!text.startsWith(TOKEN_PREFIX)) throw new Error('This is not a reader-sync location.');
  let data;
  try { data = JSON.parse(fromBase64Url(text.slice(TOKEN_PREFIX.length))); }
  catch { throw new Error('This location is damaged or incomplete — copy it again.'); }
  if (data?.v !== 1) throw new Error('This location comes from a newer version of reader-sync.');
  if (typeof data.b !== 'string' || !data.b || !isLocator(data.l))
    throw new Error('This location is damaged or incomplete — copy it again.');
  return { bookId: data.b, locator: cleanLocator(data.l) };
}

/**
 * Parses what the user typed or pasted into "Go to location":
 *   a location token            → { kind: 'token', bookId, locator }
 *   an EPUB CFI                 → { kind: 'cfi', cfi }
 *   a percentage ("42%", "42.5 %") → { kind: 'fraction', fraction }
 *   "p 12" / "page 12" (PDF)    → { kind: 'page', page }   (1-based input, 0-based result)
 */
export function parseGoTo(input) {
  const text = String(input ?? '').trim();
  if (!text) throw new Error('Paste a location, or type a percentage such as 42%.');
  if (text.replace(/\s+/g, '').startsWith(TOKEN_PREFIX)) return { kind: 'token', ...decodeLocation(text) };
  if (CFI.test(text)) return { kind: 'cfi', cfi: text };
  const percent = /^(\d{1,3}(?:[.,]\d+)?)\s*%$/.exec(text);
  if (percent) {
    const value = parseFloat(percent[1].replace(',', '.'));
    if (value > 100) throw new Error('A percentage goes from 0% to 100%.');
    return { kind: 'fraction', fraction: value / 100 };
  }
  const page = /^(?:p|page)\.?\s*(\d+)$/i.exec(text);
  if (page && Number(page[1]) >= 1) return { kind: 'page', page: Number(page[1]) - 1 };
  throw new Error('Not a location. Paste a copied location, an EPUB CFI, or type a percentage such as 42%.');
}
