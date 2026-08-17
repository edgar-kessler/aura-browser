#!/usr/bin/env python3
"""Fernsteuerung fuer Aura Browser - wie Playwright, nur fuer unseren Browser.

Der Browser muss mit --debug-port=PORT laufen (oder die Einstellung
debug_server gesetzt haben). Port und Token stehen danach in
%LOCALAPPDATA%\\AuraBrowser\\<profil>\\devcontrol.json und werden hier automatisch
gefunden.

    python tools/aura_ctl.py state
    python tools/aura_ctl.py shot bild.png
    python tools/aura_ctl.py go https://stripe.com
    python tools/aura_ctl.py tab new https://github.com
    python tools/aura_ctl.py click 40 120
    python tools/aura_ctl.py key 84 --ctrl              # Strg+T
    python tools/aura_ctl.py eval "document.title"
    python tools/aura_ctl.py cdp Page.getLayoutMetrics
    python tools/aura_ctl.py window foreground

Als Modul:

    from aura_ctl import Aura
    a = Aura()
    a.navigate("https://stripe.com")
    a.screenshot("stripe.png")
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import sys
import time
import urllib.error
import urllib.request


def find_control_file() -> str:
    base = os.path.join(os.environ.get("LOCALAPPDATA", ""), "AuraBrowser")
    hits = glob.glob(os.path.join(base, "*", "devcontrol.json"))
    if not hits:
        raise SystemExit(
            "devcontrol.json nicht gefunden - laeuft Aura mit --debug-port=PORT?"
        )
    return max(hits, key=os.path.getmtime)


class Aura:
    def __init__(self, port: int | None = None, token: str | None = None):
        if port is None or token is None:
            info = json.load(open(find_control_file(), encoding="utf-8"))
            port = port or info["port"]
            token = token or info["token"]
        self.base = f"http://127.0.0.1:{port}"
        self.token = token

    def _call(self, path: str, payload: dict | None = None, raw: bool = False):
        data = json.dumps(payload or {}).encode()
        req = urllib.request.Request(
            self.base + path,
            data=data,
            headers={"X-Aura-Token": self.token, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                body = r.read()
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{path}: {e.code} {e.read().decode(errors='replace')}")
        return body if raw else json.loads(body.decode())

    # --- Zustand ---------------------------------------------------------
    def state(self) -> dict:
        return self._call("/state")

    def tabs(self) -> list:
        return self.state()["tabs"]

    # --- Fenster ---------------------------------------------------------
    def window(self, action: str, **kw) -> dict:
        return self._call("/window", {"action": action, **kw})

    def foreground(self) -> dict:
        return self.window("foreground")

    def resize(self, width: int, height: int) -> dict:
        return self.window("size", width=width, height=height)

    # --- Navigation ------------------------------------------------------
    def navigate(self, url: str, tab: int | None = None) -> dict:
        p = {"url": url}
        if tab is not None:
            p["tab"] = tab
        return self._call("/navigate", p)

    def new_tab(self, url: str | None = None) -> dict:
        return self._call("/tab", {"action": "new", **({"url": url} if url else {})})

    def close_tab(self, index: int | None = None) -> dict:
        p = {"action": "close"}
        if index is not None:
            p["index"] = index
        return self._call("/tab", p)

    def activate(self, index: int) -> dict:
        return self._call("/tab", {"action": "activate", "index": index})

    # --- Eingabe ---------------------------------------------------------
    def click(self, x: int, y: int) -> dict:
        """Klick auf die Aura-Oberflaeche (Sidebar, Omnibox, Schild ...)."""
        return self._call("/click", {"x": x, "y": y})

    def key(self, vk: int, ctrl=False, shift=False, alt=False) -> dict:
        return self._call("/key", {"vk": vk, "ctrl": ctrl, "shift": shift, "alt": alt})

    def mouse(self, action: str, x: int = 0, y: int = 0) -> dict:
        """Einzelner Maus-Schritt: down, move oder up."""
        return self._call("/mouse", {"action": action, "x": x, "y": y})

    def drag(self, x0: int, y0: int, x1: int, y1: int, steps: int = 12) -> dict:
        """Zieht von (x0,y0) nach (x1,y1) - z. B. die Kante der Seitenleiste."""
        self.mouse("move", x0, y0)
        self.mouse("down", x0, y0)
        for i in range(1, steps + 1):
            self.mouse("move", x0 + (x1 - x0) * i // steps, y0 + (y1 - y0) * i // steps)
        return self.mouse("up", x1, y1)

    # --- Seite -----------------------------------------------------------
    def eval(self, js: str):
        return self._call("/eval", {"js": js}).get("result")

    def text(self) -> str:
        return self.eval("document.body.innerText") or ""

    def title(self) -> str:
        return self.eval("document.title") or ""

    def cdp(self, method: str, params: dict | None = None) -> dict:
        return self._call("/cdp", {"method": method, "params": params or {}})

    def click_page(self, selector: str) -> bool:
        """Klickt ein Element in der Seite ueber seinen CSS-Selektor."""
        js = (
            "(()=>{const e=document.querySelector(%s);"
            "if(!e)return false;e.click();return true})()" % json.dumps(selector)
        )
        return bool(self.eval(js))

    def wait_for(self, selector: str, timeout: float = 10.0) -> bool:
        end = time.time() + timeout
        js = "!!document.querySelector(%s)" % json.dumps(selector)
        while time.time() < end:
            if self.eval(js) is True:
                return True
            time.sleep(0.25)
        return False

    def screenshot(self, path: str = "aura.png", full: bool = False) -> str:
        """full=True nimmt das ganze Fenster auf, sonst nur die Seite."""
        png = self._call("/screenshot", {"full": full}, raw=True)
        with open(path, "wb") as f:
            f.write(png)
        return os.path.abspath(path)


def main() -> int:
    ap = argparse.ArgumentParser(description="Aura Browser fernsteuern")
    ap.add_argument("--port", type=int)
    ap.add_argument("--token")
    sub = ap.add_subparsers(dest="cmd", required=True)

    sub.add_parser("state")
    p = sub.add_parser("shot")
    p.add_argument("path", nargs="?", default="aura.png")
    p.add_argument("--full", action="store_true", help="ganzes Fenster statt nur der Seite")
    p = sub.add_parser("go"); p.add_argument("url")
    p = sub.add_parser("tab")
    p.add_argument("action", choices=["new", "close", "activate"])
    p.add_argument("value", nargs="?")
    p = sub.add_parser("click"); p.add_argument("x", type=int); p.add_argument("y", type=int)
    p = sub.add_parser("key")
    p.add_argument("vk", type=int)
    p.add_argument("--ctrl", action="store_true")
    p.add_argument("--shift", action="store_true")
    p.add_argument("--alt", action="store_true")
    p = sub.add_parser("eval"); p.add_argument("js")
    p = sub.add_parser("cdp"); p.add_argument("method"); p.add_argument("params", nargs="?", default="{}")
    p = sub.add_parser("window")
    p.add_argument("action", choices=["restore", "minimize", "maximize", "foreground", "size"])
    p.add_argument("--width", type=int, default=1440)
    p.add_argument("--height", type=int, default=900)

    a = ap.parse_args()
    aura = Aura(a.port, a.token)

    if a.cmd == "state":
        print(json.dumps(aura.state(), indent=2, ensure_ascii=False))
    elif a.cmd == "shot":
        print(aura.screenshot(a.path, a.full))
    elif a.cmd == "go":
        print(json.dumps(aura.navigate(a.url)))
    elif a.cmd == "tab":
        if a.action == "new":
            print(json.dumps(aura.new_tab(a.value)))
        elif a.action == "close":
            print(json.dumps(aura.close_tab(int(a.value)) if a.value else aura.close_tab()))
        else:
            print(json.dumps(aura.activate(int(a.value or 0))))
    elif a.cmd == "click":
        print(json.dumps(aura.click(a.x, a.y)))
    elif a.cmd == "key":
        print(json.dumps(aura.key(a.vk, a.ctrl, a.shift, a.alt)))
    elif a.cmd == "eval":
        print(json.dumps(aura.eval(a.js), ensure_ascii=False))
    elif a.cmd == "cdp":
        print(json.dumps(aura.cdp(a.method, json.loads(a.params)), ensure_ascii=False))
    elif a.cmd == "window":
        print(json.dumps(aura.window(a.action, width=a.width, height=a.height)["window"]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
