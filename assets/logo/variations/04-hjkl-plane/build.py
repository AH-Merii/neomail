"""neomail variation 04, "hjkl plane": a message typed out as Vim motions, then sent.

Regenerates every asset in this folder:
    uv run --with pillow --with uharfbuzz --with fonttools python build.py
Needs rsvg-convert and Adwaita Sans (/usr/share/fonts/Adwaita), reused via ../../src/text.py.
"""
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "src"))
from text import outline  # noqa: E402  (Adwaita Sans outlined with HarfBuzz + fontTools)

# ---------------------------------------------------------------- palette
INK = "#0D1117"        # dark background, plane on light
PAPER = "#F6F8FA"      # light background
SHEET = "#E8EEF3"      # plane on dark (paper white)
AMBER = "#FFB547"      # the motion path on dark, favicon, terminal (hue 36 deg)
AMBER_L = "#C27A00"    # the motion path on light (same family, 3.25:1 on paper)
DIM = "#8B98A5"        # terminal hint text only, not part of the mark

THEMES = {
    "light": dict(bg=PAPER, fg=INK, path=AMBER_L),
    "dark": dict(bg=INK, fg=SHEET, path=AMBER),
}

# ---------------------------------------------------------------- geometry
# Everything sits on a 16x16 cell grid (1 cell = 4 units of the 64-unit viewBox), so the
# 16 px render puts every straight edge on a pixel boundary.
U = 4
# The motion path: 7l then 2k, two cells thick, with a one-cell gap at the turn. The k
# stroke stops one cell under the plane's tail, so the plane is the third motion.
PATH = [(0, 14, 7, 2),     # l: along the floor
        (8, 12, 2, 4)]     # k: up, into the tail
# The plane: a folded dart flying to the top-right corner. Its tail is a flat two-cell
# edge exactly as wide as the k stroke and one cell above it.
NOSE, WING_L, TAIL_L, TAIL_R, WING_R = (16, 0), (2, 4), (8, 11), (10, 11), (13, 15)
PLANE = [NOSE, WING_L, TAIL_L, TAIL_R, WING_R]
# The fold is a wedge cut from the middle of the tail half way to the nose (one cell wide
# at the tail). It is a cut, not a second tone, so colour, dark and mono are one drawing.
FOLD_FROM, FOLD_WIDTH, FOLD_REACH = (9, 11), 1.0, 0.5


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def poly(pts, s=U, dx=0, dy=0):
    return "M" + "L".join(f"{fmt(x * s + dx)} {fmt(y * s + dy)}" for x, y in pts) + "Z"


def rect(x, y, w, h, s=U, dx=0, dy=0):
    return poly([(x, y), (x + w, y), (x + w, y + h), (x, y + h)], s, dx, dy)


def fold():
    """The fold wedge: its base lies on the tail edge, so the even-odd hole never leaks out."""
    (ax, ay), (bx, by) = FOLD_FROM, NOSE
    apex = (ax + (bx - ax) * FOLD_REACH, ay + (by - ay) * FOLD_REACH)
    return [(ax - FOLD_WIDTH / 2, ay), apex, (ax + FOLD_WIDTH / 2, ay)]


def svg(vw, vh, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(vw)} {fmt(vh)}" '
            f'role="img" aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def path(d, fill, evenodd=False):
    rule = ' fill-rule="evenodd"' if evenodd else ""
    return f'<path fill="{fill}"{rule} d="{d}"/>'


def plane_d(s=U, dx=0, dy=0):
    return poly(PLANE, s, dx, dy) + poly(fold(), s, dx, dy)   # even-odd: the fold is a hole


def path_d(s=U, dx=0, dy=0):
    return "".join(rect(*b, s, dx, dy) for b in PATH)


def mark_body(th, s=U, dx=0, dy=0):
    return path(path_d(s, dx, dy), th["path"]) + path(plane_d(s, dx, dy), th["fg"], evenodd=True)


def mark_mono():
    return path(path_d() + plane_d(), "currentColor", evenodd=True)


# ---------------------------------------------------------------- pixel drawings
# The 16 px master already lands the motion path on whole pixels, but its fold closes up.
# So the favicon and the terminal are hand-drawn pixel versions of the same mark,
# with the fold as a one-pixel diagonal cut. P plane, T motion path, . empty.
FAV = """
.............P
.........PPPPP
......PPPPPPP.
...PPPPPPPPPP.
..PPPPPPPPPPP.
...PPPPPPPP.P.
....PPPPPP.PP.
.....PPPP.PPP.
......PP.PPP..
.......P.PP...
..........P...
.......TT.....
TTTTTT.TT.....
TTTTTT.TT.....
""".split()   # 14x14, placed one pixel inside a 16 px ink tile
FAV_TILE_R = 3

