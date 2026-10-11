"""Build the comparison sheets for the six neomail logo variations and the current mark.

Run from anywhere:
    uv run --with pillow python build_sheet.py

Writes sheet.png, sheet-light.png and small.png next to this file. SVGs are
rendered with rsvg-convert; the terminal art is drawn cell by cell the way a
terminal draws block elements, with any other characters set in Noto Sans Mono.
"""

from __future__ import annotations

import re
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
LOGO = HERE.parent

INK = (0x0D, 0x11, 0x17)
PAPER = (0xF6, 0xF8, 0xFA)
OFFWHITE = (0xE8, 0xEE, 0xF3)
MUTED = (0x8B, 0x98, 0xA5)
PANEL_DARK = (0x16, 0x1B, 0x22)
PANEL_LIGHT = (0xFF, 0xFF, 0xFF)
LINE_DARK = (0x30, 0x36, 0x3D)
LINE_LIGHT = (0xD0, 0xD7, 0xDE)

SANS = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
MONO = "/usr/share/fonts/noto/NotoSansMono-Regular.ttf"
MONO_BOLD = "/usr/share/fonts/noto/NotoSansMono-Bold.ttf"


def sans(size: int, weight: str = "Regular") -> ImageFont.FreeTypeFont:
    f = ImageFont.truetype(SANS, size)
    try:
        f.set_variation_by_name(weight)
    except Exception:
        pass
    return f


@dataclass
class Entry:
    num: str
    name: str
    folder: Path
    rank: int
    idea: str
    verdict: str
    mark: str = "mark.svg"
    mark_dark: str = "mark-dark.svg"
    lockup_light: str = "lockup-light.svg"
    lockup_dark: str = "lockup-dark.svg"
    favicon: str = "favicon.svg"
    favicon_light: str | None = None  # favicon for light backgrounds, if different
    mark32: str | None = None  # dedicated 32 px drawing, if any
    txt: str = "logo.txt"
    ans: str = "logo.ans"


V = HERE
ENTRIES = [
    Entry("05", "Split and statusline", V / "05-split-statusline", 1,
          "Envelope as a Neovim window: :vsplit panes under a V flap, amber NORMAL chip below.",
          "16 px favicon is pixel art: envelope in a window over an amber-tipped bar."),
    Entry("03", ":wq seal", V / "03-wq-seal", 2,
          "Envelope back closed with a carmine wax seal pressed with :w.",
          "Separate drawings per size: bare seal at 32 px, plain disc with :w at 16 px.",
          mark32="mark-32.svg"),
    Entry("06", "Cell Stamp", V / "06-cell-stamp", 3,
          "Perforated postage stamp with a pixel @ whose stem is a block cursor.",
          "Favicon is pixel-exact; at 16 px the perforation and colour carry it, not the @."),
    Entry("--", "Cursor n (current)", LOGO, 4,
          "Geometric lowercase n with a teal terminal-cursor block (PR #2).",
          "Simple n and cursor; reads at 16 px, but carries no mail sign.",
          mark="neomail-mark.svg", mark_dark="neomail-mark-dark.svg",
          lockup_light="neomail-logo-light.svg", lockup_dark="neomail-logo-dark.svg"),
    Entry("02", "Tilde Inbox", V / "02-tilde-inbox", 5,
          "Envelope that is an empty Vim buffer: cursor on line 1, ~ lines below.",
          "Master tildes blur at 16 px; the pixel favicon keeps two amber squiggles.",
          favicon_light="favicon-light.svg"),
    Entry("04", "hjkl plane", V / "04-hjkl-plane", 6,
          "Paper plane launched from an l-then-k motion path on the cell grid.",
          "Pixel favicon keeps plane, fold and both motion strokes at 16 px."),
    Entry("01", "Letterfold", V / "01-prism-fold", 7,
          "A monospace n folded from one strip of letter paper.",
          "At 16 px the writing smudges, so the mark says n and nothing about mail."),
]


# ---------------------------------------------------------------- rendering

