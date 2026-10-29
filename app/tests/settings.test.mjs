import test from 'node:test';
import assert from 'node:assert/strict';
import { bookCSS, cleanSettingsPatch, withDefaults } from '../src/settings.mjs';

test('publisher styles: on by default, a boolean, and off adds the plain-layout overrides', () => {
  assert.equal(withDefaults({}).publisherStyles, true);
  assert.deepEqual(cleanSettingsPatch({ publisherStyles: false, justify: 'yes' }), { publisherStyles: false });
  assert.deepEqual(cleanSettingsPatch({ publisherStyles: 0 }), {});
  const on = bookCSS(withDefaults({}));
  const off = bookCSS(withDefaults({ publisherStyles: false }));
  assert.doesNotMatch(on, /height: auto !important/);
  assert.match(off, /height: auto !important; min-height: 0 !important; max-height: none !important;/);
  assert.match(off, /float: none !important/);
  assert.match(off, /:not\(img, svg/, 'pictures keep their size');
});

test('PDF recolouring maps white to the theme background and black to its text colour', async () => {
  const { pdfPalette, recolorPixels, THEMES } = await import('../src/settings.mjs');
  assert.equal(pdfPalette('light'), null, 'light shows pages as printed');
  const hex = h => [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16));
  for (const key of ['dark', 'sepia']) {
    const px = recolorPixels(new Uint8ClampedArray([255, 255, 255, 255, 0, 0, 0, 255, 128, 128, 128, 77]), pdfPalette(key));
    assert.deepEqual([...px.slice(0, 3)], hex(THEMES[key].bg), `${key}: paper`);
    assert.deepEqual([...px.slice(4, 7)], hex(THEMES[key].fg), `${key}: ink`);
    assert.equal(px[11], 77, 'alpha is kept');
    const mid = px.slice(8, 11);
    for (let c = 0; c < 3; c++) assert.ok(Math.abs(mid[c] - (hex(THEMES[key].bg)[c] + hex(THEMES[key].fg)[c]) / 2) <= 2, `${key}: grey lands halfway`);
  }
  assert.deepEqual(cleanSettingsPatch({ pdfZoom: 'fit-page' }), { pdfZoom: 'fit-page' });
  assert.deepEqual(cleanSettingsPatch({ pdfZoom: 2 }), {});
});
