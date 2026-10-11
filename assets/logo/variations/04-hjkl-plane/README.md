# 04 · hjkl plane

A message moves the way a Vim cursor does, `7l` along the floor and then `2k` up, in square-cornered strokes on the cell grid. Where the motions run out, the paper plane takes over and flies off to the corner: typed precisely, then sent.

## What is Neovim about it

- The path is a motion, not a swoosh. It runs only along the grid, right (`l`) then up (`k`), with a one-cell gap at the turn so each motion stays a separate keystroke. The plane is the third stroke: its tail is a flat edge exactly as wide as the `k` stroke, one cell above it, so path and plane are one gesture.
- The whole mark sits on a 16 × 16 cell grid, like a cursor in a terminal. The trail and the plane's tail are whole cells, the 16 px render puts them on whole pixels, and the terminal version is the same drawing in half blocks.
- It does not use the Neovim logo's shapes or colours. There are no prisms, no N and no green or blue.

## What is mail about it

- The paper plane is the "send" gesture everyone already knows, so the mark says "send mail" before it says anything else. The fold is cut into the plane, so it reads as folded paper, not an arrow.
- It is distinct from the mail brands. It has no envelope (Gmail, Proton Mail, Outlook, Apple Mail), no bird or globe (Thunderbird), and no blue circle (Telegram). The plane points to the top-right corner and leaves a right-angled amber path, which none of them have.

## Palette

| Role | Hex | Contrast | Notes |
| --- | --- | --- | --- |
| Ink | `#0D1117` | 17.78:1 on paper | Dark background, plane on light, favicon tile |
| Paper | `#F6F8FA` | | Light background |
| Sheet | `#E8EEF3` | 16.18:1 on ink | Plane on dark, favicon, terminal |
| Amber | `#FFB547` | 10.77:1 on ink | Motion path on dark, favicon, terminal, hue 36° |
| Amber (light) | `#C27A00` | 3.25:1 on paper | Motion path on light. Use it for graphics only, not text |

These are two colours per theme and there are no gradients. The mono mark is one even-odd path in `currentColor`, and it is the same drawing because the fold is already a cut.

## Construction

- The grid is 16 × 16 cells, and 1 cell is 4 units of the 64-unit viewBox.
- The motion path has two strokes, each 2 cells thick. `l` is `(0,14)` to `(7,16)`. A 1-cell gap separates it from `k`, which is `(8,12)` to `(10,16)`. The two strokes share a bottom edge, so the turn reads as a corner.
- The plane is a five-point dart: nose `(16,0)`, left wing tip `(2,4)`, tail `(8,11)` to `(10,11)` and right wing tip `(13,15)`. The tail is one cell above the `k` stroke and exactly as wide.
- The fold is a wedge 1 cell wide, cut from the middle of the tail half way to the nose.
- In the lockup, the `l` stroke is the floor. The word's baseline is the bottom of the motion path (y 64), and the plane takes off above the word. The wordmark is Adwaita Sans at weight 600, outlined with `../../src/text.py`, with an x-height of about 29 units (just under half the mark).
- `logo.txt` and `logo.ans` are a hand-drawn 16 × 16 pixel version shown as 16 columns by 8 rows of `▀ ▄ █`. Every edge steps by one cell, and the fold is a diagonal of single empty pixels. `logo.ans` uses 24-bit foreground colour only, so the terminal's own background shows through.
- Run `build.py` to regenerate every file.

## Small sizes

- **16 px master**: the amber path lands on whole pixels and stays crisp. The plane is a clear arrowhead pointing up-right, so the mark reads as "something launched from an L-shaped path". The fold closes to a few faint pixels on both backgrounds, and the plane's edges are soft. It can be recognised, but it is not the full drawing (see `preview.png`, "16 px, 8x").
- **Favicon**: `favicon.svg` is a hand-drawn 14 × 14 pixel version on a 16 px ink tile with a radius of 3. Every edge is on a pixel, and the fold is a 1 px diagonal cut. At 16 px it keeps the plane, the fold and both motion strokes, and the ink tile makes it work on light and dark tabs.
- **32 px and up**: the vector mark keeps the fold, and from 64 px the tail and the gap above `k` are clear.

## Review notes

Changes from the first draft, as the art director asked:

1. **One gesture in a square.** The plane is drawn on the same cell grid as the path and fills the square diagonally. The nose is in the top-right corner, the wing tips reach toward the other two corners, and the path fills the bottom-left. The tail sits exactly on the end of the last motion, one gap-cell above it and as wide. In the lockup, the path is the word's floor, so the mark no longer floats.
2. **The path reads as a path.** Each stroke is now as thick as the plane's tail, and there is only one turn, with a 1-cell gap at the corner. It reads as an L-shaped route, not a bar chart.
3. **The plane is one colour.** The fold is a cut, so the mark has two colours (plane and path), and mono is the same drawing in one colour.
4. **The terminal plane is redrawn.** It is now a clean dart with one-cell steps and a diagonal fold, made from the same 16 × 16 drawing as the favicon, not a stack of block widths.
5. **The accent is amber.** `#FFB547` has a hue of 36°, well away from Gmail red (about 5°, as the review computed), from Neovim's green and blue, and from the teal of the current logo.

Points I rebut:

- **"The last motion should be `l`."** I kept the path to `l` then `k`, so the last motion is `k`. That way the plane's tail sits on a stroke that points the way the plane flies (mostly up). A final `l` stroke ran sideways into a tail that points up-right, and that is where the old draft looked broken. The review's aim, that the path ends exactly at the plane, is met.
- **"A paper plane is generic, and it is Telegram's silhouette."** The paper plane is generic on purpose, because it is the one picture that says "send" with no caption. What makes this mark ours is the grid path the plane leaves, which no send icon has. Telegram's plane is a white glyph pointing right inside a blue circle. This one points to the corner, has no container, and comes with an amber right-angle path. At 16 px it is a different shape from Telegram's circle.
- **"The concept doesn't survive at 16 px."** This is partly true of the vector master, and the Small sizes section above says so. The hand-drawn favicon keeps the plane, the fold and both strokes at 16 px, which is why it exists.
