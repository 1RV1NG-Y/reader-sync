// Generates the test books in this directory (no copyrighted text, no dependencies):
//
//   sample.epub  EPUB 3: cover image, nav TOC, 8 chapters of numbered paragraphs long enough
//                to scroll, so a position can be checked by its "Chapter N, paragraph M" text
//   sample.pdf   6-page text PDF with the same numbering
//
//   node tests/fixtures/make-fixtures.mjs     (npm run fixtures)
import { writeFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';

const out = name => new URL(`./${name}`, import.meta.url);

// ─── deterministic filler text ──────────────────────────────────────────────────────────────
const WORDS = ('river lantern orchard quiet harbor copper meadow ledger window morning thread ' +
  'compass garden letter winter station circle bridge pocket island candle market valley ' +
  'signal mirror ladder paper season shadow harvest engine forest kettle anchor stone').split(' ');
let seed = 7;
const rand = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
const sentence = () => {
  const n = 8 + Math.floor(rand() * 10);
  const words = Array.from({ length: n }, () => WORDS[Math.floor(rand() * WORDS.length)]);
  return words.join(' ').replace(/^./, c => c.toUpperCase()) + '.';
};
const paragraph = (c, p) => `Chapter ${c}, paragraph ${p}. ` + Array.from({ length: 4 + Math.floor(rand() * 4) }, sentence).join(' ');

// ─── ZIP (stored, no compression) ───────────────────────────────────────────────────────────
const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc32 = buf => {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};
function zip(entries) {
  const parts = [];
  const central = [];
  let offset = 0;
  for (const [name, data] of entries) {
    const body = typeof data === 'string' ? Buffer.from(data, 'utf8') : data;
    const fname = Buffer.from(name, 'utf8');
    const crc = crc32(body);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0); local.writeUInt16LE(20, 4); local.writeUInt16LE(0, 6);
    local.writeUInt16LE(0, 8); local.writeUInt32LE(0, 10); local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18); local.writeUInt32LE(body.length, 22);
    local.writeUInt16LE(fname.length, 26); local.writeUInt16LE(0, 28);
    parts.push(local, fname, body);
    const cd = Buffer.alloc(46);
    cd.writeUInt32LE(0x02014b50, 0); cd.writeUInt16LE(20, 4); cd.writeUInt16LE(20, 6);
    cd.writeUInt16LE(0, 8); cd.writeUInt16LE(0, 10); cd.writeUInt32LE(0, 12); cd.writeUInt32LE(crc, 16);
    cd.writeUInt32LE(body.length, 20); cd.writeUInt32LE(body.length, 24); cd.writeUInt16LE(fname.length, 28);
    cd.writeUInt32LE(offset, 42);
    central.push(cd, fname);
    offset += 30 + fname.length + body.length;
  }
  const cdBuf = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0); end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(cdBuf.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...parts, cdBuf, end]);
}

// ─── PNG cover (solid colours, drawn bands) ─────────────────────────────────────────────────
function png(width, height, pixel) {
  const raw = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (width * 3 + 1)] = 0;
    for (let x = 0; x < width; x++) raw.set(pixel(x, y), y * (width * 3 + 1) + 1 + x * 3);
  }
  const chunk = (type, data) => {
    const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
    const td = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
    return Buffer.concat([len, td, crc]);
  };
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0); ihdr.writeUInt32BE(height, 4); ihdr[8] = 8; ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}
const cover = png(300, 450, (x, y) => (y > 150 && y < 230) ? [224, 166, 74] : (x < 18 ? [30, 60, 64] : [47, 93, 98]));

// ─── EPUB ───────────────────────────────────────────────────────────────────────────────────
const CHAPTERS = 8;
const PARAGRAPHS = 30;
const esc = s => s.replace(/&/g, '&amp;').replace(/</g, '&lt;');
const xhtml = (title, body) => `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" xml:lang="en" lang="en">
<head><meta charset="UTF-8"/><title>${esc(title)}</title><link rel="stylesheet" type="text/css" href="style.css"/></head>
<body>${body}</body>
</html>`;

const chapters = Array.from({ length: CHAPTERS }, (_, i) => {
  const c = i + 1;
  const paras = Array.from({ length: PARAGRAPHS }, (_, j) => `<p id="c${c}p${j + 1}">${esc(paragraph(c, j + 1))}</p>`).join('\n');
  const section = c === 3 ? `<h2 id="c3s2">Chapter 3, second part</h2>` : '';
  const half = Math.floor(PARAGRAPHS / 2);
  const parts = paras.split('\n');
  return xhtml(`Chapter ${c}`, `<section epub:type="chapter"><h1>Chapter ${c}</h1>\n${parts.slice(0, half).join('\n')}\n${section}\n${parts.slice(half).join('\n')}</section>`);
});

