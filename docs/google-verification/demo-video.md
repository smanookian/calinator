# Demo video script (about 3 minutes)

Google needs an unlisted YouTube video that shows the whole sign-in and how each scope is used. Record the full screen (Omarchy: `Super + Alt + Print`, or OBS). No voice needed; Google reads the screen. Use English in the app (it already is).

Before recording: use a test Google account with a few events, including one invitation from another address. Make the terminal font large enough to read.

## 1. The app and its name (15 s)
- Open a terminal, run `calinator`.
- Show the week view for a moment.

## 2. Sign-in with the OAuth consent screen (45 s) — the most important part
- Press `@`, then `a`, pick **Google**, press `enter`.
- The browser opens Google's sign-in. **Before signing in, click the address bar** so the full URL is visible: it must show `client_id=104932519162-…apps.googleusercontent.com`. Hold it 3 seconds.
- Pick the Google account.
- On the consent screen, the app name **Calinator** and both calendar permissions are visible. Hold it 3 seconds, then click **Continue**.
- Back in the terminal: "Added … Syncing your calendars…".

## 3. calendar.calendarlist.readonly (20 s)
- Left pane: the Google calendars appear with their names and colors.
- Press `tab`, move to a calendar, press `space` to hide it and again to show it.

## 4. calendar.events: read (20 s)
- Show the week view with Google events. Press `m` (month) and `g` (agenda), then `w` back.
- Open an event with `enter`: title, time, guests.

## 5. calendar.events: create, edit, delete — and it is in Google Calendar (60 s)
- Press `a`, type `Demo meeting tomorrow 3pm`, press `enter`.
- Switch to the browser, open Google Calendar: the event is there (reload if needed).
- Back in Calinator: select it (`/`, type `demo`, `enter`), press `e`, change the title to `Demo meeting (moved)` and the time to 4pm, press `enter`. Show it changed in Google Calendar.
- In Calinator press `x` to delete it. Show it is gone in Google Calendar.

## 6. Invitations (20 s)
- Press `i`: the invitation tray. Press `y` to answer yes.
- In Google Calendar, open the invitation: your answer is "Yes".

## 7. Removing access (10 s)
- In Calinator press `@`, pick the account, press `x` twice: "Removed … (nothing was deleted on the server)".

Upload to YouTube as **Unlisted**, and paste the link into the verification form.
