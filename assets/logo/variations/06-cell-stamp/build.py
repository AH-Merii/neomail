"""Cell Stamp: build every asset of this variation from one 15x15 bitmap.

    uv run --with pillow --with uharfbuzz --with fonttools python build.py

The bitmap below is the single source of truth for the picture. Each character
is one square pixel, which is also one half of a terminal cell, so the SVGs,
the favicon and the half-block art show the same drawing. The large masters
(mark*.svg, lockups) cut the perforation as round bites; the favicon and the
terminal keep square, pixel-exact holes.
"""
import math
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

OUT = Path(__file__).resolve().parent
sys.path.insert(0, str(OUT.parent.parent / "src"))
import text as T  # noqa: E402  (../../src/text.py: HarfBuzz + fontTools, Adwaita Sans)

TMP = Path("/tmp/neomail-logo-06/build")

# . transparent (a perforation hole or outside)   p stamp paper (vermilion)
# k picture field (the terminal screen)            c the @ (cream)
# a the block cursor: the right stem of the @'s "a"
STAMP = [
    ".p.p.p.p.p.p.p.",
    "ppppppppppppppp",
    ".pkkkkkkkkkkkp.",
    "ppkkkccccckkkpp",
    ".pkkckkkkkckkp.",
    "ppkckkkkkkkckpp",
    ".pkckkccaakckp.",
    "ppkckckkaakckpp",
    ".pkckckkaakckp.",
    "ppkckkccaackkpp",
    ".pkkckkkkkkkkp.",
    "ppkkkccccckkkpp",
    ".pkkkkkkkkkkkp.",
    "ppppppppppppppp",
    ".p.p.p.p.p.p.p.",
]
W = H = 15
FIELD = (2, 2, 13, 13)      # x0, y0, x1, y1 of the picture field

# Palette
BG_DARK, BG_LIGHT = "#0D1117", "#F6F8FA"
VERMILION = "#F26B3D"      # stamp paper on dark backgrounds
VERMILION_L = "#D9542B"    # stamp paper on light backgrounds and in the favicon
SCREEN = "#2B3548"         # picture field, lifted so it separates from #0D1117
CREAM = "#F7EEDC"          # the @, wordmark on dark
AMBER = "#E8A000"          # the cursor, darker than the cream in luminance too
INK = "#0D1117"            # wordmark on light

THEMES = {
    "dark": {"p": VERMILION, "k": SCREEN, "c": CREAM, "a": AMBER, "word": CREAM, "bg": BG_DARK},
    "light": {"p": VERMILION_L, "k": SCREEN, "c": CREAM, "a": AMBER, "word": INK, "bg": BG_LIGHT},
}
MONO_KEYS = "pca"          # in mono the picture field is cut out

# Perforation of the vector masters: round bites centred on the paper edge.
HOLE_R = 0.62                      # edge holes, centred at x = 2.5, 4.5 ... 12.5
CORNER_R = 0.5 + HOLE_R            # corner bite; makes every tooth the same width

WORD = "neomail"
WORD_AXES = {"wght": 640, "opsz": 32}    # same instance as the repo's main wordmark
WORD_L = 8.6                # height of the "l" in pixels
WORD_GAP = 3                # stamp edge to the ink of the "n"


def fmt(v):
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def runs(bitmap, keys, dx=0, dy=0):
    """Horizontal runs of the given keys as one path 'd' (pixel-exact rectangles)."""
    d = []
    for y, row in enumerate(bitmap):
        x = 0
        while x < len(row):
            if row[x] in keys:
                x1 = x
                while x1 < len(row) and row[x1] in keys:
                    x1 += 1
                d.append(f"M{x + dx} {y + dy}h{x1 - x}v1h{x - x1}z")
                x = x1
            else:
                x += 1
    return "".join(d)


def perforated_paper():
    """Clockwise outline of the stamp paper with round perforation bites."""
    c, r, R = W / 2, HOLE_R, CORNER_R
    rot = lambda p, k: [(c + (p[0] - c) * a - (p[1] - c) * b, c + (p[0] - c) * b + (p[1] - c) * a)
                        for a, b in [(round(math.cos(k * math.pi / 2)), round(math.sin(k * math.pi / 2)))]][0]
    d = []
    for side in range(4):               # top, right, bottom, left (rotate the top edge)
        segs = [("L", (R, 0))] if side == 0 else []   # each corner arc ends where the next side starts
        for cx in [2.5 + 2 * i for i in range(6)]:
            segs += [("L", (cx - r, 0)), ("A", r, (cx + r, 0))]
        segs += [("L", (W - R, 0)), ("A", R, (W, R))]
        for s in segs:
            x, y = rot(s[-1], side)
            if s[0] == "L":
                d.append(f"{'M' if not d else 'L'}{fmt(x)} {fmt(y)}")
            else:
                d.append(f"A{fmt(s[1])} {fmt(s[1])} 0 0 0 {fmt(x)} {fmt(y)}")
    return "".join(d) + "Z"


