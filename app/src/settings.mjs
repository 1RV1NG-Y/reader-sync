// Reading settings (PLAN.md §3): one value per setting, so each can later be its own
// last-writer-wins register in the replicated document. Pure module (runs under node --test).

export const DEFAULT_SETTINGS = Object.freeze({
  fontFamily: 'publisher', // 'publisher' | 'serif' | 'sans' | 'mono'
  fontSize: 18, // px, 12..36
  lineHeight: 1.5, // 1.0..2.4
  margins: 6, // % of the page given to side margins / column gap, 0..20
  maxWidth: 720, // px, maximum line length (column width), 400..1400
  theme: 'light', // 'light' | 'sepia' | 'dark'
  justify: true,
  hyphenate: true,
  flow: 'scrolled', // 'scrolled' | 'paginated'
  publisherStyles: true, // false: the book's own sizes, positions and spacing give way to plain text
  pdfZoom: 'fit-width', // 'fit-width' | 'fit-page'
});

export const FONT_FAMILIES = {
  publisher: { label: 'Book', stack: null },
  serif: { label: 'Serif', stack: "Literata, 'Noto Serif', Georgia, 'Times New Roman', serif" },
  sans: { label: 'Sans', stack: "'Noto Sans', 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif" },
  mono: { label: 'Mono', stack: "'Noto Sans Mono', 'Cascadia Mono', Consolas, 'DejaVu Sans Mono', monospace" },
};

export const THEMES = {
  light: { label: 'Light', bg: '#ffffff', fg: '#1f1f1f', link: '#1a5fb4', scheme: 'light' },
  sepia: { label: 'Sepia', bg: '#f4ecd8', fg: '#5b4636', link: '#8a4b0f', scheme: 'light' },
  dark: { label: 'Dark', bg: '#1d1f21', fg: '#d4d4d4', link: '#8ab4f8', scheme: 'dark' },
};

const LIMITS = { fontSize: [12, 36], lineHeight: [1, 2.4], margins: [0, 20], maxWidth: [400, 1400] };
const CHOICES = { fontFamily: Object.keys(FONT_FAMILIES), theme: Object.keys(THEMES), flow: ['scrolled', 'paginated'], pdfZoom: ['fit-width', 'fit-page'] };

/** Validates a patch: unknown keys and bad values are dropped, numbers are clamped. */
export function cleanSettingsPatch(patch) {
  const out = {};
  for (const [key, value] of Object.entries(patch ?? {})) {
    if (LIMITS[key]) {
      if (typeof value !== 'number' || !Number.isFinite(value)) continue;
      const [min, max] = LIMITS[key];
      out[key] = Math.round(Math.min(max, Math.max(min, value)) * 100) / 100;
    } else if (CHOICES[key]) {
      if (CHOICES[key].includes(value)) out[key] = value;
    } else if (key === 'justify' || key === 'hyphenate' || key === 'publisherStyles') {
      if (typeof value === 'boolean') out[key] = value;
    }
  }
  return out;
}

/** Stored settings (possibly partial or from an older version) → a complete settings object. */
export const withDefaults = stored => ({ ...DEFAULT_SETTINGS, ...cleanSettingsPatch(stored) });

/**
 * "Publisher styles" off: the book's fixed sizes, floats, positioning and spacing give way, so
 * text flows plainly at any font size (e.g. a heading in a fixed-height box that the text
 * outgrows). Pictures, maths and tables keep their own layout.
 */
const PLAIN_LAYOUT = `
    body *:not(img, svg, svg *, video, image, math, math *, table, thead, tbody, tfoot, tr, td, th, col, colgroup, hr) {
      height: auto !important; min-height: 0 !important; max-height: none !important;
      width: auto !important; min-width: 0 !important; max-width: 100% !important;
      position: static !important; float: none !important;
      letter-spacing: normal !important; word-spacing: normal !important;
      line-height: inherit !important;
    }
`;

