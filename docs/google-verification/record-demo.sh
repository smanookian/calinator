#!/bin/bash
# Records the Google verification demo video. Runs Calinator in a separate,
# empty data folder (your real calendar file is not used), types every key
# itself, and opens Google Calendar in the browser to show each change.
# You only sign in to Google when the browser asks.
#
# Before: see "Before recording" in demo-video.md.
# Run:    bash docs/google-verification/record-demo.sh
set -euo pipefail

DATA=/tmp/calinator-demo
DB=$DATA/calinator/calinator.db
CAL="Calinator demo"
TOMORROW=$(date -d tomorrow +%Y/%m/%d)
DAY_URL="https://calendar.google.com/calendar/r/day/$TOMORROW"

say() { printf '\n>>> %s\n' "$*"; }
keys() { tmux send-keys -t demo "$@"; sleep 0.6; }
text() { tmux send-keys -t demo -l "$1"; sleep 0.8; }
term() { hyprctl -q dispatch focuswindow class:com.mitchellh.ghostty; sleep 1; }
browser() { xdg-open "$1" >/dev/null 2>&1; sleep 7; }
reload() { hyprctl -q dispatch focuswindow class:chromium; sleep 0.5; hyprctl -q dispatch sendshortcut ", F5, class:chromium"; sleep 6; }

command -v calinator >/dev/null || { echo "calinator is not installed"; exit 1; }
rm -rf "$DATA"
mkdir -p "$DATA/calinator"
calinator --db "$DB" calendars >/dev/null
sqlite3 "$DB" "INSERT OR REPLACE INTO meta VALUES ('welcomed','1')"

say "Starting the screen recording (whole screen)."
omarchy-capture-screenrecording --fullscreen &
sleep 3

tmux kill-session -t demo 2>/dev/null || true
tmux new-session -d -s demo -x 200 -y 50 "env XDG_DATA_HOME=$DATA calinator"
ghostty --title=Calinator -e tmux attach -t demo &
sleep 2
term
hyprctl -q dispatch fullscreen 1
sleep 4

say "1. Sign in with Google. In the browser: click the address bar, wait 3 s, pick your account, wait 3 s on the permission screen, click Continue."
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

# Show only the demo calendar (other calendars stay private).
sqlite3 "$DB" "UPDATE calendars SET json = json_set(json, '\$.enabled', json(CASE WHEN json_extract(json,'\$.name') = '$CAL' THEN 'true' ELSE 'false' END))"
keys s
sleep 6

say "2. Calendar list: show and hide the demo calendar."
# Its place in the list (calendars are listed in the order they were added).
IDX=$(sqlite3 "$DB" "SELECT COUNT(*) FROM calendars WHERE rowid < (SELECT rowid FROM calendars WHERE json_extract(json,'\$.name') = '$CAL')")
keys Tab
for _ in $(seq "$IDX"); do keys j; done
sleep 1; keys Space; sleep 1.5; keys Space; sleep 1.5; keys Escape

say "3. Views."
keys m; sleep 2.5; keys g; sleep 2.5; keys d; sleep 2.5; keys w; sleep 2

say "4. Create an event."
keys a; text "Demo meeting tomorrow 3pm @demo"; sleep 2; keys Enter
sleep 8
browser "$DAY_URL"
term

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

say "6. Delete it."
keys /; text "Demo meeting"; keys Enter; sleep 1.5
keys x; sleep 2; keys y
sleep 8
reload
term

say "7. Answer the invitation."
keys i; sleep 3; keys y; sleep 3; keys Escape
sleep 8

say "8. Remove the account."
keys @; sleep 2; keys x; sleep 1.5; keys x; sleep 3; keys Escape; sleep 2

omarchy-capture-screenrecording --stop-recording || true
tmux kill-session -t demo 2>/dev/null || true
rm -rf "$DATA"
say "Done. The video is in ~/Videos. Tell me its file name."
