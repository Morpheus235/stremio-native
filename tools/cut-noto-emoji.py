#!/usr/bin/env python3
"""Cut pkg/appfont-emoji.ttf, the monochrome emoji face of the UI chain and the subtitle font set.

Input: Noto Emoji Regular (OFL-1.1), the static wght-400 instance Google Fonts serves:
    https://fonts.gstatic.com/s/notoemoji/v65/bMrnmSyK7YY-MEu6aWjPDs-ar6uWaGWuob-r0jwv.ttf
pinned below by sha256. Monochrome because no television can draw colour: every webOS release
ships SDL2_ttf 2.0.10 or 2.0.14 (`tools/fwcompat.py --inventory libSDL2_ttf` in the PlxNative
tree; 6.4.0 is 2.0.14), which renders FreeType outlines as 8-bit coverage and has no colour path
(no FT_LOAD_COLOR, no CBDT/sbix strike selection, no COLR). A monochrome face draws in the text
colour through the same path as every other link.

What the cut does, with nothing but the standard library and HarfBuzz's `hb-subset`:
  * keeps only the pictographs: every codepoint of the source cmap that `sn_base::fontcov`'s
    EMOJI_PARTS table classifies as an emoji part (read from crates/base/src/fontcov.rs, the one
    copy), minus the sequence marks the text path strips before drawing (ZWJ, VS15/VS16, the keycap
    mark, skin-tone modifiers, regional indicators, tags), minus anything Inter Bold carries. So no
    ASCII, no digit, no '#'/'*' and nothing Inter draws (©, ®, ™, arrows, ❤, ⚠ ...) is in it.
  * drops GSUB/GPOS/GDEF (SDL_ttf 2.0.x has no shaper; the flag and ZWJ ligature glyphs go with
    them) and the vertical metrics.
  * adds the subtitle aliases the Noto subtitle faces carry (family/full name "PlxSubs" under
    Windows langID 0x0407 and "Inter" under 0x040C), so libass — which finds faces by family name
    only — reaches this face for a codepoint no other face has.
  * sets usWinAscent/usWinDescent so libass draws the emoji em at EMOJI_SCALE of Inter Bold's em,
    split at Inter Bold's ascent (libass sizes a face by its win total, see
    crates/media/src/player/ass/fonts/tests.rs). hhea is untouched, which is what SDL_ttf reads.

Usage: tools/cut-noto-emoji.py SRC.ttf [OUT.ttf]   (OUT defaults to pkg/appfont-emoji.ttf)
"""

import hashlib
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SRC_SHA256 = "988621dc5c9a75eb6144f28faae30317a8e3421b68b28740747b3d739e2326b8"
# The emoji em against Inter's em: text.rs's EMOJI_SCALE, which this must equal.
EMOJI_SCALE = 0.8
# ZWJ, VS15/VS16, keycap, regional indicators, skin tones, tags: stripped before drawing.
MARKS = [(0x200D, 0x200D), (0xFE0E, 0xFE0F), (0x20E3, 0x20E3), (0x1F1E6, 0x1F1FF),
         (0x1F3FB, 0x1F3FF), (0xE0020, 0xE007F)]


def tables(b):
    n = struct.unpack(">H", b[4:6])[0]
    out = {}
    for i in range(n):
        tag, _, off, ln = struct.unpack(">4sIII", b[12 + 16 * i:28 + 16 * i])
        out[tag.decode("latin-1")] = b[off:off + ln]
    return out


