// Platform glue: Tauri (desktop / Android) vs plain browser preview.
export const isTauri = () => !!globalThis.__TAURI_INTERNALS__;
export const isAndroid = () => /Android/i.test(navigator.userAgent);

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
