"""Aura vs. Chrome vs. Edge — Startzeit, Seitenladezeit, RAM, Ad-Blocking.

Startet einen lokalen HTTP-Server, laesst jeden Browser dieselbe Testseite laden
und misst von aussen (Prozessstart) sowie von innen (Beacons der Seite).
Jeder Browser bekommt ein eigenes, frisches Profilverzeichnis; die laufende
Chrome-Instanz des Nutzers wird nicht angefasst.

    python bench/bench.py [--runs 3]
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
import shutil
import socketserver
import statistics
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

PORT = 8099
HOST = "127.0.0.1"
ROOT = Path(__file__).resolve().parent
REPO = ROOT.parent
RESULTS = ROOT / "results.json"

# Bekannte Ad-/Tracker-Endpunkte fuer den Blocking-Test.
AD_PROBES = [
    "https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js",
    "https://securepubads.g.doubleclick.net/tag/js/gpt.js",
    "https://www.google-analytics.com/analytics.js",
    "https://connect.facebook.net/en_US/fbevents.js",
    "https://static.criteo.net/js/ld/ld.js",
    "https://cdn.taboola.com/libtrc/unip/1/tfa.js",
    "https://widgets.outbrain.com/outbrain.js",
    "https://sc-static.net/scevent.min.js",
    "https://static.ads-twitter.com/uwt.js",
    "https://analytics.tiktok.com/i18n/pixel/events.js",
    "https://cdn.onesignal.com/sdks/OneSignalSDK.js",
    "https://script.hotjar.com/modules.js",
]

PAGE = """<!doctype html>
<html lang="de"><head><meta charset="utf-8"><title>Aura Benchmark</title>
<style>body{font-family:system-ui;background:#0b0b0e;color:#eee;padding:40px;line-height:1.6}
h1{font-size:22px}code{color:#a78bfa}</style>
%LINKS%
</head><body>
<h1>Aura Benchmark läuft …</h1>
<p id="s">Messe Ladezeit, JS-Durchsatz und Ad-Blocking …</p>
<script>
const RUN = "%RUN%";
const t0 = performance.timeOrigin;
function beacon(path, data) {
  const body = JSON.stringify(data || {});
  navigator.sendBeacon(path + "?run=" + encodeURIComponent(RUN), body);
}
addEventListener('load', () => {
  const nav = performance.getEntriesByType('navigation')[0] || {};
  beacon('/loaded', {
    domContentLoaded: Math.round(nav.domContentLoadedEventEnd || 0),
    loadEvent: Math.round(nav.loadEventEnd || performance.now()),
    transferred: Math.round(performance.getEntriesByType('resource')
      .reduce((a, r) => a + (r.transferSize || 0), 0)),
    resources: performance.getEntriesByType('resource').length,
  });
  run();
});

// Fester JS-Workload: gleiche Arbeit in jedem Browser.
function jsWork() {
  const t = performance.now();
  let acc = 0;
  const arr = new Float64Array(200000);
  for (let pass = 0; pass < 12; pass++) {
    for (let i = 0; i < arr.length; i++) arr[i] = Math.sin(i * 0.001 + pass) * 1.0001;
    for (let i = 0; i < arr.length; i++) acc += arr[i] * arr[i];
  }
  let s = 0;
  for (let i = 0; i < 120000; i++) s += JSON.parse('{"a":' + i + '}').a;
  return {ms: Math.round(performance.now() - t), checksum: Math.round(acc + s)};
}

// Ad-Probe: laedt bekannte Werbe-/Tracking-Skripte. Geblockt => Fehler.
function probe(url) {
  return new Promise(res => {
    const s = document.createElement('script');
    let done = false;
    const finish = ok => { if (!done) { done = true; s.remove(); res(ok); } };
    s.onload = () => finish(true);
    s.onerror = () => finish(false);
    setTimeout(() => finish(false), 6000);
    s.src = url;
    document.head.appendChild(s);
  });
}

async function run() {
  const js = jsWork();
  const urls = %PROBES%;
  const results = await Promise.all(urls.map(probe));
  const blocked = results.filter(r => !r).length;
  document.querySelector('#s').textContent =
    `fertig – JS ${js.ms} ms, ${blocked}/${urls.length} Ad-Requests blockiert`;
  beacon('/report', {js: js.ms, blocked, probes: urls.length});
}
</script></body></html>
"""

# Kleine statische Assets, damit die Seite realistischer laedt.
ASSET_JS = "window.__a%d = %d;\n" * 60
ASSET_CSS = ".x%d{color:#%03x}\n" * 60


class State:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.first_hit: dict[str, float] = {}
        self.loaded: dict[str, dict] = {}
        self.report: dict[str, dict] = {}
        self.events: dict[str, threading.Event] = {}

    def event(self, run: str) -> threading.Event:
        with self.lock:
            return self.events.setdefault(run, threading.Event())


STATE = State()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):  # quiet
        pass

    def _send(self, body: bytes, ctype: str) -> None:
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:  # noqa: N802
        url = urllib.parse.urlparse(self.path)
        qs = urllib.parse.parse_qs(url.query)
        run = (qs.get("run") or [""])[0]

        if url.path == "/bench":
            with STATE.lock:
                STATE.first_hit.setdefault(run, time.perf_counter())
            links = "".join(
                f'<link rel="stylesheet" href="/asset/{i}.css">' if i % 3 == 0
                else f'<script src="/asset/{i}.js"></script>'
                for i in range(18)
            )
            page = (
                PAGE.replace("%RUN%", run)
                .replace("%PROBES%", json.dumps(AD_PROBES))
                .replace("%LINKS%", links)
            )
            self._send(page.encode(), "text/html; charset=utf-8")
            return

        if url.path.startswith("/asset/"):
            name = url.path.rsplit("/", 1)[-1]
            n = int(name.split(".")[0])
            if name.endswith(".css"):
                body = (ASSET_CSS % tuple(v for i in range(60) for v in (i + n, (i * 37) % 4096))).encode()
                self._send(body, "text/css")
            else:
                body = (ASSET_JS % tuple(v for i in range(60) for v in (i + n, i * n))).encode()
                self._send(body, "application/javascript")
            return

        self.send_response(404)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self) -> None:  # noqa: N802
        url = urllib.parse.urlparse(self.path)
        qs = urllib.parse.parse_qs(url.query)
        run = (qs.get("run") or [""])[0]
        length = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            data = json.loads(raw.decode() or "{}")
        except json.JSONDecodeError:
            data = {}
        now = time.perf_counter()
        with STATE.lock:
            if url.path == "/loaded":
                STATE.loaded[run] = {"t": now, **data}
            elif url.path == "/report":
                STATE.report[run] = {"t": now, **data}
        if url.path == "/report":
            STATE.event(run).set()
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


# ---------------------------------------------------------------- Prozess-Infos
def process_tree(pid: int) -> list[dict]:
    """Alle Prozesse im Baum unter pid (inkl. pid) mit Working Set."""
    ps = (
        "Get-CimInstance Win32_Process | "
        "Select-Object ProcessId,ParentProcessId,Name,WorkingSetSize | ConvertTo-Json -Compress"
    )
    out = subprocess.run(
        ["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
        capture_output=True, text=True, timeout=60,
    ).stdout
    try:
        procs = json.loads(out)
    except json.JSONDecodeError:
        return []
    if isinstance(procs, dict):
        procs = [procs]
    by_parent: dict[int, list[dict]] = {}
    by_pid: dict[int, dict] = {}
    for p in procs:
        by_pid[p["ProcessId"]] = p
        by_parent.setdefault(p["ParentProcessId"], []).append(p)
    seen, stack, out_list = set(), [pid], []
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        if cur in by_pid:
            out_list.append(by_pid[cur])
        for child in by_parent.get(cur, []):
            stack.append(child["ProcessId"])
    return out_list


def kill_tree(pid: int) -> None:
    subprocess.run(
        ["taskkill", "/PID", str(pid), "/T", "/F"],
        capture_output=True, text=True,
    )


# ---------------------------------------------------------------- Browser
def find(*paths: str) -> str | None:
    for p in paths:
        if p and Path(p).exists():
            return p
    return None


def browsers(tmp: Path) -> list[dict]:
    pf = os.environ.get("ProgramFiles", "")
    pf86 = os.environ.get("ProgramFiles(x86)", "")
    local = os.environ.get("LOCALAPPDATA", "")
    out: list[dict] = []

    aura = REPO / "target" / "release" / "aura-browser.exe"
    if aura.exists():
        out.append({
            "name": "Aura (Shield an)",
            "exe": str(aura),
            "args": lambda url: ["--profile=Bench", url],
            "profile": None,
        })
        out.append({
            "name": "Aura (Shield aus)",
            "exe": str(aura),
            "args": lambda url: ["--profile=BenchNoShield", url],
            "profile": None,
        })

    chrome = find(
        f"{pf}\\Google\\Chrome\\Application\\chrome.exe",
        f"{pf86}\\Google\\Chrome\\Application\\chrome.exe",
        f"{local}\\Google\\Chrome\\Application\\chrome.exe",
    )
    if chrome:
        d = tmp / "chrome"
        out.append({
            "name": "Chrome",
            "exe": chrome,
            "args": lambda url, d=d: [
                f"--user-data-dir={d}", "--no-first-run", "--no-default-browser-check",
                "--disable-background-networking", "--homepage=about:blank", url,
            ],
            "profile": d,
        })

    edge = find(
        f"{pf86}\\Microsoft\\Edge\\Application\\msedge.exe",
        f"{pf}\\Microsoft\\Edge\\Application\\msedge.exe",
    )
    if edge:
        d = tmp / "edge"
        out.append({
            "name": "Edge",
            "exe": edge,
            "args": lambda url, d=d: [
                f"--user-data-dir={d}", "--no-first-run", "--no-default-browser-check",
                "--disable-background-networking", url,
            ],
            "profile": d,
        })
    return out


def one_run(b: dict, run_id: str, settle: float = 4.0, timeout: float = 75.0) -> dict | None:
    url = f"http://{HOST}:{PORT}/bench?run={run_id}"
    ev = STATE.event(run_id)
    t0 = time.perf_counter()
    proc = subprocess.Popen([b["exe"], *b["args"](url)])
    ok = ev.wait(timeout)
    if not ok:
        kill_tree(proc.pid)
        return None

    with STATE.lock:
        first = STATE.first_hit.get(run_id)
        loaded = STATE.loaded.get(run_id, {})
        report = STATE.report.get(run_id, {})

    time.sleep(settle)
    tree = process_tree(proc.pid)
    rss = sum(p.get("WorkingSetSize") or 0 for p in tree)
    result = {
        "start_ms": round((first - t0) * 1000) if first else None,
        "load_ms": round((loaded.get("t", 0) - t0) * 1000) if loaded else None,
        "page_load_ms": loaded.get("loadEvent"),
        "dcl_ms": loaded.get("domContentLoaded"),
        "resources": loaded.get("resources"),
        "js_ms": report.get("js"),
        "blocked": report.get("blocked"),
        "probes": report.get("probes"),
        "rss_mb": round(rss / 1048576, 1),
        "procs": len(tree),
    }
    kill_tree(proc.pid)
    time.sleep(1.5)
    return result


def med(values: list) -> float | None:
    vals = [v for v in values if isinstance(v, (int, float))]
    return round(statistics.median(vals), 1) if vals else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=3)
    args = ap.parse_args()

    tmp = Path(os.environ["TEMP"]) / "aura_bench_profiles"
    shutil.rmtree(tmp, ignore_errors=True)
    tmp.mkdir(parents=True, exist_ok=True)

    # Aura-Benchprofile vorbereiten: Shield an bzw. aus, Filterlisten uebernehmen.
    appdata = Path(os.environ["LOCALAPPDATA"]) / "AuraBrowser"
    src_filters = appdata / "Test" / "filters"
    for name, shield in (("Bench", "1"), ("BenchNoShield", "0")):
        d = appdata / name
        shutil.rmtree(d, ignore_errors=True)
        d.mkdir(parents=True, exist_ok=True)
        if shield == "1" and src_filters.is_dir():
            shutil.copytree(src_filters, d / "filters")
        import sqlite3
        con = sqlite3.connect(d / "aura.db")
        con.execute("CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT)")
        con.executemany(
            "INSERT OR REPLACE INTO settings(key,value) VALUES(?,?)",
            [("shield", shield), ("shield_update", "0"), ("restore_session", "0")],
        )
        con.commit()
        con.close()

    srv = Server((HOST, PORT), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    print(f"Server auf http://{HOST}:{PORT}\n")

    all_browsers = browsers(tmp)
    if not all_browsers:
        print("Kein Browser gefunden.")
        return 1

    results: dict[str, list[dict]] = {}
    for b in all_browsers:
        print(f"== {b['name']}")
        runs = []
        for i in range(args.runs + 1):  # erster Lauf = Aufwaermen
            rid = f"{b['name']}-{i}-{int(time.time()*1000)}"
            r = one_run(b, rid)
            tag = "warm-up" if i == 0 else f"Lauf {i}"
            if r is None:
                print(f"   {tag}: Timeout")
                continue
            print(f"   {tag}: Start {r['start_ms']} ms · geladen {r['load_ms']} ms · "
                  f"{r['rss_mb']} MB / {r['procs']} Prozesse · JS {r['js_ms']} ms · "
                  f"blockiert {r['blocked']}/{r['probes']}")
            if i > 0:
                runs.append(r)
        results[b["name"]] = runs

    summary = {}
    for name, runs in results.items():
        if not runs:
            continue
        summary[name] = {
            "start_ms": med([r["start_ms"] for r in runs]),
            "load_ms": med([r["load_ms"] for r in runs]),
            "page_load_ms": med([r["page_load_ms"] for r in runs]),
            "js_ms": med([r["js_ms"] for r in runs]),
            "rss_mb": med([r["rss_mb"] for r in runs]),
            "procs": med([r["procs"] for r in runs]),
            "blocked": med([r["blocked"] for r in runs]),
            "probes": runs[0]["probes"],
        }

    print("\n" + "=" * 78)
    hdr = f"{'Browser':<20}{'Start':>9}{'Geladen':>10}{'RAM':>10}{'Proz.':>7}{'JS':>8}{'Ads blockiert':>16}"
    print(hdr)
    print("-" * 78)
    for name, s in summary.items():
        print(f"{name:<20}{s['start_ms']:>7.0f}ms{s['load_ms']:>8.0f}ms"
              f"{s['rss_mb']:>8.0f}MB{s['procs']:>7.0f}{s['js_ms']:>6.0f}ms"
              f"{str(int(s['blocked'])) + '/' + str(s['probes']):>16}")
    print("=" * 78)

    RESULTS.write_text(json.dumps({"summary": summary, "runs": results}, indent=2), encoding="utf-8")
    print(f"\nDetails: {RESULTS}")
    srv.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(main())
