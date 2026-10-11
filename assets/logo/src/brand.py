"""neomail brand constants: palette, geometry and SVG helpers.

The mark is "Split and statusline" (variation 05, adopted by DEC-2026-10-11-logo): an envelope
that is a Neovim window, split into a mail list and a reader, over a statusline whose amber
powerline chip is the NORMAL mode block. All geometry here is hand-placed; build.py only writes it.
"""
import math

# ---------------------------------------------------------------- palette
INK = "#0D1117"      # dark background, foreground on light, icon tile, chip label
PAPER = "#F6F8FA"    # light background, favicon keyline
OFF = "#E8EEF3"      # foreground on dark
AMBER = "#FFB238"    # mode chip on dark and on the ink tile
AMBER_L = "#B86E00"  # mode chip on light (graphics only, never text)
MUTED = "#8B98A5"    # secondary text on dark (social card tagline)
MONO = "currentColor"
THEMES = {"light": (INK, AMBER_L), "dark": (OFF, AMBER)}

FONT = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"   # read by text.py
WORD = "neomail"
WORD_AXES = (600, 32)   # Adwaita Sans wght 600, opsz 32
WORD_GAP = 20           # mark right edge (x 60) to the ink of the "n"
MARGIN = 4              # side margin of the lockup, the same as the mark's
CLEAR = 16              # clear space around mark and lockup, in 64-unit lockup units
TAGLINE = "a fast, modal terminal email client"


# ---------------------------------------------------------------- SVG helpers
def f(v):
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def P(*pts):
    """Closed polygon path from points."""
    return "M" + "L".join(f"{f(x)} {f(y)}" for x, y in pts) + "Z"


def rr(x, y, w, h, r):
    """Rounded rectangle path (used only for app-icon tiles; the mark has square corners)."""
    a = f"A{f(r)} {f(r)} 0 0 1 "
    return (f"M{f(x + r)} {f(y)}H{f(x + w - r)}{a}{f(x + w)} {f(y + r)}V{f(y + h - r)}"
            f"{a}{f(x + w - r)} {f(y + h)}H{f(x + r)}{a}{f(x)} {f(y + h - r)}V{f(y + r)}"
            f"{a}{f(x + r)} {f(y)}Z")


def svg(w, h, body, title="neomail", size=None):
    dims = f' width="{size[0]}" height="{size[1]}"' if size else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {f(w)} {f(h)}"{dims} '
            f'role="img" aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def path(d, fill, extra=""):
    return f'<path fill="{fill}"{extra} d="{d}"/>'


# ---------------------------------------------------------------- master mark (64-unit grid)
# Every horizontal and vertical edge is on a multiple of 4 units, so on whole pixels at 16, 32
# and 64 px. Window x 4..60, y 8..40; a 4-unit V seam from the top corners to the apex (32, 24)
# is the flap; the :vsplit seam x 20..24 drops from the flap's left arm (list | reader).
# Statusline y 44..56: chip x 4..16 with a 45 degree point to x 22 (the split's centre line),
# then a 6-unit notch and the bar to x 60.
MASTER = dict(x0=4, x1=60, y0=8, y1=40, gap=4, split=(20, 24), apex=24,
              sl0=44, sl1=56, chip=16, notch=6)
# App icon (icon.svg, 64-unit tile): the master scaled by 0.75 about the tile, i.e.
# x' = 0.75 x + 8, y' = 0.75 y + 8, so the mark spans x 11..53, y 14..50. Edges land on whole
# pixels at 64, 192 and 512 px.
ICON = dict(x0=11, x1=53, y0=14, y1=38, gap=3, split=(23, 26), apex=26,
            sl0=41, sl1=50, chip=20, notch=4.5)
ICON_R, ICON_KEY = 12, 2   # tile corner radius and keyline width, in units


