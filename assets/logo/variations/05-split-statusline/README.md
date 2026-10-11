# 05 · Split and statusline

**The idea.** The envelope is a Neovim window: a V seam from the two top corners forms the flap, and a `:vsplit` separator drops from the flap's left arm at one third of the width, so the mail list and the reader are the two panes. Under the window sits a statusline whose left end is a powerline-shaped mode chip, the NORMAL block, and it is the only colour in the mark.

## Neovim and mail

- **Neovim:** window panes, a vertical split, and a statusline with a coloured mode block and a powerline arrow separator. Anyone who uses lualine or airline knows that last shape. In the terminal logo the chip carries the word `NORMAL`, so the splash rhymes with the statusline under it. No part of the Neovim logo is used: there are no prisms and no slanted N, and there is no green or blue.
- **Mail:** a solid envelope with a V flap. The split sits where a mail client puts it, with a narrow list on the left and a wide reader on the right. The chip's arrow also points right, like "send". That is a second reading, not the main one.
- **Not other mail icons:** there is no M (Gmail), no padlock or purple (Proton), no tile with an "O" (Outlook), no bird (Thunderbird), and no blue gradient tile (Apple Mail). The off-centre split and the statusline under the envelope are what make it this mark.

## Files

| File | Use |
| --- | --- |
| `mark.svg` | Full-colour mark for light backgrounds (ink window, dark-amber chip) |
| `mark-dark.svg` | Full-colour mark for dark backgrounds (off-white window, amber chip) |
| `mark-mono.svg` | One `currentColor` path. The chip is still its own shape, cut off by the notch. |
| `favicon.svg` | 16 px pixel drawing of the same layout, on an ink tile with a 1 px paper keyline |
| `lockup-light.svg`, `lockup-dark.svg` | Mark with the outlined "neomail" wordmark |
| `logo.txt` | Half-block mark, 21 columns by 8 rows, for the TUI splash (`▀ ▄ █` and spaces only) |
| `logo.ans` | The same mark in 24-bit ANSI. The window uses the terminal's own foreground (`ESC[39m`). The chip row prints `NORMAL` in ink on amber. |
| `preview.png` | Review sheet. The mock splash shows `logo.ans`, with a small plain `logo.txt` beside it. |
| `build.py` | Regenerates everything above |

All SVGs are filled paths with a `viewBox`. They contain no `<text>`, no raster images, no filters and no gradients, so no flat fallback is needed.

## Palette

Contrast ratios are WCAG values computed in `build.py`'s colours (sRGB relative luminance).

| Role | Hex | Contrast |
| --- | --- | --- |
| Ink: dark background, window on light, favicon tile, chip label | `#0D1117` | 17.8:1 on paper |
| Paper: light background, favicon keyline | `#F6F8FA` | |
| Off-white: window on dark | `#E8EEF3` | 16.2:1 on ink |
| Amber: mode chip on dark and on the favicon | `#FFB238` | 10.5:1 on ink |
| Dark amber: mode chip on light | `#B86E00` | 3.7:1 on paper (graphics only, not text) |

Amber was chosen because no mail client and no Neovim mark uses it. A NORMAL-mode chip is typically a saturated block, and amber reads as "status" without suggesting an error.

## Construction (64-unit grid, `MASTER` in `build.py`)

- **Grid:** every horizontal and vertical edge is on a multiple of 4 units, so it lands on whole pixels at 16, 32 and 64 px. All corners are square, like terminal cells. There are no radii anywhere.
- **Window:** x 4–60, y 8–40.
- **Flap:** a V seam, 4 units wide (perpendicular). Its centre lines run from the top corners (4, 8) and (60, 8) to the apex (32, 24), which gives a slope of 4:7.
- **Split:** x 20–24. It runs from the lower edge of the flap's left arm down to the bottom edge. The list pane is 16 units wide and the reader pane is 36. That leaves three pieces: the flap, the list and the reader. The apex is 8 units to the right of the split, so the two seams never merge into a Y.
- **Statusline:** y 44–56, 4 units under the window. The chip body runs from x 4 to 16, under the list pane, followed by a 45° point to x 22, which is the split's centre line. The bar starts 6 units to the right, with a matching notch. The gap is 4.2 units measured perpendicular, which is 2 px at 32 px.
- **Margins:** 4 units at the sides and 8 at the top and bottom.
- **Lockup:** the wordmark is set in Adwaita Sans (SIL OFL), instanced at wght 600 and opsz 32. It is outlined by `../../src/text.py` (HarfBuzz and fontTools). The "l" runs from the window top (y 8) to the statusline bottom (y 56), so the wordmark's baseline is the statusline's bottom edge. The gap from the mark to the word is 20 units.

