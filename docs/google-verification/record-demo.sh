#!/bin/bash
# Records the Google verification demo video. Runs Calinator in a separate,
# empty data folder (your real calendar file is not used), types every key
# itself, and shows each change in Google Calendar in the browser.
# You only sign in to Google when the browser asks.
#
# Only one area is recorded: the Calinator window and a new browser window lie
# on top of each other there, on an empty workspace. Other apps don't show.
#
# Before: see "Before recording" in demo-video.md.
# Run:    bash docs/google-verification/record-demo.sh
# Try it without recording or signing in: DRY=1 bash docs/google-verification/record-demo.sh
set -uo pipefail

DATA=/tmp/calinator-demo
DB=$DATA/calinator/calinator.db
CAL="Calinator demo"
PICK="@demo"
WS=9                       # an empty workspace
X=200 Y=120 W=1600 H=1000  # the recorded area (logical pixels)
TOMORROW=$(date -d tomorrow +%Y/%m/%d)
DAY_URL="https://calendar.google.com/calendar/r/day/$TOMORROW"
OUT="$HOME/Videos/calinator-demo-$(date +%Y-%m-%d_%H-%M).mp4"
if [ "${DRY:-}" = 1 ]; then CAL="Personal"; PICK="@personal"; fi

say() { printf '\n>>> %s\n' "$*"; }
hl() { hyprctl dispatch "$1" >/dev/null || echo "window command failed: $1"; }
state() { echo "    in $CAL: $(sqlite3 "$DB" "SELECT group_concat(json_extract(json,'\$.title') || ' ' || substr(json_extract(json,'\$.start'),12,5), ', ') FROM events WHERE calendar = (SELECT id FROM calendars WHERE json_extract(json,'\$.name') = '$CAL')")"; }
keys() { tmux send-keys -t demo "$@"; sleep 0.6; }
text() { tmux send-keys -t demo -l "$1"; sleep 0.8; }
addr_of() { hyprctl clients -j | python3 -c "import json,sys; print(next((c['address'] for c in json.load(sys.stdin) if c['class']==sys.argv[1]), ''))" "$1"; }
place() { # float the window at the recorded area
  hl "hl.dsp.window.float({ action = 'enable', window = 'address:$1' })"
  hl "hl.dsp.window.resize({ x = $W, y = $H, relative = false, window = 'address:$1' })"
  hl "hl.dsp.window.move({ x = $X, y = $Y, relative = false, window = 'address:$1' })"
}
show() { hl "hl.dsp.focus({ window = 'address:$1' })"; hl "hl.dsp.window.bring_to_top({ window = 'address:$1' })"; sleep 1; }
term() { show "$TERM_WIN"; }
web() { show "$WEB_WIN"; }
reload() { web; hl "hl.dsp.send_key_state({ mods = '', key = 'F5', state = 'down' })"; hl "hl.dsp.send_key_state({ mods = '', key = 'F5', state = 'up' })"; sleep 6; }
REC=""
cleanup() {
  [ -n "$REC" ] && kill -INT "$REC" 2>/dev/null && wait "$REC" 2>/dev/null
  tmux kill-session -t demo 2>/dev/null
  [ -n "${TERM_WIN:-}" ] && hl "hl.dsp.window.close({ window = 'address:$TERM_WIN' })"
  [ -n "${WEB_WIN:-}" ] && hl "hl.dsp.window.close({ window = 'address:$WEB_WIN' })"
  rm -rf "$DATA"
}
trap cleanup EXIT

command -v calinator >/dev/null || { echo "calinator is not installed"; exit 1; }
rm -rf "$DATA"
mkdir -p "$DATA/calinator"
calinator --db "$DB" calendars >/dev/null
sqlite3 "$DB" "INSERT OR REPLACE INTO meta VALUES ('welcomed','1')"

