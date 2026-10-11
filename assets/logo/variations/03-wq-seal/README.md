# 03 · `:wq` seal

A letter closed with a drop of carmine sealing wax pressed with `:w`. In Vim, `:w` writes the buffer; sealing a letter is the paper version of writing it, so the message is final and ready to go.

## What is Neovim about it

- The impression is the most-typed command in Vim: a colon, then `w`. Every Vim user has typed it thousands of times, often in programs where it does nothing. Here it does something.
- The `:w` is drawn as if typed in a terminal font. The colon dots and the `w` share one 5-unit stem and flat-cut ends. The `w`'s middle apex stops 5.5 units below the x-height, as in a monospace `w`.
- The splash tagline carries the joke: `:wq` seals and sends, `ZZ` does the same, faster.
- Nothing comes from the Neovim logo. There are no prisms, no N, and no green or blue.

## What is mail about it

- The full mark is the back of an envelope, and the seal closes the point of its flap. A wax seal is the oldest sign for a private, closed letter, which suits a client that puts security first.
- It stays clear of the other mail icons. Gmail's is an M made of an envelope in four colours. Proton Mail's is a purple envelope, Outlook's is a blue O with an envelope, Thunderbird's is a bird around a letter, and Apple Mail's is a white envelope front on blue. None of them uses an outlined envelope back in ink with a red seal.

## Files

| File | Use |
| --- | --- |
| `mark.svg` | Full mark for light backgrounds: an ink envelope with the carmine seal |
| `mark-dark.svg` | Full mark for dark backgrounds: a grey envelope with brighter wax |
| `mark-mono.svg` | One path in `currentColor`; the flap lines stop 2.4 units short of the wax, and `:w` is cut out |
| `mark-32.svg` | Pixel-drawn 32×32 seal for 24 to 48 px (the bare seal, with its lobes) |
| `favicon.svg` | Pixel-drawn 16×16 seal for 16 px (the bare disc) |
| `lockup-light.svg`, `lockup-dark.svg` | Full mark and outlined `neomail` wordmark (248×64 units) |
| `logo.txt` | Half-block seal for the TUI splash, 16 columns by 8 rows |
| `logo.ans` | The same seal in 24-bit ANSI, with the cream `:w` (`cat logo.ans`) |
| `preview.png` | Review sheet |
| `build.py` | Regenerates everything listed above |

All SVGs are paths with a `viewBox`. They have no `<text>`, rasters, filters or gradients. The master is flat colour, so no fallback is needed.

## Palette

| Role | Hex | Notes |
| --- | --- | --- |
| Wax | `#B3203F` | Carmine, hue 347°. Seal on light backgrounds and in `favicon.svg` and `mark-32.svg`. 6.17:1 on `#F6F8FA` |
| Wax, dark | `#CF2E4A` | Hue 350°. Seal on dark backgrounds and in `logo.ans`. 3.73:1 on `#0D1117` |
| Impression | `#FFF3E0` | The `:w`. 5.99:1 on `#B3203F` and 4.62:1 on `#CF2E4A` |
| Ink | `#0D1117` | Envelope line and wordmark on light. 17.78:1 on `#F6F8FA` |
| Envelope line, dark | `#8B98A5` | Cool grey. 6.43:1 on `#0D1117` |
| Paper | `#EDE7DD` | Wordmark on dark. 15.39:1 on `#0D1117` |

The contrast ratios use the WCAG 2 relative-luminance formula, and the hues come from Python's `colorsys`. Both were computed from these hex values for this revision. The idea depends on warm wax against cool ink: the seal is the only warm thing in the brand, and everything else is ink, grey or near-black.

## Construction (64-unit grid)