/** CSS injected into each book document by foliate-js (`renderer.setStyles`). */
export function bookCSS(s) {
  const theme = THEMES[s.theme] ?? THEMES.light;
  const stack = FONT_FAMILIES[s.fontFamily]?.stack;
  const align = s.justify ? 'justify' : 'start';
  const hyphens = s.hyphenate ? 'auto' : 'manual';
  return `
    @namespace epub "http://www.idpf.org/2007/ops";
    html {
      color-scheme: ${theme.scheme};
      font-size: ${s.fontSize}px !important;
    }
    html, body {
      background: ${theme.bg} !important;
      color: ${theme.fg} !important;
    }
    body *:not(a, img, svg, video, image) {
      color: inherit !important;
      background-color: transparent !important;
      border-color: currentColor !important;
    }
    a:link, a:visited { color: ${theme.link} !important; }
    ${stack ? `body, body *:not(code, pre, kbd, samp, tt, code *, pre *) { font-family: ${stack} !important; }` : ''}
    body, p, li, blockquote, dd, div {
      line-height: ${s.lineHeight} !important;
    }
    p, li, blockquote, dd {
      text-align: ${align};
      -webkit-hyphens: ${hyphens};
      hyphens: ${hyphens};
      -webkit-hyphenate-limit-before: 3;
      -webkit-hyphenate-limit-after: 2;
      -webkit-hyphenate-limit-lines: 2;
      hanging-punctuation: allow-end last;
      widows: 2;
    }
    /* keep explicit alignment attributes */
    [align="left"] { text-align: left; }
    [align="right"] { text-align: right; }
    [align="center"] { text-align: center; }
    [align="justify"] { text-align: justify; }
    pre { white-space: pre-wrap !important; }
    img, svg, video { max-width: 100%; }
    aside[epub|type~="endnote"], aside[epub|type~="footnote"],
    aside[epub|type~="note"], aside[epub|type~="rearnote"] { display: none; }
    ${s.publisherStyles === false ? PLAIN_LAYOUT : ''}
  `;
}

/**
 * PDF pages in the sepia and dark themes are recoloured pixel by pixel: each pixel's luminance
 * is mapped onto the theme, white paper → background and black ink → text colour, so a page
 * looks like an EPUB in that theme. Pictures turn into tinted greyscale; the book's "Original
 * colours" switch turns this off. Returns null for "show the page as printed".
 * (Done on the pixels rather than with a CSS/SVG filter: WebKitGTK ignores SVG filters on HTML.)
 */
export function pdfPalette(themeKey) {
  if (themeKey === 'light' || !THEMES[themeKey]) return null;
  const rgb = hex => [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16));
  return { paper: rgb(THEMES[themeKey].bg), ink: rgb(THEMES[themeKey].fg) };
}

/** Recolours RGBA pixel data (an ImageData's `data`) in place for `palette` (pdfPalette). */
export function recolorPixels(data, { paper, ink }) {
  // One lookup table per channel: luminance 0..255 → ink..paper.
  const lut = [0, 1, 2].map(c => Uint8ClampedArray.from({ length: 256 }, (_, l) => ink[c] + (paper[c] - ink[c]) * l / 255));
  const [lr, lg, lb] = lut;
  for (let i = 0; i < data.length; i += 4) {
    const l = (data[i] * 54 + data[i + 1] * 183 + data[i + 2] * 19) >> 8; // Rec. 709 weights
    data[i] = lr[l];
    data[i + 1] = lg[l];
    data[i + 2] = lb[l];
  }
  return data;
}

/** Attributes for foliate-js's paginator (reflowable books). */
export const paginatorAttributes = s => ({
  flow: s.flow,
  gap: `${s.margins}%`,
  // Paginated: height of the running head/foot. Scrolled: foliate-js still treats this band at
  // the top as hidden when it computes the visible range, but scrolls a restored location to
  // the very top; anything but 0 would make every save/restore cycle creep forward.
  margin: s.flow === 'scrolled' ? '0px' : '40px',
  'max-inline-size': `${s.maxWidth}px`,
  'max-block-size': '1600px',
  'max-column-count': '2',
});
