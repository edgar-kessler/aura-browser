# Aura Browser

Ein nativer Windows-Browser in Rust — ohne Electron, ohne mitgeliefertes Chromium.
Die Oberfläche zeichnet Direct2D über DirectComposition, die Seiten rendert die
WebView2-Laufzeit, die Windows ohnehin mitbringt.

**3,5 MB Programmdatei.** Kein Installer nötig, kein Hintergrunddienst.

## Was drin ist

**Aura Shield** — eingebauter Werbe- und Trackerblocker mit eigener Engine.
Adblock-Plus-Syntax, Token-Index auf die seltensten Tokens, reine `||host^`-Regeln
in einem Hash-Set. Mit EasyList, EasyPrivacy, EasyList Germany, uBlock-Listen und
Peter Lowes Liste sind das **~122.000 Regeln bei 1,9 µs pro Anfrage**. Dazu
Kosmetikfilter, Ersatzobjekte für geblockte Werbe-SDKs (Seiten laufen weiter statt
„Adblocker deaktivieren" zu zeigen) und Popunder-Abwehr.

**Sicherheit** — Nur-HTTPS mit Ausnahmen pro Seite, DNT/Sec-GPC, ein strenger Modus
pro Seite (keine Drittanbieter, keine Downloads, keine Berechtigungen, Popups aus),
Cookies und Speicher pro Seite löschen. Die Engine läuft voll gesandboxed.

**Oberfläche** — Seitenleiste mit Tabs statt Tableiste, Liquid Glass über
Mica/Acryl, zeitbasierte Animationen, frei wählbare Akzentfarbe, hell/dunkel/System.

**Funktionen** — Leseliste, Passwortverwaltung (DPAPI), Task-Manager mit echten
Prozessdaten, Tab-Suche, Befehlspalette, Chrome-Import (Lesezeichen, Verlauf,
Passwörter über CSV), geteilte Ansicht, Bild-in-Bild, Schlafmodus für Tabs,
Sitzungswiederherstellung mit verzögertem Laden, Auto-Update über GitHub Releases.

## Zahlen

Gegen Chrome und Edge, je drei Läufe mit frischen Profilen auf dieselbe Testseite
([`bench/bench.py`](bench/bench.py)):

| Browser | Start | Geladen | RAM | Prozesse | Ads blockiert |
|---|---|---|---|---|---|
| **Aura (Shield an)** | 509 ms | 651 ms | **361 MB** | 10 | **12/12** |
| Aura (Shield aus) | 517 ms | 667 ms | 404 MB | 8 | 1/12 |
| Chrome | 443 ms | 508 ms | 619 MB | 11 | 1/12 |
| Edge | 421 ms | 559 ms | 642 MB | 15 | 11/12 |

Mit 150 wiederhergestellten Tabs: **471 MB**, weil nur der sichtbare Tab einen
Renderer startet ([`bench/stress_tabs.py`](bench/stress_tabs.py)).

## Bauen

Voraussetzungen: Rust (stable), die WebView2-Laufzeit (auf Windows 11 vorhanden).

```bash
cargo build --release
```

Mit der GNU-Toolchain muss `dlltool` einen Assembler finden — dafür gehört das
`bin`-Verzeichnis von MinGW in den `PATH`. `WebView2Loader.dll` kopiert
[`build.rs`](build.rs) automatisch neben die Programmdatei.

```bash
cargo test --release          # Filter-Engine und Versionsvergleich
python bench/bench.py         # Vergleich gegen installierte Browser
python bench/stress_tabs.py   # Speicher bei vielen Tabs
```

## Aufbau

| Datei | Inhalt |
|---|---|
| [`src/app.rs`](src/app.rs) | Fenster, Chrome-Rendering, Layout, Eingaben, Aktionen |
| [`src/adblock.rs`](src/adblock.rs) | Filter-Engine: Parser, Token-Index, Matching, Kosmetik |
| [`src/tabs.rs`](src/tabs.rs) | WebView2-Umgebung, Controller, Ereignisse |
| [`src/gfx.rs`](src/gfx.rs) | Direct2D, DirectComposition, Swapchain |
| [`src/pages.rs`](src/pages.rs) | Interne Seiten, Chrome-Import, Task-Manager |
| [`src/storage.rs`](src/storage.rs) | SQLite: Verlauf, Lesezeichen, Sitzung, Passwörter |
| [`src/update.rs`](src/update.rs) | Auto-Update über GitHub Releases |
| [`assets/`](assets) | Interne Seiten (`aura://…`), gemeinsames Stylesheet, Basis-Filterliste |

## Lizenz

MIT — siehe [LICENSE](LICENSE).

Die heruntergeladenen Filterlisten stehen unter ihren eigenen Lizenzen
(EasyList: CC BY-SA 3.0 / GPLv3, uBlock-Listen: GPLv3). Sie werden zur Laufzeit
geladen und sind nicht Teil dieses Repositorys; die mitgelieferte Basisliste
[`assets/filters/aura-base.txt`](assets/filters/aura-base.txt) ist eigenständig.
