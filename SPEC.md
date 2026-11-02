# Calinator

A modern terminal calendar with easy two-way sync. Linux/Omarchy first.
Status: planned (2026-10-28). Research: `RESEARCH.md`. Idea list: `~/Projects/ideas/IDEAS.md`.

## 1. What it is
- TUI calendar. Keyboard first (vim keys), mouse optional.
- Two-way sync with your online calendars, set up in one minute.
- Same look family as Musinator and Chatinator.
- Free, MIT (paid extras: decide later).

## 2. Why it can win (from research)
- No terminal calendar has two-way sync for Google + Outlook + iCloud + CalDAV in one app.
- Every terminal tool makes you create your own Google Cloud project. Google login breaking is the top complaint (gcalcli #497: 57 👍, #580: 53 👍).
- Outlook/M365 in the terminal: almost nothing (one 4 ★ viewer).
- Repeating-event edits are buggy everywhere (khal #524 open since 2016).
- Closest rival: chroncal (230 ★): no Outlook, own Google project, manual sync, only ANSI colors.

## 3. Accounts
Order:
1. **v1:** Google (our own verified app, one click), any CalDAV (Fastmail, Nextcloud, Radicale), iCloud (app-specific password, guided).
2. **v1.1:** Outlook / Microsoft 365 (Microsoft Graph, our own Entra app).
3. Later: HEY (if an API allows), read-only `.ics` / webcal links (easy, can be v1 if cheap).
- Several accounts and many calendars at once. Each calendar: on/off, color, default for new events.
- Design a `Provider` interface from day one.

### Sign-in rules
- **Google:** our own Google Cloud app. Calendar is a *sensitive* scope (not restricted): needs brand verification, privacy policy on stevinator.com, demo video, scope reason; review ~3-5 business days; **no CASA audit**. Until verified: up to 100 test users, they re-login every 7 days. Ask for the narrowest scope that works (`calendar.events` + `calendar.calendarlist.readonly` if enough; else `calendar`).
- **Microsoft:** multitenant + personal accounts, `Calendars.ReadWrite` + `offline_access` (no admin needed). Publisher verification (free, Microsoft partner program) so work accounts don't block it.
- **iCloud:** guided steps to make an app-specific password, link to Apple's page.
- Tokens and passwords in the system keyring. Calendar data never goes to any server of ours.

## 4. Sync
- **Two-way.** Create, edit, delete, move between calendars; changes made on the phone show up here.
- **Background sync built in** (while the app runs) + optional user service `calinator sync --daemon` so the bar widget stays fresh when the app is closed.
- **Offline:** changes wait in a queue and sync later. Never lose an edit.
- **Conflicts:** server wins by default, but show the user what changed and keep a copy.
- **Backend choice (decide in planning, with a short test):**
  - A. Our own sync (Go): CalDAV (sync-collection, RFC 6578), Google Calendar API (sync tokens), Graph (delta queries).
  - B. Use **caldir** (https://github.com/t4t5/caldir, MIT, Rust CLI, already does Google/iCloud/Outlook/CalDAV two-way, one `.ics` per event; renCal uses it).
  - Pick based on: reliability, recurrence and timezone handling, can it use OUR OAuth app, one-binary install, license.

## 4b. Tasks and the shared sync core (decided 2026-10-31)
Steve decided: **both** a Tasks view inside Calinator **and** a separate todo app (name TBD, research in `~/Projects/ideas/research/todo.md`).
- **Changed 2026-11-01 (Steve):** Calinator and Todonator stay **separate apps**. No shared core module; each app has its own accounts, keyring entries and daemon. The rest of this section no longer applies.
- The data model must support tasks from day one: CalDAV VTODO, Google Tasks API, Microsoft To Do (Graph), with subtasks, due dates, recurrence, reminders, priority. (Todoist later, in the todo app.)
- v1: core supports tasks; Calinator shows due tasks on the day/week grid only if cheap. v1.1: full Tasks view in Calinator (list + due on calendar).
- Note: Google CalDAV has no VTODO (use Google Tasks API); iCloud Reminders has no CalDAV since iOS 13.

## 5. Look (same family)
Borrow ideas (not code) from Allison's Tide apps: https://github.com/allisonhere/tide , https://github.com/allisonhere/tidemail , https://github.com/allisonhere/tideui
Style reference: `style-reference-musinator.svg`.
- Bordered panes, accent border on focus, selected item always visible.
- Key hints in pane titles, quiet status bar.
- Floating dialogs with soft shadow, live theme picker, many themes + **match-omarchy** (reads Omarchy theme files, repaints live on theme change, not just ANSI colors).
- Contrast test for every theme. Event colors from the calendar, adjusted to stay readable.
- Every setting in the app; every key remappable.

## 6. Screens
- **Left pane:** mini month, list of accounts and calendars (toggle with `space`), "Upcoming" list.
- **Main pane:** views: **Day, Week (default), Month, Agenda.** Week/day = time grid with event blocks, a "now" line, all-day row on top. Overlapping events side by side.
- **Details pane (or popup):** title, time + timezone, calendar, location, video link (`J` joins), guests with their answers, description, reminders.
- **Quick add:** `a` opens one line: "lunch with Sarah tomorrow 1pm" → preview → `enter`.
- **Event form:** full edit, all fields.
- **Invites tray:** new invites stay visible until answered (Yes / Maybe / No).
- Status bar: sync state per account (● synced / ↻ / ⚠), next event countdown.

## 7. Must get right
- **Repeating events:** edit/delete with 3 choices: *this event / this and following / all events*. Support RRULE, RDATE, EXDATE, overrides.
- **Timezones:** show in local time; events keep their own zone; a second zone column option; DST correct.
- **Invites:** RSVP from the app. When editing an event with guests, ask "Send update to guests?" (never email guests by surprise).
- **Reminders:** desktop notifications from event alarms, even when the app is closed (via the daemon).

## 8. Keys (defaults, remappable)
| Key | Action |
|-----|--------|
| `h/j/k/l` | move (day/time) |
| `d` `w` `m` `g` | day / week / month / agenda view |
| `t` | today |
| `[` `]` | previous / next period |
| `a` | quick add · `A` full form |
| `enter` | open event · `e` edit · `x` delete |
| `y` `n` `?`… | RSVP yes / no / maybe (in invites) |
| `J` | join video call |
| `/` | search events |
| `space` | toggle calendar (left pane) |
| `s` | sync now |
| `T` · `S` · `?` | themes · settings · help |

## 9. Desktop integration
- `calinator --bar`: next event + countdown JSON for the Omarchy bar / Waybar; settings toggle "Show in Omarchy bar".
- Headless CLI with JSON for scripts and agents: `calinator agenda --json`, `events list --from --to`, `events create`, `search`.
- `.ics` file / `webcal:` link handler (import or subscribe).
- `calinator doctor`: checks accounts, keyring, daemon, theme.

## 10. Not in v1
Outlook (v1.1). Tasks view (v1.1, see section 4b). Scheduling with other people's free/busy. Booking pages. Mobile. AI features. Calendar sharing settings.

## 11. Tech (proposal, builder can push back)
- **Decided 2026-11-01 (Steve):** Rust + ratatui, like Todonator. Code and patterns are copied from Todonator, not shared.
- Local store: SQLite (one file, shared by app, CLI and daemon).
- One binary. AUR (`calinator-bin`) + install script.
- Build order changed: the calendar works fully on this computer first; sync (CalDAV, iCloud, Google) comes after.

## 12. Done when (v1)
1. Install from AUR or script; first run opens the account wizard.
2. Add Google with one click (unverified-app warning OK in beta), CalDAV (Fastmail/Nextcloud test server) and iCloud with guided password.
3. Week, day, month, agenda views match the look; now-line and overlaps correct.
4. Create / edit / delete / move events here → appear on Google web and phone within 1 minute; and the other way.
5. Repeating events: all 3 edit scopes work on Google and CalDAV; a test suite of tricky RRULEs passes.
6. Timezones: an event made in New York shows right in Dubai; DST week correct.
7. Offline edit → syncs later, nothing lost. Conflict shows a clear message.
8. Invites: RSVP works; editing an event with guests asks before notifying.
9. Reminders notify with the app closed (daemon on).
10. Quick add understands common phrases (tomorrow, fri, next mon 9:30, 3pm-4pm, "every monday").
11. match-omarchy repaints live; contrast test passes; every key remappable; every setting in the app.
12. `--bar`, CLI JSON and `doctor` work.

## 13. Open questions
- Name: "Calinator" chosen by Steve 2026-10-28. AUR: free. GitHub: 1 repo (profile, 0 ★). App stores not checked.
- Own sync or caldir (section 4).
- Steve to do in parallel: Google brand + sensitive-scope verification (stevinator.com privacy page, demo video), Microsoft partner program enrollment.
- Paid later? (e.g. Outlook / many accounts.)
