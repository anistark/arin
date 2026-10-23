#!/usr/bin/env python3
"""Build the data Arin writes other scripts in by hand.

Two kinds. Stroke packs are the strokes of a set of characters, in the order a person
writes them, from an open source that recorded that order. Script fonts are OFL handwriting
fonts for the scripts no such source exists for, which Arin writes the way it writes a font
the person installed: in the font's own shapes, revealed in a derived order.

Run through `just handwriting-data`, which writes the packs to `assets/strokes` and the fonts
to `assets/fonts/scripts`, each beside its licence. Both are committed, like the app icon,
so that building Arin never reaches the network.

Every source is pinned to a commit or release, so running this twice writes the same bytes.

Pack format, little endian:

    b"ARINSTRK"  u16 version  u16 flags
    f32 ascent  f32 descent  f32 origin_x  f32 origin_y  f32 span
    u32 notice length, then the notice as UTF-8: where the strokes came from, under what
        licence, and how and when they were changed, which the Arphic licence asks every
        modified file to say
    u32 count
    count x (u32 codepoint, u32 offset into the glyph data)    sorted by codepoint
    glyph data: per glyph, u8 advance, u8 stroke count, then per stroke u8 point count
                and that many (u8 x, u8 y)

Flag 1 says the stroke order is a person's and must be kept as it is. Without it the order
came from a plotter, and Arin tidies it the way it tidies a single-line font's.

Coordinates are in ems with y growing downward from the baseline, quantised as
origin + q / 255 * span. An advance is q / 255 * span.
"""

import gzip
import json
import math
import re
import struct
import sys
import tempfile
import urllib.request
import xml.etree.ElementTree as ElementTree
from pathlib import Path

KICAD = "0cea285df6f6742b9ba5b13b706436963f15d1b9"
NEWSTROKE = f"https://gitlab.com/kicad/code/kicad/-/raw/{KICAD}/tools/newstroke"
KANJIVG = "https://github.com/KanjiVG/kanjivg/releases/download/r20260714/kanjivg-20260714.xml.gz"
MAKEMEAHANZI = "https://raw.githubusercontent.com/skishore/makemeahanzi/bddc96d41bef78427ed0e034e9f7e31d71fd1b92"
ANIMCJK = "https://raw.githubusercontent.com/parsimonhi/animCJK/3e1253b341cc7be14c3540bb3c045e6163557777"

GOOGLE_FONTS = "https://raw.githubusercontent.com/google/fonts/e7db6ca603fd4862010383ff17ffd6cc4ffac1c0/ofl"

# A handwriting font for each script with no open stroke data, all OFL, from Google Fonts.
# Chosen for looking written by hand and for covering the script. Aref Ruqaa is Ruqaa, the
# style Arabic is written in day to day.
SCRIPT_FONTS = [
    ("arefruqaa", "ArefRuqaa-Regular.ttf", "Arabic"),
    ("playpensanshebrew", "PlaypenSansHebrew[wght].ttf", "Hebrew"),
    ("kalam", "Kalam-Regular.ttf", "Devanagari"),
    ("atma", "Atma-Regular.ttf", "Bengali"),
    ("arima", "Arima[wght].ttf", "Tamil and Malayalam"),
    ("gurajada", "Gurajada-Regular.ttf", "Telugu"),
    ("itim", "Itim-Regular.ttf", "Thai"),
]

# When the conversion below last changed, for the notice each pack carries. Not the date it
# was run, which would make two runs of the same sources write different bytes.
CONVERTED = "2026-10-23"

CHANGES = (
    f"Converted on {CONVERTED} by packaging/strokes/build.py in Arin "
    "(https://github.com/anistark/arin): each stroke's centre line was flattened to a polyline, "
    "simplified, and quantised to 8 bits per coordinate."
)

