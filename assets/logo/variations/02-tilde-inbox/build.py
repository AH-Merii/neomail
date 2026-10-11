"""Tilde Inbox: build every asset in this folder.

    uv run --with pillow --with uharfbuzz --with fonttools python build.py

Needs rsvg-convert, Adwaita Sans (wordmark, via ../../src/text.py)
and Adwaita Mono (terminal mock in preview.png only).
"""
import math
import subprocess
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True     # never write __pycache__ into ../../src
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "src"))   # shared text.py / brand.py

# ---------------------------------------------------------------- palette
INK = "#0D1117"       # dark background; envelope and cursor on light
PAPER = "#F6F8FA"     # light background
OFF = "#E8EEF3"       # envelope and cursor on dark
AMBER = "#F2A93B"     # tildes on dark: phosphor amber
AMBER_L = "#9A5500"   # tildes on light: burnt amber
MUTED = "#8B98A5"     # secondary text in the terminal mock

# ---------------------------------------------------------------- geometry (64-unit grid, 2 units = 1 px at 32 px)
BOX = (2, 4, 62, 60)  # envelope outer edge: square corners, like a terminal window
STROKE = 6            # frame and flap weight
APEX = 24             # lower edge of the flap's apex, on x = 32 (a cell boundary at 16 and 32 px)
CURSOR = (12, 20, 18, 28)          # block cursor on line 1: x0, y0, x1, y1
TILDE_X, TILDE_W, TILDE_AMP, TILDE_STROKE = 12, 18, 2.2, 3.6
TILDE_YS = (35.5, 46.5)            # two end-of-buffer lines; 3 units of clear space between ink


def fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def area(pts):
    return sum(x0 * y1 - x1 * y0 for (x0, y0), (x1, y1) in zip(pts, pts[1:] + pts[:1])) / 2


def oriented(pts, positive=True):
    """Fix winding so solids and holes combine under the nonzero rule in one path."""
    return pts if (area(pts) > 0) == positive else pts[::-1]


def poly_d(pts):
    return "M" + " ".join(f"{fmt(x)} {fmt(y)}" for x, y in pts) + "Z"


def arc(cx, cy, r, a0, a1, n):
    return [(cx + r * math.cos(a0 + (a1 - a0) * i / n), cy + r * math.sin(a0 + (a1 - a0) * i / n))
            for i in range(n + 1)]


def stroke_outline(pts, h, n=10):
    """Outline of an open polyline stroked with width 2h, round caps and round joins.

    Built for smooth curves (dense samples) and for single sharp bends such as the flap."""
    def unit(a, b):
        dx, dy = b[0] - a[0], b[1] - a[1]
        L = math.hypot(dx, dy)
        return dx / L, dy / L
    segs = [unit(a, b) for a, b in zip(pts, pts[1:])]
    norms = [(-dy, dx) for dx, dy in segs]
    left, right = [], []
    for i, p in enumerate(pts):
        ns = [norms[j] for j in (i - 1, i) if 0 <= j < len(norms)]
        if len(ns) == 1 or abs(ns[0][0] * ns[1][0] + ns[0][1] * ns[1][1]) > 0.98:
            nx, ny = ns[-1] if len(ns) == 1 else ((ns[0][0] + ns[1][0]) / 2, (ns[0][1] + ns[1][1]) / 2)
            L = math.hypot(nx, ny)
            left.append((p[0] + h * nx / L, p[1] + h * ny / L))
            right.append((p[0] - h * nx / L, p[1] - h * ny / L))
            continue
        (ax, ay), (bx, by) = ns
        turn = segs[i - 1][0] * segs[i][1] - segs[i - 1][1] * segs[i][0]
        k = h / (1 + ax * bx + ay * by)            # miter point on the inner side
        a0, a1 = math.atan2(ay, ax), math.atan2(by, bx)
        if turn < 0:                                 # bend turns toward -n: outer side is +n
            if a1 > a0:                              # normals rotate clockwise here
                a1 -= 2 * math.pi
            left += arc(p[0], p[1], h, a0, a1, n)
            right.append((p[0] - k * (ax + bx), p[1] - k * (ay + by)))
        else:
            left.append((p[0] + k * (ax + bx), p[1] + k * (ay + by)))
            b0, b1 = a0 + math.pi, a1 + math.pi
            if b1 < b0:
                b1 += 2 * math.pi
            right += arc(p[0], p[1], h, b0, b1, n)
    end_a = math.atan2(norms[-1][1], norms[-1][0])
    start_a = math.atan2(norms[0][1], norms[0][0])
    cap_end = arc(pts[-1][0], pts[-1][1], h, end_a, end_a - math.pi, n)[1:-1]
    cap_start = arc(pts[0][0], pts[0][1], h, start_a + math.pi, start_a, n)[1:-1]
    return left + cap_end + right[::-1] + cap_start


