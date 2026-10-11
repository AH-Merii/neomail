"""neomail logo variation 03, ":wq seal". Regenerates every asset in this folder.

    uv run --with pillow --with uharfbuzz --with fonttools python build.py

Needs rsvg-convert on PATH. The wordmark is outlined from Adwaita Sans
through ../../src/text.py, so no SVG depends on an installed font.
"""
import math
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "src"))
from text import outline, metric  # noqa: E402  (Adwaita Sans, wght 640 opsz 32)

# ---------------------------------------------------------------- palette
WAX = "#B3203F"        # carmine sealing wax, light backgrounds (hue 347)
WAX_D = "#CF2E4A"      # sealing wax, dark backgrounds (hue 350)
CREAM = "#FFF3E0"      # the impression (:w)
INK = "#0D1117"        # brand ink: wordmark and envelope line on light
LINE_D = "#8B98A5"     # cool grey: envelope line on dark
PAPERW = "#EDE7DD"     # warm paper: wordmark on dark
BG_DARK, BG_LIGHT = "#0D1117", "#F6F8FA"


def f(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s

def blob(cx, cy, R, harm, n=24):
    """Closed smooth curve r(t)=R*(1+sum a*cos(k t + p)) via Catmull-Rom -> cubic."""
    pts = []
    for i in range(n):
        t = 2 * math.pi * i / n
        r = R * (1 + sum(a * math.cos(k * t + p) for k, a, p in harm))
        pts.append((cx + r * math.cos(t), cy + r * math.sin(t)))
    d = f"M{f(pts[0][0])} {f(pts[0][1])}"
    for i in range(n):
        p0, p1, p2, p3 = pts[i - 1], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        d += f"C{f(c1[0])} {f(c1[1])} {f(c2[0])} {f(c2[1])} {f(p2[0])} {f(p2[1])}"
    return d + "Z"

def circle(cx, cy, r, ccw=False):
    s = 0 if ccw else 1
    return (f"M{f(cx - r)} {f(cy)}A{f(r)} {f(r)} 0 1 {s} {f(cx + r)} {f(cy)}"
            f"A{f(r)} {f(r)} 0 1 {s} {f(cx - r)} {f(cy)}Z")

def rrect(x, y, w, h, r, ccw=False):
    d = f"M{f(x + r)} {f(y)}H{f(x + w - r)}A{f(r)} {f(r)} 0 0 1 {f(x + w)} {f(y + r)}V{f(y + h - r)}"
    d += f"A{f(r)} {f(r)} 0 0 1 {f(x + w - r)} {f(y + h)}H{f(x + r)}A{f(r)} {f(r)} 0 0 1 {f(x)} {f(y + h - r)}"
    d += f"V{f(y + r)}A{f(r)} {f(r)} 0 0 1 {f(x + r)} {f(y)}Z"
    if ccw:
        d = (f"M{f(x + r)} {f(y)}A{f(r)} {f(r)} 0 0 0 {f(x)} {f(y + r)}V{f(y + h - r)}"
             f"A{f(r)} {f(r)} 0 0 0 {f(x + r)} {f(y + h)}H{f(x + w - r)}"
             f"A{f(r)} {f(r)} 0 0 0 {f(x + w)} {f(y + h - r)}V{f(y + r)}"
             f"A{f(r)} {f(r)} 0 0 0 {f(x + w - r)} {f(y)}Z")
    return d

def _isect(p, d, q, e):
    den = d[0] * e[1] - d[1] * e[0]
    if abs(den) < 1e-9:
        return p
    t = ((q[0] - p[0]) * e[1] - (q[1] - p[1]) * e[0]) / den
    return (p[0] + t * d[0], p[1] + t * d[1])

def stroke(pts, w, top, bot):
    """Polygon of a polyline stroked with width w, miter joins, clipped to top<=y<=bot."""
    h = w / 2
    segs = []
    for a, b in zip(pts, pts[1:]):
        dx, dy = b[0] - a[0], b[1] - a[1]
        L = math.hypot(dx, dy)
        nx, ny = -dy / L, dx / L
        segs.append((a, (dx, dy), (nx, ny)))
    def side(sg):
        out = []
        for i, (a, d, n) in enumerate(segs):
            p = (a[0] + sg * h * n[0], a[1] + sg * h * n[1])
            if i == 0:
                out.append(_isect(p, d, (0, pts[0][1] - 50 * d[1] / abs(d[1]) if d[1] else 0), (1, 0)))
            else:
                a0, d0, n0 = segs[i - 1]
                q = (a0[0] + sg * h * n0[0], a0[1] + sg * h * n0[1])
                out.append(_isect(q, d0, p, d))
        a, d, n = segs[-1]
        p = (a[0] + sg * h * n[0], a[1] + sg * h * n[1])
        out.append(_isect(p, d, (0, pts[-1][1] + 50 * d[1] / abs(d[1]) if d[1] else 0), (1, 0)))
        return out
    poly = side(1) + side(-1)[::-1]
    for y0, keep in ((top, lambda p: p[1] >= top), (bot, lambda p: p[1] <= bot)):
        res = []
        for i in range(len(poly)):
            c, n = poly[i], poly[(i + 1) % len(poly)]
            if keep(c):
                res.append(c)
            if keep(c) != keep(n):
                t = (y0 - c[1]) / (n[1] - c[1])
                res.append((c[0] + t * (n[0] - c[0]), y0))
        poly = res
    return poly

def poly_d(poly, ccw=False):
    area = sum(a[0] * b[1] - b[0] * a[1] for a, b in zip(poly, poly[1:] + poly[:1]))
    if (area < 0) != ccw:
        poly = poly[::-1]
    return "M" + "L".join(f"{f(x)} {f(y)}" for x, y in poly) + "Z"

def svg(w, h, body, title="neomail"):
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" role="img" '
            f'aria-label="{title}"><title>{title}</title>{body}</svg>\n')