const nav = xhtml('Contents', `<nav epub:type="toc" id="toc"><h1>Contents</h1><ol>
${Array.from({ length: CHAPTERS }, (_, i) => `<li><a href="ch${i + 1}.xhtml">Chapter ${i + 1}</a>${i === 2 ? '<ol><li><a href="ch3.xhtml#c3s2">Chapter 3, second part</a></li></ol>' : ''}</li>`).join('\n')}
</ol></nav>`);

const coverPage = xhtml('Cover', `<div style="text-align:center"><img src="cover.png" alt="Cover" style="max-height:95vh"/></div>`);
const titlePage = xhtml('Title page', `<h1 style="text-align:center">The Sample Lighthouse</h1><p style="text-align:center">A generated test book for reader-sync</p>`);

const opf = `<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid" xml:lang="en">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="uid">urn:uuid:5b0c6c1e-6a52-4c39-9d43-reader-sync-fixture</dc:identifier>
<dc:title>The Sample Lighthouse</dc:title>
<dc:creator>Fixture Author</dc:creator>
<dc:language>en</dc:language>
<meta property="dcterms:modified">2026-10-27T00:00:00Z</meta>
<meta name="cover" content="cover-image"/>
</metadata>
<manifest>
<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
<item id="cover-image" href="cover.png" media-type="image/png" properties="cover-image"/>
<item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/>
<item id="title" href="title.xhtml" media-type="application/xhtml+xml"/>
<item id="css" href="style.css" media-type="text/css"/>
${Array.from({ length: CHAPTERS }, (_, i) => `<item id="ch${i + 1}" href="ch${i + 1}.xhtml" media-type="application/xhtml+xml"/>`).join('\n')}
</manifest>
<spine>
<itemref idref="cover" linear="no"/>
<itemref idref="title"/>
<itemref idref="nav"/>
${Array.from({ length: CHAPTERS }, (_, i) => `<itemref idref="ch${i + 1}"/>`).join('\n')}
</spine>
</package>`;

const container = `<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>`;

// A book script that must NOT run (the app's CSP blocks scripts in book content).
const scriptProbe = `<script>document.title = 'BOOK SCRIPT RAN'</script>`;

writeFileSync(out('sample.epub'), zip([
  ['mimetype', 'application/epub+zip'],
  ['META-INF/container.xml', container],
  ['OEBPS/content.opf', opf],
  ['OEBPS/nav.xhtml', nav],
  ['OEBPS/cover.xhtml', coverPage],
  ['OEBPS/title.xhtml', titlePage.replace('</body>', `${scriptProbe}</body>`)],
  ['OEBPS/cover.png', cover],
  ['OEBPS/style.css', 'body { margin: 0 1em; font-family: serif; } h1 { text-align: center; } p { text-indent: 1.5em; margin: 0 0 .4em; }'],
  ...chapters.map((html, i) => [`OEBPS/ch${i + 1}.xhtml`, html]),
]));

// ─── PDF ────────────────────────────────────────────────────────────────────────────────────
function pdf(pages) {
  const objects = [];
  const add = body => { objects.push(body); return objects.length; };
  const catalog = add(null);
  const pagesId = add(null);
  const font = add('<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>');
  const info = add('<< /Title (Sample Lighthouse PDF) /Author (Fixture Author) >>');
  const kids = [];
  for (const lines of pages) {
    const text = lines.map((l, i) => `BT /F1 11 Tf 56 ${780 - i * 16} Td (${l.replace(/[()\\]/g, '\\$&')}) Tj ET`).join('\n');
    const content = add(`<< /Length ${Buffer.byteLength(text)} >>\nstream\n${text}\nendstream`);
    kids.push(add(`<< /Type /Page /Parent ${pagesId} 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${content} 0 R >>`));
  }
  objects[catalog - 1] = `<< /Type /Catalog /Pages ${pagesId} 0 R >>`;
  objects[pagesId - 1] = `<< /Type /Pages /Kids [${kids.map(k => `${k} 0 R`).join(' ')}] /Count ${kids.length} >>`;
  let body = '%PDF-1.4\n';
  const offsets = [];
  objects.forEach((o, i) => { offsets.push(Buffer.byteLength(body)); body += `${i + 1} 0 obj\n${o}\nendobj\n`; });
  const xref = Buffer.byteLength(body);
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n` + offsets.map(o => `${String(o).padStart(10, '0')} 00000 n \n`).join('');
  body += `trailer\n<< /Size ${objects.length + 1} /Root ${catalog} 0 R /Info ${info} 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(body, 'latin1');
}
const wrap = (text, width = 90) => text.match(new RegExp(`.{1,${width}}(\\s|$)`, 'g')).map(s => s.trim());
writeFileSync(out('sample.pdf'), pdf(Array.from({ length: 6 }, (_, i) => [
  `Page ${i + 1}`, '',
  ...Array.from({ length: 6 }, (_, j) => wrap(paragraph(i + 1, j + 1)).concat([''])).flat(),
].slice(0, 46))));

console.log('Wrote sample.epub and sample.pdf');