NOTICES = {
    "newstroke": (
        "Strokes from KiCad's newstroke font by Vladimir Uryvaev, "
        f"https://gitlab.com/kicad/code/kicad/-/tree/{KICAD}/tools/newstroke, released under CC0."
    ),
    "kanjivg": (
        "Strokes from KanjiVG, copyright Ulrich Apel, http://kanjivg.tagaini.net, release r20260714. "
        "This adaptation is distributed under the Creative Commons Attribution-Share Alike 3.0 "
        "licence, http://creativecommons.org/licenses/by-sa/3.0/."
    ),
    "makemeahanzi": (
        "Stroke medians from Make Me a Hanzi graphics.txt, https://github.com/skishore/makemeahanzi "
        "at bddc96d4, derived from Arphic PL KaitiM GB and Arphic PL UKai. Distributed under the "
        "Arphic Public License, in ARPHICPL.TXT beside this file."
    ),
    "animcjk": (
        "Stroke medians from AnimCJK graphicsZhHant.txt, https://github.com/parsimonhi/animCJK "
        "at 3e1253b3, derived from Arphic fonts. Distributed under the Arphic Public License, in "
        "ARPHICPL.TXT beside this file."
    ),
}

# The box CJK glyphs are drawn in, from the top of the em to the bottom, and where the
# baseline sits in it.
CJK_BASELINE = 0.88

# Newstroke's units: a capital is 21 tall, so a 30 unit em puts it at 0.7 of one, which is
# where EMS Tech's capitals sit.
NEWSTROKE_EM = 30.0

ORIGIN = (-0.3, -1.05)
SPAN = 1.5


def fetch(url, into):
    path = Path(into) / re.sub(r"[^A-Za-z0-9._-]", "_", url.split("://", 1)[-1])
    if not path.exists():
        print(f"  fetching {url}", file=sys.stderr)
        with urllib.request.urlopen(url) as response:
            path.write_bytes(response.read())
    return path


# newstroke

TRANSFORMS = {
    "!": (-1, 1, 0),
    "-": (1, -1, -14),
    "=": (1, -1, -21),
    "~": (1, -1, -16),
    "+": (-1, -1, -14),
    "%": (-1, -1, -21),
    "*": (-1, -1, -16),
    "^": (1, 1, -13),
    "`": (-1, 1, -13),
    ".": (1, 1, 6),
    ",": (-1, 1, 6),
}


def sexp(text):
    tokens = re.findall(r'\(|\)|"(?:[^"\\]|\\.)*"|[^\s()]+', text)
    stack = [[]]
    for token in tokens:
        if token == "(":
            stack.append([])
        elif token == ")":
            done = stack.pop()
            stack[-1].append(done)
        else:
            stack[-1].append(token.strip('"'))
    return stack[0]


def units(mm):
    return round(float(mm) / 0.0254 - 0.1) // 50


def newstroke_glyphs(path):
    glyphs = {}
    for item in sexp(path.read_text(encoding="utf-8"))[0]:
        if not isinstance(item, list) or item[0] != "symbol":
            continue
        name = re.sub(r"__.*$", "", item[1])
        anchors, strokes = {"-": (0, 0)}, []
        for unit in item[2:]:
            if not (isinstance(unit, list) and unit[0] == "symbol"):
                continue
            for part in unit[1:]:
                if part[0] == "polyline":
                    pts = next(p for p in part[1:] if p[0] == "pts")
                    strokes.append([(units(x), units(-float(y))) for _, x, y in pts[1:]])
                elif part[0] == "pin":
                    at = next(p for p in part[1:] if p[0] == "at")
                    point = (units(at[1]), units(-float(at[2])))
                    pin_name = next(p for p in part[1:] if p[0] == "name")[1]
                    if pin_name in ("~", ""):
                        pin_name = "P" if point[0] <= 0 else "S"
                    anchors[pin_name] = point
        left, right = anchors.get("P", (0, 0))[0], anchors.get("S", (0, 0))[0]
        glyphs[name] = {"strokes": strokes, "anchors": anchors, "metrics": (left, right)}
    return glyphs