say "Opening the windows on workspace $WS."
hl "hl.dsp.focus({ workspace = '$WS' })"
tmux kill-session -t demo 2>/dev/null
tmux new-session -d -s demo -x 160 -y 45 "env XDG_DATA_HOME=$DATA calinator"
tmux set -t demo status off >/dev/null
ghostty --class=calinator.demo --font-size=12 -e tmux attach -t demo >/dev/null 2>&1 &
before=$(hyprctl clients -j | python3 -c "import json,sys; print(' '.join(c['address'] for c in json.load(sys.stdin) if c['class']=='chromium'))")
chromium --new-window "$DAY_URL" >/dev/null 2>&1 &
sleep 4
TERM_WIN=$(addr_of calinator.demo)
WEB_WIN=$(hyprctl clients -j | python3 -c "import json,sys; old=sys.argv[1].split(); print(next((c['address'] for c in json.load(sys.stdin) if c['class']=='chromium' and c['address'] not in old), ''))" "$before")
[ -n "$TERM_WIN" ] && [ -n "$WEB_WIN" ] || { echo "Could not find the windows (terminal: '$TERM_WIN', browser: '$WEB_WIN')."; exit 1; }
place "$TERM_WIN"; place "$WEB_WIN"
# The sign-in page opens in the browser window used last: ours.
web; term
sleep 2

if [ "${DRY:-}" != 1 ]; then
  say "Recording starts."
  mkdir -p "$(dirname "$OUT")"
  gpu-screen-recorder -w region -region "${W}x${H}+${X}+${Y}" -f 30 -o "$OUT" >/dev/null 2>&1 &
  REC=$!
  sleep 3

  say "1. Sign in with Google: click the address bar, wait 3 s, pick your account, Advanced → Go to Calinator, wait 3 s, Continue."
  keys @; keys a; keys Enter
  for _ in $(seq 300); do
    [ "$(sqlite3 "$DB" 'SELECT COUNT(*) FROM accounts' 2>/dev/null)" = 1 ] && break
    sleep 1
  done
  sleep 2
  term
  sleep 3
  keys Escape
  sleep 10
fi

# Show only the demo calendar (other calendars stay private).
sqlite3 "$DB" "UPDATE calendars SET json = json_set(json, '\$.enabled', json(CASE WHEN json_extract(json,'\$.name') = '$CAL' THEN 'true' ELSE 'false' END))"
keys s
sleep 6

say "2. Calendar list: show and hide the demo calendar."
IDX=$(sqlite3 "$DB" "SELECT COUNT(*) FROM calendars WHERE rowid < (SELECT rowid FROM calendars WHERE json_extract(json,'\$.name') = '$CAL')")
keys BTab
for _ in $(seq "$IDX"); do keys j; done
sleep 1; keys Space; sleep 1.5; keys Space; sleep 1.5; keys Escape

say "3. Views."
keys m; sleep 2.5; keys g; sleep 2.5; keys d; sleep 2.5; keys w; sleep 2

say "4. Create an event."
keys a; text "Demo meeting tomorrow 3pm $PICK"; sleep 2; keys Enter
sleep 8
reload
term

state

say "5. Edit it."
keys /; text "Demo meeting"; keys Enter; sleep 1.5
keys e; sleep 1.5
keys C-u; text "Demo meeting (moved)"
keys Down; keys Down; keys Down; keys C-u; text "4pm"
keys Down; keys Down; keys C-u; text "5pm"
sleep 1.5; keys Enter
sleep 8
reload
term

state

say "6. Delete it."
keys /; text "Demo meeting"; keys Enter; sleep 1.5
keys x; sleep 2; keys y
sleep 8
reload
term

state

if [ "${DRY:-}" != 1 ]; then
  say "7. Answer the invitation."
  keys i; sleep 3; keys y; sleep 3; keys Escape
  sleep 8

  say "8. Remove the account."
  keys @; sleep 2; keys x; sleep 1.5; keys x; sleep 3; keys Escape; sleep 2
  kill -INT "$REC"; wait "$REC" 2>/dev/null; REC=""
  say "Done. The video: $OUT"
else
  say "Dry run done (nothing recorded)."
fi
hl "hl.dsp.focus({ workspace = '2' })"
