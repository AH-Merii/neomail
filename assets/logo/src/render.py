"""Render PNG deliverables and review sheets from the SVG masters.
    uv run --with pillow python render.py [review_dir]
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image

OUT = Path(__file__).resolve().parent.parent
REVIEW = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("/tmp/neomail-logo-review")
INK, PAPER = "#0D1117", "#F6F8FA"


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


def rsvg(src, dst, w=None, h=None, bg=None):
    args = ["rsvg-convert"]
    if w:
        args += ["-w", w]
    if h:
        args += ["-h", h]
    if bg:
        args += ["-b", bg]
    run(*args, src, "-o", dst)


def deliverables():
    fav = OUT / "favicon.svg"
    rsvg(fav, OUT / "favicon-32.png", 32, 32)
    rsvg(fav, OUT / "icon-192.png", 192, 192)
    rsvg(fav, OUT / "icon-512.png", 512, 512)
    rsvg(OUT / "social-preview.svg", OUT / "social-preview.png", 1280, 640)
    for p in ("favicon-32.png", "icon-192.png", "icon-512.png", "social-preview.png"):
        run("magick", OUT / p, "-strip", OUT / p)


def zoom(src, size, factor, bg, dst):
    """Render at `size` px, then enlarge with nearest-neighbour to inspect pixels."""
    tmp = dst.with_suffix(".raw.png")
    rsvg(src, tmp, size, size, bg)
    run("magick", tmp, "-filter", "point", "-resize", f"{factor * 100}%", dst)


def review():
    REVIEW.mkdir(parents=True, exist_ok=True)
    rows = []
    cases = [("neomail-mark-dark.svg", INK), ("neomail-mark.svg", PAPER),
             ("neomail-mark-mono.svg", "#FFFFFF"), ("favicon.svg", INK),
             ("favicon.svg", PAPER), ("icon-mono.svg", "#FFFFFF")]
    for i, (name, bg) in enumerate(cases):
        cells = []
        for size in (256, 64, 32, 16):
            c = REVIEW / f"c{i}-{size}.png"
            rsvg(OUT / name, c, size, size, bg)
            cells.append(c)
        z16, z32 = REVIEW / f"c{i}-z16.png", REVIEW / f"c{i}-z32.png"
        zoom(OUT / name, 16, 16, bg, z16)
        zoom(OUT / name, 32, 8, bg, z32)
        row = REVIEW / f"row{i}.png"
        run("magick", "-background", bg, *cells, z16, z32, "-gravity", "center",
            "-bordercolor", bg, "-border", "12", "+append", row)
        rows.append(row)
    run("magick", "-background", "#808080", *rows, "-gravity", "north",
        "-splice", "0x4", "-gravity", "west", "-append", REVIEW / "marks.png")
    lrows = []
    for theme, bg in (("light", PAPER), ("dark", INK), ("mono", "#FFFFFF")):
        cells = []
        for h in (128, 32, 20):
            c = REVIEW / f"l-{theme}-{h}.png"
            rsvg(OUT / f"neomail-logo-{theme}.svg", c, None, h, bg)
            cells.append(c)
        row = REVIEW / f"lrow-{theme}.png"
        run("magick", "-background", bg, *cells, "-gravity", "center",
            "-bordercolor", bg, "-border", "16", "+append", row)
        lrows.append(row)
    run("magick", "-background", "#808080", *lrows, "-gravity", "west",
        "-append", REVIEW / "lockups.png")


def halfblocks(svg, size=16):
    """Threshold a `size` px render into rows of Unicode half blocks (2 px per cell)."""
    tmp = REVIEW / f"hb-{svg.stem}.png"
    rsvg(svg, tmp, size, size)
    im = Image.open(tmp).convert("RGBA")
    px = [[im.getpixel((x, y))[3] >= 128 for x in range(size)] for y in range(size)]
    rows = [y for y in range(size) if any(px[y])]
    cols = [x for x in range(size) if any(px[y][x] for y in range(size))]
    y0, y1, x0, x1 = rows[0], rows[-1], cols[0], cols[-1]
    if (y1 - y0) % 2 == 0:
        y1 += 1
    lines = []
    for y in range(y0, y1 + 1, 2):
        line = ""
        for x in range(x0, x1 + 1):
            top = px[y][x]
            bot = px[y + 1][x] if y + 1 < size else False
            line += {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}[(top, bot)]
        lines.append(line.rstrip())
    return lines


def grid(d, size=16):
    """Coverage grid (>= 50 %) of one 64-unit path rendered at `size` px."""
    tmp = REVIEW / "grid.svg"
    tmp.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
                   f'<path d="{d}"/></svg>')
    rsvg(tmp, tmp.with_suffix(".png"), size, size)
    im = Image.open(tmp.with_suffix(".png")).convert("RGBA")
    return [[im.getpixel((x, y))[3] >= 128 for x in range(size)] for y in range(size)]


def term_art():
    """Write logo.txt (plain Unicode) and logo.ans (24-bit ANSI) from the 16 px mark."""
    sys.path.insert(0, str(Path(__file__).parent))
    import brand as B
    n, c = grid(B.MARK_N), grid(B.MARK_CURSOR)
    teal = "\x1b[38;2;72;220;193m"
    bold, dim, reset = "\x1b[1m", "\x1b[2m", "\x1b[0m"
    side = {2: ("neomail", bold), 3: ("vim-style terminal mail, sorted by AI", dim)}
    plain, ansi = [], []
    for row, y in enumerate(range(2, 14, 2)):          # mark spans px 2..13
        p_line, a_line = "", ""
        for x in range(2, 14):
            cells = {}
            for name, g in (("n", n), ("c", c)):
                cells[name] = (g[y][x], g[y + 1][x])
            kind = "c" if any(cells["c"]) else "n"
            top, bot = cells[kind]
            ch = {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}[(top, bot)]
            p_line += ch
            a_line += f"{teal}{ch}{reset}" if kind == "c" and ch != " " else ch
        if row in side:
            word, style = side[row]
            p_line, a_line = f"{p_line:12}   {word}", f"{a_line}   {style}{word}{reset}"
        plain.append(p_line.rstrip())
        ansi.append(a_line.replace(reset + teal, "").rstrip())
    (OUT / "logo.txt").write_text("\n".join(plain) + "\n")
    (OUT / "logo.ans").write_text("\n".join(ansi) + "\n")
    return plain


if __name__ == "__main__":
    REVIEW.mkdir(parents=True, exist_ok=True)
    deliverables()
    review()
    print("\n".join(term_art()))
