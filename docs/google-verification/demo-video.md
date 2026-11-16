# Demo video

Google needs an unlisted YouTube video that shows the sign-in with the permission screen, and how each permission is used. `record-demo.sh` records it: it types every key in Calinator and opens Google Calendar to show each change. You only sign in. No voice is needed.

It uses a separate, empty data folder, shows only the "Calinator demo" calendar, and removes the account at the end. Your real calendar file is not touched.

What the video shows, in order:
1. Sign-in: the browser address with Calinator's client ID, the permission screen, Continue.
2. `calendar.calendarlist.readonly`: the calendar list; hiding and showing a calendar.
3. `calendar.events` (read): month, agenda, day and week views.
4. `calendar.events` (write): an event is created, edited and deleted in Calinator; Google Calendar shows each change.
5. An invitation is answered with "yes".
6. The account is removed.

## Before recording (once)

1. **English:** set your Google account language to English at https://myaccount.google.com/language, and Chromium to English if it isn't.
2. **Demo calendar:** at https://calendar.google.com create a calendar named exactly `Calinator demo`.
3. **Only that calendar in the browser:** in Google Calendar's left sidebar, untick every calendar except `Calinator demo` (tick them again after recording).
4. **An invitation:** from jclstefan@yahoo.de, invite stepan.manookian@gmail.com to an event for **tomorrow at 6pm** named `Demo invite`. Don't answer it.
5. **Quiet screen:** close other windows and turn on Do Not Disturb, so no notifications appear.
6. Be signed in to Google in Chromium.

## Recording

```bash
cd ~/Projects/Calinator
bash docs/google-verification/record-demo.sh
```

Your screen switches to workspace 9. A Calinator window and a new browser window open there; only that area is recorded. After a few seconds the browser shows Google's sign-in. Then:
1. **Click the browser's address bar** and wait 3 seconds (the reviewer must see `client_id=104932519162-…`).
2. Pick your Google account.
3. "Google hasn't verified this app": click **Advanced → Go to Calinator (unsafe)**.
4. On the permission screen: wait 3 seconds, then click **Continue**.
5. **Don't touch anything after that.** The script switches between Calinator and the browser by itself (about 2 minutes) and stops the recording.

At the end you're back on workspace 2. The video is saved in `~/Videos` as `calinator-demo-….mp4`. Upload it to YouTube as **Unlisted** and paste the link into the verification form.
