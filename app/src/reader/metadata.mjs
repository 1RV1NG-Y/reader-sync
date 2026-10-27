// Title, author and a small cover thumbnail for a book being imported (local backend only;
// the sync core reads the OPF itself). Runs in the webview: uses foliate-js and a canvas.
import { bookAuthor, bookTitle, loadFoliate } from './engine.mjs';

const THUMB_WIDTH = 240;
const THUMB_MAX_HEIGHT = 360;

export async function extractMetadata(file) {
  const { makeBook } = await loadFoliate();
  const book = await makeBook(file);
  try {
    let cover = null;
    try {
      const blob = await book.getCover?.();
      if (blob) cover = await thumbnail(blob);
    } catch (e) {
      console.warn('no cover thumbnail', e);
    }
    return { title: bookTitle(book.metadata), author: bookAuthor(book.metadata), cover };
  } finally {
    try { book.destroy?.(); } catch { /* ignore */ }
  }
}

/** Blob (any image the webview can decode, including SVG) → small JPEG data: URL. */
async function thumbnail(blob) {
  const url = URL.createObjectURL(blob);
  try {
    const img = new Image();
    img.src = url;
    await img.decode();
    const w = img.naturalWidth || 300;
    const h = img.naturalHeight || 450;
    const scale = Math.min(THUMB_WIDTH / w, THUMB_MAX_HEIGHT / h, 1);
    const canvas = document.createElement('canvas');
    canvas.width = Math.max(1, Math.round(w * scale));
    canvas.height = Math.max(1, Math.round(h * scale));
    const ctx = canvas.getContext('2d');
    ctx.fillStyle = '#fff';
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(img, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL('image/jpeg', 0.82);
  } finally {
    URL.revokeObjectURL(url);
  }
}
