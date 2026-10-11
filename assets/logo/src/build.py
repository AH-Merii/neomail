"""Rebuild the whole neomail logo set in assets/logo/ with one command, from anywhere:

    python3 assets/logo/src/build.py

Needs rsvg-convert, magick, Adwaita Sans in /usr/share/fonts/Adwaita/, and Pillow, uharfbuzz and
fontTools. If the Python packages are missing, the script re-runs itself under
`uv run --with pillow --with uharfbuzz --with fonttools`. A re-run gives byte-identical files.
"""
import math
import os
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # keep src/ free of __pycache__
HERE = Path(__file__).resolve().parent
OUT = HERE.parent
sys.path.insert(0, str(HERE))

try:
    import fontTools  # noqa: F401
    import PIL  # noqa: F401
    import uharfbuzz  # noqa: F401
except ImportError:
    if os.environ.get("NEOMAIL_LOGO_UV"):
        raise
    os.environ["NEOMAIL_LOGO_UV"] = "1"
    cmd = ["uv", "run", "--no-project", "--with", "pillow", "--with", "uharfbuzz",
           "--with", "fonttools", "python3", os.path.abspath(sys.argv[0]), *sys.argv[1:]]
    os.execvp(cmd[0], cmd)

import brand as B  # noqa: E402
import text as T  # noqa: E402


# ---------------------------------------------------------------- marks and lockups
def mark_body(theme, g=B.MASTER):
    win, bar, chip = B.mark_paths(g)
    if theme == "mono":
        return B.path(win + bar + chip, B.MONO)
    fg, accent = B.THEMES[theme]
    return B.path(win + bar, fg) + B.path(chip, accent)


def word_geometry():
    """Scale, pen start and lockup width: the "l" runs from the window top (y 8) to the
    statusline bottom (y 56), so the baseline is the statusline's bottom edge."""
    wg, op = B.WORD_AXES
    g = B.MASTER
    scale = (g["sl1"] - g["y0"]) / T.metric("l", wg, op, "ymax")
    x = g["x1"] + B.WORD_GAP - T.metric("n", wg, op, "xmin") * scale
    last_x = T.shape(B.WORD, wg, op)[0][-1][1]          # pen position of the final "l"
    right = x + (last_x + T.metric("l", wg, op, "xmax")) * scale
    return scale, x, math.ceil(right + B.MARGIN)


def lockup_body(theme):
    scale, x, w = word_geometry()
    d, _ = T.outline(B.WORD, x, B.MASTER["sl1"], scale, *B.WORD_AXES)
    fg = B.MONO if theme == "mono" else B.THEMES[theme][0]
    return mark_body(theme) + B.path(d, fg), w


# ---------------------------------------------------------------- icons
def favicon_body():
    return (B.path(B.TILE16, B.PAPER) + B.path(B.TILE16_IN, B.INK)
            + B.path(B.pixel_path(B.FAV, "#", B.FAV_AT), B.OFF)
            + B.path(B.pixel_path(B.FAV, "a", B.FAV_AT), B.AMBER))


def icon_body():
    k = B.ICON_KEY
    return (B.path(B.rr(0, 0, 64, 64, B.ICON_R), B.PAPER)
            + B.path(B.rr(k, k, 64 - 2 * k, 64 - 2 * k, B.ICON_R - k), B.INK)
            + mark_body("dark", B.ICON))


def icon_mono_body():
    """One colour: the favicon's pixel drawing cut out of the tile (even-odd)."""
    return B.path(B.TILE16 + B.pixel_path(B.FAV, "#a", B.FAV_AT), B.MONO,
                  ' fill-rule="evenodd"')


# ---------------------------------------------------------------- social card
SOCIAL = (1280, 640)
BAR = "#21262D"            # statusline bar colour on the card (the TUI mock's bar)


def text_px(s, x, baseline, px, wght, opsz, fill):
    upm = T.instance(wght, opsz)[0]["head"].unitsPerEm
    d, adv = T.outline(s, x, baseline, px / upm, wght, opsz)
    return B.path(d, fill), adv


def text_width(s, px, wght, opsz):
    upm = T.instance(wght, opsz)[0]["head"].unitsPerEm
    return T.shape(s, wght, opsz)[1] * px / upm


def social_body():
    W, H = SOCIAL
    sl_h = 48                                  # statusline along the bottom edge
    body, lw = lockup_body("dark")
    k = 2.5                                    # lockup 160 px tall
    tag_px, tag_axes, lead = 34, (430, 20), 92  # tagline size, axes, lockup bottom to baseline
    block = 64 * k + lead
    ly = round((H - sl_h - block) / 2)
    lx = round((W - lw * k) / 2)
    out = f'<rect width="{W}" height="{H}" fill="{B.INK}"/>'
    out += f'<g transform="translate({lx} {ly}) scale({k})">{body}</g>'
    tw = text_width(B.TAGLINE, tag_px, *tag_axes)
    p, _ = text_px(B.TAGLINE, (W - tw) / 2, ly + 64 * k + lead - 24, tag_px, *tag_axes, B.MUTED)
    out += p
    # statusline: amber NORMAL chip with a 45 degree point, a notch, then the bar
    y0, y1 = H - sl_h, H
    c, tip, n = 184, 184 + sl_h / 2, 12
    out += B.path(B.P((0, y0), (c, y0), (tip, (y0 + y1) / 2), (c, y1), (0, y1)), B.AMBER)
    out += B.path(B.P((c + n, y0), (W, y0), (W, y1), (c + n, y1), (tip + n, (y0 + y1) / 2)), BAR)
    lab, _ = text_px(B.LABEL, 40, y1 - 16, 22, 700, 20, B.INK)
    right, _ = text_px("utf-8  1:1", W - 40 - text_width("utf-8  1:1", 20, 430, 20),
                       y1 - 17, 20, 430, 20, B.MUTED)
    return out + lab + right