# ---------------------------------------------------------------- the seal (64-unit grid)
# Wax edge: a round disc of radius 25 with four deliberate drops of wax squeezed
# out from under the stamp. Each drop is a small circle; the edge is the soft
# union of disc and drops, r = k*ln(sum exp(r_i/k)) along each ray, so every
# drop meets the disc with a smooth fillet and the outline stays one curve.
# Angles are SVG angles (0 = right, 90 = down).
R, SOFT = 25, 1.1
LOBES = [(20, 3.6, 7.0), (124, 4.0, 8.0), (172, 2.6, 5.5), (256, 3.2, 6.5)]  # angle, reach, radius


def seal_r(t):
    acc = math.exp(R / SOFT)
    for deg, reach, rl in LOBES:
        c = R + reach - rl
        d = t - math.radians(deg)
        q = c * math.sin(d)
        if abs(q) < rl and math.cos(d) > 0:
            acc += math.exp((c * math.cos(d) + math.sqrt(rl * rl - q * q)) / SOFT)
    return SOFT * math.log(acc)


def seal(cx, cy, k=1, n=120):
    """The wax edge, scaled by k, as one closed Catmull-Rom curve of cubics."""
    pts = [(cx + k * seal_r(t) * math.cos(t), cy + k * seal_r(t) * math.sin(t))
           for t in (2 * math.pi * i / n for i in range(n))]
    d = f"M{f(pts[0][0])} {f(pts[0][1])}"
    for i in range(n):
        p0, p1, p2, p3 = pts[i - 1], pts[i], pts[(i + 1) % n], pts[(i + 2) % n]
        c1 = (p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6)
        c2 = (p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6)
        d += f"C{f(c1[0])} {f(c1[1])} {f(c2[0])} {f(c2[1])} {f(p2[0])} {f(p2[1])}"
    return d + "Z"


# ':w', typed: one 5-unit stem for the colon dots and the w, flat cut terminals,
# and a w whose middle apex stops short of the x-height, as in a monospace w.
STROKE = 5
XH = 15                        # x-height of the impression
W_W, GAP, MID = 22, 4, 5.5     # w width, colon-to-w gap, middle apex drop
GLYPH_W = STROKE + GAP + W_W   # 31


