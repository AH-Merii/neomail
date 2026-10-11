"""Shape text with HarfBuzz and outline it with fontTools (no <text> in assets)."""
from functools import lru_cache

import uharfbuzz as hb
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

from brand import FONT


def _fmt(v):
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


@lru_cache(maxsize=None)
def instance(wght, opsz):
    """Static instance of the variable font (returned font, glyph set, glyph order)."""
    f = instancer.instantiateVariableFont(TTFont(FONT), {"wght": wght, "opsz": opsz})
    return f, f.getGlyphSet(), f.getGlyphOrder()


def metric(ch, wght, opsz, which):
    """Ink bound of a glyph in font units: which in xmin/ymin/xmax/ymax."""
    f, gs, _ = instance(wght, opsz)
    bp = BoundsPen(gs)
    gs[f.getBestCmap()[ord(ch)]].draw(bp)
    return dict(zip(("xmin", "ymin", "xmax", "ymax"), bp.bounds))[which]


def shape(text, wght, opsz, kern=True):
    """Return [(glyph_id, x_pos, y_off)] in font units, plus total advance."""
    blob = hb.Blob.from_file_path(FONT)
    font = hb.Font(hb.Face(blob))
    font.set_variations({"wght": wght, "opsz": opsz})
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf, {"kern": kern})
    out, x = [], 0
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        out.append((info.codepoint, x + pos.x_offset, pos.y_offset))
        x += pos.x_advance
    return out, x


def outline(text, x, baseline, scale, wght, opsz, kern=True):
    """One SVG path 'd' for the whole string; pen starts at x on baseline."""
    _, gs, order = instance(wght, opsz)
    glyphs, adv = shape(text, wght, opsz, kern)
    pen = SVGPathPen(gs, _fmt)
    for gid, gx, gy in glyphs:
        t = TransformPen(pen, (scale, 0, 0, -scale, x + gx * scale, baseline - gy * scale))
        gs[order[gid]].draw(t)
    return pen.getCommands(), adv * scale


def advances(text, wght, opsz):
    """Per-glyph advances in font units (used to verify zero kerning)."""
    plain = [shape(c, wght, opsz)[1] for c in text]
    return plain, shape(text, wght, opsz)[1]