def cmap(t):
    """Codepoints a cmap maps to a real glyph (formats 4 and 12, Unicode subtables)."""
    out = set()
    for i in range(struct.unpack(">H", t[2:4])[0]):
        plat, enc, off = struct.unpack(">HHI", t[4 + 8 * i:12 + 8 * i])
        if not (plat == 0 or (plat == 3 and enc in (1, 10))):
            continue
        s = t[off:]
        fmt = struct.unpack(">H", s[:2])[0]
        if fmt == 12:
            for g in range(struct.unpack(">I", s[12:16])[0]):
                lo, hi, gid = struct.unpack(">III", s[16 + 12 * g:28 + 12 * g])
                out.update(cp for cp in range(lo, hi + 1) if gid + cp - lo)
        elif fmt == 4:
            seg2 = struct.unpack(">H", s[6:8])[0]
            for k in range(seg2 // 2):
                end, = struct.unpack(">H", s[14 + 2 * k:16 + 2 * k])
                start, = struct.unpack(">H", s[16 + seg2 + 2 * k:18 + seg2 + 2 * k])
                delta, = struct.unpack(">H", s[16 + 2 * seg2 + 2 * k:18 + 2 * seg2 + 2 * k])
                ro_at = 16 + 3 * seg2 + 2 * k
                ro, = struct.unpack(">H", s[ro_at:ro_at + 2])
                for cp in range(start, min(end, 0xFFFE) + 1):
                    if ro == 0:
                        g = (cp + delta) & 0xFFFF
                    else:
                        a = ro_at + ro + 2 * (cp - start)
                        g, = struct.unpack(">H", s[a:a + 2])
                        g = (g + delta) & 0xFFFF if g else 0
                    if g:
                        out.add(cp)
    return out


def emoji_parts():
    src = (REPO / "crates/base/src/fontcov.rs").read_text()
    body = src[src.index("const EMOJI_PARTS"):]
    body = body[:body.index("];")]
    return [(int(a, 16), int(b, 16)) for a, b in re.findall(r"\(0x([0-9A-F]+), 0x([0-9A-F]+)\)", body)]


def within(cp, ranges):
    return any(lo <= cp <= hi for lo, hi in ranges)


def add_aliases(name):
    """The name table with "PlxSubs" (0x407) and "Inter" (0x40C) as family (1) and full name (4)."""
    fmt, count, strings = struct.unpack(">HHH", name[:6])
    assert fmt == 0, "name table format 1 is not handled"
    recs = [struct.unpack(">HHHHHH", name[6 + 12 * i:18 + 12 * i]) for i in range(count)]
    pool = bytearray(name[strings:])
    for lang, text in ((0x407, "PlxSubs"), (0x40C, "Inter")):
        data = text.encode("utf-16-be")
        for nid in (1, 4):
            recs.append((3, 1, lang, nid, len(data), len(pool)))
        pool += data
    recs.sort()
    head = struct.pack(">HHH", 0, len(recs), 6 + 12 * len(recs))
    return head + b"".join(struct.pack(">HHHHHH", *r) for r in recs) + bytes(pool)


def checksum(b):
    b = b + b"\0" * (-len(b) % 4)
    return sum(struct.unpack(">%dI" % (len(b) // 4), b)) & 0xFFFFFFFF


def build(tabs):
    """An sfnt from {tag: bytes}, with table checksums and head.checkSumAdjustment."""
    tags = sorted(tabs)
    n = len(tags)
    es = max(i for i in range(16) if (1 << i) <= n)
    out = bytearray(struct.pack(">IHHHH", 0x00010000, n, 16 << es, es, n * 16 - (16 << es)))
    off = 12 + 16 * n
    body = bytearray()
    head = bytearray(tabs["head"])
    head[8:12] = b"\0\0\0\0"
    tabs = dict(tabs, head=bytes(head))
    head_at = 0
    for t in tags:
        data = tabs[t]
        if t == "head":
            head_at = off + len(body)
        out += struct.pack(">4sIII", t.encode("latin-1"), checksum(data), off + len(body), len(data))
        body += data + b"\0" * (-len(data) % 4)
    font = out + body
    adj = (0xB1B0AFBA - checksum(bytes(font))) & 0xFFFFFFFF
    font[head_at + 8:head_at + 12] = struct.pack(">I", adj)
    return bytes(font)


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    src = Path(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) == 3 else REPO / "pkg/appfont-emoji.ttf"
    raw = src.read_bytes()
    if hashlib.sha256(raw).hexdigest() != SRC_SHA256:
        sys.exit(f"{src}: sha256 is not the pinned Noto Emoji Regular ({SRC_SHA256})")
    inter = cmap(tables((REPO / "pkg/appfont-bold.ttf").read_bytes())["cmap"])
    parts = emoji_parts()
    keep = sorted(cp for cp in cmap(tables(raw)["cmap"])
                  if cp >= 0x80 and within(cp, parts) and not within(cp, MARKS) and cp not in inter)
    with tempfile.TemporaryDirectory() as tmp:
        sub = Path(tmp) / "sub.ttf"
        subprocess.run(["hb-subset", f"--font-file={src}", f"--output-file={sub}",
                        "--unicodes=" + ",".join(f"{cp:X}" for cp in keep),
                        "--drop-tables+=GSUB,GPOS,GDEF,vhea,vmtx,STAT", "--name-IDs=*"], check=True)
        tabs = tables(sub.read_bytes())
    inter_os2 = tables((REPO / "pkg/appfont-bold.ttf").read_bytes())["OS/2"]
    ia, idsc = struct.unpack(">HH", inter_os2[74:78])
    total = (ia + idsc) / EMOJI_SCALE  # same upem (2048) as Inter, checked below
    asc = round(total * ia / (ia + idsc))
    os2 = bytearray(tabs["OS/2"])
    os2[74:78] = struct.pack(">HH", asc, round(total) - asc)
    assert struct.unpack(">H", tabs["head"][18:20])[0] == 2048
    tabs["OS/2"] = bytes(os2)
    tabs["name"] = add_aliases(tabs["name"])
    font = build(tabs)
    out.write_bytes(font)
    print(f"{out}: {len(keep)} codepoints, {len(font)} bytes, sha256 {hashlib.sha256(font).hexdigest()}")


if __name__ == "__main__":
    main()
