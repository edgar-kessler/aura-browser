"""Stresstest: viele Tabs, RAM beobachten.

Legt eine Sitzung mit N Tabs an, startet Aura und misst den Arbeitsspeicher des
gesamten Prozessbaums – einmal direkt nach dem Start (Tabs liegen schlafend in
der Leiste) und danach, waehrend nacheinander Tabs aktiviert werden.

    python bench/stress_tabs.py [--tabs 120] [--activate 25]
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXE = REPO / "target" / "release" / "aura-browser.exe"
PROFILE = "Stress"

# Echte Seiten, damit die Renderer auch wirklich etwas zu tun bekommen.
SITES = [
    "https://www.heise.de/", "https://www.golem.de/", "https://news.ycombinator.com/",
    "https://www.rust-lang.org/", "https://developer.mozilla.org/de/",
    "https://github.com/", "https://stackoverflow.com/", "https://www.wikipedia.org/",
    "https://www.spiegel.de/", "https://www.zeit.de/", "https://www.tagesschau.de/",
    "https://www.reddit.com/", "https://crates.io/", "https://docs.rs/",
    "https://www.chip.de/", "https://www.computerbase.de/", "https://arstechnica.com/",
    "https://www.theverge.com/", "https://lwn.net/", "https://www.phoronix.com/",
]


def ps_tree(pid: int) -> list[dict]:
    ps = (
        "Get-CimInstance Win32_Process | "
        "Select-Object ProcessId,ParentProcessId,Name,WorkingSetSize | ConvertTo-Json -Compress"
    )
    out = subprocess.run(
        ["powershell", "-NoProfile", "-NonInteractive", "-Command", ps],
        capture_output=True, text=True, timeout=90,
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


def sample(pid: int) -> tuple[float, int]:
    tree = ps_tree(pid)
    rss = sum(p.get("WorkingSetSize") or 0 for p in tree)
    return round(rss / 1048576, 1), len(tree)


# --- Tastatureingaben an das Fenster (Ctrl+Tab) -----------------------------
user32 = ctypes.windll.user32
VK_CONTROL, VK_TAB = 0x11, 0x09
KEYEVENTF_KEYUP = 0x0002


def find_main_window(pid: int) -> int:
    found = []

    @ctypes.WINFUNCTYPE(ctypes.c_bool, ctypes.c_void_p, ctypes.c_void_p)
    def cb(hwnd, _lp):
        wpid = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(wpid))
        if wpid.value == pid and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(64)
            user32.GetClassNameW(hwnd, buf, 64)
            if buf.value == "AuraMainWindow":
                found.append(hwnd)
                return False
        return True

    user32.EnumWindows(cb, None)
    return found[0] if found else 0


def ctrl_tab(hwnd: int) -> None:
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.05)
    user32.keybd_event(VK_CONTROL, 0, 0, 0)
    user32.keybd_event(VK_TAB, 0, 0, 0)
    user32.keybd_event(VK_TAB, 0, KEYEVENTF_KEYUP, 0)
    user32.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--tabs", type=int, default=120)
    ap.add_argument("--activate", type=int, default=25)
    ap.add_argument("--settle", type=float, default=20.0)
    args = ap.parse_args()

    if not EXE.exists():
        print("Release-Build fehlt – erst `cargo build --release`.")
        return 1

    # Frisches Profil mit einer Sitzung aus N Tabs.
    prof = Path(os.environ["LOCALAPPDATA"]) / "AuraBrowser" / PROFILE
    shutil.rmtree(prof, ignore_errors=True)
    prof.mkdir(parents=True, exist_ok=True)
    con = sqlite3.connect(prof / "aura.db")
    con.execute("CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT)")
    con.executemany(
        "INSERT OR REPLACE INTO settings(key,value) VALUES(?,?)",
        [("restore_session", "1"), ("shield", "1"), ("shield_update", "0"),
         ("sleep_tabs", "0")],  # Schlafmodus aus, damit die Messung ehrlich bleibt
    )
    con.commit()
    con.close()

    tabs = [
        {"url": SITES[i % len(SITES)], "title": f"Tab {i + 1}", "pinned": False, "group": None}
        for i in range(args.tabs)
    ]
    (prof / "session.json").write_text(
        json.dumps({"tabs": tabs, "active": 0}, indent=1), encoding="utf-8"
    )

    print(f"Starte Aura mit {args.tabs} Tabs …")
    t0 = time.perf_counter()
    proc = subprocess.Popen([str(EXE), f"--profile={PROFILE}"])
    time.sleep(args.settle)
    rss, n = sample(proc.pid)
    print(f"  nach {args.settle:.0f}s: {rss} MB, {n} Prozesse  "
          f"(nur der sichtbare Tab hat einen Renderer)")

    hwnd = find_main_window(proc.pid)
    if not hwnd:
        print("  Fenster nicht gefunden – überspringe das Durchklicken.")
    else:
        print(f"Aktiviere {args.activate} Tabs mit Strg+Tab …")
        marks = []
        for i in range(args.activate):
            ctrl_tab(hwnd)
            time.sleep(1.2)
            if (i + 1) % 5 == 0:
                rss, n = sample(proc.pid)
                marks.append((i + 1, rss, n))
                print(f"  {i + 1:3d} Tabs aktiv: {rss} MB, {n} Prozesse")
        if marks:
            first, last = marks[0], marks[-1]
            per = (last[1] - first[1]) / max(1, last[0] - first[0])
            print(f"\n  ~{per:.1f} MB pro zusätzlich geladenem Tab")

    time.sleep(3)
    rss, n = sample(proc.pid)
    print(f"\nEnde: {rss} MB über {n} Prozesse, Laufzeit {time.perf_counter() - t0:.0f}s")
    subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
