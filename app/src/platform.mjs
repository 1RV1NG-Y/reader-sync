// Platform glue: Tauri (desktop / Android) vs plain browser preview.
export const isTauri = () => !!globalThis.__TAURI_INTERNALS__;
export const isAndroid = () => /Android/i.test(navigator.userAgent);

export async function androidSyncControls() {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke('android_sync_controls');
}
androidSyncControls.available = isTauri() && isAndroid();

/** Toggles fullscreen: the Tauri window on desktop, the Fullscreen API in a browser. */
export async function toggleFullscreen() {
  if (isTauri() && !isAndroid()) {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    const win = getCurrentWindow();
    const next = !(await win.isFullscreen());
    await win.setFullscreen(next);
    return next;
  }
  if (document.fullscreenElement) {
    await document.exitFullscreen();
    return false;
  }
  await document.documentElement.requestFullscreen?.();
  return !!document.fullscreenElement;
}

export const canFullscreen = () => !isAndroid() && (isTauri() || !!document.documentElement.requestFullscreen);

/**
 * Runs `beforeClose` (e.g. saving the reading position) before the desktop window closes.
 * Returns an unsubscribe function. No-op outside Tauri desktop (the browser preview relies
 * on visibilitychange/pagehide instead).
 */
export function onWindowClose(beforeClose) {
  if (!isTauri() || isAndroid()) return () => {};
  const off = import('@tauri-apps/api/window').then(({ getCurrentWindow }) =>
    getCurrentWindow().onCloseRequested(async () => {
      try { await beforeClose(); } catch (e) { console.error(e); }
    }));
  return () => { off.then(f => f()); };
}

const desktopTauri = () => isTauri() && !isAndroid();
const invokeDesktop = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);

/** Desktop: closing the window leaves the app syncing in the tray (false where there's no tray). */
let keepsRunning;
export const keepsRunningInBackground = () =>
  keepsRunning ??= desktopTauri() ? invokeDesktop('keeps_running_in_background').catch(() => false) : Promise.resolve(false);

/** Desktop: starting in the tray when the user logs in. */
export const startAtLogin = {
  available: desktopTauri(),
  get: () => invokeDesktop('start_at_login'),
  set: on => invokeDesktop('set_start_at_login', { on }),
};

/** Android Back button. handler() returns true when it handled the press; otherwise the app exits. */
export function onBackButton(handler) {
  if (!isTauri() || !isAndroid()) return () => {};
  const listener = import('@tauri-apps/api/app').then(({ onBackButtonPress }) =>
    onBackButtonPress(async () => {
      if (await handler()) return;
      const { exit } = await import('@tauri-apps/api/app');
      await exit(0);
    }));
  return () => { listener.then(l => l.unregister()); };
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export async function readClipboard() {
  try { return await navigator.clipboard.readText(); } catch { return ''; }
}

/** Linux desktop: the window has no title bar, so the app draws its own controls. */
export const ownWindowControls = () => isTauri() && !isAndroid() && /Linux/.test(navigator.userAgent);

const BOOK_FILE = /\.(epub|pdf)$/i;
const baseName = path => path.split(/[\\/]/).pop();

/**
 * Files dragged from the file manager onto the desktop window (Tauri's native drag-and-drop,
 * which also works while reading). `handler` gets `{type: 'enter'}`, `{type: 'leave'}`, and
 * `{type: 'drop', files: File[], rejected: [{name, message}]}`. No-op elsewhere; the browser
 * preview uses the page's own drop events.
 */
export function onNativeFileDrop(handler) {
  if (!isTauri() || isAndroid()) return () => {};
  const off = (async () => {
    const [{ getCurrentWebview }, { invoke }] = await Promise.all([import('@tauri-apps/api/webview'), import('@tauri-apps/api/core')]);
    return getCurrentWebview().onDragDropEvent(async ({ payload }) => {
      if (payload.type === 'enter') return handler({ type: 'enter' });
      if (payload.type === 'leave') return handler({ type: 'leave' });
      if (payload.type !== 'drop') return;
      handler({ type: 'leave' });
      const files = [];
      const rejected = [];
      for (const path of payload.paths) {
        const name = baseName(path);
        if (!BOOK_FILE.test(name)) {
          rejected.push({ name, message: 'only EPUB and PDF files can be added.' });
          continue;
        }
        try {
          files.push(new File([await readDropped(invoke, path)], name));
        } catch (e) {
          rejected.push({ name, message: String(e?.message ?? e) });
        }
      }
      handler({ type: 'drop', files, rejected });
    });
  })();
  return () => { off.then(f => f()); };
}

// The window records dropped paths as the same event reaches the page, so allow it a moment.
async function readDropped(invoke, path) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await invoke('read_dropped_file', { path });
    } catch (e) {
      if (attempt >= 5 || !String(e).includes('wasn’t dropped') && !String(e).includes("wasn't dropped")) throw e;
      await new Promise(r => setTimeout(r, 50));
    }
  }
}

/** Scans a QR code with the camera (Android). `scanQrCode.available` says whether it can. */
export async function scanQrCode() {
  const { scan, Format, checkPermissions, requestPermissions } = await import('@tauri-apps/plugin-barcode-scanner');
  let permission = await checkPermissions();
  if (permission !== 'granted') permission = await requestPermissions();
  if (permission !== 'granted') throw new Error('Camera access is needed to scan a code. You can paste the code instead.');
  return (await scan({ formats: [Format.QRCode], windowed: false })).content;
}
scanQrCode.available = isTauri() && isAndroid();
