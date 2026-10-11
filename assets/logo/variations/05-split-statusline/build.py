"""neomail logo variation 05, "split and statusline": regenerate every asset in this folder.

    uv run --with pillow --with uharfbuzz --with fonttools python build.py

Needs rsvg-convert on PATH and Adwaita Sans (outlined via ../../src/text.py).
"""
import math
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # keep ../../src free of __pycache__
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "src"))  # reuse text.py (HarfBuzz + fontTools outlines)

# ---------------------------------------------------------------- palette
INK = "#0D1117"      # dark background, foreground on light
PAPER = "#F6F8FA"    # light background
OFF = "#E8EEF3"      # foreground on dark
AMBER = "#FFB238"    # mode chip on dark (10.5:1 on INK)
AMBER_L = "#B86E00"  # mode chip on light (3.7:1 on PAPER)
THEMES = {"light": (INK, AMBER_L), "dark": (OFF, AMBER)}


def f(v):
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def P(*pts):
    """Closed polygon path from points."""
    return "M" + "L".join(f"{f(x)} {f(y)}" for x, y in pts) + "Z"


# ---------------------------------------------------------------- master mark (64-unit grid)
# Every horizontal and vertical edge sits on a multiple of 4 units, i.e. on whole pixels at
# 16, 32 and 64 px. All corners are square, like terminal cells.
# The window body spans x 4..60, y 8..40. Two seams, 4 units wide, are cut through it:
# a V from the two top corners down to a centred apex (the envelope flap), and the :vsplit
# separator at x 20..24 (about one third: mail list | reader), which runs from the flap's
# left arm down to the bottom edge. Three pieces remain: the flap, the list pane and the
# reader pane. Under it, 4 units lower, the statusline (y 44..56): a powerline-shaped mode
# chip (the NORMAL block, the only accent) under the list pane, whose 45 degree tip points
# at the split's centre line, and the rest of the bar behind a 6-unit notch.
MASTER = dict(x0=4, x1=60, y0=8, y1=40,   # window body
              gap=4,                       # perpendicular width of the flap seam
              split=(20, 24),              # the :vsplit seam
              apex=24,                     # where the flap seam's centre lines meet (x 32)
              sl0=44, sl1=56,              # statusline top and bottom
              chip=16,                     # chip body ends at x 16; 45 degree tip at x 22
              notch=6)                     # horizontal chip-to-bar gap
X0, X1, Y0, Y1, SL1 = (MASTER[k] for k in ("x0", "x1", "y0", "y1", "sl1"))


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


