"""Letterfold (folder 01-prism-fold): build every asset in this folder from the geometry below.

    uv run --with pillow --with uharfbuzz --with fonttools python build.py
"""
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "src"))  # reuse text.py (Adwaita Sans outlines)

# ---------------------------------------------------------------- palette
INK, PAPER, OFF = "#0D1117", "#F6F8FA", "#E8EEF3"
PAL = {
    # face of the sheet: (light-background value, dark-background value)
    "front": ("#9C1B4D", "#F0436A"),  # mulberry on paper, raspberry on ink
    "back":  ("#B87400", "#FFC24B"),  # saffron: the inside of the letter
}
FACE = {"stem": "front", "bar": "back", "leg": "front"}

# ---------------------------------------------------------------- geometry
# Upright, on a 64-unit grid. One strip of letter paper in three panels, like a
# letter folded in thirds. The stem stands in front and rises 6 units above the
# fold (the spur of a monospace n). The middle panel shows the inside of the
# sheet with two ruled lines of writing cut out of it; its left end tucks behind
# the stem like an envelope's tuck flap. At the shoulder it folds down over the
# right leg along a 45-degree crease, which is the only diagonal in the mark.
# Every edge sits on a multiple of 4 units, so at 16 px (1 px = 4 units) the
# stems, counter, spur and bar land on whole pixels; the ruled lines sit on
# multiples of 2, so they are whole pixels at 32 px.
STEM_X, LEG_X, W = 8, 44, 12       # stems 12 wide (3 px at 16), counter 24 (6 px)
TOP, BAR_TOP, BAR_BOT, FOOT = 4, 12, 28, 60   # spur 8 units (2 px at 16)
LINES = [(24, 16, 40, 18), (24, 22, 34, 24)]  # x0, y0, x1, y1 of the writing


def facets(gap=0.0, top=TOP, foot=FOOT):
    """Polygons per panel; `gap` opens the two seams (used by the mono mark)."""
    r = LEG_X + W
    crease = r - (BAR_BOT - BAR_TOP)   # 45 degrees: as deep as the bar is tall
    return {
        "stem": [(STEM_X, top), (STEM_X + W, top), (STEM_X + W, foot), (STEM_X, foot)],
        "bar": [(STEM_X + W + gap, BAR_TOP), (crease, BAR_TOP), (r, BAR_BOT),
                (STEM_X + W + gap, BAR_BOT)],
        "leg": [(LEG_X, BAR_BOT + gap), (r, BAR_BOT + gap), (r, foot), (LEG_X, foot)],
    }


def holes():
    return [[(a, b), (c, b), (c, e), (a, e)] for a, b, c, e in LINES]