def colon_w(cx, cy, k=1, ccw=False):
    t, top, bot = STROKE * k, cy - XH * k / 2, cy + XH * k / 2
    x = cx - GLYPH_W * k / 2
    d = rrect(x, bot - t, t, t, t * .2, ccw) + rrect(x, top, t, t, t * .2, ccw)
    x0, w2 = x + t + GAP * k + t / 2, (W_W - STROKE) * k
    pts = [(x0, top), (x0 + w2 * .23, bot), (x0 + w2 * .5, top + MID * k),
           (x0 + w2 * .77, bot), (x0 + w2, top)]
    return d + poly_d(stroke(pts, t, top, bot), ccw)


# The letter: the full mark is the back of an envelope, drawn as an ink outline,
# with the seal closing the point of the flap. The envelope is a 60 x 46 frame
# with a 2.6-unit line; the flap lines run from the inner corners to the seal's
# centre. The seal is the 64-unit seal scaled by SK, so ':w' keeps its drawing.
ENV_X, ENV_Y, ENV_W, ENV_H, ENV_R, LINE = 2, 9, 60, 46, 5, 2.6
SK = .67                       # seal scale inside the envelope
SCX, SCY = 32, 32.7            # seal centre = point of the flap
CLEAR = 2.4                    # mono: paper between the seal and the ink lines


def envelope(gap=None):
    """Frame ring plus both flap lines, all clockwise. gap=None runs the flap
    lines to the seal centre (hidden under the wax); a number stops them that
    far outside the wax edge, for the one-colour mark."""
    d = (rrect(ENV_X, ENV_Y, ENV_W, ENV_H, ENV_R)
         + rrect(ENV_X + LINE, ENV_Y + LINE, ENV_W - 2 * LINE, ENV_H - 2 * LINE,
                 ENV_R - LINE, ccw=True))
    h = LINE / 2
    for ex in (ENV_X + LINE * .8, ENV_X + ENV_W - LINE * .8):
        ey = ENV_Y + LINE * .8
        dx, dy = SCX - ex, SCY - ey
        L = math.hypot(dx, dy)
        ux, uy = dx / L, dy / L
        nx, ny = -uy, ux
        if gap is None:
            ix, iy = SCX, SCY
        else:
            D = SK * seal_r(math.atan2(-uy, -ux)) + gap
            ix, iy = SCX - ux * D, SCY - uy * D
        d += poly_d([(ex + nx * h, ey + ny * h), (ix + nx * h, iy + ny * h),
                     (ix - nx * h, iy - ny * h), (ex - nx * h, ey - ny * h)])
    return d


SEAL_M = seal(SCX, SCY, SK)          # full mark: the seal on the envelope


def mark_body(wax, line, ink):
    return (f'<path fill="{line}" d="{envelope()}"/><path fill="{wax}" d="{SEAL_M}"/>'
            f'<path fill="{ink}" d="{colon_w(SCX, SCY, SK)}"/>')


def mono_d():
    """One path, nonzero fill: envelope lines kept clear of the wax, then the
    wax with ':w' cut out. Nothing overlaps the seal, so no winding surprises."""
    return envelope(gap=CLEAR) + SEAL_M + colon_w(SCX, SCY, SK, ccw=True)


# ---------------------------------------------------------------- favicon / terminal (16 px grid)
# Drawn pixel by pixel: '#' wax, '.' impression, ' ' empty. At 16 px the lobes
# would only blur, so this is the bare disc; the ':w' sits on whole pixels. The
# w's outer stems step one pixel inward into two separate feet and the middle
# stem starts two rows down, so it reads as w (two Vs), not as a three-stem ш.
PIX = [
    "     ######     ",
    "   ##########   ",
    "  ############  ",
    " ############## ",
    " ############## ",
    "##..##..####..##",
    "##..##..####..##",
    "######..#..#..##",
    "######..#..#..##",
    "##..###..##..###",
    "##..###..##..###",
    " ############## ",
    " ############## ",
    "  ############  ",
    "   ##########   ",
    "      #####     ",
]
assert len(PIX) == 16 and all(len(r) == 16 for r in PIX)

