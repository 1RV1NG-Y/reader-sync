# Vendored: foliate-js

- Upstream: https://github.com/johnfactotum/foliate-js
- Pinned commit: `78914aef4466eb960965702401634c2cb348e9b1`
  (2026-05-01, "Use original hrefs for external links and add isExternal in fb2.js (#129)")
- License: MIT, Copyright (c) 2022 John Factotum — see `LICENSE` in this directory.

Bundled third-party code that came with that commit (unchanged):

| Path | Project | License |
|---|---|---|
| `vendor/zip.js` | zip.js (Gildas Lormeau) | BSD-3-Clause |
| `vendor/fflate.js` | fflate | MIT |
| `vendor/pdfjs/` | PDF.js 5.5.207 (Mozilla) | Apache-2.0 |
| `vendor/pdfjs/standard_fonts/` | Foxit fonts / Liberation fonts | see `LICENSE_FOXIT`, `LICENSE_LIBERATION` there |

## What was copied

Everything at that commit **except**: `.git/`, `.github/`, `tests/`, `rollup/`, `rollup.config.js`,
`eslint.config.js`, `package-lock.json`, `.gitignore`, `.gitattributes`, the demo app
(`reader.html`, `reader.js`, `ui/`), and the PDF.js source maps (`vendor/pdfjs/*.map`, ~7 MB;
the `sourceMappingURL` comments that point at them are left in place and are harmless).

No file was modified. Do not edit files here; if a fix is needed, patch it in our own code or
re-vendor a newer upstream commit and update this file.

## How the app uses it

`vite.config.js` serves this directory unbundled at `/foliate-js/` (dev) and copies it to
`dist/foliate-js/` (build), so foliate-js's own relative `import()`s and
`new URL('vendor/pdfjs/…', import.meta.url)` lookups keep working exactly as upstream. The app
loads it with a runtime `import()` in `src/reader/engine.mjs`.

## Updating

```sh
git clone https://github.com/johnfactotum/foliate-js /some/empty/dir/foliate-js
git -C /some/empty/dir/foliate-js checkout <commit>
rsync -a --delete --exclude VENDORED.md --exclude .git --exclude .github --exclude tests \
  --exclude rollup --exclude rollup.config.js --exclude eslint.config.js --exclude package-lock.json \
  --exclude .gitignore --exclude .gitattributes --exclude reader.html --exclude reader.js \
  --exclude ui --exclude '*.map' /some/empty/dir/foliate-js/ app/vendor/foliate-js/
```

Then update the commit above and re-check the reader (scrolled + paginated, EPUB + PDF): the
foliate-js API is not stable.
