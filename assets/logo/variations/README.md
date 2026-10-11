# neomail logo variations

Six alternatives to the current "cursor n" mark (`../neomail-mark.svg`, PR #2), shown side by side with it.

| Sheet | What it shows |
| --- | --- |
| `sheet.png` | All seven marks on `#0D1117`: 256 px `mark-dark.svg`, 32 px, the `logo.ans` half-block art in a terminal mock, and `lockup-dark.svg` |
| `sheet-light.png` | The same on `#F6F8FA` with `mark.svg` and `lockup-light.svg`. The terminal art is the plain `logo.txt` in ink, because each `logo.ans` is tuned for dark terminals |
| `small.png` | 16 px (`favicon.svg`, or `favicon-light.svg` on light where it exists) and 32 px (`mark-32.svg` where it exists, otherwise the full mark) at 1x and 8x, on both backgrounds |

Each folder's own `preview.png` has the full review sheet for that variation.

## The set, in art director ranking order

The ranking comes from the art director. The art director gave no per-variation comments, so the last column is not a quote. It sums up how each mark behaves at small sizes, taken from the "Small sizes" section of that folder's README.

| Rank | No. | Name | Idea | Palette | Small-size note |
| --- | --- | --- | --- | --- | --- |
| 1 | 05 | Split and statusline | The envelope is a Neovim window: a V flap, a `:vsplit` separator dividing list from reader, and a statusline whose amber powerline chip is the NORMAL mode block | ink `#0D1117`, off-white `#E8EEF3`, amber `#FFB238` (dark) / `#B86E00` (light) | The 16 px favicon is pixel art: an envelope in a window over an amber-tipped bar |
| 2 | 03 | `:wq` seal | The back of an envelope closed with carmine wax pressed with `:w`. Writing the buffer is sealing the letter | wax `#B3203F` / `#CF2E4A` (dark), impression `#FFF3E0`, ink `#0D1117`, grey line `#8B98A5` | A separate drawing per size: the bare seal at 32 px (`mark-32.svg`), a plain disc with `:w` at 16 px |
| 3 | 06 | Cell Stamp | A perforated postage stamp whose picture is a pixel `@`, with a block cursor as the stem of its `a` | vermilion `#F26B3D` (dark) / `#D9542B` (light), field `#2B3548`, cream `#F7EEDC`, cursor `#E8A000` | The favicon is pixel-exact. At 16 px the perforation and the colour carry it, not the `@` |
| 4 | – | Cursor n (current) | A geometric lowercase n with a teal terminal-cursor block | ink `#0D1117`, off-white `#E8EEF3`, teal `#48DCC1` (dark) / `#008F7A` (light) | The n and cursor read at 16 px, but the mark carries no mail sign |
| 5 | 02 | Tilde Inbox | An envelope that is an empty Vim buffer: a cursor on line 1 and `~` lines below. Inbox zero is `:enew` | ink `#0D1117`, off-white `#E8EEF3`, amber `#F2A93B` (dark) / `#9A5500` (light) | The master's tildes blur at 16 px; the pixel favicon keeps two amber squiggles |
| 6 | 04 | hjkl plane | A paper plane that takes off from an `l`-then-`k` motion path on the cell grid: typed, then sent | ink `#0D1117`, sheet `#E8EEF3`, amber `#FFB547` (dark) / `#C27A00` (light) | The pixel favicon keeps the plane, the fold and both motion strokes at 16 px |
| 7 | 01 | Letterfold | A monospace `n` folded from one strip of letter paper, with writing on the inside panel | mulberry `#9C1B4D` / raspberry `#F0436A`, saffron `#B87400` / `#FFC24B` | At 16 px the writing smudges, so the mark says "n" and nothing about mail |

The ideas and hex values come from each folder's `README.md`. The current palette comes from `../README.md`.

## Rebuilding

Each variation regenerates its own files from its folder:

```sh
cd 01-prism-fold   # or 02-tilde-inbox, 03-wq-seal, 04-hjkl-plane, 05-split-statusline, 06-cell-stamp
uv run --with pillow --with uharfbuzz --with fonttools python build.py
```

The current mark is built from `../src/`:

```sh
cd ../src
uv run --with fonttools --with uharfbuzz python build.py   # SVG masters
uv run --with pillow python render.py /tmp/neomail-logo-review   # PNGs, logo.txt/.ans, review sheets
```

Then rebuild the comparison sheets in this folder:

```sh
uv run --with pillow python build_sheet.py
```

`build_sheet.py` renders the SVGs with `rsvg-convert`. It draws the half-block art cell by cell, as a terminal draws block elements, and sets any other characters in Noto Sans Mono. The labels use Adwaita Sans. Both fonts are read from `/usr/share/fonts`.
