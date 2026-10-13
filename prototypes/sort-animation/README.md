# sort-animation prototype

Three concepts for the live sorting animation in neomail. While jev sorts mail into groups,
the inbox animates each result. The demo is a unified inbox of 40 messages across a Gmail,
an Outlook and a Proton account. A seeded, simulated jev stream sends results with latencies
of 80–600 ms. Results below the confidence threshold go to "other", and spam usually arrives
with high confidence.

## Styles

| Style | What it does |
| --- | --- |
| `machine` | An envelope chip lifts out of the row and flies along a shallow curve, with a fading trail, into the group's bin in the sidebar. The bin flashes and shows "+n". At most 3 chips fly at once; further results join a chip (shown as ², ³ …) or land straight away. Without a sidebar, the tab only flashes. |
| `stamp` | A light beam sweeps the row while jev reads. On the result, the coloured group stamp slams in with a small shake, and the confidence bar and percentage fill in. Stamps land in place. In the grouped view (`g`), rows glide to their sections in batches. |
| `decode` | While jev reads, a 5-cell scramble window drifts across the subject. On the result, a decoder cursor sweeps over the subject and tag, tinting them in the group colour. The tint fades back over 1.5 s, and the counter sparkles. |

Motion modes:

- **`full`** (default): the style's animation.
- **`reduced`**: a brief static highlight of the row and bin. Nothing moves.
- **`off`**: results appear instantly.

Windows under 60x16 use reduced motion automatically. Windows under 100 columns or 24 rows
show group tabs instead of the sidebar.

## Run

```sh
export CARGO_TARGET_DIR=$HOME/.cache/neomail-targets/sort-animation
cargo run --release -- --style stamp
cargo run --release -- --style machine --sim-speed 4      # long sync: bulk mode
cargo run --release -- --style stamp --grouped --loop
./record.sh 13                                            # casts, GIFs, MP4s
```

| Flag | Meaning |
| --- | --- |
| `--style machine\|stamp\|decode` | Concept to show (default `machine`). |
| `--motion full\|reduced\|off` | Motion mode (default `full`). |
| `--speed <x>` | Animation speed, 0.1–10. |
| `--sim-speed <x>` | Speed of the simulated jev stream, 0.1–20. |
| `--seed <n>` | Simulation seed (default 7). |
| `--duration <secs>` | Exit after this many seconds (for recordings). |
| `--loop` | Restart once everything is sorted. |
| `--grouped` | Start in the grouped view. |

Keys:

| Key | Action |
| --- | --- |
| `1` `2` `3` | Switch style |
| `m` | Cycle motion mode |
| `g` | Toggle grouped view |
| `j` `k` | Move the selection; the selected row stays on its screen row |
| `r` | Restart |
| `q` | Quit |

## Stack

ratatui 0.30, crossterm 0.29 and tachyonfx 0.25.2, the latest stable release. tachyonfx
0.25.2 depends on `ratatui-core ^0.1.2`, the core crate of ratatui 0.30. It lists
`ratatui ^0.30.2` only as a dev-dependency. The effects use tachyonfx for the bin flashes,
the counter sparkles and the easing curves. The flights, beams, stamps and scrambles are
drawn by hand.

## Design notes for the real TUI

The code has four layers: `jev` (simulated classifier), `model`, `ui` and `anim`. Only
`anim` is meant to move into neomail, and it does not import `ui`.

- **The model never waits.** Every `JevEvent` is applied to the model first, and the base
  UI always renders the real state, including the counters and tags. Effects only draw on
  top. Nothing is held back, even at `--speed 0.1`.
- **Integration contract.** Forward events to `Animator::on_event` after the model, and
  publish a `Geometry`: row positions, bins, counters and the `stage`, which is the area a
  flight may cross. Then call `Animator::overlay`, and sleep until
  `Animator::next_frame()`. A return of `None` means idle, with no wake-ups.
- **Reading effects come from the model.** The animator works out which rows are being
  read from the model on every frame. Styles keep state only for short result animations.
  When the effective motion mode changes (the `m` key or a resize), all animation state is
  cleared. So a row can never stay stuck as "reading".
- **Styles.** Each style implements `AnimationStyle`: `on_result`, `update`, `overlay`,
  `next_frame`, `backlog` and `finish_all`. A new style is one file.
- **Backlog.**
  - From 4 pending results, animation speeds up, to at most ×3.5.
  - At 12, every running animation skips to its final state.
  - At 16 or more results within 2 s (a long sync), bulk mode animates only the counters.
- **Frame pacing.**
  - Each style asks for its next frame: as soon as possible while something moves, 30 Hz
    for the beam, 24 Hz for the scramble, and one timed wake-up for reduced motion.
  - The frame interval also adapts to draw cost and output size, keeping output under
    about 100 KB/s for SSH. It ranges from 60 fps down to 12 fps.
  - Each frame is wrapped in a synchronized update (DEC mode 2026).
- **Grouped view.** Grouping is a view setting, not part of a style. Section changes are
  applied in batches (at most every 0.6 s), and the selected row stays anchored on its
  screen row.
- **Glyphs.** None of the glyphs has the Unicode Emoji property, checked against
  emoji-data 16.0. This avoids colour-emoji rendering on Windows Terminal under WSL. They
  still need a check in Windows Terminal itself.