## Small sizes

- **Master at 32 px and above:** crisp. The split is x 10–12, the window and bar edges are on whole pixels, and the chip's point and the notch are both visible.
- **Master at 16 px:** the split is a clean 1 px column (x 5) and the chip and bar are crisp. The flap's diagonals are anti-aliased to grey, so the envelope reads, but softly. Use `favicon.svg` below 24 px.
- **`favicon.svg`:** this is pixel art on the 16 px grid (`FAV` in `build.py`), so every edge is on a whole pixel. It has an ink tile with a 1 px paper keyline, and a 12 × 8 window. The flap is a 1:1 stepped V, deeper than the master's, because in a side-by-side 16 px test a 2-column-step V (close to the master's 4:7 slope) read as a flat line rather than a flap. The split is the column under the left arm. The statusline is 3 px tall with a 3 px chip and a 1 px pixel arrow, and the 1 px notch follows the arrow. At 16 px it reads as an envelope in a window over an amber-tipped bar. On a #0D1117 tab bar the keyline draws the tile's edge. On a light tab bar the keyline disappears into the background and the ink tile is the edge. Checked on #0D1117, #F6F8FA, #DEE1E6 and #35363A.
- **Terminal:** `logo.txt` is pixel art (`TERM` in `build.py`), 21 × 16 px, printed as 8 rows of half blocks. The flap steps 2 columns per pixel row. The split is column 6, and the statusline's one-cell notch is the same column, so the notch reads as the split carrying on into the statusline. In `logo.ans` the six chip cells print `NORMAL`.

## Review notes

What changed after the art director's review:

1. **Y shape:** fixed. The split moved from the centre to x 20–24 (list | reader). It now drops from the flap's left arm, and the apex is 8 units clear of it. The flap is a clean V.
2. **Half-pixel seam:** fixed. The split is 4 units on the 4-unit grid, so it is 1 px at 16 px, 2 px at 32 px and 4 px at 64 px.
3. **Mixed corners:** fixed. All corners are square, including the statusline ends, to match the terminal-cell language. Rounded corners were rejected because the flap seam starts at the top corners, so a radius there would be cut off anyway.
4. **Notch and chip point:** done. The notch is 6 units horizontal, and the chip's point is a full 45° (6 units on a 12-unit bar). Both survive at 32 px. At 16 px the master's notch is 1.5 px. The favicon handles 16 px with a whole-pixel notch.
5. **Favicon tile on dark:** done. There is a 1 px paper keyline. The favicon was also redrawn as whole-pixel art, which removes the grey anti-aliased diagonals of the first draft.
6. **Terminal label:** done, with one deviation. The chip prints `NORMAL` as explicit ink on amber (`ESC[38;2;13;17;23;48;2;255;178;56m`), not as SGR 7 reverse video. Reverse video would draw the label in the terminal's background colour. On a light theme that is paper on amber, about 1.7:1, which is unreadable. On dark themes the result looks the same as reverse video.

Rebutted:

- **"logo.txt can only separate chip from bar with a one-cell notch":** kept on purpose. The one-cell notch now sits in the split's column, so it reads as the split, not as a compromise. A 45° point needs at least 3 pixel rows. That would make the statusline 2 cells tall in the terminal, and it would no longer look like a real one-row statusline.

## Rebuild

```sh
uv run --with pillow --with uharfbuzz --with fonttools python build.py
```

This needs `rsvg-convert` and Adwaita Sans at `/usr/share/fonts/Adwaita/`. The preview uses Adwaita Mono for the mock terminal. Temporary renders go to `/tmp/neomail-logo-05/build`.