def tilde_pts(x, y, w, a, n=40):
    """One full sine period: two round bumps, the shape of a typed ~."""
    return [(x + w * i / n, y - a * math.sin(2 * math.pi * i / n)) for i in range(n + 1)]


def flap_pts():
    """The V flap as a band hanging from the outer top corners, STROKE thick at right angles."""
    x0, y0, x1, _ = BOX
    mid, half, t = (x0 + x1) / 2, (x1 - x0) / 2, STROKE
    for _ in range(30):                        # slope and vertical thickness depend on each other
        s = (APEX - y0 - t) / half
        t = STROKE * math.hypot(1, s)
    return [(x0, y0), (mid, y0 + s * half), (x1, y0), (x1, y0 + t), (mid, APEX), (x0, y0 + t)]


def envelope_d():
    x0, y0, x1, y1 = BOX
    w = STROKE
    outer = oriented([(x0, y0), (x1, y0), (x1, y1), (x0, y1)], True)
    inner = oriented([(x0 + w, y0 + w), (x1 - w, y0 + w), (x1 - w, y1 - w), (x0 + w, y1 - w)], False)
    return poly_d(outer) + poly_d(inner) + poly_d(oriented(flap_pts(), True))


def cursor_d():
    x0, y0, x1, y1 = CURSOR
    return f"M{x0} {y0}H{x1}V{y1}H{x0}Z"


def tildes_d(x=TILDE_X, ys=TILDE_YS, w=TILDE_W, a=TILDE_AMP, sw=TILDE_STROKE):
    return "".join(poly_d(oriented(stroke_outline(tilde_pts(x, y, w, a), sw / 2, 8), True)) for y in ys)


def mark_body(fg, accent):
    return path(envelope_d() + cursor_d(), fg) + path(tildes_d(), accent)


