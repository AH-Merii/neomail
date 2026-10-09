# neomail logo

The mark is a geometric lowercase **n** whose right leg is a **terminal cursor**: a 12×20 block, the proportions of a terminal cell. It sits on a 64-unit grid with a 12-unit stroke. The flat top runs into a quarter-circle shoulder (outer radius 24, inner radius 12). A 4-unit gap separates the cursor from the shoulder. The cursor is the only accent in the whole brand.

## Files

| File | Use |
| --- | --- |
| `neomail-mark.svg` | Mark for light backgrounds (ink n, teal cursor) |
| `neomail-mark-dark.svg` | Mark for dark backgrounds (off-white n, teal cursor) |
| `neomail-mark-mono.svg` | Single-colour mark, one path (recolour freely) |
| `neomail-logo-light.svg` | Mark and wordmark for light backgrounds |
| `neomail-logo-dark.svg` | Mark and wordmark for dark backgrounds |
| `neomail-logo-mono.svg` | Single-colour lockup |
| `favicon.svg` | App icon and favicon (ink tile, drawn separately for 16 px) |
| `icon-mono.svg` | Single-colour app icon (n and cursor cut out of the tile) |
| `favicon-32.png`, `icon-192.png`, `icon-512.png` | Raster app icons rendered from `favicon.svg` |
| `social-preview.png` (`.svg`) | 1280×640 repository and social card |
| `logo.txt` | Plain Unicode half-block lockup for the README and TUI splash (6 lines) |
| `logo.ans` | The same lockup with a 24-bit ANSI teal cursor (`cat logo.ans`) |
| `src/` | Build scripts that generate everything above |

All SVGs are hand-built paths with a `viewBox`. They contain no `<text>` and no raster images.

## Palette

| Role | Hex | Notes |
| --- | --- | --- |
| Ink | `#0D1117` | Dark background, light-theme foreground, icon tile |
| Paper | `#F6F8FA` | Light background (ink on paper 17.8:1) |
| Off-white | `#E8EEF3` | Dark-theme foreground (16.2:1 on ink) |
| Teal | `#48DCC1` | Accent on dark and on the ink tile (11.1:1 on ink) |
| Teal (light) | `#008F7A` | Accent graphics on light backgrounds (3.79:1 on paper) |
| Teal text (light) | `#037A69` | Accent **text** on light backgrounds (4.94:1, WCAG AA) |
| Muted | `#8B98A5` | Secondary text on dark, for example the tagline (6.4:1 on ink) |

Do not use `#48DCC1` on light backgrounds (1.6:1). Do not use `#008F7A` for small text. The retired concept colours `#3DDC97` and `#0A9963` are too close to Android, Nuxt and Supabase green.

## Wordmark

- Font: Adwaita Sans (SIL OFL; derived from Inter), instanced at `wght 640, opsz 32` (Inter Display style). The text is outlined, so the font is not needed at runtime.
- Always set the wordmark in lowercase, `neomail`, in one colour: ink on light, off-white on dark, black or white in mono. Never split it into neo|mail colours or add accent letters.
- Tracking is 0 with no pair adjustments. The font applies no kerning to this string.
- Use `nmail` only for the binary name in code or commands, never as a logo.

## Lockup geometry (64 units tall)

- The mark's bottom sits on the baseline (y 56). Its top lines up with the top of the "l" (y 8). Word scale: 0.03221 per font unit, giving an x-height of 34 units.
- The gap between mark and word is 24 units, measured from the cursor's right edge (x 56) to the ink of the "n".
- **Clear space:** 24 units (half the mark height) on all sides.
- **Minimum size:** the lockup at 20 px tall; the mark alone at 16 px. Below 20 px, use the mark alone.

## Usage rules

- Use the dark variants on dark backgrounds and the light variants on light backgrounds. On photos or busy backgrounds, use mono or the app icon.
- On GitHub, switch with `<picture>`:

  ```html
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/neomail-logo-dark.svg">
    <img alt="neomail" src="assets/logo/neomail-logo-light.svg" height="48">
  </picture>
  ```

- Use the tile (`favicon.svg`) only as an app-icon or favicon container. The main mark is the freestanding n.
- For the favicon and sizes of 32 px or less, use `favicon.svg` or the PNGs. Scaling the master mark blurs it at 16 px; the icon is drawn on its own grid (stem 3 px, counter 4 px, cursor 3×4 px at 16 px).
- Never put the n on a solid teal square or any other bright square. That is npm's territory.
- Don't stretch, rotate, outline, add shadows or gradients, recolour the n with the accent, or close the gap between the cursor and the shoulder.
- Don't move the cursor to the left leg or make it blink in static assets. Animating the cursor block is fine in the TUI and in motion work.
- Keep the sorting chevrons and lanes for the AI-sorting animation, not for the logo.

## Terminal rendition

At 16 px, the mark maps exactly onto half-block characters: each terminal cell is two pixels tall, so the mark is 12 columns by 6 rows. `logo.txt` and `logo.ans` can be printed in the splash screen or by `nmail --version`.

```
████████▄▄
███▀▀▀▀▀███
███      ███   neomail
███      ▄▄▄   vim-style terminal mail, sorted by AI
███      ███
███      ███
```

## Rebuilding

```sh
cd assets/logo/src
uv run --with fonttools --with uharfbuzz python build.py   # SVG masters
uv run --with pillow python render.py /tmp/neomail-logo-review   # PNGs, logo.txt/.ans, review sheets
```

This needs `rsvg-convert`, `magick` and Adwaita Sans at `/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf`.
