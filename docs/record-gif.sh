#!/bin/bash
# Records the short demo for the website (docs/screenshots/demo.gif) with
# sample events, on an empty workspace. Needs no account and no input.
# Prepare the sample data first:
#   XDG_DATA_HOME=/tmp/gif-demo cargo run --release --example demo
set -uo pipefail

DATA=/tmp/gif-demo
WS=9
X=200 Y=120 W=1520 H=800       # recorded area (logical pixels), ~1.9:1 like Musinator's
OUT_MP4=/tmp/calinator-gif.mp4
OUT_GIF="$(cd "$(dirname "$0")" && pwd)/screenshots/demo.gif"

hl() { hyprctl dispatch "$1" >/dev/null || echo "window command failed: $1"; }
keys() { tmux send-keys -t gif "$@"; sleep "${PAUSE:-0.5}"; }
typed() { local s=$1 i; for ((i = 0; i < ${#s}; i++)); do tmux send-keys -t gif -l "${s:i:1}"; sleep 0.07; done; }
REC="" WIN=""
cleanup() {
  [ -n "$REC" ] && kill -INT "$REC" 2>/dev/null && wait "$REC" 2>/dev/null
  tmux kill-session -t gif 2>/dev/null
  [ -n "$WIN" ] && hl "hl.dsp.window.close({ window = 'address:$WIN' })"
  hl "hl.dsp.focus({ workspace = '2' })"
}
trap cleanup EXIT

[ -f "$DATA/calinator/calinator.db" ] || { echo "Prepare the sample data first (see the top of this script)."; exit 1; }
hl "hl.dsp.focus({ workspace = '$WS' })"
tmux kill-session -t gif 2>/dev/null
tmux new-session -d -s gif -x 150 -y 40 "sleep 4; env -u NO_COLOR TERM=xterm-256color COLORTERM=truecolor XDG_DATA_HOME=$DATA calinator"
tmux set -t gif status off >/dev/null
ghostty --class=calinator.gif --font-size=16 -e tmux attach -t gif >/dev/null 2>&1 &
sleep 3
WIN=$(hyprctl clients -j | python3 -c "import json,sys; print(next((c['address'] for c in json.load(sys.stdin) if c['class']=='calinator.gif'), ''))")
[ -n "$WIN" ] || { echo "Calinator window not found"; exit 1; }
hl "hl.dsp.window.float({ action = 'enable', window = 'address:$WIN' })"
hl "hl.dsp.window.resize({ x = $W, y = $H, relative = false, window = 'address:$WIN' })"
hl "hl.dsp.window.move({ x = $X, y = $Y, relative = false, window = 'address:$WIN' })"
hl "hl.dsp.focus({ window = 'address:$WIN' })"
sleep 4

gpu-screen-recorder -w region -region "${W}x${H}+${X}+${Y}" -f 30 -cursor no -o "$OUT_MP4" >/dev/null 2>&1 &
REC=$!
sleep 2.5                                           # 1. the week

keys a; sleep 0.3                                   # 2. quick add
typed "coffee with Alex fri 10am"; sleep 1.6
keys Enter; sleep 1.8

PAUSE=1.3 keys m; PAUSE=1.3 keys g; PAUSE=1.0 keys w   # 3. views

keys T; sleep 0.8                                   # 4. themes, live preview
for _ in 1 2 3 4 5; do PAUSE=0.9 keys j; done
sleep 0.4; keys Escape; sleep 1.5                   # back to Retro Stripes

kill -INT "$REC"; wait "$REC" 2>/dev/null; REC=""
ffmpeg -loglevel error -y -i "$OUT_MP4" -vf "fps=12,scale=1000:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle" "$OUT_GIF"
echo "$OUT_GIF ($(du -h "$OUT_GIF" | cut -f1))"