def num(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def d(pts):
    return "M" + "L".join(f"{num(x)} {num(y)}" for x, y in pts) + "Z"


def svg(vw, vh, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {num(vw)} {num(vh)}" '
            f'role="img" aria-label="{title}"><title>{title}</title>{body}</svg>\n')


ORDER = ("stem", "leg", "bar")


def placed(gap=0.0):
    return facets(gap)  # already centred: x 8-56, y 4-60


def panel_d(k, f):
    return d(f[k]) + ("".join(d(h) for h in holes()) if k == "bar" else "")


def mark_body(theme, gap=0.0):
    i = 0 if theme == "light" else 1
    f = placed(gap)
    return "".join(f'<path fill="{PAL[FACE[k]][i]}" fill-rule="evenodd" d="{panel_d(k, f)}"/>'
                   for k in ORDER)


MONO_GAP = 2.5


def mono_d():
    f = placed(MONO_GAP)
    return "".join(panel_d(k, f) for k in ORDER)


# ---------------------------------------------------------------- favicon
# The master already sits on the 16 px grid, so the favicon is the same three
# panels without the ruled lines (at 16 px they only add grey noise), on an ink
# tile so the dark palette works on any background. Stem and leg are 1 px
# shorter at each end so the mark keeps a 2 px margin inside the tile.
def fav_facets():
    return facets(top=8, foot=56)


TILE = "M12 0H52A12 12 0 0 1 64 12V52A12 12 0 0 1 52 64H12A12 12 0 0 1 0 52V12A12 12 0 0 1 12 0Z"


def favicon():
    f = fav_facets()
    body = f'<path fill="{INK}" d="{TILE}"/>' + "".join(
        f'<path fill="{PAL[FACE[k]][1]}" d="{d(f[k])}"/>' for k in ORDER)
    return svg(64, 64, body)


# ---------------------------------------------------------------- lockups
WORD, WORD_AXES, WORD_GAP = "neomail", (640, 32), 16
WORD_RATIO = 0.74  # ascender of the word / height of the mark


def bbox(f):
    xs = [x for v in f.values() for x, _ in v]
    ys = [y for v in f.values() for _, y in v]
    return min(xs), min(ys), max(xs), max(ys)


def lockup(theme):
    from text import metric, outline
    x0, top, x1, base = bbox(placed())
    wght, opsz = WORD_AXES
    scale = WORD_RATIO * (base - top) / metric("l", wght, opsz, "ymax")  # baseline = mark foot
    start = x1 + WORD_GAP - metric("n", wght, opsz, "xmin") * scale
    wd, adv = outline(WORD, start, base, scale, wght, opsz)
    right = start + adv
    pad = 4
    w = right - x0 + 2 * pad
    fg = INK if theme == "light" else OFF
    body = (f'<g transform="translate({num(pad - x0)} {num(pad - top)})">'
            + mark_body(theme) + f'<path fill="{fg}" d="{wd}"/></g>')
    return svg(w, base - top + 2 * pad, body)


# ---------------------------------------------------------------- half-blocks
# Drawn by hand on the pixel grid (two pixels per terminal cell): s = stem,
# b = bar, l = leg. Same build as the favicon: 2-pixel spur, 5-pixel bar with
# one ruled line cut out of it, and the crease stepping one column per row.
PIXELS = [
    "sssss............",
    "sssss............",
    "sssssbbbbbbbb....",
    "sssssbbbbbbbbb...",
    "sssssbb....bbbb..",
    "sssssbbbbbbbbbbb.",
    "sssssbbbbbbbbbbbb",
] + ["sssss.......lllll"] * 9


def pixel_grid():
    names = {"s": "stem", "b": "bar", "l": "leg"}
    return [[names.get(ch) for ch in row] for row in PIXELS]


def half_blocks(grid):
    txt, ans = [], []
    rgb = lambda h: tuple(int(h[i:i + 2], 16) for i in (1, 3, 5))
    for top, bot in zip(grid[0::2], grid[1::2]):
        t, a = "", ""
        for u, v in zip(top, bot):
            if u and v:
                t += "█"
                if FACE[u] == FACE[v]:
                    a += "\x1b[38;2;%d;%d;%dm█" % rgb(PAL[FACE[u]][1])
                else:
                    a += "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm▀\x1b[49m" % (rgb(PAL[FACE[u]][1]) + rgb(PAL[FACE[v]][1]))
            elif u or v:
                t += "▀" if u else "▄"
                a += "\x1b[38;2;%d;%d;%dm%s" % (rgb(PAL[FACE[u or v]][1]) + ("▀" if u else "▄",))
            else:
                t += " "
                a += " "
        txt.append(t.rstrip())
        ans.append(a.rstrip() + "\x1b[0m")
    return "\n".join(txt) + "\n", "\n".join(ans) + "\n"


# ---------------------------------------------------------------- preview
def run(*a):
    subprocess.run([str(x) for x in a], check=True)


def png(src, size=None, h=None):
    """Render an SVG to a PIL image (transparent background)."""
    from PIL import Image
    tmp = Path("/tmp/neomail-01-render.png")
    args = ["rsvg-convert"] + (["-w", size, "-h", size] if size else []) + (["-h", h] if h else [])
    run(*args, src, "-o", tmp)
    return Image.open(tmp).convert("RGBA").copy()


def font(size, mono=False):
    from PIL import ImageFont
    name = ("/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf" if mono
            else "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf")
    try:
        return ImageFont.truetype(name, size)
    except OSError:
        return ImageFont.load_default(size)


def zoomed(src, size, k, bg):
    from PIL import Image
    im = Image.new("RGBA", (size, size), bg)
    im.alpha_composite(png(src, size))
    return im.resize((size * k, size * k), Image.NEAREST)


def preview(grid):
    from PIL import Image, ImageDraw
    W, H = 1600, 1000
    sheet = Image.new("RGBA", (W, H), "#2A2F37")
    dr = ImageDraw.Draw(sheet)
    lab = font(15)
    for i, (theme, bg, fg, src) in enumerate((("dark", INK, OFF, "mark-dark.svg"),
                                             ("light", PAPER, INK, "mark.svg"))):
        ox = 16 + i * 792
        dr.rectangle((ox, 16, ox + 776, 560), fill=bg)
        dr.text((ox + 16, 26), f"mark on {bg}", fill=fg, font=lab)
        sheet.alpha_composite(png(HERE / src, 256), (ox + 16, 56))
        sheet.alpha_composite(zoomed(HERE / src, 32, 8, bg), (ox + 288, 56))
        sheet.alpha_composite(zoomed(HERE / src, 16, 8, bg), (ox + 560, 56))
        dr.text((ox + 288, 318), "32 px, 8x", fill=fg, font=lab)
        dr.text((ox + 560, 190), "16 px, 8x", fill=fg, font=lab)
        sheet.alpha_composite(zoomed(HERE / "favicon.svg", 16, 8, bg), (ox + 560, 220))
        dr.text((ox + 560, 354), "favicon 16 px, 8x", fill=fg, font=lab)
        x = ox + 16
        for s in (64, 32, 16):
            sheet.alpha_composite(png(HERE / src, s), (x, 400 + (64 - s) // 2))
            x += s + 24
        for s in (32, 16):
            sheet.alpha_composite(png(HERE / "favicon.svg", s), (x, 400 + (64 - s) // 2))
            x += s + 24
        dr.text((ox + 16, 480), "64 / 32 / 16 px  ·  favicon 32 / 16 px", fill=fg, font=lab)
        mono = png(HERE / "mark-mono.svg", 96)
        tint = Image.new("RGBA", mono.size, fg)
        tint.putalpha(mono.getchannel("A"))
        sheet.alpha_composite(tint, (ox + 560, 400))
        dr.text((ox + 672, 440), "mono", fill=fg, font=lab)
    # lockups
    for i, (name, bg) in enumerate((("lockup-light.svg", PAPER), ("lockup-dark.svg", INK))):
        y = 576 + i * 136
        dr.rectangle((16, y, 1000, y + 120), fill=bg)
        im = png(HERE / name, h=88)
        sheet.alpha_composite(im, (40, y + 16))
        sm = png(HERE / name, h=24)
        sheet.alpha_composite(sm, (40 + im.width + 48, y + 48))
    # terminal splash
    tx, ty, tw, th = 1016, 576, 568, 408
    dr.rounded_rectangle((tx, ty, tx + tw, ty + th), 10, fill="#0B0E13", outline="#3A414C")
    for j, c in enumerate(("#FF5F57", "#FEBC2E", "#28C840")):
        dr.ellipse((tx + 14 + j * 20, ty + 12, tx + 26 + j * 20, ty + 24), fill=c)
    cw, ch = 12, 24
    gx = tx + (tw - len(grid[0]) * cw) // 2
    gy = ty + 64
    for r, row in enumerate(grid):
        for c, k in enumerate(row):
            if k:
                y0 = gy + r * ch // 2
                dr.rectangle((gx + c * cw, y0, gx + (c + 1) * cw - 1, y0 + ch // 2 - 1), fill=PAL[FACE[k]][1])
    m = font(18, mono=True)
    dr.text((gx, gy + 8 * ch + 20), "neomail 0.1", fill=OFF, font=m)
    dr.text((gx, gy + 8 * ch + 48), "inbox 12  ·  sorted 9", fill="#8B98A5", font=m)
    dr.text((tx + 20, ty + th - 36), ":", fill=OFF, font=m)
    dr.rectangle((tx + 34, ty + th - 36, tx + 44, ty + th - 14), fill=PAL["back"][1])
    dr.text((24, 856), "Letterfold  ·  logo.txt in a 24-bit terminal (right), 12x24 px cells",
            fill="#C9D1D9", font=lab)
    sheet.convert("RGB").save(HERE / "preview.png", optimize=True)


# ---------------------------------------------------------------- main
def main():
    w = lambda name, s: (HERE / name).write_text(s)
    w("mark.svg", svg(64, 64, mark_body("light")))
    w("mark-dark.svg", svg(64, 64, mark_body("dark")))
    w("mark-mono.svg", svg(64, 64, f'<path fill="currentColor" fill-rule="evenodd" d="{mono_d()}"/>'))
    w("favicon.svg", favicon())
    w("lockup-light.svg", lockup("light"))
    w("lockup-dark.svg", lockup("dark"))
    grid = pixel_grid()
    txt, ans = half_blocks(grid)
    w("logo.txt", txt)
    w("logo.ans", ans)
    preview(grid)
    print(txt)


if __name__ == "__main__":
    main()
