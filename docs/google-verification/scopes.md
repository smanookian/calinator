# Scope justifications (for the Google verification form)

Copy each text into the "How will the scopes be used?" fields.

## https://www.googleapis.com/auth/calendar.events (sensitive)

Calinator is a calendar app for the Linux terminal. It shows the user's Google Calendar events in day, week, month and agenda views, and saves events the user creates, edits, moves or deletes in Calinator back to Google Calendar (two-way sync). It also sends the user's answer (yes, maybe, no) to event invitations. Read access alone is not enough, because the core feature is editing events and syncing them back. All data stays on the user's computer and goes only between the computer and Google; Calinator has no servers.

## https://www.googleapis.com/auth/calendar.calendarlist.readonly (non-sensitive in the console)

Calinator shows the list of the user's calendars with their names and colors, so the user can show or hide each calendar and pick which calendar new events go to. It only reads the list; it never creates, changes or deletes calendars. We picked this read-only scope instead of the full `calendar` scope because Calinator does not need to manage calendars.

## openid, email (not sensitive)

Used to show which Google account is connected, and to find the user's own answer in event invitations (the attendee whose email is theirs).

## Why not a narrower scope?

- `calendar.readonly` / `calendar.events.readonly`: Calinator must save the user's edits back to Google Calendar.
- `calendar.events.owned`: the user also needs to answer invitations to events organized by other people, which this scope does not allow.
- `calendar` (full): not needed; Calinator does not create or change calendars or their sharing settings.
