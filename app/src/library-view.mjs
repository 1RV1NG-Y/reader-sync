// Library list: search and sort (pure, so it's tested without a DOM).

export const SORTS = {
  recent: 'Recently read',
  title: 'Title',
  author: 'Author',
  added: 'Recently added',
};

/** Lowercase without accents, so "émile" finds "Emile". */
const fold = s => String(s ?? '').normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

/** Books whose title, author or file name contain every word of the query. */
export function filterBooks(books, query) {
  const words = fold(query).split(/\s+/).filter(Boolean);
  if (!words.length) return books;
  return books.filter(b => {
    const text = fold(`${b.title} ${b.author} ${b.originalName ?? ''}`);
    return words.every(w => text.includes(w));
  });
}

const collator = new Intl.Collator(undefined, { sensitivity: 'base', numeric: true });
/** "The Left Hand of Darkness" sorts under L. */
const titleKey = t => String(t ?? '').replace(/^(the|a|an)\s+/i, '');
/** "Ursula K. Le Guin" sorts under L, "Walter M. Miller Jr." under M; "Le Guin, Ursula K." stays. */
const authorKey = a => {
  const first = String(a ?? '').split(/\s*(?:&|;|\band\b)\s*/)[0].trim();
  if (!first) return '￿'; // books without an author last
  if (first.includes(',')) return first;
  const parts = first.replace(/,?\s+(jr|sr|[ivx]+)\.?$/i, '').split(/\s+/);
  const particle = parts.length > 2 && /^(le|la|de|van|von|der|du|di|da)$/i.test(parts.at(-2)) ? 2 : 1;
  return `${parts.slice(-particle).join(' ')}, ${parts.slice(0, -particle).join(' ')}`;
};

export function sortBooks(books, by) {
  const out = [...books];
  const byTitle = (a, b) => collator.compare(titleKey(a.title), titleKey(b.title));
  if (by === 'title') out.sort(byTitle);
  else if (by === 'author') out.sort((a, b) => collator.compare(authorKey(a.author), authorKey(b.author)) || byTitle(a, b));
  else if (by === 'added') out.sort((a, b) => b.addedAt.localeCompare(a.addedAt));
  else out.sort((a, b) => (b.lastReadAt ?? b.addedAt).localeCompare(a.lastReadAt ?? a.addedAt));
  return out;
}