- **Wax edge.** The edge is a disc of radius 25 with four drops of wax pressed out from under the stamp. The drops sit at 20°, 124°, 172° and 256° (SVG angles, so 90° points down), reach 2.6 to 4 units past the disc and are 5.5 to 8 units in radius. The edge is the soft union of the disc and the drops: along each ray, r = k·ln Σ exp(rᵢ/k) with k = 1.1. Each drop meets the disc with a smooth fillet, and the outline is a single curve. It is sampled at 120 points and joined with Catmull-Rom curves converted to cubic Béziers.
- **Envelope.** The envelope is a 60×46 rounded rectangle at (2, 9) with a corner radius of 5. Its outline is a 2.6-unit ring. The two flap lines are 2.6 units wide and run from the inner corners to the seal's centre at (32, 32.7). The seal is the 64-unit seal scaled by 0.67, so the stamp keeps its proportions. `build.py` checks that the seal, plus 2.4 units of clearance, stays inside the envelope's inner edge.
- **`:w`.** The x-height is 15 units and the stem is 5. Each colon dot is a 5×5 square with corners rounded at 20 %. A 4-unit gap follows, then a 22-unit `w`. The `w` is a five-point polyline stroked with mitre joins and clipped flat to the x-height band. Its feet sit at 23 % and 77 % of its width, and its middle apex is 5.5 units below the top. The whole `:w` is 31 units wide and centred on the seal.
- **Mono.** The mono mark is one nonzero path. The envelope ring, the flap lines and the wax run clockwise, and the `:w` runs anticlockwise so it is cut out. The flap lines stop 2.4 units before the wax edge. Nothing overlaps the seal, so the winding is never ambiguous.
- **32 px.** The edge of `mark-32.svg` is sampled from the same r(t): a pixel is wax when its centre lies inside the edge. That keeps the four drops in place. The `:w` is drawn by hand: 3×3 colon dots, a 2 px gap and a 12 px `w`. The `w`'s middle stem starts two rows down, and each half closes in its own foot.
- **Wordmark.** The wordmark is Adwaita Sans (SIL OFL), instanced at `wght 640, opsz 32` and outlined with `../../src/text.py` (HarfBuzz and fontTools). Its x-height is 24 units, centred on the mark. The gap from the mark to the ink of the `n` is 18 units, and kerning is off.
- **Clear space.** Leave at least 16 units around the mark.

## Small-size behaviour (honest)

Each size has its own drawing:

| Size | File | What survives |
| --- | --- | --- |
| 64 px and up | `mark.svg` | The envelope, the seal and `:w`. At 64 px the `:w` is 21 px wide (31 × 0.67 units) and still clean |
| 24 to 48 px | `mark-32.svg` | The envelope is dropped. At 32 px its lines are 1.3 px wide and the `:w` is only 10 px wide with 1.7 px stems, which blurs into a smudge (`32 env` in `preview.png`). The bare seal keeps its four drops as clear bumps, and `:w` reads |
| 16 px | `favicon.svg` | A bare round disc. The drops would only blur. The colon is 2×2 dots. The `w`'s outer stems step one pixel inward into two separate feet, and the middle stem starts two rows down, so the shape is two Vs, not a three-stem ш |

At 16 px the favicon reads as a red seal with a cream mark. Someone who knows Vim reads `:w`; anyone else sees a seal. The `w` is better than in the first draft, but it is still 8×6 pixels, so it will never be crisp. On `#0D1117` the favicon's `#B3203F` is a quieter 2.88:1. It still separates from the background in the 8× zoom, but it is less vivid than on light.

`logo.txt` is the favicon grid packed two pixel rows per cell, 16 columns by 8 rows. In the plain version the `:w` is made of spaces, so the terminal background shows through and forms the impression. `logo.ans` paints the `:w` in cream.

## Review notes

The art director's six changes, and what happened to each:

1. **Off Gmail red. Done.** The wax moved from hue 4° to carmine, `#B3203F` (347°) on light and `#CF2E4A` (350°) on dark. Gmail's `#EA4335` is at 4.6° (`colorsys`). I didn't use the suggested `#D9364F` for dark: cream on it is only 4.17:1, under the 4.5:1 target. `#CF2E4A` keeps the hue and gives 4.62:1.
2. **Inner ring. Dropped.** It was 1.7:1 against the wax and only added noise. The `:w` impression now does all the work.
3. **Wax edge. Done.** The five-harmonic wobble is replaced by four large drops on a round disc, and `mark-32.svg` is drawn for 32 px.
4. **Typed `w`. Done.** The colon and the `w` share one stem and flat ends, and the `w`'s middle apex is lowered. The favicon `w` now steps diagonally into two feet.
5. **Hint at the letter. Done, but not as a bare chevron.** I first drew two thin flap lines behind the disc. Rendered alone, the lines read as antennae on a red bug, or as a pendant on a cord, not as a letter. A full envelope back, an ink outline with the flap, reads as a sealed letter immediately, at the cost of a smaller seal. Since `mark-32.svg` and `favicon.svg` already handle the small sizes, the full mark can afford it. The lockups use the full mark.
6. **Light wordmark in `#0D1117`. Done.**

## Rebuild

```sh
cd assets/logo/variations/03-wq-seal
uv run --with pillow --with uharfbuzz --with fonttools python build.py
```

This needs `rsvg-convert`, Adwaita Sans at `/usr/share/fonts/Adwaita/` and DejaVu Sans Mono. DejaVu Sans Mono is used only for the text in the terminal mock on the preview sheet.
