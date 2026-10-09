#!/usr/bin/env bash
# Records each animation style headless (tmux + asciinema 3), then renders
# GIF (agg) and MP4 (ffmpeg). Usage: ./record.sh [secs] [style...]
set -euo pipefail

cd "$(dirname "$0")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/neomail-targets/sort-animation}"
OUT="${OUT:-$HOME/.local/share/neomail-dev/recordings}"
SECS="${1:-12}"
shift || true
STYLES=("$@")
[ ${#STYLES[@]} -eq 0 ] && STYLES=(machine stamp decode)
SOCK=neomail-anim

cargo build --release --quiet
BIN="$CARGO_TARGET_DIR/release/sort-animation"
mkdir -p "$OUT"

for style in "${STYLES[@]}"; do
    cast="$OUT/$style.cast"
    rm -f "$cast"
    tmux -L "$SOCK" kill-session -t "rec-$style" 2>/dev/null || true
    # Status line off so the pane is exactly 120x36.
    tmux -L "$SOCK" -f /dev/null start-server \; set -g status off \; \
        new-session -d -s "rec-$style" -x 120 -y 36 \
        "TERM=xterm-256color COLORTERM=truecolor asciinema rec -q --overwrite --window-size 120x36 \
         -c '$BIN --style $style --duration $SECS' '$cast'"
    # Wait for the session (and the recording) to finish.
    while tmux -L "$SOCK" has-session -t "rec-$style" 2>/dev/null; do sleep 0.5; done
    agg --font-family "DejaVu Sans Mono" --font-size 14 --fps-cap 30 --idle-time-limit 3 \
        "$cast" "$OUT/$style.gif" >/dev/null 2>&1
    ffmpeg -loglevel error -y -i "$OUT/$style.gif" -movflags +faststart -pix_fmt yuv420p \
        -vf "scale=trunc(iw/2)*2:trunc(ih/2)*2" -c:v libx264 -crf 20 "$OUT/$style.mp4"
    echo "recorded $style -> $OUT/$style.{cast,gif,mp4}"
done
