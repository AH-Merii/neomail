"""neomail brand constants: palette and hand-drawn geometry (64-unit grid)."""

# Palette
INK = "#0D1117"        # dark background, light-theme foreground, icon tile
PAPER = "#F6F8FA"      # light background
OFF = "#E8EEF3"        # dark-theme foreground
TEAL = "#48DCC1"       # accent on dark / on the ink tile (11.1:1 on INK)
TEAL_L = "#008F7A"     # accent graphics on light (3.79:1 on PAPER)
TEAL_TXT = "#037A69"   # accent text on light (4.94:1 on PAPER)
MUTED = "#8B98A5"      # secondary text on dark (social preview tagline)
MONO = "#000000"

THEMES = {
    "dark": {"bg": INK, "fg": OFF, "accent": TEAL},
    "light": {"bg": PAPER, "fg": INK, "accent": TEAL_L},
    "mono": {"bg": "#FFFFFF", "fg": MONO, "accent": MONO},
}

# Master mark, viewBox 0 0 64 64. Stroke 12, shoulder radii 24/12,
# cursor is a 12x20 terminal cell separated from the shoulder by a 4-unit gap.
MARK_N = "M8 56V8H32A24 24 0 0 1 56 32H44A12 12 0 0 0 32 20H20V56Z"
MARK_CURSOR = "M44 36H56V56H44Z"

# App icon / favicon, drawn separately for 16 px (stem 3 px, counter 4 px).
ICON_TILE = ("M14 0H50A14 14 0 0 1 64 14V50A14 14 0 0 1 50 64H14"
             "A14 14 0 0 1 0 50V14A14 14 0 0 1 14 0Z")
ICON_N = "M12 52V12H32A20 20 0 0 1 52 32H40A8 8 0 0 0 32 24H24V52Z"
ICON_CURSOR = "M40 36H52V52H40Z"

# Lockup (64 units tall): mark bottom on the baseline, mark top = top of "l".
BASELINE = 56
ASCENT_TOP = 8
MARK_RIGHT = 56
WORD_GAP = 24          # = mark counter width (two strokes)
CLEAR = 24             # clear space on all sides, in lockup units
WORD = "neomail"
WORD_AXES = {"wght": 640, "opsz": 32}
FONT = "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf"


def svg(view_w, view_h, body, title="neomail", w=None, h=None):
    size = f' width="{w}" height="{h}"' if w else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view_w} {view_h}"{size} '
            f'role="img" aria-label="{title}"><title>{title}</title>{body}</svg>\n')


def path(d, fill, extra=""):
    return f'<path fill="{fill}"{extra} d="{d}"/>'
