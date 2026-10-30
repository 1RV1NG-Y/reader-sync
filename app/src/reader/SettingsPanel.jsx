import React from 'react';
import { readingBars } from '../platform.mjs';
import { DEFAULT_SETTINGS, FONT_FAMILIES, THEMES } from '../settings.mjs';
import { Sheet } from '../ui.jsx';

function Segmented({ label, value, options, onChange }) {
  return (
    <fieldset className="field">
      <legend>{label}</legend>
      <div className="segmented" role="radiogroup" aria-label={label}>
        {options.map(([key, text, style]) => (
          <button key={key} type="button" role="radio" aria-checked={value === key} className={value === key ? 'selected' : ''}
            style={style} onClick={() => onChange(key)}>{text}</button>
        ))}
      </div>
    </fieldset>
  );
}

function Slider({ label, value, min, max, step, format, onChange }) {
  return (
    <label className="field slider">
      <span className="field-label">{label}<output>{format(value)}</output></span>
      <span className="slider-row">
        <button type="button" aria-label={`Less ${label.toLowerCase()}`} onClick={() => onChange(Math.max(min, +(value - step).toFixed(2)))}>−</button>
        <input type="range" min={min} max={max} step={step} value={value} onChange={e => onChange(Number(e.target.value))} aria-label={label} />
        <button type="button" aria-label={`More ${label.toLowerCase()}`} onClick={() => onChange(Math.min(max, +(value + step).toFixed(2)))}>+</button>
      </span>
    </label>
  );
}

function Toggle({ label, checked, onChange }) {
  return (
    <label className="field toggle">
      <span>{label}</span>
      <input type="checkbox" role="switch" checked={checked} onChange={e => onChange(e.target.checked)} />
    </label>
  );
}

export default function SettingsPanel({ settings: s, onChange, onClose, pdf, originalColors, onOriginalColors }) {
  const set = key => value => onChange({ [key]: value });
  return (
    <Sheet side="right" title="Reading settings" onClose={onClose} className="settings-sheet">
      <Segmented label="Theme" value={s.theme} onChange={set('theme')}
        options={Object.entries(THEMES).map(([k, t]) => [k, t.label, { background: t.bg, color: t.fg }])} />
      {pdf ? <>
        <Segmented label="Page" value={s.pdfZoom} onChange={set('pdfZoom')} options={[['fit-width', 'Fit width'], ['fit-page', 'Whole page']]} />
        {s.theme !== 'light' && <Toggle label="Original colours (this book)" checked={originalColors} onChange={onOriginalColors} />}
        <p className="hint">{s.theme === 'light'
          ? 'Pages are shown as printed. In Sepia and Dark they’re recoloured to match.'
          : originalColors
            ? 'This book’s pages are shown as printed.'
            : 'Pages are recoloured to match the theme, which also turns pictures into one colour. For comics or picture books, turn on Original colours.'}</p>
        <p className="hint">Text settings and layout don’t apply to PDF pages.</p>
      </> : <>
        <Segmented label="Layout" value={s.flow} onChange={set('flow')} options={[['scrolled', 'Scrolled'], ['paginated', 'Pages']]} />
        <Segmented label="Font" value={s.fontFamily} onChange={set('fontFamily')}
          options={Object.entries(FONT_FAMILIES).map(([k, f]) => [k, f.label, f.stack ? { fontFamily: f.stack } : undefined])} />
        <Slider label="Font size" value={s.fontSize} min={12} max={36} step={1} format={v => `${v}px`} onChange={set('fontSize')} />
        <Slider label="Line height" value={s.lineHeight} min={1} max={2.4} step={0.1} format={v => v.toFixed(1)} onChange={set('lineHeight')} />
        <Slider label="Margins" value={s.margins} min={0} max={20} step={1} format={v => `${v}%`} onChange={set('margins')} />
        <Slider label="Line width" value={s.maxWidth} min={400} max={1400} step={20} format={v => `${v}px`} onChange={set('maxWidth')} />
        <Toggle label="Justify text" checked={s.justify} onChange={set('justify')} />
        <Toggle label="Hyphenation" checked={s.hyphenate} onChange={set('hyphenate')} />
        <Toggle label="Publisher styles" checked={s.publisherStyles} onChange={set('publisherStyles')} />
        {!s.publisherStyles && <p className="hint">The book’s own sizes, positions and spacing are ignored, so text flows plainly. Turn this back on if pictures or tables look wrong.</p>}
        {s.publisherStyles && <p className="hint">Turn off if headings or text overlap or get cut off at larger font sizes.</p>}
      </>}
      {readingBars.available && <>
        <Toggle label="Hide status bar" checked={s.hideStatusBar} onChange={set('hideStatusBar')} />
        <p className="hint">{s.hideStatusBar
          ? 'The clock and notifications are hidden while a book is open. Swipe down from the top edge to see them for a moment.'
          : 'The clock and notifications stay visible while reading.'}</p>
      </>}
      <button type="button" className="reset" onClick={() => onChange({ ...DEFAULT_SETTINGS })}>Reset to defaults</button>
    </Sheet>
  );
}