def svg(vw, vh, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {fmt(vw)} {fmt(vh)}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def path(d, fill):
    return f'<path fill="{fill}" d="{d}"/>'


def runs_d(cells):
    """Pixel cells -> one path of horizontal runs."""
    rows = {}
    for x, y in cells:
        rows.setdefault(y, set()).add(x)
    d = []
    for y in sorted(rows):
        xs = sorted(rows[y])
        start = prev = xs[0]
        for x in xs[1:] + [None]:
            if x != prev + 1 if x is not None else True:
                d.append(f"M{start} {y}h{prev - start + 1}v1h-{prev - start + 1}Z")
                start = x
            prev = x if x is not None else prev
    return "".join(d)


def pattern(rows, ox=0, oy=0):
    return [(ox + x, oy + y) for y, r in enumerate(rows) for x, c in enumerate(r) if c == "#"]


# ---------------------------------------------------------------- favicon (hand-placed 16 px grid)
# The mark's own geometry at 16 px: frame 1 px, flap 1 px falling one row per 2 px with its apex
# on the x = 8 boundary, a 2 x 3 cursor on line 1 and two 2-row tildes with one clear row between.
FAV_FRAME = ["." * 16] + ["#" * 16] + ["#" + "." * 14 + "#"] * 13 + ["#" * 16]   # rows 1..15
FAV_FLAP = [(1, 2), (2, 2), (13, 2), (14, 2), (3, 3), (4, 3), (11, 3), (12, 3),
            (5, 4), (6, 4), (9, 4), (10, 4), (7, 5), (8, 5)]
FAV_CURSOR = [(x, y) for x in (2, 3) for y in (5, 6, 7)]
FAV_TILDE = [".##..#", "#..##."]            # 6 x 2: one up bump, one down bump
FAV_TILDES = [(2, 9), (2, 12)]


def favicon_svg(light=False):
    """Default: an ink tile with off-white strokes (any background). light=True: ink strokes, no tile."""
    fg, accent = (INK, AMBER_L) if light else (OFF, AMBER)
    strokes = pattern(FAV_FRAME) + FAV_FLAP + FAV_CURSOR
    tildes = [c for ox, oy in FAV_TILDES for c in pattern(FAV_TILDE, ox, oy)]
    tile = "" if light else path("M0 1H16V16H0Z", INK)
    return svg(16, 16, f'<g shape-rendering="crispEdges">{tile}{path(runs_d(strokes), fg)}'
                       f'{path(runs_d(tildes), accent)}</g>')


# ---------------------------------------------------------------- lockup
WORD, AXES = "neomail", {"wght": 700, "opsz": 32}   # 700 sits with the 6-unit frame
WORD_GAP = 16                       # from the envelope's right edge to the ink of the n
X_HEIGHT = 28                       # word x-height in mark units


def lockup_svg(fg, accent):
    import text as T
    right = BOX[2]
    m = lambda ch, k: T.metric(ch, AXES["wght"], AXES["opsz"], k)
    scale = X_HEIGHT / m("x", "ymax")
    l_top = m("l", "ymax") * scale
    base = 32 + l_top / 2           # centre the word's ink (baseline to top of l) on the mark
    x = right + WORD_GAP - m("n", "xmin") * scale
    d, adv = T.outline(WORD, x, base, scale, AXES["wght"], AXES["opsz"], kern=False)
    l_right = m("l", "xmax")
    l_adv = T.shape("l", AXES["wght"], AXES["opsz"])[1]
    width = x + adv - (l_adv - l_right) * scale + BOX[0]
    return svg(width, 64, mark_body(fg, accent) + path(d, fg))


# ---------------------------------------------------------------- terminal half-block art (22 x 16 px = 22 x 8 cells)
TW, TH = 22, 16
TERM_FRAME = {(x, y) for x in range(TW) for y in (0, TH - 1)} | {(x, y) for x in (0, TW - 1) for y in range(TH)}
TERM_FLAP = {(x, y) for y, (a, b) in enumerate([(1, 2), (3, 4), (5, 6), (7, 8)], 1) for x in (a, b, TW - 1 - a, TW - 1 - b)}
TERM_FLAP |= {(x, 5) for x in range(9, 13)}
TERM_CURSOR = {(2, 4), (2, 5)}               # exactly one character cell: a real █ on text row 2
TERM_TILDE = [".###...#", "#...###."]        # half-blocks: ▄▀▀▀▄▄▄▀
TERM_TILDES = [(2, 8), (2, 12)]              # text rows 4 and 6, one tilde per row
HALF = {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}


def term_art():
    fg = TERM_FRAME | TERM_FLAP | TERM_CURSOR
    til = {c for ox, oy in TERM_TILDES for c in pattern(TERM_TILDE, ox, oy)}
    plain, ansi, cells = [], [], []
    rgb = lambda c: f"\x1b[38;2;{int(c[1:3], 16)};{int(c[3:5], 16)};{int(c[5:7], 16)}m"
    for y in range(0, TH, 2):
        p, a, row, cur = "", "", [], None
        for x in range(TW):
            on = lambda px: px in fg or px in til
            ch = HALF[(on((x, y)), on((x, y + 1)))]
            both = {(x, y), (x, y + 1)}
            assert not (both & til and both & fg), "a cell may carry only one colour"
            col = AMBER if both & til else OFF
            p += ch
            row.append((ch, col))
            if ch != " " and col != cur:
                a += rgb(col)
                cur = col
            a += ch
        plain.append(p.rstrip())
        ansi.append(a.rstrip() + "\x1b[0m")
        cells.append(row)
    return plain, ansi, cells


def vim_art(plain):
    """Bonus: the same envelope with a real cursor cell and real '~' characters, exactly like Vim."""
    rows = [list(r.ljust(TW)) for r in plain]
    for r in (4, 6):
        rows[r][2:10] = list("~" + " " * 7)
    return ["".join(r).rstrip() for r in rows]


# ---------------------------------------------------------------- write
def write_all():
    files = {
        "mark.svg": svg(64, 64, mark_body(INK, AMBER_L)),
        "mark-dark.svg": svg(64, 64, mark_body(OFF, AMBER)),
        "mark-mono.svg": svg(64, 64, path(envelope_d() + cursor_d() + tildes_d(), "currentColor")),
        "favicon.svg": favicon_svg(),
        "favicon-light.svg": favicon_svg(light=True),
        "lockup-light.svg": lockup_svg(INK, AMBER_L),
        "lockup-dark.svg": lockup_svg(OFF, AMBER),
    }
    plain, ansi, cells = term_art()
    files["logo.txt"] = "\n".join(plain) + "\n"
    files["logo.ans"] = "\n".join(ansi) + "\n"
    files["logo-vim.txt"] = "\n".join(vim_art(plain)) + "\n"
    for name, body in files.items():
        (HERE / name).write_text(body)
    return cells


# ---------------------------------------------------------------- preview sheet
SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONO_FONT = "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf"
MONO_BOLD = "/usr/share/fonts/Adwaita/AdwaitaMono-Bold.ttf"


def raster(svg_text, w, h=None, bg=None):
    from PIL import Image
    with tempfile.TemporaryDirectory() as t:
        src, dst = Path(t) / "a.svg", Path(t) / "a.png"
        src.write_text(svg_text)
        args = ["rsvg-convert", "-w", str(w)] + (["-h", str(h)] if h else []) + (["-b", bg] if bg else [])
        subprocess.run(args + [str(src), "-o", str(dst)], check=True)
        return Image.open(dst).convert("RGBA").copy()


def preview(cells):
    from PIL import Image, ImageDraw, ImageFont
    W, H = 1600, 1000
    im = Image.new("RGBA", (W, H), "#1F2630")
    dr = ImageDraw.Draw(im)
    lab = ImageFont.truetype(SANS, 15)
    mark = {INK: (HERE / "mark-dark.svg").read_text(), PAPER: (HERE / "mark.svg").read_text()}
    favs = {INK: (HERE / "favicon.svg").read_text(), PAPER: (HERE / "favicon-light.svg").read_text()}

    def put(img, x, y):
        im.alpha_composite(img, (x, y))

    for oy, bg in ((0, INK), (340, PAPER)):
        fg_lab = MUTED if bg == INK else "#57606A"
        dr.rectangle((0, oy, W, oy + 336), fill=bg)
        name = "dark #0D1117" if bg == INK else "light #F6F8FA"
        dr.text((30, oy + 14), f"{name}   ·   mark at 256, 64, 32, 16 px   ·   8× zooms   ·   " + ("favicon.svg" if bg == INK else "favicon-light.svg") + " (drawn on a 16 px grid)", font=lab, fill=fg_lab)
        put(raster(mark[bg], 256, 256, bg), 30, oy + 50)
        fav = favs[bg]
        for s, x in ((64, 312), (32, 396), (16, 448)):
            put(raster(mark[bg], s, s, bg), x, oy + 178 - s // 2)
        put(raster(mark[bg], 16, 16, bg).resize((128, 128), Image.NEAREST), 490, oy + 114)
        put(raster(mark[bg], 32, 32, bg).resize((256, 256), Image.NEAREST), 640, oy + 50)
        dr.line((925, oy + 50, 925, oy + 306), fill=fg_lab, width=1)
        put(raster(fav, 16, 16, bg), 955, oy + 170)
        put(raster(fav, 16, 16, bg).resize((128, 128), Image.NEAREST), 995, oy + 114)
        put(raster(fav, 32, 32, bg), 1145, oy + 162)
        put(raster(fav, 32, 32, bg).resize((256, 256), Image.NEAREST), 1205, oy + 50)
        for x, t in ((30, "mark 256"), (312, "64 · 32 · 16"), (490, "mark 16 ×8"), (640, "mark 32 ×8"),
                     (945, "16"), (995, "favicon 16 ×8"), (1145, "32"), (1205, "favicon 32 ×8")):
            dr.text((x, oy + 312), t, font=lab, fill=fg_lab)

    # mono marks
    mono = (HERE / "mark-mono.svg").read_text()
    dr.rectangle((30, 700, 230, 840), fill="#FFFFFF")
    put(raster(mono, 120), 70, 710)
    dr.rectangle((30, 845, 230, 985), fill=INK)
    put(raster(mono.replace("currentColor", "#FFFFFF"), 120), 70, 855)
    dr.text((30, 682), "mono (currentColor)", font=lab, fill=OFF)
    # lockups
    dr.text((260, 682), "lockups", font=lab, fill=OFF)
    for y, name, bg in ((700, "lockup-light.svg", PAPER), (845, "lockup-dark.svg", INK)):
        dr.rectangle((260, y, 860, y + 140), fill=bg)
        text_ = (HERE / name).read_text()
        vw, vh = map(float, text_.split('viewBox="')[1].split('"')[0].split()[2:])
        lk = raster(text_, round(84 * vw / vh), 84)
        put(lk, 260 + (600 - lk.width) // 2, y + 28)
    # terminal splash mock
    tx, ty, tw, th = 890, 700, 680, 285
    dr.text((tx, 682), "TUI splash (logo.ans, 22 × 8 cells)", font=lab, fill=OFF)
    dr.rounded_rectangle((tx, ty, tx + tw, ty + th), 10, fill=INK, outline="#30363D")
    cw, ch = 12, 26
    ox, oy = tx + 40, ty + 38
    for r, row in enumerate(cells):
        for c, (glyph, col) in enumerate(row):
            x0, y0 = ox + c * cw, oy + r * ch
            if glyph in "▀█":
                dr.rectangle((x0, y0, x0 + cw - 1, y0 + ch // 2 - 1), fill=col)
            if glyph in "▄█":
                dr.rectangle((x0, y0 + ch // 2, x0 + cw - 1, y0 + ch - 1), fill=col)
    mf, mb = ImageFont.truetype(MONO_FONT, 19), ImageFont.truetype(MONO_BOLD, 19)
    tx2 = ox + 25 * cw
    dr.text((tx2, oy + 2 * ch + 4), "neomail", font=mb, fill=OFF)
    dr.text((tx2, oy + 3 * ch + 4), "inbox zero.", font=mf, fill=MUTED)
    dr.text((tx2, oy + 5 * ch + 4), "~", font=mf, fill=AMBER)
    dr.text((tx2 + cw, oy + 5 * ch + 4), " :e inbox", font=mf, fill=MUTED)
    im.convert("RGB").save(HERE / "preview.png")


if __name__ == "__main__":
    preview(write_all())
    print((HERE / "logo.txt").read_text())
