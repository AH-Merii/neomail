# 02 · Tilde Inbox

A square-cornered envelope that is also an empty Vim buffer: a block cursor on line 1, then a column of `~` for the lines that do not exist. Your inbox is empty, so it looks like a freshly opened buffer: inbox zero as `:enew`.

## What is Neovim about it, what is mail about it

- **Neovim:** an empty buffer looks like this. The cursor sits in column 1 of line 1, and every line past the end of the buffer shows a `~` in column 1. The mark draws exactly that, flush left, with the tildes in an accent colour, as a colourscheme draws `NonText`. The frame has square corners and a heavy, even stroke, so it reads as a terminal window. Nothing is taken from the Neovim logo: there are no prisms and no N, and its green and blue are not used.
- **Mail:** an envelope with a V flap that hangs from the top corners. For people who have never used Vim, the tildes are the wavy writing lines of a letter and the cursor is where you start typing.

## Files

| File | Use |
| --- | --- |
| `mark.svg` | Full-colour mark for light backgrounds: ink envelope and cursor, burnt-amber tildes |
| `mark-dark.svg` | Full-colour mark for dark backgrounds: off-white envelope and cursor, amber tildes |
| `mark-mono.svg` | One path, `fill="currentColor"` |
| `favicon.svg` | Pixel drawing on a 16 px grid: an ink tile with off-white strokes. Works on any background. Use it below 32 px |
| `favicon-light.svg` | The same pixels as ink strokes on a transparent background, for light UIs |
| `lockup-light.svg`, `lockup-dark.svg` | Mark plus the outlined `neomail` wordmark |
| `logo.txt` | Half-block mark, 22 columns × 8 rows, only `▀ ▄ █` and spaces |
| `logo.ans` | The same art with 24-bit ANSI colours (`cat logo.ans`) |
| `logo-vim.txt` | Bonus: the same envelope with real `~` characters. It breaks the half-block-only rule, so it is optional |
| `preview.png` | Review sheet: sizes, zooms, both favicons, mono, lockups and a TUI splash mock |
| `build.py` | Regenerates everything above |

The SVGs are pure filled paths with a `viewBox`. `build.py` converts the strokes to outlines, and there is no `<text>`, raster or filter (`grep -c "<text\|<image\|filter" *.svg` returns 0 for every file). There are no gradients, so no fallback is needed.

## Palette

| Role | Hex | Contrast (WCAG) |
| --- | --- | --- |
| Ink: envelope and cursor on light, dark background | `#0D1117` | 17.78:1 on paper |
| Paper: light background | `#F6F8FA` | |
| Off-white: envelope and cursor on dark | `#E8EEF3` | 16.18:1 on ink |
| Amber: tildes on dark | `#F2A93B` | 9.47:1 on ink |
| Burnt amber: tildes on light | `#9A5500` | 5.37:1 on paper |
| Muted: secondary text in the terminal mock | `#8B98A5` | |

The amber family sits at a hue of 33° to 36°. It recalls the phosphor of amber terminals and is far from the violet and blue that most mail icons use. Do not use `#F2A93B` on light backgrounds: it measures 1.88:1 on paper. The neutrals are shared with the main brand (`../../README.md`). The teal cursor colour of the main mark is deliberately not used: the cursor here is drawn in the envelope's colour.

## Construction (64-unit grid; 2 units = 1 px at 32 px, 4 units = 1 px at 16 px)