# 32 px: the wax edge is sampled from the master (pixel centre inside r(t)), so
# the four lobes keep their places; the ':w' is drawn by hand on a 3 px stem.
GLYPH32 = [                    # 3 px colon dots, 2 px gap, a 12 px w
    "###..###......###",
    "###..###......###",
    "###..###..##..###",
    "......##..##..##.",
    "......###.##.###.",
    "###....###..###..",
    "###....###..###..",
    "###....##....##..",
]
G32_X, G32_Y = 7, 12           # top-left of GLYPH32 on the 32 px grid


def pix32():
    rows = []
    for y in range(32):
        row = ""
        for x in range(32):
            dx, dy = (x + .5) * 2 - 32, (y + .5) * 2 - 32
            row += "#" if math.hypot(dx, dy) <= seal_r(math.atan2(dy, dx)) else " "
        rows.append(row)
    for j, g in enumerate(GLYPH32):
        r = list(rows[G32_Y + j])
        for i, c in enumerate(g):
            if c == "#":
                r[G32_X + i] = "."
        rows[G32_Y + j] = "".join(r)
    return rows


def runs(ch, grid=PIX):
    """Merge one pixel class of a grid into a single path of row runs."""
    d = ""
    for y, row in enumerate(grid):
        x = 0
        while x < len(row):
            if row[x] == ch:
                x0 = x
                while x < len(row) and row[x] == ch:
                    x += 1
                d += f"M{x0} {y}H{x}V{y + 1}H{x0}Z"
            else:
                x += 1
    return d


# ---------------------------------------------------------------- SVG writers
def write(name, text):
    (HERE / name).write_text(text)


def marks():
    write("mark.svg", svg(64, 64, mark_body(WAX, INK, CREAM), "neomail"))
    write("mark-dark.svg", svg(64, 64, mark_body(WAX_D, LINE_D, CREAM), "neomail"))
    write("mark-mono.svg", svg(64, 64, f'<path fill="currentColor" d="{mono_d()}"/>', "neomail"))
    fav = f'<path fill="{WAX}" d="{runs("#")}"/><path fill="{CREAM}" d="{runs(".")}"/>'
    write("favicon.svg", svg(16, 16, fav, "neomail"))
    g = pix32()
    m32 = f'<path fill="{WAX}" d="{runs("#", g)}"/><path fill="{CREAM}" d="{runs(".", g)}"/>'
    write("mark-32.svg", svg(32, 32, m32, "neomail"))


WORD_XH = 24                   # x-height of the wordmark in mark units
WORD_GAP = 18                  # seal edge (x 64) to the ink of the n


def lockups():
    xh = metric("x", 640, 32, "ymax")
    scale = WORD_XH / xh
    base = 32 + WORD_XH / 2    # x-height centred on the seal's centre
    nx = metric("n", 640, 32, "xmin") * scale
    d, adv = outline("neomail", 64 + WORD_GAP - nx, base, scale, 640, 32, kern=False)
    lx = metric("l", 640, 32, "xmax") * scale
    # trim the right side bearing of the final l so the viewBox hugs the ink
    rsb = outline("l", 0, 0, scale, 640, 32)[1] - lx
    width = math.ceil(64 + WORD_GAP - nx + adv - rsb)
    for theme, wax, line, word in (("light", WAX, INK, INK), ("dark", WAX_D, LINE_D, PAPERW)):
        body = mark_body(wax, line, CREAM) + f'<path fill="{word}" d="{d}"/>'
        write(f"lockup-{theme}.svg", svg(width, 64, body, "neomail"))


# ---------------------------------------------------------------- terminal
HALF = {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}
ANSI_RGB = {"#": WAX_D, ".": CREAM}


