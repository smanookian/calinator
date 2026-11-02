# Calinator: tasks for Steve

Things only you can do (accounts, money, legal, testing). The AI builder cannot do these.
Do them **before or while** the app is being built. Tick them off here.

## Before the build starts
- [ ] Make a **test Google account** (not your main one). Add a few calendars and repeating events.
- [ ] Optional: a **test iCloud** account or use your own with a separate test calendar.
- [ ] Optional: a **Fastmail trial** (real CalDAV server to test against).

## Google sign-in (start early: approval takes ~3-5 business days after you submit)
1. [ ] Create a Google Cloud project "Calinator" with your stevinator.com Google account: https://console.cloud.google.com
2. [ ] Enable the **Google Calendar API** and the **Google Tasks API** (tasks are shared with the future todo app). Add the CalDAV API if the builder asks. Ask for both scopes in one verification so you only go through Google review once.
3. [ ] OAuth consent screen: app name "Calinator", logo, support email, app type **External**.
4. [ ] Create an OAuth client of type **Desktop app**. Give the client ID to the builder when it asks (step 5 of its build order).
5. [ ] Add your test account as a **test user** (works before approval; logins expire every 7 days, that's normal).
6. [ ] Put a **Calinator page** on stevinator.com/apps: what it does, link to the privacy policy.
7. [ ] Write a **privacy policy** on stevinator.com: calendar data stays on your computer, nothing goes to our servers, tokens in your system keyring, how to remove access. Link it from the Calinator page.
8. [ ] Verify **stevinator.com** in Google Search Console with the same Google account: https://search.google.com/search-console
9. [ ] Record the **demo video** (YouTube, unlisted) once sign-in + create/edit event work: show the Google sign-in screen with the scopes, then the app using the calendar.
10. [ ] Write the **scope reason**: "Calinator shows and edits the user's calendar events in the terminal and syncs changes back to Google Calendar."
11. [ ] Submit for **verification**. No paid security audit is needed for calendar scopes.

## Microsoft / Outlook (needed before v1.1, free)
1. [ ] Join the **Microsoft AI Cloud Partner Program** with your business details (free): https://partner.microsoft.com/partnership
2. [ ] Wait for the partner account to be **verified**; note your **Partner One ID**.
3. [ ] Register the **Calinator app in Microsoft Entra** (multitenant + personal accounts), with permissions `Calendars.ReadWrite`, `Tasks.ReadWrite` (for Todonator / Tasks view) and `offline_access`. Give the client ID to the builder.
4. [ ] Do **publisher verification** with your Partner One ID: https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview
5. [ ] A test Outlook.com account (and a work 365 account if you have one).

## While the build runs
- [ ] Answer the builder's plan check (stack + sync backend choice: own sync vs caldir).
- [ ] Test each "Done when" item yourself (SPEC section 12), in at least two Omarchy themes.
- [ ] Check app stores / domain for the name "Calinator" before announcing.

## Before release
- [ ] Google verification approved (no more "unverified app" screen).
- [ ] README screenshots + a short demo GIF/video for stevinator.com and YouTube.
- [ ] AUR account ready to publish `calinator-bin`.
- [ ] Decide: free forever, or paid extras later (SPEC section 13).
