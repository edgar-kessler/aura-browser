"""Erzeugt assets/aura.ico – die violette Aura-Kugel in allen Groessen.

Reines Python, keine Abhaengigkeiten: die Bilder werden als PNG geschrieben
(Windows Vista und neuer versteht PNG-Eintraege in ICO-Dateien) und in einen
ICO-Container gepackt.

    python tools/make_icon.py
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "assets" / "aura.ico"
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

# Verlauf von hell innen nach Aura-Violett aussen, wie der Orb in der Leiste.
INNER = (198, 184, 245)
OUTER = (110, 91, 208)
RIM = (86, 68, 178)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def orb_pixels(size: int) -> bytes:
    """RGBA-Zeilen der Kugel, weich ausgeblendet und leicht angeleuchtet."""
    rows = bytearray()
    c = (size - 1) / 2.0
    radius = size / 2.0 - max(0.5, size * 0.035)
    # Lichtpunkt oben links, wie beim gezeichneten Orb.
    lx, ly = c - radius * 0.34, c - radius * 0.38
    for y in range(size):
        rows.append(0)  # PNG-Filter: None
        for x in range(size):
            dx, dy = x - c, y - c
            dist = math.hypot(dx, dy)
            # Kante ueber einen Pixel weich auslaufen lassen (Anti-Aliasing).
            edge = radius - dist
            alpha = max(0.0, min(1.0, edge / max(1.0, size * 0.045)))
            if alpha <= 0.0:
                rows.extend((0, 0, 0, 0))
                continue
            t = min(1.0, dist / radius)
            # Abstand zum Lichtpunkt hellt zusaetzlich auf.
            hl = max(0.0, 1.0 - math.hypot(x - lx, y - ly) / (radius * 1.5))
            mix = min(1.0, t ** 0.85)
            r = lerp(INNER[0], OUTER[0], mix) + hl * 34
            g = lerp(INNER[1], OUTER[1], mix) + hl * 34
            b = lerp(INNER[2], OUTER[2], mix) + hl * 20
            # Aeusserster Ring etwas dunkler, damit die Kugel Kontur bekommt.
            if t > 0.86:
                k = (t - 0.86) / 0.14
                r = lerp(r, RIM[0], k * 0.7)
                g = lerp(g, RIM[1], k * 0.7)
                b = lerp(b, RIM[2], k * 0.7)
            rows.extend((
                int(max(0, min(255, r))),
                int(max(0, min(255, g))),
                int(max(0, min(255, b))),
                int(alpha * 255),
            ))
    return bytes(rows)


def png(size: int) -> bytes:
    def chunk(tag: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8 bit, RGBA
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(orb_pixels(size), 9))
            + chunk(b"IEND", b""))


def bmp(size: int) -> bytes:
    """Klassischer ICO-Eintrag: BITMAPINFOHEADER, BGRA von unten nach oben.

    Aeltere Programme – und .NETs System.Drawing – koennen mit PNG-Eintraegen
    nichts anfangen. Die kleinen Groessen liegen deshalb als BMP vor.
    """
    src = orb_pixels(size)
    stride = 1 + size * 4  # Filterbyte je Zeile
    pixels = bytearray()
    for y in range(size - 1, -1, -1):  # BMP steht auf dem Kopf
        row = src[y * stride + 1:(y + 1) * stride]
        for x in range(size):
            r, g, b, a = row[x * 4:x * 4 + 4]
            pixels.extend((b, g, r, a))
    # AND-Maske: alles sichtbar, Zeilen auf 4 Byte aufgefuellt
    mask_row = (size + 31) // 32 * 4
    mask = bytes(mask_row * size)
    header = struct.pack(
        "<IiiHHIIiiII",
        40, size, size * 2, 1, 32, 0, len(pixels) + len(mask), 0, 0, 0, 0,
    )
    return header + bytes(pixels) + mask


def main() -> None:
    # Kleine Groessen als BMP, grosse als PNG – so macht es jedes Icon-Werkzeug.
    images = [(s, bmp(s) if s <= 64 else png(s)) for s in SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = bytearray(), bytearray()
    for size, data in images:
        entries += struct.pack(
            "<BBBBHHII",
            0 if size >= 256 else size,   # Breite (0 bedeutet 256)
            0 if size >= 256 else size,   # Hoehe
            0, 0, 1, 32, len(data), offset,
        )
        blobs += data
        offset += len(data)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(header + bytes(entries) + bytes(blobs))
    print(f"{OUT}  ({OUT.stat().st_size} Bytes, {len(images)} Groessen)")


if __name__ == "__main__":
    main()