GRID = """
...............P
...........PPPPP
.......PPPPPPPP.
....PPPPPPPPPPP.
..PPPPPPPPPPPPP.
...PPPPPPPPP.PP.
....PPPPPPPP.PP.
.....PPPPPP.PP..
......PPPP.PPP..
.......PPP.PPP..
........P.PPPP..
..........PPPP..
........TT.PP...
........TT..P...
TTTTTTT.TT......
TTTTTTT.TT......
""".split()   # 16x16 pixels = 16 columns x 8 rows of half blocks
TERM_RGB = {"P": SHEET, "T": AMBER}


def pixels(grid, key, dx=0, dy=0):
    """Merge horizontal runs of one colour into rects (one path per colour)."""
    d = ""
    for y, row in enumerate(grid):
        x = 0
        while x < len(row):
            if row[x] == key:
                x0 = x
                while x < len(row) and row[x] == key:
                    x += 1
                d += rect(x0, y, x - x0, 1, 1, dx, dy)
            else:
                x += 1
    return d


def favicon():
    r = FAV_TILE_R
    t = (f"M{r} 0H{16 - r}A{r} {r} 0 0 1 16 {r}V{16 - r}A{r} {r} 0 0 1 {16 - r} 16H{r}"
         f"A{r} {r} 0 0 1 0 {16 - r}V{r}A{r} {r} 0 0 1 {r} 0Z")
    return svg(16, 16, path(t, INK) + path(pixels(FAV, "T", 1, 1), AMBER)
               + path(pixels(FAV, "P", 1, 1), SHEET))


# Lockup, 64 units tall: the l stroke is the floor, so the word's baseline is the bottom of
# the motion path (y 64) and the plane takes off above the word. x-height about 29, just under half the mark.
WORD_SCALE, BASELINE, WORD_GAP = 0.028, 64, 12
AXES = (600, 32)


def lockup(th):
    d, adv = outline("neomail", 64 + WORD_GAP, BASELINE, WORD_SCALE, *AXES, kern=False)
    w = 64 + WORD_GAP + adv + 4
    return svg(w, 64, mark_body(th) + path(d, th["fg"]))


# ---------------------------------------------------------------- terminal
def rgb(h):
    return tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))


def halfblock(color):
    lines = []
    for r in range(0, len(GRID), 2):
        top, bot, out = GRID[r], GRID[r + 1], []
        for a, b in zip(top, bot):
            if a == "." and b == ".":
                out.append(" ")
            elif not color:
                out.append("█" if a != "." and b != "." else ("▀" if a != "." else "▄"))
            elif a != "." and b != ".":
                if a == b:
                    out.append("\x1b[38;2;%d;%d;%dm█\x1b[0m" % rgb(TERM_RGB[a]))
                else:
                    out.append("\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm▀\x1b[0m"
                               % (rgb(TERM_RGB[a]) + rgb(TERM_RGB[b])))
            else:
                ch, c = ("▀", a) if a != "." else ("▄", b)
                out.append("\x1b[38;2;%d;%d;%dm%s\x1b[0m" % (rgb(TERM_RGB[c]) + (ch,)))
        lines.append("".join(out).rstrip())
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- write
def rsvg(src, dst, w=None, h=None, bg=None):
    args = ["rsvg-convert"] + (["-w", str(w)] if w else []) + (["-h", str(h)] if h else [])
    args += (["-b", bg] if bg else []) + [str(src), "-o", str(dst)]
    subprocess.run(args, check=True)


def write_assets():
    files = {
        "mark.svg": svg(64, 64, mark_body(THEMES["light"])),
        "mark-dark.svg": svg(64, 64, mark_body(THEMES["dark"])),
        "mark-mono.svg": svg(64, 64, mark_mono()),
        "favicon.svg": favicon(),
        "lockup-light.svg": lockup(THEMES["light"]),
        "lockup-dark.svg": lockup(THEMES["dark"]),
        "logo.txt": halfblock(False),
        "logo.ans": halfblock(True),
    }
    for name, body in files.items():
        (HERE / name).write_text(body)
    txt = files["logo.txt"].splitlines()
    assert len(txt) <= 8 and all(len(line) <= 24 for line in txt)
    assert all(len(row) == 16 for row in GRID) and all(len(row) == 14 for row in FAV)


# ---------------------------------------------------------------- preview
SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONO_FONT = "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf"