def newstroke_codepoints(charlist, glyphs):
    """Compose every code point the character list describes, as newstroke itself does."""
    compositions = []
    missing = object()

    def lookup(name):
        transform = TRANSFORMS.get(name[0], (1, 1, 0))
        if name[0] in TRANSFORMS:
            name = name[1:]
        return glyphs.get(name, missing), transform

    def placed(glyph, transform, offset):
        sx, sy, oy = transform
        return [[(x * sx + offset[0], y * sy + oy + offset[1]) for x, y in s] for s in glyph["strokes"]]

    def metrics(glyph, transform, offset):
        a = glyph["metrics"][0] * transform[0] + offset[0]
        b = glyph["metrics"][1] * transform[0] + offset[0]
        return min(a, b), max(a, b)

    skip = (0, 0)
    for raw in charlist.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0]
        tokens = line.split()
        if not tokens:
            continue
        command, rest = tokens[0], tokens[1:]
        if command == "startchar":
            compositions = [None] * int(rest[0])
        elif command == "skipcodes":
            compositions.extend([None] * int(rest[0]))
            skip = (0, 0)
        elif command in ("+", "+w", "+p", "+(", "+|", "+)"):
            if command not in ("+|", "+)"):
                compositions.append({"strokes": [], "metrics": None, "real": True})
                skip = (0, 0)
            current = compositions[-1]
            base, base_transform = lookup(rest[0])
            if base is missing:
                current["real"] = False
                continue
            if skip != (0, 0):
                skip = (skip[0] - base["metrics"][0], 0)
            current["strokes"] += placed(base, base_transform, skip)
            current["metrics"] = merge(current["metrics"], metrics(base, base_transform, skip))
            names = rest[1:]
            while names:
                name = names.pop(0)
                sub, transform = lookup(name)
                if sub is missing:
                    current["real"] = False
                    break
                offset = skip
                if names:
                    parts = names.pop(0).split("=")
                    if len(parts) == 2:
                        sx, sy, oy = base_transform
                        a = base["anchors"].get(parts[0], (0, 0))
                        b = sub["anchors"].get(parts[1], (0, 0))
                        a = (a[0] * sx, a[1] * sy + oy)
                        b = (b[0] * transform[0], b[1] * transform[1] + transform[2])
                        offset = (offset[0] + a[0] - b[0], offset[1] + a[1] - b[1])
                current["strokes"] += placed(sub, transform, offset)
                if command in ("+w", "+p"):
                    current["metrics"] = merge(current["metrics"], metrics(sub, transform, offset))
            skip = (skip[0] + base["metrics"][1], 0)
    out = {}
    for codepoint, composition in enumerate(compositions):
        if not composition or not composition["real"] or composition["metrics"] is None:
            continue
        left, right = composition["metrics"]
        strokes = [
            [((x - left) / NEWSTROKE_EM, y / NEWSTROKE_EM) for x, y in stroke]
            for stroke in composition["strokes"]
            if len(stroke) >= 1
        ]
        out[codepoint] = ((right - left) / NEWSTROKE_EM, strokes)
    return out


def merge(a, b):
    if a is None or a[0] == a[1]:
        return b
    if b[0] == b[1]:
        return a
    return min(a[0], b[0]), max(a[1], b[1])


def newstroke(cache):
    glyphs = {}
    for name in ("symbol", "font", "hiragana", "katakana"):
        glyphs.update(newstroke_glyphs(fetch(f"{NEWSTROKE}/{name}.kicad_sym", cache)))
    return newstroke_codepoints(fetch(f"{NEWSTROKE}/charlist.txt", cache), glyphs)


# CJK