def rgb(h):
    return ";".join(str(int(h[i:i + 2], 16)) for i in (1, 3, 5))


def terminal():
    txt, ans = [], []
    for y in range(0, 16, 2):
        top, bot = PIX[y], PIX[y + 1]
        txt.append("".join(HALF[(t == "#", b == "#")] for t, b in zip(top, bot)).rstrip())
        line = ""
        for t, b in zip(top, bot):
            ct, cb = ANSI_RGB.get(t), ANSI_RGB.get(b)
            if ct and cb:
                line += (f"\x1b[38;2;{rgb(ct)}m█" if ct == cb
                         else f"\x1b[38;2;{rgb(ct)};48;2;{rgb(cb)}m▀\x1b[49m")
            elif ct or cb:
                line += f"\x1b[38;2;{rgb(ct or cb)}m{'▀' if ct else '▄'}"
            else:
                line += " "
        ans.append(line.rstrip() + "\x1b[0m")
    write("logo.txt", "\n".join(txt) + "\n")
    write("logo.ans", "\n".join(ans) + "\n")


# ---------------------------------------------------------------- preview sheet
def run(*a):
    subprocess.run([str(x) for x in a], check=True)


def png(src, w, h, bg, dst):
    run("rsvg-convert", "-w", w, "-h", h, "-b", bg, src, "-o", dst)
    return dst


def check_bounds():
    """The bare seal fits the 64 square; the envelope seal plus its clearance
    stays inside the envelope's inner edge, so the mono mark never overlaps."""
    ts = [i / 100 for i in range(629)]
    for k, cx, cy, lo_x, hi_x, lo_y, hi_y in (
            (1, 32, 32, 0, 64, 0, 64),
            (SK, SCX, SCY, ENV_X + LINE + CLEAR, ENV_X + ENV_W - LINE - CLEAR,
             ENV_Y + LINE + CLEAR, ENV_Y + ENV_H - LINE - CLEAR)):
        xs = [cx + k * seal_r(t) * math.cos(t) for t in ts]
        ys = [cy + k * seal_r(t) * math.sin(t) for t in ts]
        assert lo_x <= min(xs) and max(xs) <= hi_x, (k, min(xs), max(xs))
        assert lo_y <= min(ys) and max(ys) <= hi_y, (k, min(ys), max(ys))