def preview():
    from PIL import Image, ImageDraw, ImageFont

    tmp = Path(tempfile.mkdtemp(prefix="nm04-"))
    sheet = Image.new("RGB", (1600, 1000), "#808890")
    d = ImageDraw.Draw(sheet)
    label = ImageFont.truetype(SANS, 15)
    lab = {"dark": "#8B98A5", "light": "#5B6673"}

    def png(src, size, bg, key, h=None):
        out = tmp / f"{key}.png"
        rsvg(src, out, size if not h else None, h, bg)
        return Image.open(out).convert("RGB")

    def zoom(src, size, bg, key):
        return png(src, size, bg, key).resize((size * 8, size * 8), Image.NEAREST)

    # Rows 1 and 2: the mark on dark and on light, 8x zooms, then the favicon.
    for row, (theme, src) in enumerate((("dark", "mark-dark.svg"), ("light", "mark.svg"))):
        bg = THEMES[theme]["bg"]
        y0 = row * 300
        d.rectangle([0, y0, 1600, y0 + 296], fill=bg)
        x, base = 24, y0 + 270
        items = [(png(HERE / src, s, bg, f"{theme}{s}"), f"{s} px") for s in (256, 64, 32, 16)]
        items += [(zoom(HERE / src, 16, bg, f"{theme}z16"), "16 px, 8x"),
                  (zoom(HERE / src, 32, bg, f"{theme}z32"), "32 px, 8x")]
        items += [(png(HERE / "favicon.svg", s, bg, f"fav{theme}{s}"), f"favicon {s}") for s in (32, 16)]
        items += [(zoom(HERE / "favicon.svg", 16, bg, f"fav{theme}z"), "favicon 16, 8x")]
        for im, text in items:
            sheet.paste(im, (x, base - im.height))
            d.text((x, base + 6), text, font=label, fill=lab[theme])
            x += max(im.width, 70) + 30

    # Row 3: mono, lockups, terminal splash.
    y0 = 600
    d.rectangle([0, y0, 1600, 1000], fill="#FFFFFF")
    mono = (HERE / "mark-mono.svg").read_text()
    for i, (col, bg) in enumerate((("#000000", "#FFFFFF"), ("#FFFFFF", INK))):
        f = tmp / f"mono{i}.svg"
        f.write_text(mono.replace("currentColor", col))
        box = Image.new("RGB", (168, 168), bg)
        box.paste(png(f, 128, bg, f"mono{i}"), (20, 20))
        sheet.paste(box, (24, y0 + 24 + i * 184))
    d.text((200, y0 + 30), "mono", font=label, fill="#5B6673")
    for i, theme in enumerate(("light", "dark")):
        bg = THEMES[theme]["bg"]
        im = png(HERE / f"lockup-{theme}.svg", None, bg, f"lock{theme}", h=96)
        box = Image.new("RGB", (im.width + 64, 168), bg)
        box.paste(im, (32, 18))
        small = png(HERE / f"lockup-{theme}.svg", None, bg, f"locks{theme}", h=24)
        box.paste(small, (32, 132))
        sheet.paste(box, (260, y0 + 24 + i * 184))
    # Mock terminal splash, cell 12x24 px, drawn from the same grid as logo.txt.
    tx, ty, tw, th = 1000, y0 + 24, 576, 352
    d.rounded_rectangle([tx, ty, tx + tw, ty + th], 10, fill=INK, outline="#30363D", width=2)
    for i, c in enumerate(("#FF5F57", "#FEBC2E", "#28C840")):
        d.ellipse([tx + 14 + i * 20, ty + 12, tx + 26 + i * 20, ty + 24], fill=c)
    term = ImageFont.truetype(MONO_FONT, 19)
    cw, chh, ox, oy = 12, 24, tx + 36, ty + 52
    for r, line in enumerate(GRID):
        for c, ch in enumerate(line):
            if ch != ".":
                px, py = ox + c * cw, oy + r * (chh // 2)
                d.rectangle([px, py, px + cw - 1, py + chh // 2 - 1], fill=TERM_RGB[ch])
    d.text((ox + 19 * cw, oy + 4 * chh + 4), "neomail", font=ImageFont.truetype(MONO_FONT, 26), fill=SHEET)
    d.text((ox + 19 * cw, oy + 6 * chh + 2), "v0.1  7l 2k :send", font=term, fill=DIM)
    hints = [("i", "compose"), ("j/k", "move"), ("gs", "send"), (":q", "quit")]
    hy, hx = oy + 8 * chh + 26, ox
    for key, what in hints:
        d.text((hx, hy), key, font=term, fill=AMBER)
        hx += int(d.textlength(key + " ", font=term))
        d.text((hx, hy), what, font=term, fill=DIM)
        hx += int(d.textlength(what + "   ", font=term))
    d.text((ox, hy + 34), "~", font=term, fill="#30363D")
    d.text((ox, hy + 58), "NORMAL", font=term, fill=AMBER)
    sheet.save(HERE / "preview.png", optimize=True)


if __name__ == "__main__":
    write_assets()
    preview()
    print("built:", ", ".join(sorted(p.name for p in HERE.iterdir() if p.is_file())))
