You are building Calinator, a modern-looking terminal (TUI) calendar for Linux/Omarchy with easy two-way sync.
Work in ~/Projects/Calinator. Read SPEC.md and RESEARCH.md fully first, and look at style-reference-musinator.svg. Follow the spec. Ask me if anything is unclear; do not guess.

v1 = Google + CalDAV + iCloud. Outlook (Microsoft Graph) is v1.1. Design a Provider interface from day one so Outlook and others plug in without rewrites. Do not build anything in "Not in v1".

Look and feel (borrow ideas, do NOT copy code) from Allison's Tide apps:
https://github.com/allisonhere/tide , https://github.com/allisonhere/tidemail , https://github.com/allisonhere/tideui
Bordered panes, accent border on focus, key hints in pane titles, quiet status bar, floating dialogs with shadow, live theme picker, match-omarchy that repaints live, every setting in the app, every key remappable.

Hard rules:
- Never lose an edit: offline queue, safe conflict handling, keep a copy on conflict.
- Repeating events: this / this and following / all — correct on Google and CalDAV.
- Timezones and DST correct.
- Never email guests by surprise: ask before sending updates.
- Tokens and passwords in the system keyring. No server of ours touches calendar data.
- Google: our own OAuth app (I will create it; tell me when you need the client ID). Ask for the narrowest scope that works.
- UI never freezes during sync.
- Check every library's license; tell me about any GPL/AGPL.
- Simple English in all user text and docs. Keep it short.

Build order:
1. Plan: confirm the stack (spec proposes Go + Bubble Tea). Then do a short test of the sync backend choice in SPEC section 4: our own sync vs caldir. Test both on a real Google calendar and a local CalDAV server (Radicale in Docker) with repeating events and timezones. Show me the results and your pick. Wait for my OK.
2. Test setup: local CalDAV server, fake provider for UI tests, a test suite of tricky repeating events and timezone/DST cases.
3. Sync engine + local store + offline queue.
4. Screens: week (default), day, month, agenda, left pane with calendars, details, event form, quick add.
5. Google sign-in (one click, unverified warning OK for now), CalDAV and guided iCloud setup in an account wizard.
6. Repeating-event edits (3 scopes), timezones, invites + RSVP tray.
7. Reminders + background daemon (systemd user service).
8. Themes + match-omarchy + contrast test, settings, key remapping, help.
9. --bar, CLI JSON (agenda, events list/create, search), .ics/webcal handler, doctor.
10. Packaging: AUR (calinator-bin) + install script, CI with tests. README with install, keys, screenshots.

Done means every item in section 12 of SPEC.md works. Show me each one working, with real screenshots in at least two themes. Use a test Google account, not my main one.

Your own tasks (accounts, Google/Microsoft approval) are in STEVE-TODO.md. When you need the Google or Microsoft client ID, stop and ask me.
Also read SPEC section 4b: build sync, accounts, store and daemon as a separate Go module that a second app (a todo app) will reuse. The data model must support tasks (CalDAV VTODO, Google Tasks, Microsoft To Do) from day one, even though the Tasks view comes in v1.1.