def flatten_svg_path(d):
    """Polylines from an SVG path of moves, lines and cubic curves, relative or absolute."""
    numbers = re.findall(r"[A-Za-z]|-?\d*\.?\d+(?:e-?\d+)?", d)
    strokes, stroke = [], []
    x = y = 0.0
    last_control = None
    command = None
    i = 0

    def take(n):
        nonlocal i
        values = [float(v) for v in numbers[i:i + n]]
        i += n
        return values

    while i < len(numbers):
        if numbers[i].isalpha():
            command = numbers[i]
            i += 1
            if command in "Zz":
                if stroke:
                    stroke.append(stroke[0])
                continue
        relative = command.islower()
        kind = command.upper()
        if kind == "M":
            dx, dy = take(2)
            x, y = (x + dx, y + dy) if relative else (dx, dy)
            if stroke:
                strokes.append(stroke)
            stroke = [(x, y)]
            command = "l" if relative else "L"
            last_control = None
        elif kind == "L":
            dx, dy = take(2)
            x, y = (x + dx, y + dy) if relative else (dx, dy)
            stroke.append((x, y))
            last_control = None
        elif kind in ("C", "S"):
            if kind == "C":
                c1x, c1y, c2x, c2y, ex, ey = take(6)
                if relative:
                    c1x, c1y, c2x, c2y, ex, ey = c1x + x, c1y + y, c2x + x, c2y + y, ex + x, ey + y
            else:
                c2x, c2y, ex, ey = take(4)
                if relative:
                    c2x, c2y, ex, ey = c2x + x, c2y + y, ex + x, ey + y
                c1x, c1y = (2 * x - last_control[0], 2 * y - last_control[1]) if last_control else (x, y)
            for k in range(1, 9):
                t = k / 8
                u = 1 - t
                stroke.append((
                    u * u * u * x + 3 * u * u * t * c1x + 3 * u * t * t * c2x + t * t * t * ex,
                    u * u * u * y + 3 * u * u * t * c1y + 3 * u * t * t * c2y + t * t * t * ey,
                ))
            last_control = (c2x, c2y)
            x, y = ex, ey
        else:
            i += 1
    if stroke:
        strokes.append(stroke)
    return strokes


def kanjivg(cache):
    path = fetch(KANJIVG, cache)
    tree = ElementTree.fromstring(gzip.decompress(path.read_bytes()))
    out = {}
    for kanji in tree:
        identifier = kanji.get("id", "")
        match = re.fullmatch(r"kvg:kanji_([0-9a-f]+)", identifier)
        if not match:
            continue
        strokes = []
        for element in kanji.iter():
            if element.tag == "path" and element.get("d"):
                for stroke in flatten_svg_path(element.get("d")):
                    strokes.append([(px / 109.0, py / 109.0 - CJK_BASELINE) for px, py in stroke])
        if strokes:
            out[int(match.group(1), 16)] = (1.0, strokes)
    return out


def medians(path):
    """Characters from a Make Me a Hanzi style graphics file, from their stroke medians."""
    out = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        entry = json.loads(line)
        character = entry["character"]
        if len(character) != 1:
            continue
        strokes = [
            [(px / 1024.0, (900.0 - py) / 1024.0 - CJK_BASELINE) for px, py in median]
            for median in entry["medians"]
        ]
        out[ord(character)] = (1.0, strokes)
    return out


# packing

def simplify(points, epsilon):
    if len(points) < 3:
        return points
    (ax, ay), (bx, by) = points[0], points[-1]
    length = math.hypot(bx - ax, by - ay)
    best, index = 0.0, 0
    for i in range(1, len(points) - 1):
        px, py = points[i]
        if length > 0:
            d = abs((bx - ax) * (ay - py) - (ax - px) * (by - ay)) / length
        else:
            d = math.hypot(px - ax, py - ay)
        if d > best:
            best, index = d, i
    if best <= epsilon:
        return [points[0], points[-1]]
    return simplify(points[: index + 1], epsilon)[:-1] + simplify(points[index:], epsilon)


def quantise(value, origin):
    return max(0, min(255, round((value - origin) / SPAN * 255)))