# ---------------------------------------------------------------- terminal logo
def term_cells(grid):
    """Pair pixel rows into half-block cells: rows of (char, top, bottom)."""
    out = []
    for y in range(0, len(grid), 2):
        row = []
        for t, b in zip(grid[y], grid[y + 1]):
            ch = {(1, 1): "█", (1, 0): "▀", (0, 1): "▄", (0, 0): " "}[(t != ".", b != ".")]
            row.append((ch, t, b))
        out.append(row)
    return out


def chip_label(row):
    """Map the chip columns of a full-chip cell row to the centred LABEL characters."""
    cols = [i for i, (_, t, b) in enumerate(row) if t == "a" and b == "a"]
    if len(cols) < len(B.LABEL):
        return {}
    pad = (len(cols) - len(B.LABEL)) // 2
    return {cols[pad + k]: ch for k, ch in enumerate(B.LABEL)}


def term_logo():
    """(logo.txt, logo.ans): the mark, with the wordmark and tagline beside it."""
    cells = term_cells(B.TERM)
    width = len(B.TERM[0])
    label_row = max(i for i, row in enumerate(cells) if chip_label(row))
    ink_on_amber = "\x1b[38;2;%d;%d;%d;48;2;%d;%d;%dm" % (B.rgb(B.INK) + B.rgb(B.AMBER))
    txt, ans = [], []
    for i, row in enumerate(cells):
        labels = chip_label(row) if i == label_row else {}
        # The chip's label goes into the plain text too: without colour, NORMAL and
        # the blank notch are what tell the chip from the bar.
        plain = "".join(labels.get(j, c) for j, (c, _, _) in enumerate(row)).rstrip()
        s, cur = "", "start"
        for j, (ch, t, b) in enumerate(row):
            if j in labels:
                state, code, ch = "label", ink_on_amber, labels[j]
            else:
                ink = t if ch in "█▀" else b
                state = "amber" if ink == "a" else None   # the window uses the terminal's fg
                code = ("\x1b[0;38;2;%d;%d;%dm" % B.rgb(B.AMBER)) if state else "\x1b[0;39m"
            # A blank cell may keep the current SGR unless it sets a background (the
            # chip label): then reset, so the notch prints on the terminal's own bg.
            if (ch != " " or cur == "label") and state != cur:
                s += code
                cur = state
            s += ch
        s = s.rstrip() + "\x1b[0m"
        if i in B.TERM_TEXT:
            words, sgr = B.TERM_TEXT[i]
            pad = " " * (width - len(plain) + B.TERM_GAP)
            plain += pad + words
            s += pad + f"\x1b[{sgr}m{words}\x1b[0m"
        txt.append(plain)
        ans.append(s)
    return "\n".join(txt) + "\n", "\n".join(ans) + "\n"


# ---------------------------------------------------------------- PNGs
def png(src, dst, w, h):
    subprocess.run(["rsvg-convert", "-w", str(w), "-h", str(h), str(src), "-o", str(dst)],
                   check=True)
    subprocess.run(["magick", str(dst), "-strip", str(dst)], check=True)


def main():
    files = {
        "neomail-mark.svg": B.svg(64, 64, mark_body("light")),
        "neomail-mark-dark.svg": B.svg(64, 64, mark_body("dark")),
        "neomail-mark-mono.svg": B.svg(64, 64, mark_body("mono")),
        "favicon.svg": B.svg(16, 16, favicon_body()),
        "icon.svg": B.svg(64, 64, icon_body()),
        "icon-mono.svg": B.svg(16, 16, icon_mono_body()),
        "social-preview.svg": B.svg(*SOCIAL, social_body(), size=SOCIAL),
    }
    for theme in ("light", "dark", "mono"):
        body, w = lockup_body(theme)
        files[f"neomail-logo-{theme}.svg"] = B.svg(w, 64, body)
    files["logo.txt"], files["logo.ans"] = term_logo()
    for name, content in files.items():
        (OUT / name).write_text(content)
    png(OUT / "favicon.svg", OUT / "favicon-32.png", 32, 32)
    png(OUT / "icon.svg", OUT / "icon-192.png", 192, 192)
    png(OUT / "icon.svg", OUT / "icon-512.png", 512, 512)
    png(OUT / "social-preview.svg", OUT / "social-preview.png", *SOCIAL)
    for name in sorted(list(files) + ["favicon-32.png", "icon-192.png", "icon-512.png",
                                      "social-preview.png"]):
        print(f"{name:24} {(OUT / name).stat().st_size:7} bytes")
    scale, x, w = word_geometry()
    print(f"word scale {scale:.5f}  pen start {x:.2f}  lockup width {w}")


if __name__ == "__main__":
    main()