- **Envelope:** the outer edge runs x 2–62 and y 4–60, with square corners and a stroke of 6. All of its edges land on whole pixels at 32 px.
- **Flap:** a band 6 units thick, measured at right angles, that hangs from the outer top corners. Its lower edge meets at (32, 24), on the centre cell boundary. Because the band starts at the corners rather than inside the frame, the left column stays free for line 1.
- **Cursor:** a block from (12, 20) to (18, 28), on whole pixels at 32 px, in the envelope colour.
- **Tildes:** two of them. Each is one full sine period (one round bump up and one down), 18 wide with amplitude 2.2 and stroke 3.6 with round ends. The stroke is lighter than the frame, like glyphs inside a window. The left end is at x 12, under the cursor, and the centre lines are at y 35.5 and 46.5. Each tilde's ink is 8 units tall. That leaves 3.5 units of clear background between the cursor and tilde 1, 3 between the two tildes, and 3.5 between tilde 2 and the frame.
- **Lockup:** Adwaita Sans (SIL OFL, outlined with HarfBuzz and fontTools through `../../src/text.py`), `wght 700, opsz 32`, no kerning, lowercase. The x-height is 28 units. The word's ink, from baseline to the top of the l, is centred on the mark's centre (y 32). There are 16 units from the envelope's right edge to the n.
- **Favicon:** hand-placed pixels. The frame is 1 px over rows 1–15, and the flap is 1 px, falling one row per 2 px to a 2 px apex centred on the x = 8 boundary. The cursor is 2 × 3 px. The tildes are a 6 × 2 pixel shape (`.##..#` / `#..##.`), with one clear row between every element.
- **Terminal:** its own 22 × 16 pixel drawing, two pixels per cell. The cursor is exactly one character cell, a real `█` on text row 2. Each tilde fills one text row as `▄▀▀▀▄▄▄▀`, on rows 4 and 6. Tildes never share a cell with the frame (`build.py` asserts this), so each cell has a single foreground colour.

## Small sizes, honestly

- **256 px and up:** the joke lands: an envelope with a cursor and two `~`.
- **64 px:** still clearly a cursor and two tildes.
- **32 px (master mark):** the two tildes stay separate, with about 1.5 px of background between them (see `mark 32 ×8` in `preview.png`). They read as waves. The cursor is crisp.
- **16 px (master mark):** the envelope and the cursor survive, but the tildes blur into two amber bars. Use the favicon instead.
- **16 px (`favicon.svg`):** a clear envelope with a white cursor block and two amber squiggles. Zoomed 8×, the 2-row tildes look stepped. At 1× they read as small waves. I compared them with anti-aliased vector tildes at 1× and the pixels read better: the vector ones turned into soft bars. Vim users will see `~`. Everyone else will see lines of writing, which is also correct.
- **Light backgrounds:** `favicon.svg` is an ink tile, so its silhouette holds on paper. `favicon-light.svg` is there for UIs that want a tile-free icon.
- **Terminal:** `logo.ans` reads at 22 × 8 cells. The cursor is a single `█` cell, as in a real terminal.

## Review notes

What changed after the art director's review, and where I disagreed:

1. **Stock container: changed.** The rounded outline envelope is now a square-cornered terminal frame with a stroke of 6 (it was 5), and the flap hangs from the outer corners with its apex on the centre cell boundary. Together with the cursor and the tilde column, the container is now specific to this mark. I did not draw a separate line-number gutter: at 16 px it would cost two of the 14 interior pixels, and an empty buffer shows `~` in column 1 anyway.
2. **Tildes merging at 32 px: fixed.** There are two tildes at every size, as true two-bump sine waves, separated by 3 units of clear background (1.5 px at 32 px). I did **not** reach a full tilde-height of space (8 units). The cursor on line 1 needs that room, and the 32 px zoom shows the tildes already separate. Opening the gap further would mean a shallower flap, and the shallow-flap drafts read as a tray rather than an envelope.
3. **Zigzag tildes at 16 px: partly fixed.** The favicon now uses a 6 × 2 two-bump tilde in place of the 7 × 3 three-step one. At 1× it reads as a wave. At 8× it still looks stepped, as any 2-pixel-tall curve does. The 16 px note above says which version to use.
4. **Lopsided, flush-left content: kept flush left, with balance added.** The block cursor on line 1 turns the tildes into a deliberate text column (cursor, `~`, `~`), and the deeper flap carries weight across the top. I kept everything in column 1 on purpose: an empty Vim buffer is flush left, and centring the tildes would lose the joke. The empty right-hand side is the empty inbox.
5. **Favicon rim vanishing on light: fixed** with `favicon-light.svg` (ink strokes, no tile). The default `favicon.svg` still holds on light because its silhouette is the dark tile.
6. **Periwinkle near Proton: changed.** The accent is now amber (hue 33° to 36°), well away from violet and from the blues of mail icons. The cursor uses the envelope colour, not an accent, to stay distinct from the main mark's teal cursor.

## Rebuild

```sh
uv run --with pillow --with uharfbuzz --with fonttools python build.py
```

This needs `rsvg-convert`, plus Adwaita Sans and Adwaita Mono under `/usr/share/fonts/Adwaita/`. Mono is used only for the terminal mock in the preview.
