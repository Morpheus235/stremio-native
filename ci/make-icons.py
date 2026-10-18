#!/usr/bin/env python3
"""Draw the app's launcher artwork: original, programmatic, no third-party logo.

    python3 ci/make-icons.py            regenerate every PNG under pkg/
    python3 ci/make-icons.py --check    exit 1 when a committed PNG differs from what this draws

The mark is a neutral one — a rounded play triangle on a deep blue-to-teal tile — so it says
"plays video" without borrowing any service's identity. Every size is drawn at 4x and reduced
with a Lanczos filter, so small icons are anti-aliased rather than stair-stepped.

  pkg/icon.png           80x80    appinfo `icon` (the launcher's small tile)
  pkg/largeIcon.png      130x130  appinfo `largeIcon`
  pkg/icon160.png        160x160  the Homebrew Channel listing (`ci/hbrepo.py`)
  pkg/icon320.png        320x320  release notes and the repository page
  pkg/splash.png         1920x1080 appinfo `splashBackground`
  pkg/beta/*.png         the four icons with an amber BETA band (the `beta` install's tile)
  pkg/debug/*.png        the four icons with a grey DEV band (the `debug` install's tile)

The flavoured sets keep the SAME basenames: the Makefile stages them over the stable ones, so the
flavour lives in the directory a file is read from, never in the name appinfo.json names
(`ci/flavor.py`). Needs Pillow; the PNGs are committed, so a build never runs this.
"""
from __future__ import annotations

import io
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
PKG = ROOT / "pkg"
SS = 4  # supersampling factor

TOP = (30, 22, 70)        # deep indigo (Stremio-like night violet)
BOTTOM = (123, 91, 245)   # Stremio-like violet accent
GLYPH = (246, 248, 250)
BADGES = {"beta": ("BETA", (242, 169, 0), (24, 20, 8)), "debug": ("DEV", (120, 128, 138), (250, 250, 250))}
ICONS = {"icon.png": 80, "largeIcon.png": 130, "icon160.png": 160, "icon320.png": 320}


def gradient(w: int, h: int) -> Image.Image:
    """A vertical gradient from TOP to BOTTOM."""
    col = Image.new("RGB", (1, h))
    for y in range(h):
        t = y / max(1, h - 1)
        col.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(TOP, BOTTOM)))
    return col.resize((w, h), Image.Resampling.NEAREST)


def play_triangle(draw: ImageDraw.ImageDraw, cx: float, cy: float, r: float, fill) -> None:
    """A play triangle centred on its centroid (so it looks centred), corners softened by a thick
    rounded outline in the same colour."""
    # an equilateral triangle pointing right, circumradius r, centroid at (cx, cy)
    pts = [(cx + r, cy), (cx - r / 2, cy - r * 0.866), (cx - r / 2, cy + r * 0.866)]
    draw.polygon(pts, fill=fill)
    draw.line(pts + [pts[0]], fill=fill, width=max(1, round(r * 0.18)), joint="curve")
    for x, y in pts:
        k = r * 0.09
        draw.ellipse((x - k, y - k, x + k, y + k), fill=fill)


def tile(size: int, badge: "str | None" = None) -> Image.Image:
    n = size * SS
    art = gradient(n, n).convert("RGBA")
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, n - 1, n - 1), radius=round(n * 0.22), fill=255)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    out.paste(art, (0, 0), mask)
    # a quiet ring behind the glyph, BLENDED over the tile (ImageDraw writes RGBA verbatim, so a
    # translucent outline drawn straight onto the tile would punch a see-through hole in it)
    ring = n * 0.34
    c = n / 2
    overlay = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    ImageDraw.Draw(overlay).ellipse((c - ring, c - ring, c + ring, c + ring), outline=(255, 255, 255, 72),
                                    width=max(1, round(n * 0.035)))
    out = Image.alpha_composite(out, overlay)
    d = ImageDraw.Draw(out)
    play_triangle(d, c, c, n * 0.18, GLYPH + (255,))  # centroid on the ring centre: optically centred
    if badge:
        word, bg, fg = BADGES[badge]
        band = round(n * 0.26)
        band_img = Image.new("RGBA", (n, band), bg + (255,))
        bmask = Image.new("L", (n, n), 0)
        ImageDraw.Draw(bmask).rectangle((0, n - band, n, n), fill=255)
        bmask = Image.composite(bmask, Image.new("L", (n, n), 0), mask)
        layer = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        layer.paste(band_img, (0, n - band))
        out = Image.composite(layer, out, bmask)
        d = ImageDraw.Draw(out)
        font = ImageFont.truetype(str(PKG / "appfont-bold.ttf"), round(band * 0.62))
        l, t, r, b = d.textbbox((0, 0), word, font=font)
        d.text(((n - (r - l)) / 2 - l, n - band + (band - (b - t)) / 2 - t), word, font=font, fill=fg + (255,))
    return out.resize((size, size), Image.Resampling.LANCZOS)


def splash() -> Image.Image:
    w, h = 1920, 1080
    img = Image.new("RGBA", (w, h), (6, 6, 6, 255))
    mark = tile(256)
    img.paste(mark, ((w - 256) // 2, (h - 256) // 2 - 40), mark)
    return img.convert("RGB")


def png(img: Image.Image) -> bytes:
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def artwork() -> "dict[Path, bytes]":
    out = {}
    for name, size in ICONS.items():
        out[PKG / name] = png(tile(size))
        for flav in BADGES:
            out[PKG / flav / name] = png(tile(size, flav))
    out[PKG / "splash.png"] = png(splash())
    return out


def main(argv: "list[str]") -> int:
    files = artwork()
    if argv == ["--check"]:
        stale = [p for p, b in files.items() if not p.is_file() or p.read_bytes() != b]
        for p in stale:
            print(f"stale: {p.relative_to(ROOT)} (run python3 ci/make-icons.py)")
        return 1 if stale else 0
    for p, b in files.items():
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(b)
        print(f"wrote {p.relative_to(ROOT)} ({len(b)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
