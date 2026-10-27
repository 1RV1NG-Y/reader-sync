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
const CHOICES = { fontFamily: Object.keys(FONT_FAMILIES), theme: Object.keys(THEMES), flow: ['scrolled', 'paginated'] };

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
    } else if (key === 'justify' || key === 'hyphenate') {
      if (typeof value === 'boolean') out[key] = value;
    }
  }
  return out;
}

/** Stored settings (possibly partial or from an older version) → a complete settings object. */
export const withDefaults = stored => ({ ...DEFAULT_SETTINGS, ...cleanSettingsPatch(stored) });

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
  `;
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