def svg(w, h, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {f(w)} {f(h)}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def mark_body(fg, accent, dx=0.0):
    win, bar, chip = mark_paths()
    t = f' transform="translate({f(dx)} 0)"' if dx else ""
    return (f'<g{t}><path fill="{fg}" d="{win}{bar}"/>'
            f'<path fill="{accent}" d="{chip}"/></g>')


def mono_body():
    win, bar, chip = mark_paths()
    return f'<path fill="currentColor" d="{win}{bar}{chip}"/>'


def write_marks():
    (HERE / "mark.svg").write_text(svg(64, 64, mark_body(*THEMES["light"])))
    (HERE / "mark-dark.svg").write_text(svg(64, 64, mark_body(*THEMES["dark"])))
    (HERE / "mark-mono.svg").write_text(svg(64, 64, mono_body()))


# ---------------------------------------------------------------- small sizes
# At 16 px the master's diagonals are anti-aliased grey. favicon.svg is therefore drawn as
# pixel art on the 16 px grid (every edge on a whole pixel, so it renders crisp): an ink
# tile with a 1 px keyline (a floating-window border, and the tile's edge on dark tab bars),
# and inside it the same layout: a 12 x 8 window with a 1:1 stepped V flap (deeper than the
# master's, so it still reads as a flap at 16 px), the split dropping from the flap's left arm, and a 3 px statusline whose chip ends in a pixel arrow under the split.
# "#" = window/bar, "a" = mode chip, "." = tile.
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
FAV_AT = (2, 2)            # top-left pixel of the drawing inside the 16 px tile
KEYLINE = PAPER            # the tile edge on dark tab bars; melts into light ones
TILE = "M3 0H13A3 3 0 0 1 16 3V13A3 3 0 0 1 13 16H3A3 3 0 0 1 0 13V3A3 3 0 0 1 3 0Z"
TILE_IN = "M3 1H13A2 2 0 0 1 15 3V13A2 2 0 0 1 13 15H3A2 2 0 0 1 1 13V3A2 2 0 0 1 3 1Z"

# The terminal logo is pixel art on its own grid: 21 x 16 px, printed as 8 rows of half
# blocks (a cell is 1 px wide and 2 px tall). Same layout again: the split (column 6)
# drops from the flap's left arm, and the statusline's one-cell notch sits under it.
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
LABEL = "NORMAL"           # printed in the chip cells of logo.ans, ink on amber


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


def write_favicon():
    body = (f'<path fill="{KEYLINE}" d="{TILE}"/><path fill="{INK}" d="{TILE_IN}"/>'
            f'<path fill="{OFF}" d="{pixel_path(FAV, "#", FAV_AT)}"/>'
            f'<path fill="{AMBER}" d="{pixel_path(FAV, "a", FAV_AT)}"/>')
    (HERE / "favicon.svg").write_text(svg(16, 16, body))


def term_cells(grid):
    """Pair pixel rows into half-block cells: list of rows of (char, top, bottom)."""
    out = []
    for y in range(0, len(grid), 2):
        top, bot = grid[y], grid[y + 1] if y + 1 < len(grid) else "." * len(grid[y])
        row = []
        for t, b in zip(top, bot):
            ch = {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}[(t != ".", b != ".")]
            row.append((ch, t, b))
        out.append(row)
    return out


def rgb(hexc):
    return tuple(int(hexc[i:i + 2], 16) for i in (1, 3, 5))


def chip_label(row):
    """Map chip columns of a full-chip cell row to the centred LABEL characters."""
    cols = [i for i, (_, t, b) in enumerate(row) if t == "a" and b == "a"]
    if len(cols) < len(LABEL):
        return {}
    pad = (len(cols) - len(LABEL)) // 2
    return {cols[pad + k]: ch for k, ch in enumerate(LABEL)}


def write_term():
    cells = term_cells(TERM)
    txt = ["".join(c for c, _, _ in row).rstrip() for row in cells]
    (HERE / "logo.txt").write_text("\n".join(txt) + "\n")
    label_row = max(i for i, row in enumerate(cells) if chip_label(row))
    lines = []
    for i, row in enumerate(cells):
        labels = chip_label(row) if i == label_row else {}
        s, cur = "", "start"
        for j, (ch, t, b) in enumerate(row):
            if j in labels:   # reverse-video look: ink letter on an amber cell
                state = "label"
                code = "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm" % (rgb(INK) + rgb(AMBER))
                ch = labels[j]
            else:
                ink = t if ch in "█▀" else b
                state = AMBER if ink == "a" else None   # the window uses the terminal's own fg
                code = ("\x1b[0;38;2;%d;%d;%dm" % rgb(AMBER)) if state else "\x1b[0;39m"
            if ch != " " and state != cur:
                s += code
                cur = state
            s += ch
        lines.append(s.rstrip() + "\x1b[0m")
    (HERE / "logo.ans").write_text("\n".join(lines) + "\n")


# ---------------------------------------------------------------- lockups
WORD_AXES = (600, 32)     # Adwaita Sans wght 600, opsz 32
WORD_GAP = 20             # mark right edge (x 60) to the ink of the "n"
MARGIN = 4                # same side margin as the mark


def write_lockups():
    import text
    wght, opsz = WORD_AXES
    top = text.metric("l", wght, opsz, "ymax")
    scale = (SL1 - Y0) / top                  # "l" spans window top (8) to statusline bottom (56)
    x = X1 + WORD_GAP - text.metric("n", wght, opsz, "xmin") * scale
    d, _ = text.outline("neomail", x, SL1, scale, wght, opsz)
    last_x = text.shape("neomail", wght, opsz)[0][-1][1]   # pen position of the final "l"
    right = x + (last_x + text.metric("l", wght, opsz, "xmax")) * scale
    w = math.ceil(right + MARGIN)
    for name, (fg, accent) in (("lockup-light", THEMES["light"]), ("lockup-dark", THEMES["dark"])):
        body = mark_body(fg, accent) + f'<path fill="{fg}" d="{d}"/>'
        (HERE / f"{name}.svg").write_text(svg(w, 64, body))


# ---------------------------------------------------------------- preview sheet
TMP = Path("/tmp/neomail-logo-05/build")
SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONOF = "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf"
MUTED = "#8B98A5"


def render(name, w=None, h=None, color=None):
    """rsvg-convert an SVG from this folder to a PIL image (transparent background)."""
    from PIL import Image
    TMP.mkdir(parents=True, exist_ok=True)
    src = HERE / name
    if color:  # recolour the mono master
        src = TMP / f"{color[1:]}-{name}"
        src.write_text((HERE / name).read_text().replace("currentColor", color))
    out = TMP / f"{src.stem}-{w}x{h}.png"
    args = ["rsvg-convert"] + (["-w", str(w)] if w else []) + (["-h", str(h)] if h else [])
    subprocess.run(args + [str(src), "-o", str(out)], check=True)
    return Image.open(out).convert("RGBA")


def write_preview():
    from PIL import Image, ImageDraw, ImageFont
    W, H = 1600, 1000
    sheet = Image.new("RGB", (W, H), "#24292F")
    dr = ImageDraw.Draw(sheet)
    font = ImageFont.truetype(SANS, 14)
    mono = ImageFont.truetype(MONOF, 16)

    def put(im, x, y, flat_bg=None):
        if flat_bg:  # composite on a flat background first, so 8x zooms show real pixels
            base = Image.new("RGBA", im.size, flat_bg)
            base.alpha_composite(im)
            im = base
        sheet.paste(im, (x, y), im)

    def zoom(name, size, bg, k=8):
        base = Image.new("RGBA", (size, size), bg)
        base.alpha_composite(render(name, size, size))
        return base.resize((size * k, size * k), Image.NEAREST)

    for i, (bg, fg, mark) in enumerate(((INK, OFF, "mark-dark.svg"), (PAPER, INK, "mark.svg"))):
        x0 = i * 800
        dr.rectangle([x0, 0, x0 + 799, 639], fill=bg)
        tone = MUTED if bg == INK else "#57606A"
        for y, name, label in ((44, mark, mark), (340, "favicon.svg", "favicon.svg (drawn for 16 px)")):
            dr.text((x0 + 16, y - 26), f"{label}   256 / 64 / 32 / 16 px, 8x zooms of 16 and 32",
                    font=font, fill=tone)
            put(render(name, 256, 256), x0 + 16, y)
            put(render(name, 64, 64), x0 + 288, y)
            put(render(name, 32, 32), x0 + 288, y + 80)
            put(render(name, 16, 16), x0 + 288, y + 128)
            put(zoom(name, 16, bg), x0 + 376, y)
            put(zoom(name, 32, bg), x0 + 520, y)

    # lockups and mono
    dr.rectangle([0, 640, 759, 819], fill=PAPER)
    dr.rectangle([0, 820, 759, 999], fill=INK)
    put(render("lockup-light.svg", None, 96), 32, 682)
    put(render("lockup-dark.svg", None, 96), 32, 862)
    dr.rectangle([760, 640, 999, 819], fill="#FFFFFF")
    dr.rectangle([760, 820, 999, 999], fill="#000000")
    for y, col in ((650, "#000000"), (830, "#FFFFFF")):
        put(render("mark-mono.svg", 128, 128, col), 776, y + 16)
        put(render("mark-mono.svg", 32, 32, col), 920, y + 56)
        put(render("mark-mono.svg", 16, 16, col), 960, y + 64)
    dr.text((776, 646), "mark-mono.svg", font=font, fill="#57606A")

    # mock terminal splash with logo.txt
    tx, ty, tw, th = 1016, 656, 568, 328
    dr.rounded_rectangle([tx, ty, tx + tw, ty + th], 10, fill=INK, outline="#30363D")
    dr.text((tx + tw // 2, ty + 14), "nmail - ghostty", font=font, fill=MUTED, anchor="mm")
    dr.line([tx, ty + 28, tx + tw, ty + 28], fill="#30363D")
    def draw_cells(lx, ly, cw, ch, labels=True):
        """Paint logo.txt as a terminal would; with labels, the chip row of logo.ans."""
        cells = term_cells(TERM)
        lab_row = max(i for i, row in enumerate(cells) if chip_label(row))
        cfont = ImageFont.truetype(MONOF, int(ch * 0.8))
        for r, line in enumerate((HERE / "logo.txt").read_text().splitlines()):
            lab = chip_label(cells[r]) if labels and r == lab_row else {}
            for c, glyph in enumerate(line):
                col = AMBER if "a" in TERM[2 * r][c] + TERM[2 * r + 1][c] else OFF
                x, y = lx + c * cw, ly + r * ch
                if glyph in "█▀":
                    dr.rectangle([x, y, x + cw - 1, y + ch // 2 - 1], fill=col)
                if glyph in "█▄":
                    dr.rectangle([x, y + ch // 2, x + cw - 1, y + ch - 1], fill=col)
                if c in lab:
                    dr.text((x + cw / 2, y + ch / 2), lab[c], font=cfont, fill=INK, anchor="mm")

    lx, ly = tx + 36, ty + 60
    draw_cells(lx, ly, 10, 20)                       # logo.ans
    draw_cells(tx + 290, ty + 214, 4, 8, labels=False)  # logo.txt, plain
    dr.text((tx + 384, ty + 246), "logo.txt (no label)", font=font, fill=MUTED, anchor="lm")
    info = ("neomail 0.1.0", "vim-modal terminal mail", "", "i  inbox     12", "/  search", ":q quit")
    for k, s in enumerate(info):
        dr.text((lx + 254, ly + 4 + k * 24), s, font=mono, fill=OFF if k == 0 else MUTED)
    sy = ty + th - 34
    dr.rectangle([tx + 1, sy, tx + tw - 1, sy + 22], fill="#21262D")
    dr.rectangle([tx + 1, sy, tx + 90, sy + 22], fill=AMBER)
    dr.polygon([(tx + 91, sy), (tx + 102, sy + 11), (tx + 91, sy + 22)], fill=AMBER)
    dr.text((tx + 12, sy + 11), "NORMAL", font=mono, fill=INK, anchor="lm")
    dr.text((tx + 116, sy + 11), "inbox  12 unread", font=mono, fill=OFF, anchor="lm")
    dr.text((tx + tw - 12, sy + 11), "1:1", font=mono, fill=MUTED, anchor="rm")
    sheet.save(HERE / "preview.png", optimize=True)


if __name__ == "__main__":
    write_marks()
    write_favicon()
    write_term()
    write_lockups()
    write_preview()
    print("built", ", ".join(sorted(p.name for p in HERE.iterdir() if p.is_file())))
