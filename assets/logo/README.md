# neomail logo

**Split and statusline**, adopted by [DEC-2026-10-11-logo](../../docs/decisions.md#dec-2026-10-11-logo). It was variation 05 of the drafts in [PR #2](https://github.com/AH-Merii/neomail/pull/2). The other drafts and the retired cursor-n set were removed once 05 was chosen; they stay in that PR's history.

## The idea

The envelope is a Neovim window. A V seam from the two top corners forms the flap. A `:vsplit` separator drops from the flap's left arm at one third of the width, so the mail list and the reader are the two panes. Under the window sits a statusline whose left end is a powerline-shaped mode chip, the NORMAL block. The chip is the only colour in the mark.

- **Neovim:** panes, a vertical split, and a statusline with a coloured mode block and a powerline arrow, the shape lualine and airline users know. No part of the Neovim logo is used: no prisms, no slanted N, no green or blue.
- **Mail:** a solid envelope with a V flap, split where a mail client splits: a narrow list on the left, a wide reader on the right. The chip's arrow points right, like "send".
- **Not other mail icons:** no M (Gmail), no padlock or purple (Proton), no "O" tile (Outlook), no bird (Thunderbird), no blue gradient tile (Apple Mail).

## Files

| File | Use |
| --- | --- |
| `neomail-mark.svg` | Mark for light backgrounds (ink window, dark-amber chip) |
| `neomail-mark-dark.svg` | Mark for dark backgrounds (off-white window, amber chip) |
| `neomail-mark-mono.svg` | Single-colour mark, one `currentColor` path. The chip stays its own shape, cut off by the notch. |
| `neomail-logo-light.svg` | Mark and wordmark for light backgrounds |
| `neomail-logo-dark.svg` | Mark and wordmark for dark backgrounds |
| `neomail-logo-mono.svg` | Single-colour lockup, `currentColor` |
| `favicon.svg` | Favicon: 16 px pixel art of the same layout on an ink tile with a 1 px paper keyline |
| `icon.svg` | App icon for 48 px and above: the master mark at 75 % on the ink tile with a paper keyline |
| `icon-mono.svg` | Single-colour app icon: the favicon drawing cut out of the tile, `currentColor` |
| `favicon-32.png` | Rendered from `favicon.svg` (each drawing pixel becomes 2 × 2) |
| `icon-192.png`, `icon-512.png` | Rendered from `icon.svg`, where the 16 px pixel steps would look wrong |
| `social-preview.svg`, `social-preview.png` | 1280 × 640 repository and social card: lockup, tagline, and a statusline along the bottom |
| `logo.txt` | Plain Unicode lockup for the README and the TUI splash: the half-block mark (21 columns × 8 rows) with `neomail` and the tagline beside it, 59 columns wide |
| `logo.ans` | The same in 24-bit ANSI (`cat logo.ans`). The window uses the terminal's own foreground (`ESC[39m`), so it works on light and dark themes. The chip prints `NORMAL` in ink on amber; the wordmark is bold and the tagline dim. |
| `src/` | The generator: `brand.py` (palette and geometry), `build.py` (writes every file above), `text.py` (HarfBuzz and fontTools outlines), `review.py` (review sheets) |

All SVGs are filled paths with a `viewBox`. They contain no `<text>`, no raster images, no filters and no gradients. The mono files use `currentColor`, which is black in an `<img>` and inherits the text colour when inlined.

## Palette

WCAG 2 contrast ratios (sRGB relative luminance), computed by `brand.contrast()` in `src/brand.py`.

| Role | Hex | Contrast |
| --- | --- | --- |
| Ink: dark background, window on light, icon tile, chip label | `#0D1117` | 17.78:1 on paper |
| Paper: light background, icon keyline | `#F6F8FA` | |
| Off-white: window on dark | `#E8EEF3` | 16.18:1 on ink |
| Amber: mode chip on dark and on the ink tile | `#FFB238` | 10.52:1 on ink; ink label on amber 10.52:1 |
| Dark amber: mode chip on light | `#B86E00` | 3.74:1 on paper (graphics only, never text) |
| Muted: secondary text on dark (card tagline) | `#8B98A5` | 6.43:1 on ink, 5.17:1 on the card's bar |
| Bar: the card's statusline | `#21262D` | decorative; amber on it 8.46:1 |

Amber was chosen because no mail client and no Neovim mark uses it, and it reads as "status" without suggesting an error. Never use `#FFB238` on light backgrounds (1.69:1 on paper); use `#B86E00` there.

## Wordmark

- Adwaita Sans (SIL OFL), instanced at `wght 600, opsz 32`, outlined by `src/text.py`, so the font is not needed at runtime.
- Always lowercase, `neomail`, in one colour: ink on light, off-white on dark, `currentColor` in mono. Never colour part of the word or give it the accent.
- Tracking 0, the font's own spacing.
- `nmail` is the binary name in code and commands, never a logo.

## Geometry (64-unit grid, `MASTER` in `src/brand.py`)

- Every horizontal and vertical edge is on a multiple of 4 units, so it lands on whole pixels at 16, 32 and 64 px. All corners are square, like terminal cells; only the app-icon tile is rounded.
- Window x 4–60, y 8–40. Flap: a 4-unit V seam from the top corners to the apex (32, 24). Split: x 20–24, from the flap's left arm to the bottom edge, 8 units clear of the apex, so the seams never form a Y.
- Statusline y 44–56. The chip runs x 4–16 with a 45° point to x 22 (the split's centre line); a 6-unit notch separates it from the bar.
- **Lockup:** the "l" runs from the window top (y 8) to the statusline bottom (y 56), so the baseline is the statusline's bottom edge. The word scale is 0.03221 per font unit. The gap from the mark (x 60) to the ink of the "n" is 20 units. The lockup is 317 × 64 units.

## Clear space

Keep 16 units (a quarter of the mark's 64-unit height) clear on every side of the mark's or lockup's `viewBox`. The `viewBox` already holds 4 units of margin at the sides and 8 at the top and bottom.

## Minimum sizes

| Asset | Minimum | Below that |
| --- | --- | --- |
| Mark (`neomail-mark*.svg`) | 24 px | `favicon.svg`. At 16 px the master's flap diagonals blur to grey. |
| Lockup (`neomail-logo-*.svg`) | 32 px tall | The mark alone |
| `favicon.svg`, `icon-mono.svg` | 16 px | Drawn for 16 px; crisp at every whole multiple (32, 64…) |
| `icon.svg` | 48 px | `favicon.svg`. It is crisp at 64, 192 and 512 px, and soft at 16. |
| `logo.txt` / `logo.ans` | 59 columns | The mark alone: the first 21 columns of each line |

## Usage

- Dark variants on dark backgrounds, light variants on light ones. On photos or busy backgrounds use mono or the app icon.
- On GitHub, switch with `<picture>`:

  ```html
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/neomail-logo-dark.svg">
    <img alt="neomail" src="assets/logo/neomail-logo-light.svg" height="48">
  </picture>
  ```

- Use the tile only as an app-icon or favicon container. The main mark stands free.

## Don'ts

- Don't stretch, rotate, outline, shadow, or add gradients or rounded corners to the mark.
- Don't colour the window or the bar with the accent, or the chip with anything but amber (dark amber on light, or the mono colour).
- Don't move the split to the centre: the seams would meet in a Y, and the list | reader reading is lost.
- Don't close the notch between the chip and the bar, or turn the chip's point into a round end.
- Don't put the chip's label `NORMAL` into the SVG mark; it belongs to the terminal logo and the card.
- Don't print `logo.ans` with reverse video for the label: on light themes that gives paper on amber, 1.69:1.
- Don't use `#FFB238` on light backgrounds or `#B86E00` for text.

## Terminal logo

```
▄▄▀▀█████████████▀▀▄▄
████▄▄▀▀█████▀▀▄▄████
██████ █▄▄▀▄▄████████
██████ ██████████████   neomail
██████ ██████████████   a fast, modal terminal email client
██████ ██████████████
▀▀▀▀▀▀ ▀▀▀▀▀▀▀▀▀▀▀▀▀▀
NORMAL ██████████████
```

The mark is pixel art (`TERM` in `src/brand.py`), 21 × 16 px printed as 8 rows of half blocks. The split is column 6, and the statusline's one-cell notch is the same column. The bottom row prints `NORMAL` in the chip's six cells, in `logo.txt` too, so the chip still reads without colour; in `logo.ans` the SGR resets before the notch, so the notch takes the terminal's own background. The wordmark sits beside the window's middle rows, 3 columns from the mark, which keeps the whole lockup at 59 columns.

## Rebuilding

```sh
python3 assets/logo/src/build.py                            # every file above
python3 assets/logo/src/review.py /tmp/neomail-logo-review  # review-dark.png, review-light.png
```

This needs `rsvg-convert`, `magick`, Adwaita Sans in `/usr/share/fonts/Adwaita/` and, for the review sheets, Noto Sans Mono in `/usr/share/fonts/noto/`. If Pillow, uharfbuzz or fontTools are missing, the scripts re-run themselves under `uv run --with pillow --with uharfbuzz --with fonttools`. A re-run gives byte-identical files.