def preview():
    from PIL import Image, ImageDraw, ImageFont
    sans = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"
    mono = "/usr/share/fonts/TTF/DejaVuSansMono.ttf"
    lab = ImageFont.truetype(sans, 15)
    W, H = 1600, 1000
    sheet = Image.new("RGB", (W, H), "#596270")
    dr = ImageDraw.Draw(sheet)
    tmp = Path(tempfile.mkdtemp(prefix="nm03-"))

    def img(src, w, h, bg, zoom=1, name=None):
        p = png(HERE / src if isinstance(src, str) else src, w, h, bg,
                tmp / (name or f"{Path(str(src)).stem}-{w}-{bg[1:]}.png"))
        im = Image.open(p).convert("RGB")
        return im.resize((w * zoom, h * zoom), Image.NEAREST) if zoom > 1 else im

    # --- marks on dark (left) and light (right): 256, 64, 32, 16 and 8x zooms
    for px, (bg, fg, mk) in ((0, (BG_DARK, "#8B98A5", "mark-dark.svg")),
                             (800, (BG_LIGHT, "#57606A", "mark.svg"))):
        dr.rectangle([px, 0, px + 799, 459], fill=bg)
        sheet.paste(img(mk, 256, 256, bg), (px + 32, 56))
        dr.text((px + 32, 24), f"{mk} 256 px", font=lab, fill=fg)
        x = px + 330
        for src, s, tag in ((mk, 64, "64"), ("mark-32.svg", 32, "32"), (mk, 32, "32 env"),
                            ("favicon.svg", 16, "16"), (mk, 16, "16 env")):
            sheet.paste(img(src, s, s, bg), (x, 56 + 64 - s))
            dr.text((x, 128), tag, font=lab, fill=fg)
            x += max(s, 48) + 22
        sheet.paste(img("favicon.svg", 16, 16, bg, 8), (px + 330, 180))
        dr.text((px + 330, 162), "favicon.svg @16, 8x", font=lab, fill=fg)
        sheet.paste(img("mark-32.svg", 32, 32, bg, 8, f"z32-{bg[1:]}.png"), (px + 500, 180))
        dr.text((px + 500, 162), "mark-32.svg @32, 8x", font=lab, fill=fg)

    # --- lockups and mono (left column, lower half)
    dr.rectangle([0, 460, 799, 609], fill=BG_LIGHT)
    sheet.paste(img("lockup-light.svg", *lock_size(96), BG_LIGHT), (40, 487))
    dr.rectangle([0, 610, 799, 759], fill=BG_DARK)
    sheet.paste(img("lockup-dark.svg", *lock_size(96), BG_DARK), (40, 637))
    dr.rectangle([0, 760, 799, 999], fill="#FFFFFF")
    white = tmp / "mono-white.svg"
    white.write_text((HERE / "mark-mono.svg").read_text().replace("currentColor", "#FFFFFF"))
    sheet.paste(img("mark-mono.svg", 160, 160, "#FFFFFF"), (40, 810))
    dr.rectangle([230, 800, 409, 979], fill=INK)
    sheet.paste(img(white, 160, 160, INK, name="mono-w.png"), (240, 810))
    sheet.paste(img("lockup-light.svg", *lock_size(32), "#FFFFFF"), (450, 830))
    sheet.paste(img("lockup-light.svg", *lock_size(20), "#FFFFFF"), (450, 900))
    dr.text((40, 772), "mark-mono.svg (currentColor), on paper and on ink        lockup at 32 and 20 px", font=lab, fill="#57606A")

    # --- terminal splash mock (right column, lower half), painted cell by cell
    tx, ty, tw, th = 830, 490, 740, 480
    dr.rounded_rectangle([tx, ty, tx + tw, ty + th], 12, fill="#0B0E13")
    dr.rounded_rectangle([tx, ty, tx + tw, ty + 34], 12, fill="#1A1F27")
    dr.rectangle([tx, ty + 22, tx + tw, ty + 34], fill="#1A1F27")
    for i, c in enumerate(("#FF5F57", "#FEBC2E", "#28C840")):
        dr.ellipse([tx + 16 + i * 22, ty + 11, tx + 28 + i * 22, ty + 23], fill=c)
    dr.text((tx + tw / 2, ty + 17), "nmail — ghostty", font=lab, fill="#8B98A5", anchor="mm")
    cw, ch = 13, 26                       # one terminal cell; each half is 13 x 13
    ox, oy = tx + tw / 2 - 8 * cw, ty + 90
    for y, row in enumerate(PIX):
        for x, p in enumerate(row):
            if p != " ":
                col = WAX_D if p == "#" else CREAM
                dr.rectangle([ox + x * cw, oy + y * ch / 2, ox + (x + 1) * cw - 1,
                              oy + (y + 1) * ch / 2 - 1], fill=col)
    mf = ImageFont.truetype(mono, 21)
    cy = oy + 8 * ch + 34
    for text, col in (("neomail 0.1.0", PAPERW),
                      ("vim-modal mail, sorted by AI", "#8B98A5"),
                      ("", None),
                      (":wq  seal and send     ZZ  same, faster", "#8B98A5")):
        if text:
            dr.text((tx + tw / 2, cy), text, font=mf, fill=col, anchor="mm")
        cy += 30
    sheet.save(HERE / "preview.png")


def lock_size(h):
    vb = (HERE / "lockup-light.svg").read_text().split('viewBox="0 0 ')[1].split('"')[0].split()
    return round(float(vb[0]) * h / 64), h


if __name__ == "__main__":
    check_bounds()
    marks()
    lockups()
    terminal()
    preview()
    print("built:", ", ".join(sorted(p.name for p in HERE.iterdir() if p.is_file())))
