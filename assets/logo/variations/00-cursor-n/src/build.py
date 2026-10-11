"""Write the neomail SVG masters into assets/logo/. Run from this directory:
    uv run --with fonttools --with uharfbuzz python build.py
"""
import math
from pathlib import Path

import brand as B
import text as T

OUT = Path(__file__).resolve().parent.parent
TAGLINE = "a fast, secure, vim-style terminal mail client, sorted by AI"
TAG_AXES = (430, 20)  # wght, opsz for the tagline


def word_geometry():
    """Scale, pen start and ink right edge of the outlined wordmark."""
    w, o = B.WORD_AXES["wght"], B.WORD_AXES["opsz"]
    asc = T.metric("l", w, o, "ymax")
    s = (B.BASELINE - B.ASCENT_TOP) / asc          # top of "l" = top of mark
    x0 = B.MARK_RIGHT + B.WORD_GAP - T.metric("n", w, o, "xmin") * s
    _, adv = T.shape(B.WORD, w, o)
    adv_l = T.shape("l", w, o)[1]
    right = x0 + (adv - adv_l + T.metric("l", w, o, "xmax")) * s
    return s, x0, right


def mark_body(theme):
    t = B.THEMES[theme]
    if theme == "mono":
        return B.path(B.MARK_N + B.MARK_CURSOR, t["fg"])
    return B.path(B.MARK_N, t["fg"]) + B.path(B.MARK_CURSOR, t["accent"])


def lockup_body(theme):
    s, x0, right = word_geometry()
    d, _ = T.outline(B.WORD, x0, B.BASELINE, s, **B.WORD_AXES)
    width = math.ceil(right + 8)
    return mark_body(theme) + B.path(d, B.THEMES[theme]["fg"]), width


def icon_body(mono=False):
    if mono:
        return B.path(B.ICON_TILE + B.ICON_N + B.ICON_CURSOR, B.MONO, ' fill-rule="evenodd"')
    return (B.path(B.ICON_TILE, B.INK) + B.path(B.ICON_N, B.OFF)
            + B.path(B.ICON_CURSOR, B.TEAL))


def social_body():
    W, H = 1280, 640
    body, lw = lockup_body("dark")
    k = 2.5
    lead = 88                                 # lockup baseline to tagline baseline
    lx = (W - lw * k) / 2
    fs = 34                                   # tagline font size in px
    ly = round(H / 2 - ((B.BASELINE - B.ASCENT_TOP) * k + lead) / 2 - B.ASCENT_TOP * k)
    out = f'<rect width="{W}" height="{H}" fill="{B.INK}"/>'
    out += f'<g transform="translate({lx:.2f} {ly}) scale({k})">{body}</g>'
    wg, op = TAG_AXES
    s = fs / 2048
    _, adv = T.shape(TAGLINE, wg, op)
    ch = round(T.metric("I", wg, op, "ymax") * s)   # cursor = cap height
    cur_w, cur_gap = round(ch * 0.6), round(T.shape(" ", wg, op)[1] * s)
    tw = adv * s + cur_gap + cur_w
    tx = (W - tw) / 2
    base = ly + B.BASELINE * k + lead
    d, a = T.outline(TAGLINE, tx, base, s, wg, op)
    out += B.path(d, B.MUTED)
    cx = tx + a + cur_gap
    out += (f'<rect x="{cx:.2f}" y="{base - ch}" width="{cur_w}" height="{ch}" '
            f'fill="{B.TEAL}"/>')
    return out, W, H


def main():
    files = {
        "neomail-mark.svg": B.svg(64, 64, mark_body("light")),
        "neomail-mark-dark.svg": B.svg(64, 64, mark_body("dark")),
        "neomail-mark-mono.svg": B.svg(64, 64, mark_body("mono")),
        "favicon.svg": B.svg(64, 64, icon_body()),
        "icon-mono.svg": B.svg(64, 64, icon_body(mono=True)),
    }
    for theme in ("light", "dark", "mono"):
        body, w = lockup_body(theme)
        files[f"neomail-logo-{theme}.svg"] = B.svg(w, 64, body)
    body, w, h = social_body()
    files["social-preview.svg"] = B.svg(w, h, body, w=w, h=h)
    for name, content in files.items():
        (OUT / name).write_text(content)
        print(f"{name:26} {len(content):6} bytes")
    s, x0, right = word_geometry()
    print(f"word scale {s:.5f}  pen start {x0:.2f}  ink right {right:.2f}")


if __name__ == "__main__":
    main()
