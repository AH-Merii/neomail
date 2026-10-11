"""Render review sheets of the built logo set (run build.py first):

    python3 assets/logo/src/review.py [out_dir]     # default /tmp/neomail-logo-review

Writes review-dark.png and review-light.png: every icon at 16, 32, 64, 192 and 512 px with
8x/4x pixel zooms, the marks at 24, 48 and 128 px, both lockups, the mono files on three
backgrounds, the social card, and logo.ans drawn cell by cell as a terminal draws it.
"""
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # keep src/ free of __pycache__
import build  # noqa: F401  (re-runs this script under uv if Pillow is missing)
import brand as B
from PIL import Image, ImageDraw, ImageFont

LOGO = build.OUT
DEST = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("/tmp/neomail-logo-review")
TMP = DEST / "tmp"
SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONO = "/usr/share/fonts/noto/NotoSansMono-Regular.ttf"
MONO_BOLD = "/usr/share/fonts/noto/NotoSansMono-Bold.ttf"
SIZES = (16, 32, 64, 192, 512)
SHIPPED = {("favicon.svg", 32): "favicon-32.png", ("icon.svg", 192): "icon-192.png",
           ("icon.svg", 512): "icon-512.png"}
_n = [0]


def svg(name, w=None, h=None, color=None):
    """Render an SVG from assets/logo/ (mono files recoloured) to a PIL image."""
    src = LOGO / name
    if color:
        _n[0] += 1
        src = TMP / f"c{_n[0]}-{name}"
        src.write_text((LOGO / name).read_text().replace("currentColor", color))
    _n[0] += 1
    out = TMP / f"r{_n[0]}.png"
    args = ["rsvg-convert"] + (["-w", str(w)] if w else []) + (["-h", str(h)] if h else [])
    subprocess.run(args + [str(src), "-o", str(out)], check=True)
    return Image.open(out).convert("RGBA")


def icon(name, size, color=None):
    shipped = SHIPPED.get((name, size))
    if shipped:
        return Image.open(LOGO / shipped).convert("RGBA")
    return svg(name, size, size, color)


def zoom(im, bg, k):
    base = Image.new("RGBA", im.size, bg)
    base.alpha_composite(im)
    return base.resize((im.width * k, im.height * k), Image.NEAREST)


SGR = re.compile(r"\x1b\[([0-9;]*)m")


def parse_ans(text):
    """Rows of (char, fg, bg, bold, dim); fg/bg None = terminal default."""
    rows = []
    for line in text.rstrip("\n").split("\n"):
        cells, fg, bg, bold, dim, pos = [], None, None, False, False, 0
        for m in SGR.finditer(line):
            cells += [(ch, fg, bg, bold, dim) for ch in line[pos:m.start()]]
            pos = m.end()
            codes = [int(c) if c else 0 for c in m.group(1).split(";")]
            i = 0
            while i < len(codes):
                c = codes[i]
                if c == 0:
                    fg = bg = None
                    bold = dim = False
                elif c in (1, 2):
                    bold, dim = (True, dim) if c == 1 else (bold, True)
                elif c == 39:
                    fg = None
                elif c in (38, 48) and codes[i + 1] == 2:
                    if c == 38:
                        fg = tuple(codes[i + 2:i + 5])
                    else:
                        bg = tuple(codes[i + 2:i + 5])
                    i += 4
                i += 1
        cells += [(ch, fg, bg, bold, dim) for ch in line[pos:]]
        rows.append(cells)
    return rows