def field_rect():
    x0, y0, x1, y1 = FIELD
    return f"M{x0} {y0}H{x1}V{y1}H{x0}Z"


def svg(vb, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def stamp_paths(theme, round_holes=True):
    if theme == "mono":
        if round_holes:
            return (f'<path fill="currentColor" fill-rule="evenodd" d="{perforated_paper()}{field_rect()}"/>'
                    f'<path fill="currentColor" d="{runs(STAMP, "ca")}"/>')
        return f'<path fill="currentColor" d="{runs(STAMP, MONO_KEYS)}"/>'
    t = THEMES[theme]
    paper = perforated_paper() if round_holes else runs(STAMP, "p")
    field = field_rect() if round_holes else runs(STAMP, "k")
    return (f'<path fill="{t["p"]}" d="{paper}"/><path fill="{t["k"]}" d="{field}"/>'
            f'<path fill="{t["c"]}" d="{runs(STAMP, "c")}"/><path fill="{t["a"]}" d="{runs(STAMP, "a")}"/>')


def wordmark():
    """Outlined WORD: x-height centred on the stamp, 'n' ink WORD_GAP right of the stamp."""
    w, o = WORD_AXES["wght"], WORD_AXES["opsz"]
    s = WORD_L / T.metric("l", w, o, "ymax")
    baseline = H / 2 + T.metric("x", w, o, "ymax") * s / 2
    x0 = W + WORD_GAP - T.metric("n", w, o, "xmin") * s
    d, adv = T.outline(WORD, x0, baseline, s, w, o)
    right = x0 + adv - (T.shape("l", w, o)[1] - T.metric("l", w, o, "xmax")) * s
    return d, right


def build_svgs():
    files = {}
    # Masters: 15 px art centred in a 16-unit square. Pixel edges land on whole
    # device pixels at 32, 64, 128 ... px; at 16 px use favicon.svg instead.
    vb = "-0.5 -0.5 16 16"
    files["mark.svg"] = svg(vb, stamp_paths("light"))
    files["mark-dark.svg"] = svg(vb, stamp_paths("dark"))
    files["mark-mono.svg"] = svg(vb, stamp_paths("mono"))
    # Favicon: the bitmap with square holes, flush top-left on an exact 16 px
    # grid, so it is pixel-exact at 16 and 32 px.
    files["favicon.svg"] = svg("0 0 16 16", stamp_paths("light", round_holes=False))
    d, right = wordmark()
    vw = fmt(right + 1)
    for theme in ("light", "dark"):
        t = THEMES[theme]
        body = stamp_paths(theme) + f'<path fill="{t["word"]}" d="{d}"/>'
        files[f"lockup-{theme}.svg"] = svg(f"-0.5 -0.5 {vw} 16", body)
    for name, text in files.items():
        (OUT / name).write_text(text)
    return files


# ---------------------------------------------------------------- terminal art

def cells(bitmap):
    """Pair pixel rows into terminal rows: [(top_key, bottom_key)] per cell."""
    rows = bitmap + ["." * len(bitmap[0])] * (len(bitmap) % 2)
    return [[(rows[y][x], rows[y + 1][x]) for x in range(len(rows[0]))]
            for y in range(0, len(rows), 2)]


def logo_txt():
    """Plain half blocks only: stamp paper, @ and cursor are ink; field and holes are blank."""
    lines = []
    for row in cells(STAMP):
        line = ""
        for top, bot in row:
            t, b = top in MONO_KEYS, bot in MONO_KEYS
            line += {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}[(t, b)]
        lines.append(line.rstrip())
    return "\n".join(lines) + "\n"


def rgb(hexcol):
    h = hexcol.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


def logo_ans(theme="dark"):
    """24-bit ANSI: ▀ with fg = top pixel and bg = bottom pixel; holes stay transparent."""
    pal = THEMES[theme]
    fg = lambda c: "\x1b[38;2;%d;%d;%dm" % rgb(c)
    bg = lambda c: "\x1b[48;2;%d;%d;%dm" % rgb(c)
    reset = "\x1b[0m"
    lines = []
    for row in cells(STAMP):
        line = ""
        for top, bot in row:
            if top == "." and bot == ".":
                line += reset + " "
            elif bot == ".":
                line += reset + fg(pal[top]) + "▀"
            elif top == ".":
                line += reset + fg(pal[bot]) + "▄"
            elif top == bot:
                line += reset + fg(pal[top]) + "█"
            else:
                line += fg(pal[top]) + bg(pal[bot]) + "▀"
        lines.append(line + reset)
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------- preview sheet

SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONO = "/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf"
MONO_B = "/usr/share/fonts/Adwaita/AdwaitaMono-Bold.ttf"


def render(name, w=None, h=None, bg=None, color=None):
    """rsvg-convert one SVG of this folder to a PIL image (optionally recolouring currentColor)."""
    TMP.mkdir(parents=True, exist_ok=True)
    src = OUT / name
    if color:
        src = TMP / f"{color.lstrip('#')}-{name}"
        src.write_text((OUT / name).read_text().replace("currentColor", color))
    dst = TMP / f"{src.stem}-{w}x{h}-{(bg or 'none').lstrip('#')}.png"
    args = ["rsvg-convert"] + (["-w", str(w)] if w else []) + (["-h", str(h)] if h else [])
    args += (["-b", bg] if bg else []) + [str(src), "-o", str(dst)]
    subprocess.run(args, check=True)
    return Image.open(dst).convert("RGBA")


def zoom(img, k):
    return img.resize((img.width * k, img.height * k), Image.NEAREST)


def draw_cells(d, x, y, grid, cw, ch):
    """grid: rows of (top_colour|None, bottom_colour|None) half-block cells."""
    for r, row in enumerate(grid):
        for c, (top, bot) in enumerate(row):
            x0, y0 = x + c * cw, y + r * ch
            if top:
                d.rectangle([x0, y0, x0 + cw - 1, y0 + ch // 2 - 1], fill=top)
            if bot:
                d.rectangle([x0, y0 + ch // 2, x0 + cw - 1, y0 + ch - 1], fill=bot)


def txt_grid(text, fg):
    """Parse logo.txt back into half-block cells (fails loudly on any other character)."""
    table = {"█": (fg, fg), "▀": (fg, None), "▄": (None, fg), " ": (None, None)}
    return [[table[ch] for ch in line] for line in text.splitlines()]


def paste(sheet, im, x, y):
    sheet.paste(im, (x, y), im)


def preview():
    fonts = {"label": ImageFont.truetype(SANS, 17), "head": ImageFont.truetype(SANS, 22),
             "mono": ImageFont.truetype(MONO, 22), "mono_b": ImageFont.truetype(MONO_B, 22)}
    sheet = Image.new("RGB", (1600, 1000), "#1C2128")
    d = ImageDraw.Draw(sheet)
    for i, (mark, bg, fg) in enumerate((("mark-dark.svg", BG_DARK, "#8B949E"),
                                        ("mark.svg", BG_LIGHT, "#57606A"))):
        ox = i * 800
        d.rectangle([ox, 0, ox + 799, 519], fill=bg)
        d.text((ox + 40, 22), f"{mark} on {bg}", fill=fg, font=fonts["head"])
        paste(sheet, render(mark, 256, 256), ox + 40, 64)
        d.text((ox + 40, 332), "256 px", fill=fg, font=fonts["label"])
        x = ox + 340
        for s, name in ((64, mark), (32, mark), (16, "favicon.svg")):
            paste(sheet, render(name, s, s), x, 64 + (64 - s) // 2)
            d.text((x, 140), f"{s}" + (" favicon" if s == 16 else ""), fill=fg, font=fonts["label"])
            x += s + 40
        z16 = zoom(render("favicon.svg", 16, 16, bg), 8)
        z32 = zoom(render(mark, 32, 32, bg), 8)
        sheet.paste(z16, (ox + 340, 200))
        sheet.paste(z32, (ox + 500, 200))
        d.text((ox + 340, 466), "favicon 16 ×8", fill=fg, font=fonts["label"])
        d.text((ox + 500, 466), "mark 32 ×8", fill=fg, font=fonts["label"])
        d.rectangle([ox + 339, 199, ox + 468, 328], outline="#808080")
        d.rectangle([ox + 499, 199, ox + 756, 456], outline="#808080")
        # grayscale proof: the cursor must not fuse with the cream @
        gray = render(mark, 128, 128, bg).convert("L").convert("RGBA")
        sheet.paste(gray, (ox + 40, 372))
        d.text((ox + 180, 490), "128 px, grayscale", fill=fg, font=fonts["label"])
    for j, (name, bg) in enumerate((("lockup-light.svg", BG_LIGHT), ("lockup-dark.svg", BG_DARK))):
        y = 520 + j * 130
        d.rectangle([0, y, 799, y + 129], fill=bg)
        paste(sheet, render(name, None, 80), 40, y + 25)
        sm = render(name, None, 32)
        paste(sheet, sm, 800 - 40 - sm.width, y + 49)
    # mono mark (black on white, white on black) and the plain logo.txt
    d.rectangle([0, 780, 399, 999], fill="#FFFFFF")
    d.rectangle([400, 780, 799, 999], fill="#000000")
    x = 30
    for s in (128, 64, 32):
        paste(sheet, render("mark-mono.svg", s, s, color="#000000"), x, 812 + (128 - s) // 2)
        x += s + 24
    d.text((30, 962), "mark-mono.svg", fill="#57606A", font=fonts["label"])
    draw_cells(d, 430, 812, txt_grid((OUT / "logo.txt").read_text(), "#D0D7DE"), 10, 20)
    paste(sheet, render("mark-mono.svg", 64, 64, color="#FFFFFF"), 610, 840)
    d.text((430, 962), "logo.txt     mark-mono (white)", fill="#8B949E", font=fonts["label"])
    return sheet, d, fonts


def splash(sheet, d, fonts):
    """Mock nmail start screen in a Neovim-style empty buffer, drawn cell by cell from logo.ans.

    The stamp is logo.ans; "neomail" and everything else is ordinary terminal text."""
    cw, ch, cols, rows = 13, 26, 52, 14
    pad, bar, x, y = 16, 30, 846, 540
    w, h = cols * cw + 2 * pad, rows * ch + 2 * pad + bar
    d.rounded_rectangle([x, y, x + w, y + h], 12, fill="#0D1117", outline="#30363D", width=2)
    d.text((x + w // 2, y + bar // 2 + 4), "nmail", fill="#8B949E", font=fonts["label"], anchor="mm")
    gx, gy = x + pad, y + bar + pad
    dim, muted, fg = "#3D4B5C", "#8B949E", "#E6EDF3"

    def put(r, c, s, col, bold=False):
        d.text((gx + c * cw, gy + r * ch + ch // 2), s, fill=col,
               font=fonts["mono_b" if bold else "mono"], anchor="lm")

    for r in range(1, rows - 1):
        put(r, 0, "~", dim)
    pal = THEMES["dark"]
    grid = [[(pal.get(t), pal.get(b)) for t, b in row] for row in cells(STAMP)]
    c0 = (cols - W) // 2
    draw_cells(d, gx + c0 * cw, gy + 1 * ch, grid, cw, ch)
    put(9, (cols - len(WORD)) // 2, WORD, fg, bold=True)
    line = "vim-modal mail for the terminal"
    put(10, (cols - len(line)) // 2, line, muted)
    c = (cols - 24) // 2
    for part, col in (("type  ", muted), (":q<Enter>", CREAM), ("  to exit", muted)):
        put(11, c, part, col)
        c += len(part)
    d.rectangle([gx, gy + 13 * ch, gx + cols * cw - 1, gy + 14 * ch - 1], fill="#161B22")
    d.rectangle([gx, gy + 13 * ch, gx + 8 * cw - 1, gy + 14 * ch - 1], fill=AMBER)
    put(13, 1, "NORMAL", "#0D1117", bold=True)
    put(13, 10, "inbox", muted)
    put(13, cols - 9, "0 unread", muted)
    d.rectangle([gx, gy, gx + cw - 1, gy + ch - 1], fill=AMBER)   # the buffer's own cursor


def main():
    build_svgs()
    (OUT / "logo.txt").write_text(logo_txt())
    (OUT / "logo.ans").write_text(logo_ans())
    sheet, d, fonts = preview()
    splash(sheet, d, fonts)
    sheet.save(OUT / "preview.png", optimize=True)
    for name in sorted(p.name for p in OUT.iterdir() if p.is_file()):
        print(name)


if __name__ == "__main__":
    main()