def write_pack(path, glyphs, ascent, descent, human_order, sources):
    index, data = [], bytearray()
    for codepoint in sorted(glyphs):
        advance, strokes = glyphs[codepoint]
        packed = []
        for stroke in strokes:
            stroke = simplify(stroke, SPAN / 255 * 0.6)
            q = [(quantise(x, ORIGIN[0]), quantise(y, ORIGIN[1])) for x, y in stroke]
            q = [p for k, p in enumerate(q) if k == 0 or p != q[k - 1]] or q[:1]
            if len(q) == 1:
                q = q * 2
            while len(q) > 255:
                q = q[::2]
            packed.append(q)
        if len(packed) > 255:
            continue
        index.append((codepoint, len(data)))
        data.append(max(0, min(255, round(advance / SPAN * 255))))
        data.append(len(packed))
        for q in packed:
            data.append(len(q))
            for x, y in q:
                data += bytes((x, y))
    notice = " ".join([NOTICES[source] for source in sources] + [CHANGES]).encode("utf-8")
    header = b"ARINSTRK" + struct.pack("<HH5f", 1, int(human_order), ascent, descent, ORIGIN[0], ORIGIN[1], SPAN)
    header += struct.pack("<I", len(notice)) + notice + struct.pack("<I", len(index))
    table = b"".join(struct.pack("<II", cp, offset) for cp, offset in index)
    path.write_bytes(header + table + bytes(data))
    print(f"  {path.name}: {len(index)} characters, {path.stat().st_size // 1024} KB", file=sys.stderr)


def pick(source, ranges):
    return {cp: g for cp, g in source.items() if any(lo <= cp <= hi for lo, hi in ranges)}


SHARED = [(0x20, 0x7E), (0x2010, 0x2027), (0x3000, 0x303F), (0xFF01, 0xFF5E)]


def script_fonts(out, cache):
    out.mkdir(parents=True, exist_ok=True)
    for family, file, script in SCRIPT_FONTS:
        font = fetch(f"{GOOGLE_FONTS}/{family}/{file.replace('[', '%5B').replace(']', '%5D')}", cache)
        licence = fetch(f"{GOOGLE_FONTS}/{family}/OFL.txt", cache)
        name = file.split("[")[0].split(".")[0]
        (out / f"{name}.ttf").write_bytes(font.read_bytes())
        (out / f"{name}-OFL.txt").write_bytes(licence.read_bytes())
        print(f"  {name}.ttf: {script}, {font.stat().st_size // 1024} KB", file=sys.stderr)


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("assets/strokes")
    fonts = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("assets/fonts/scripts")
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as cache:
        print("script fonts", file=sys.stderr)
        script_fonts(fonts, cache)
        print("newstroke", file=sys.stderr)
        strokes = newstroke(cache)
        shared = pick(strokes, SHARED)

        greek_cyrillic = pick(strokes, SHARED + [(0xA0, 0xFF), (0x370, 0x3FF), (0x400, 0x52F)])
        write_pack(out / "greek-cyrillic.strk", greek_cyrillic, 0.8, 0.25, False, ["newstroke"])

        maths = pick(strokes, SHARED + [(0x2190, 0x21FF), (0x2200, 0x22FF), (0x370, 0x3FF)])
        write_pack(out / "maths.strk", maths, 0.8, 0.25, False, ["newstroke"])

        print("KanjiVG", file=sys.stderr)
        japanese = {**shared, **kanjivg(cache)}
        write_pack(out / "japanese.strk", japanese, CJK_BASELINE, 1 - CJK_BASELINE + 0.05, True, ["kanjivg", "newstroke"])

        print("Make Me a Hanzi", file=sys.stderr)
        simplified = {**shared, **medians(fetch(f"{MAKEMEAHANZI}/graphics.txt", cache))}
        write_pack(out / "chinese-simplified.strk", simplified, CJK_BASELINE, 1 - CJK_BASELINE + 0.05, True, ["makemeahanzi", "newstroke"])

        print("AnimCJK", file=sys.stderr)
        traditional = {**shared, **medians(fetch(f"{ANIMCJK}/graphicsZhHant.txt", cache))}
        write_pack(out / "chinese-traditional.strk", traditional, CJK_BASELINE, 1 - CJK_BASELINE + 0.05, True, ["animcjk", "newstroke"])


if __name__ == "__main__":
    main()