def draw_term(d, rows, x, y, cw, ch, dfg, dbg, dimfg):
    fonts = {b: ImageFont.truetype(MONO_BOLD if b else MONO, int(ch * 0.7)) for b in (0, 1)}
    for r, cells in enumerate(rows):
        for c, (g, fg, bg, bold, dim) in enumerate(cells):
            fg = fg or (dimfg if dim else dfg)
            x0, y0 = x + c * cw, y + r * ch
            if bg:
                d.rectangle((x0, y0, x0 + cw - 1, y0 + ch - 1), fill=bg)
            if g == "█":
                d.rectangle((x0, y0, x0 + cw - 1, y0 + ch - 1), fill=fg)
            elif g == "▀":
                d.rectangle((x0, y0, x0 + cw - 1, y0 + ch // 2 - 1), fill=fg)
            elif g == "▄":
                d.rectangle((x0, y0 + ch // 2, x0 + cw - 1, y0 + ch - 1), fill=fg)
            elif g != " ":
                d.text((x0 + cw / 2, y0 + ch / 2), g, fill=fg, font=fonts[bold], anchor="mm")


def sheet(dark):
    bg, fg = (B.INK, B.OFF) if dark else (B.PAPER, B.INK)
    muted, line = (B.MUTED, "#30363D") if dark else ("#57606A", "#D0D7DE")
    W = 2000
    img = Image.new("RGBA", (W, 4000), bg)
    d = ImageDraw.Draw(img)
    f14, f18, f30 = (ImageFont.truetype(SANS, s) for s in (16, 20, 34))
    put = lambda im, x, y: img.alpha_composite(im, (int(x), int(y)))
    d.text((40, 30), f"neomail logo, {'dark' if dark else 'light'} review ({bg})", fill=fg, font=f30)

    def title(y, s):
        d.text((40, y), s, fill=fg, font=f18)
        d.line((40, y + 30, W - 40, y + 30), fill=line)
        return y + 44

    y = title(100, "Icons at 16 / 32 / 64 / 192 / 512 px (shipped PNGs where they exist), "
                   "then 8x zoom of 16 px and 4x zoom of 32 px")
    for name, col in (("favicon.svg", None), ("icon.svg", None), ("icon-mono.svg", fg)):
        d.text((40, y), name, fill=fg, font=f18)
        d.text((40, y + 28), "recoloured " + col if col else "", fill=muted, font=f14)
        x = 300
        for s in SIZES:
            put(icon(name, s, col), x, y)
            d.text((x, y - 2 + max(s, 16) + 6), f"{s}", fill=muted, font=f14)
            x += s + 40
        put(zoom(icon(name, 16, col), bg, 8), x, y)
        put(zoom(icon(name, 32, col), bg, 4), x + 168, y)
        d.text((x, y + 134), "16 px x8", fill=muted, font=f14)
        d.text((x + 168, y + 134), "32 px x4", fill=muted, font=f14)
        y += 512 + 60

    y = title(y, "Marks at 24 / 48 / 128 px: full colour, then mono (recoloured "
                 f"{fg}); both lockups at 96 px tall on their own backgrounds")
    x = 40
    for name, col in ((("neomail-mark-dark.svg" if dark else "neomail-mark.svg"), None),
                      ("neomail-mark-mono.svg", fg)):
        for s in (24, 48, 128):
            put(svg(name, s, s, col), x, y + 128 - s)
            x += s + 32
        d.text((x - 128 - 32, y + 136), name, fill=muted, font=f14)
        x += 80
    y += 180
    for k, (lk, pbg) in enumerate((("neomail-logo-light.svg", B.PAPER),
                                   ("neomail-logo-dark.svg", B.INK))):
        px = 40 + k * 970
        d.rectangle((px, y, px + 940, y + 180), fill=pbg, outline=line)
        put(svg(lk, None, 96), px + 40, y + 42)
        d.text((px + 12, y + 8), lk, fill=B.MUTED if pbg == B.INK else "#57606A", font=f14)
    y += 220

    y = title(y, "Mono files on three backgrounds: neomail-mark-mono 128, icon-mono 64 / 32 / 16, "
                 "neomail-logo-mono 64 tall")
    trios = ((B.INK, B.OFF), ("#35363A", "#FFFFFF"), (B.AMBER, B.INK)) if dark else \
            ((B.PAPER, B.INK), ("#DEE1E6", B.INK), ("#2F6FEB", "#FFFFFF"))
    for k, (pbg, col) in enumerate(trios):
        px = 40 + k * 647
        d.rectangle((px, y, px + 620, y + 300), fill=pbg, outline=line)
        d.text((px + 12, y + 8), f"{col} on {pbg}", fill=col, font=f14)
        put(svg("neomail-mark-mono.svg", 128, 128, col), px + 24, y + 40)
        for j, s in enumerate((64, 32, 16)):
            put(svg("icon-mono.svg", s, s, col), px + 180 + (0, 88, 144)[j], y + 104 - s)
        put(svg("neomail-logo-mono.svg", None, 64, col), px + 24, y + 200)
    y += 340

    y = title(y, "social-preview.png at 75 %, and logo.ans drawn cell by cell (14 x 28 px cells, "
                 "terminal default fg " + fg + ")")
    card = Image.open(LOGO / "social-preview.png").convert("RGBA")
    put(card.resize((960, 480), Image.LANCZOS), 40, y)
    d.rectangle((39, y - 1, 1000, y + 480), outline=line)
    rows = parse_ans((LOGO / "logo.ans").read_text())
    cols = max(len(r) for r in rows)
    tx, tw, th = 1040, cols * 14 + 48, len(rows) * 28 + 48
    d.rectangle((tx, y, tx + tw, y + th), fill=bg, outline=line)
    draw_term(d, rows, tx + 24, y + 24, 14, 28, B.rgb(fg), B.rgb(bg), B.rgb(muted))
    d.text((tx, y + th + 12), f"logo.ans: {cols} columns x {len(rows)} rows", fill=muted, font=f14)
    y += 500
    return img.crop((0, 0, W, y + 20)).convert("RGB")


def main():
    TMP.mkdir(parents=True, exist_ok=True)
    for dark in (True, False):
        out = DEST / f"review-{'dark' if dark else 'light'}.png"
        sheet(dark).save(out, optimize=True)
        print(out)
    shutil.rmtree(TMP)


if __name__ == "__main__":
    main()
