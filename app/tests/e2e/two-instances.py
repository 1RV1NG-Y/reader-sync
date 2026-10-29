# Two desktop instances on this machine (separate data dirs, WebKitWebDriver): import on A,
# pair, B receives the file, A reads and hands off, B continues at A's place.
# Build first: (cd app && npx tauri build --debug --no-bundle). Usage: python3 app/tests/e2e/two-instances.py <scratch dir>
import json, subprocess, time, urllib.request, base64, os, sys
from pathlib import Path
ROOT = Path(__file__).resolve().parents[3]
S = sys.argv[1]; BIN = str(ROOT / "target/debug/reader-sync")
procs = []
sessions = []
def start(name, port):
    d = f"{S}/inst-{name}"
    os.makedirs(d, exist_ok=True)
    env = dict(os.environ, TAURI_WEBVIEW_AUTOMATION="true", XDG_DATA_HOME=f"{d}/data", XDG_CONFIG_HOME=f"{d}/cfg", XDG_CACHE_HOME=f"{d}/cache", READER_SYNC_DEVICE_NAME=name, READER_SYNC_MULTI_INSTANCE="1")
    procs.append(subprocess.Popen(["WebKitWebDriver", f"--port={port}"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
    time.sleep(1)
    def req(method, path, body=None):
        r = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method, data=json.dumps(body).encode() if body is not None else None, headers={"Content-Type": "application/json"})
        return json.loads(urllib.request.urlopen(r, timeout=25).read())["value"]
    sid = req("POST", "/session", {"capabilities": {"alwaysMatch": {"webkitgtk:browserOptions": {"binary": BIN, "args": []}}}})["sessionId"]
    sessions.append(lambda: req("DELETE", f"/session/{sid}"))
    def js(code):
        return req("POST", f"/session/{sid}/execute/async", {"script": "const done = arguments[arguments.length-1]; const rpc = (method, params) => window.__TAURI_INTERNALS__.invoke('rpc', { method, params: params ?? null }); (async () => { " + code + " })().then(v => done(v), e => done('ERR ' + (e?.message ?? e)));", "args": []})
    def shot(f): open(f, "wb").write(base64.b64decode(req("GET", f"/session/{sid}/screenshot")))
    return js, shot, lambda: req("DELETE", f"/session/{sid}")
def wait(js, what, cond, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        v = js(cond)
        if v is True: return
        time.sleep(0.5)
    raise SystemExit(f"timed out: {what} (last: {v})")
try:
    A, shotA, endA = start("Test A", 4444)
    B, shotB, endB = start("Test B", 4445)
    time.sleep(3)
    b64 = base64.b64encode((ROOT / "app/tests/fixtures/sample.epub").read_bytes()).decode()
    print("A import:", A("""
      const bytes = Uint8Array.from(atob('""" + b64 + """'), c => c.charCodeAt(0));
      const dt = new DataTransfer(); dt.items.add(new File([bytes], 'sample.epub'));
      document.querySelector('.library').dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
      for (let i = 0; i < 40 && !(await rpc('books')).length; i++) await new Promise(r => setTimeout(r, 250));
      const b = (await rpc('books'))[0]; return JSON.stringify({ title: b?.title, heldBy: b?.heldBy?.length, cover: !!document.querySelector('img.cover') });"""))
    ticket = A("return (await rpc('createInvite')).ticket")
    print("ticket kind:", ticket[:6], "B join:", B(f"return JSON.stringify(await rpc('join', {{ ticket: '{ticket}' }}))"))
    wait(B, "B has the book file", "const b = (await rpc('books'))[0]; const me = (await rpc('status')).deviceId; return !!b && b.heldBy.includes(me);")
    print("B got the file")
    print("A read:", A("""
      document.querySelector('.book-card').click(); await new Promise(r => setTimeout(r, 3000));
      const v = document.querySelector('foliate-view'); await v.goToFraction(0.6); await new Promise(r => setTimeout(r, 1500));
      document.querySelector('.handoff').click(); await new Promise(r => setTimeout(r, 300));
      return document.querySelector('.handoff').className + ' | ' + document.querySelector('.handoff').getAttribute('aria-label');"""))
    wait(A, "A shows B has the place", "return document.querySelector('.handoff')?.classList.contains('handoff-ready') === true", 30)
    print("A chip:", A("return document.querySelector('.handoff').getAttribute('aria-label')"))
    saved = A("const id = (await rpc('books'))[0].id; return (await rpc('peekPosition', { id })).position;")
    received = B("const id = (await rpc('books'))[0].id; return (await rpc('peekPosition', { id })).position;")
    assert received['locator']['cfi'] == saved['locator']['cfi'], (saved, received)
    assert abs(saved['fraction'] - .6) < .03, saved
    opened = B("""
      for (let i = 0; i < 20 && !document.querySelector('.book-card'); i++) await new Promise(r => setTimeout(r, 250));
      document.querySelector('.book-card').click(); await new Promise(r => setTimeout(r, 3500));
      const v = document.querySelector('foliate-view'); return { fraction: v?.lastLocation?.fraction, notice: document.querySelector('.notice')?.innerText };""")
    print("B UI opens:", opened)
    assert abs(opened['fraction'] - saved['fraction']) < .01, (saved, opened)
finally:
    for end in reversed(sessions):
        try: end()
        except Exception: pass
    for p in procs: p.terminate()