_TMP = Path(tempfile.mkdtemp(prefix="nm-sheet-"))
_cache: dict[tuple, Image.Image] = {}


def svg(path: Path, w: int | None = None, h: int | None = None) -> Image.Image:
    key = (str(path), w, h)
    if key not in _cache:
        out = _TMP / f"{len(_cache)}.png"
        cmd = ["rsvg-convert", "-o", str(out)]
        if w:
            cmd += ["-w", str(w)]
        if h:
            cmd += ["-h", str(h)]
        if w and h:
            cmd += ["--keep-aspect-ratio"]
        subprocess.run(cmd + [str(path)], check=True)
        _cache[key] = Image.open(out).convert("RGBA")
    return _cache[key]


def paste(img: Image.Image, src: Image.Image, x: int, y: int) -> None:
    img.alpha_composite(src, (int(x), int(y)))


def zoom(src: Image.Image, factor: int) -> Image.Image:
    return src.resize((src.width * factor, src.height * factor), Image.NEAREST)


def flat(src: Image.Image, bg) -> Image.Image:
    """Composite a small render onto its background before zooming, so the zoom shows real pixels."""
    base = Image.new("RGBA", src.size, bg + (255,))
    base.alpha_composite(src)
    return base


# ---------------------------------------------------------------- terminal art

SGR = re.compile(r"\x1b\[([0-9;]*)m")


def parse_ans(text: str):
    """Return rows of (char, fg, bg, bold); fg/bg None means terminal default."""
    rows = []
    for line in text.split("\n"):
        cells = []
        fg = bg = None
        bold = False
        pos = 0
        for m in SGR.finditer(line):
            for ch in line[pos:m.start()]:
                cells.append((ch, fg, bg, bold))
            pos = m.end()
            codes = [int(c) if c else 0 for c in m.group(1).split(";")] if m.group(1) else [0]
            i = 0
            while i < len(codes):
                c = codes[i]
                if c == 0:
                    fg = bg = None
                    bold = False
                elif c == 1:
                    bold = True
                elif c == 22:
                    bold = False
                elif c == 39:
                    fg = None
                elif c == 49:
                    bg = None
                elif c in (38, 48) and i + 4 < len(codes) and codes[i + 1] == 2:
                    rgb = tuple(codes[i + 2:i + 5])
                    if c == 38:
                        fg = rgb
                    else:
                        bg = rgb
                    i += 4
                i += 1
        for ch in line[pos:]:
            cells.append((ch, fg, bg, bold))
        rows.append(cells)
    while rows and not rows[-1]:
        rows.pop()
    return rows


def draw_term(img: Image.Image, rows, x: int, y: int, cw: int, ch: int, dfg, dbg) -> None:
    d = ImageDraw.Draw(img)
    mono = ImageFont.truetype(MONO, int(ch * 0.78))
    monob = ImageFont.truetype(MONO_BOLD, int(ch * 0.78))
    half = ch // 2
    for r, cells in enumerate(rows):
        for c, (glyph, fg, bg, bold) in enumerate(cells):
            fg = fg or dfg
            bg = bg or dbg
            x0, y0 = x + c * cw, y + r * ch
            box = (x0, y0, x0 + cw - 1, y0 + ch - 1)
            top = (x0, y0, x0 + cw - 1, y0 + half - 1)
            bot = (x0, y0 + half, x0 + cw - 1, y0 + ch - 1)
            if bg != dbg:
                d.rectangle(box, fill=bg)
            if glyph == "█":
                d.rectangle(box, fill=fg)
            elif glyph == "▀":
                d.rectangle(top, fill=fg)
            elif glyph == "▄":
                d.rectangle(bot, fill=fg)
            elif glyph != " ":
                d.text((x0 + cw / 2, y0 + ch / 2), glyph, fill=fg,
                       font=monob if bold else mono, anchor="mm")


def term_rows(e: Entry, dark: bool):
    if dark:
        return parse_ans((e.folder / e.ans).read_text())
    # Light terminals: the .ans files are tuned for dark terminals, so show the
    # plain logo.txt in the terminal's foreground colour instead.
    return [[(g, None, None, False) for g in line] for line in
            (e.folder / e.txt).read_text().rstrip("\n").split("\n")]