def mark_paths(g=MASTER):
    """(window, bar, chip) path data: window = flap + list pane + reader pane."""
    x0, x1, y0, y1 = g["x0"], g["x1"], g["y0"], g["y1"]
    sl0, sl1, c, n = g["sl0"], g["sl1"], g["chip"], g["notch"]
    sa, sb = g["split"]
    cx, h = (x0 + x1) / 2, g["gap"] / 2
    m = (g["apex"] - y0) / (cx - x0)            # slope of the flap's centre line
    dv = h * math.sqrt(1 + m * m)               # vertical offset for a perpendicular h
    low = lambda x: y0 + dv + m * (x - x0)      # lower edge of the left arm
    lst = P((x0, low(x0)), (sa, low(sa)), (sa, y1), (x0, y1))
    reader = P((sb, low(sb)), (cx, low(cx)), (x1, low(x0)), (x1, y1), (sb, y1))
    xt = x0 + dv / m                            # where the flap's edge meets the top
    flap = P((xt, y0), (2 * cx - xt, y0), (cx, y0 - dv + m * (cx - x0)))
    mid = (sl0 + sl1) / 2
    tip = c + (sl1 - sl0) / 2                   # 45 degree point
    chip = P((x0, sl0), (c, sl0), (tip, mid), (c, sl1), (x0, sl1))
    bar = P((c + n, sl0), (x1, sl0), (x1, sl1), (c + n, sl1), (tip + n, mid))
    return lst + reader + flap, bar, chip


# ---------------------------------------------------------------- pixel drawings
# favicon.svg: pixel art on the 16 px grid, inside an ink tile with a 1 px paper keyline.
# A 12 x 8 window with a 1:1 stepped V flap, the split under the left arm, and a 3 px
# statusline whose chip ends in a pixel arrow. "#" = window/bar, "a" = chip, "." = tile.
FAV = [
    ".##########.",
    "#.########.#",
    "##.######.##",
    "###.####.###",
    "###..##.####",
    "###.#..#####",
    "###.########",
    "###.########",
    "............",
    "aaa.########",
    "aaaa.#######",
    "aaa.########",
]
FAV_AT = (2, 2)             # top-left pixel of the drawing inside the 16 px tile
TILE16 = rr(0, 0, 16, 16, 3)
TILE16_IN = rr(1, 1, 14, 14, 2)

# logo.txt / logo.ans: pixel art 21 x 16 px, printed as 8 rows of half blocks (a cell is
# 1 px wide and 2 px tall). The split is column 6; the statusline's one-cell notch sits under it.
TERM = [
    "..#################..",
    "##..#############..##",
    "####..#########..####",
    "######..#####..######",
    "######.#..#..########",
    "######.###.##########",
    "######.##############",
    "######.##############",
    "######.##############",
    "######.##############",
    "######.##############",
    "######.##############",
    "######.##############",
    ".....................",
    "aaaaaa.##############",
    "aaaaaa.##############",
]
LABEL = "NORMAL"            # printed in the chip cells of logo.ans, ink on amber
TERM_GAP = 3                # columns between the mark and the text
TERM_TEXT = {3: (WORD, "1"), 4: (TAGLINE, "2")}   # cell row: (text, SGR: bold / dim)


def pixel_path(grid, keep, at=(0, 0)):
    """Merge horizontal runs of the given grid characters into one path of rectangles."""
    d = ""
    for y, row in enumerate(grid):
        x = 0
        while x < len(row):
            if row[x] in keep:
                s = x
                while x < len(row) and row[x] in keep:
                    x += 1
                d += f"M{s + at[0]} {y + at[1]}h{x - s}v1h{s - x}Z"
            else:
                x += 1
    return d


def rgb(hexc):
    return tuple(int(hexc[i:i + 2], 16) for i in (1, 3, 5))


def contrast(a, b):
    """WCAG 2.x contrast ratio of two sRGB hex colours."""
    def lum(c):
        ch = [v / 255 for v in rgb(c)]
        ch = [v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in ch]
        return 0.2126 * ch[0] + 0.7152 * ch[1] + 0.0722 * ch[2]
    hi, lo = sorted((lum(a), lum(b)), reverse=True)
    return (hi + 0.05) / (lo + 0.05)
