# Calinator privacy policy

Last updated: November 3, 2026

Calinator is a calendar app for the terminal, made by Stepan Manookian. It runs on your own computer. This page explains what it does with your data.

## The short version

- Your calendar data stays on your computer and with your calendar service (Google, iCloud, or your CalDAV server).
- Calinator has no servers. We never receive, see, store, sell or share your calendar data.
- Your sign-in (Google) and passwords (iCloud, CalDAV) are kept in your computer's system keyring.
- There are no ads, no tracking and no analytics.

## Google user data

When you connect a Google account, Calinator asks Google for these permissions:

| Permission | What Calinator does with it |
|---|---|
| See, edit, create and delete your calendar events (`calendar.events`) | Shows your events, and saves the events you create, change, move or delete in Calinator back to Google Calendar. Also sends your answers to invitations. |
| See the list of your calendars (`calendar.calendarlist.readonly`) | Shows your calendars by name and color, so you can pick which ones to see and where new events go. Calinator cannot create, change or delete calendars. |
| Your email address (`email`, `openid`) | Shows which Google account is connected, and finds your own answer in event invitations. |

**How it is used:** only to show your calendars in Calinator and to sync the changes you make. Calinator sends data only between your computer and Google. Calinator never sends Google user data to us or to anyone else.

**Where it is stored:** a copy of your events is saved in a file on your computer (`~/.local/share/calinator/calinator.db`), so the app works offline. Your Google sign-in token is saved in your system keyring (for example GNOME Keyring or KeePassXC), not in that file.

**Sharing:** we do not share, sell or transfer Google user data. We do not use it for ads. No person reads it. It is not used to train AI or machine learning models.

**Guests:** Calinator only asks Google to email the guests of an event when you choose "Send" in Calinator.

Calinator's use and transfer of information received from Google APIs to any other app will adhere to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including the Limited Use requirements.

## iCloud and CalDAV

For iCloud and other CalDAV servers, Calinator connects straight to that server with the user name and password you enter. The password is kept in your system keyring. Your events are copied to the same file on your computer as above. Nothing goes to us.

## Removing your data

- **In Calinator:** press `@`, pick the account and press `x`. This deletes the account, its copied events and its saved sign-in from your computer. Nothing is deleted on Google, iCloud or your server.
- **Everything:** delete the folder `~/.local/share/calinator`, and remove the `calinator` entries from your system keyring.
- **Google access:** you can remove Calinator's access to your Google account at any time at [myaccount.google.com/permissions](https://myaccount.google.com/permissions).

## Children

Calinator is not made for children under 13.

## Changes

If this policy changes, the new version will be on this page with a new date.

## Contact

Questions: info@stevinator.com