# ---------------------------------------------------------------- sheets


def sheet(dark: bool) -> Image.Image:
    W, H = 2400, 1520
    bg = INK if dark else PAPER
    fg = OFFWHITE if dark else INK
    panel = PANEL_DARK if dark else PANEL_LIGHT
    line = LINE_DARK if dark else LINE_LIGHT
    img = Image.new("RGBA", (W, H), bg + (255,))
    d = ImageDraw.Draw(img)

    d.text((48, 34), "neomail logo variations", fill=fg, font=sans(40, "Bold"))
    d.text((48, 86), ("dark backgrounds (#0D1117): mark-dark, lockup-dark, logo.ans" if dark else
                      "light backgrounds (#F6F8FA): mark, lockup-light, logo.txt") +
           "   ·   ordered by art director ranking", fill=MUTED, font=sans(22))

    cols, gap, top = 4, 24, 134
    cw = (W - 2 * 48 - (cols - 1) * gap) // cols  # 558
    chh = (H - top - 40 - gap) // 2
    title_f, small_f, idea_f = sans(28, "Bold"), sans(17), sans(18)

    for i, e in enumerate(ENTRIES):
        cx = 48 + (i % cols) * (cw + gap)
        cy = top + (i // cols) * (chh + gap)
        d.rounded_rectangle((cx, cy, cx + cw, cy + chh), 14, fill=panel, outline=line, width=2)
        x, y = cx + 22, cy + 18
        badge = f"#{e.rank}"
        d.text((x, y), badge, fill=MUTED, font=sans(24, "Bold"))
        d.text((x + 50, y - 2), f"{e.num}  {e.name}" if e.num != "--" else e.name, fill=fg, font=title_f)
        y += 46
        # 256 px mark + 32 px mark
        m = svg(e.folder / (e.mark_dark if dark else e.mark), 256, 256)
        paste(img, m, x, y)
        sx = x + 256 + 28
        d.text((sx, y), "256 px", fill=MUTED, font=small_f)
        d.text((sx, y + 60), "32 px", fill=MUTED, font=small_f)
        paste(img, svg(e.folder / (e.mark_dark if dark else e.mark), 32, 32), sx, y + 86)
        # wrapped idea text
        words, lines_, cur = e.idea.split(), [], ""
        for w_ in words:
            t = (cur + " " + w_).strip()
            if d.textlength(t, font=idea_f) > cw - 256 - 28 - 44:
                lines_.append(cur)
                cur = w_
            else:
                cur = t
        lines_.append(cur)
        for k, ln in enumerate(lines_):
            d.text((sx, y + 140 + k * 24), ln, fill=fg, font=idea_f)
        y += 256 + 18
        # terminal art
        rows = term_rows(e, dark)
        ncols = max(len(r) for r in rows)
        cellw = min(11, (cw - 44 - 24) // ncols)
        cellh = cellw * 2
        th = len(rows) * cellh + 24
        tbg = INK if dark else PAPER
        d.rounded_rectangle((x, y, cx + cw - 22, y + th), 8, fill=tbg, outline=line, width=1)
        draw_term(img, rows, x + 12, y + 12, cellw, cellh, fg, tbg)
        y += th + 18
        # lockup
        lk = e.lockup_dark if dark else e.lockup_light
        limg = svg(e.folder / lk, None, 60)
        if limg.width > cw - 44:
            limg = svg(e.folder / lk, cw - 44, 60)
        paste(img, limg, x, y + (76 - limg.height) // 2)

    # 8th cell: legend
    i = len(ENTRIES)
    cx = 48 + (i % cols) * (cw + gap)
    cy = top + (i // cols) * (chh + gap)
    d.rounded_rectangle((cx, cy, cx + cw, cy + chh), 14, outline=line, width=2)
    x, y = cx + 22, cy + 20
    d.text((x, y), "Art director ranking", fill=fg, font=title_f)
    y += 46
    for e in ENTRIES:
        d.text((x, y), f"#{e.rank}", fill=MUTED, font=sans(20, "Bold"))
        d.text((x + 40, y), e.name, fill=fg, font=sans(20, "Bold"))
        y += 26
        words, cur = e.verdict.split(), ""
        for w_ in words:
            t = (cur + " " + w_).strip()
            if d.textlength(t, font=small_f) > cw - 84:
                d.text((x + 40, y), cur, fill=MUTED, font=small_f)
                y += 22
                cur = w_
            else:
                cur = t
        d.text((x + 40, y), cur, fill=MUTED, font=small_f)
        y += 28
    d.text((x, y + 4), "Notes: small-size behaviour, from each folder's", fill=MUTED, font=small_f)
    d.text((x, y + 26), "README. Compare in small.png.", fill=MUTED, font=small_f)
    return img.convert("RGB")


def small_sheet() -> Image.Image:
    """One row per background: 16 px and 32 px renders, each with an 8x pixel zoom."""
    n = len(ENTRIES)
    colw = 330
    W = 48 * 2 + n * colw
    band = 600
    H = 130 + 2 * band - 10
    img = Image.new("RGBA", (W, H), (0x22, 0x27, 0x2E, 255))
    d = ImageDraw.Draw(img)
    d.text((48, 30), "Small sizes: what ships at 16 px and 32 px, with 8x pixel zooms",
           fill=OFFWHITE, font=sans(34, "Bold"))
    d.text((48, 80), "16 px = favicon.svg (favicon-light.svg where one exists).  32 px = mark-32.svg where one exists, "
           "otherwise the full mark.  Zooms are nearest-neighbour, so every pixel is real.",
           fill=MUTED, font=sans(19))
    for b, dark in enumerate((True, False)):
        bg = INK if dark else PAPER
        fg = OFFWHITE if dark else INK
        y0 = 130 + b * band
        d.rectangle((0, y0, W, y0 + band - 10), fill=bg)
        d.text((48, y0 + 12), "on #0D1117" if dark else "on #F6F8FA", fill=MUTED, font=sans(18))
        for i, e in enumerate(ENTRIES):
            x = 48 + i * colw
            d.text((x, y0 + 44), f"#{e.rank}  {e.name}", fill=fg, font=sans(20, "Bold"))
            fav = e.favicon if dark or not e.favicon_light else e.favicon_light
            m32 = e.mark32 or (e.mark_dark if dark else e.mark)
            r16 = svg(e.folder / fav, 16, 16)
            r32 = svg(e.folder / m32, 32, 32)
            yy = y0 + 80
            # 1x renders, side by side
            paste(img, r16, x, yy + 8)
            paste(img, r32, x + 36, yy)
            d.text((x + 80, yy + 8), "1x: 16 and 32 px", fill=MUTED, font=sans(15))
            # 16 px at 8x
            zy = yy + 48
            z16 = zoom(flat(r16, bg), 8)
            paste(img, z16, x, zy)
            d.rectangle((x - 1, zy - 1, x + 128, zy + 128), outline=MUTED)
            d.text((x + 140, zy + 4), "16 px @8x", fill=MUTED, font=sans(15))
            d.text((x + 140, zy + 26), fav, fill=MUTED, font=sans(14))
            # 32 px at 8x
            zy2 = zy + 128 + 20
            z32 = zoom(flat(r32, bg), 8)
            paste(img, z32, x, zy2)
            d.rectangle((x - 1, zy2 - 1, x + 256, zy2 + 256), outline=MUTED)
            d.text((x, zy2 + 264), f"32 px @8x  ·  {m32}", fill=MUTED, font=sans(14))
    return img.convert("RGB")


def main() -> None:
    sheet(True).save(HERE / "sheet.png", optimize=True)
    sheet(False).save(HERE / "sheet-light.png", optimize=True)
    small_sheet().save(HERE / "small.png", optimize=True)
    for f in ("sheet.png", "sheet-light.png", "small.png"):
        im = Image.open(HERE / f)
        print(f, im.size)


if __name__ == "__main__":
    main()
