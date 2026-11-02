# Research: terminal calendar (Calinator)

Date: 2026-10-28. Two research agents. Linked from `../IDEAS.md`. Reddit blocked.

## 1. Short answer
- Room exists, but smaller than it looks. **chroncal** (230 ★, Go, created 2026-03, 2.5k commits) already does a lot.
- Nobody does all of: nice look + two-way sync to **Google, Outlook/M365, iCloud, CalDAV** + **no "make your own Google Cloud project"** + live Omarchy theme.
- Google sign-in for calendar is cheap: calendar is a "sensitive" scope, not "restricted". Review 3-5 business days, **no paid audit** (unlike Gmail).

## 2. Existing terminal calendars
| App | ★ | Sync | Weak spot |
|-----|---|------|-----------|
| calcure https://github.com/anufrievroman/calcure | 2,380 | read-only `.ics` | can't edit cloud events; maintainer refused CalDAV (#13) |
| khal + vdirsyncer https://github.com/pimutils/khal | 3,085 + 1,899 | two-way CalDAV via vdirsyncer | hard setup; Google OAuth broke (#975); vdirsyncer maintainer tired (#946, 32 reactions); recurring edits buggy |
| calcurse https://github.com/lfos/calcurse | - | "alpha" CalDAV script, one calendar only | multiple calendars #68 (51 👍, open since 2017) |
| gcalcli https://github.com/insanum/gcalcli | 3,778 | Google only (CLI, not TUI) | "Google blocked this app" #497 (57 👍), #580 (53 👍); users must make own Google project |
| chroncal https://github.com/DouglasdeMoura/chroncal | 230 | two-way CalDAV, Google, iCloud | **no Outlook**; own Google Cloud project needed; sync manual (`s`) or separate service; follows 16 ANSI colors only; Omarchy bar plugin |
| aion https://github.com/semos-labs/aion | 27 | Google + CalDAV | TS/Bun, quiet since 2026-05 |
| vimalender https://github.com/Sadoaz/vimalender | 147 | none (.ics import) | nice vim UI |
| lazycal https://github.com/pranavkarthik10/lazycal | 44 | Google, view only | |
| 365cal-tui https://github.com/chimbida/365cal-tui | 4 | Office 365 viewer | only M365 TUI found |
| caldir https://github.com/t4t5/caldir | 79 | two-way Google, iCloud, Outlook, CalDAV (CLI sync layer, .ics files) | not a UI; renCal uses it. **Could be a backend option.** |

Desktop (context): renCal (Google, iCloud, Outlook, CalDAV), OmaCal (no Outlook), crmne/omarchy-calendar. ~35 Omarchy calendar plugins, mostly read-only bar widgets (max 44 ★).

## 3. What people complain about
1. **Google login breaks** (gcalcli 57 + 53 👍, vdirsyncer 15-28 comments per issue). Every tool makes users create their own Google Cloud project.
2. **Recurring events** ("this and future", deleting one instance): khal #524 open since 2016, #793 crash.
3. **Many calendars / accounts:** calcurse #68 (51 👍).
4. **Setup = glue of 2-3 tools** (khal + vdirsyncer + cron).
5. **Timezones** (khal #1092, 18 👍).
6. **Alarms/reminders** (khal #148, 52 comments).
7. **Look:** calcure won on looks alone (HN 211 pts).

## 4. Provider rules
| Provider | Way in | Review | Cost |
|----------|--------|--------|------|
| Google | Calendar API (or CalDAV, OAuth only) | sensitive scope: domain, privacy policy, demo video; 3-5 business days. No CASA. Testing mode: 100 users, re-login every 7 days. | free (needs domain + homepage) |
| Microsoft 365 / Outlook.com | Graph (no CalDAV). `Calendars.ReadWrite` needs no admin; works for personal accounts | publisher verification (free, Partner Center) else work tenants may block | free |
| iCloud | CalDAV + app-specific password | none | free |
| Fastmail, Nextcloud, etc. | CalDAV | none | free |
Sources: https://developers.google.com/identity/protocols/oauth2/production-readiness/sensitive-scope-verification , https://support.google.com/cloud/answer/13464321 , https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview
Google has no tasks (VTODO) over CalDAV; Calendar API quota tiers changed 2026-05-01.

## 5. Demand
- HN: calcure 211 pts (2022); new TUIs 2-6 pts. Small but steady.
- Omarchy: 50 calendar issues/PRs (mostly the clock popup). PR #3021 calendar tooltip 24 reactions. ~35 calendar plugins. Omarchy may ship its own local calendar (#10215, open) and defaults to HEY.

## 6. Gaps nobody fills
1. One TUI with two-way Google + Outlook + iCloud + CalDAV.
2. Google sign-in with our own verified app (no Cloud project). Cheap moat: days, not weeks, no audit.
3. Outlook/M365 read-write in a terminal.
4. Live match-omarchy theme in a full TUI.
5. Background sync built in.
6. Invites/RSVP in the terminal, with "email guests only on purpose".
7. Recurring edits done right (this / this and future / all).

## 7. Idea
- Calinator: modern TUI calendar, same family as Musinator/Chatinator.
- v1: Google (verified app) + CalDAV/iCloud. Outlook (Graph) right after. Month/week/day/agenda, recurring edits with 3 scopes, reminders, invites/RSVP, natural-language quick add, background sync, match-omarchy, bar widget, CLI/JSON for agents.
- Backend option: build sync ourselves (Go) or use caldir. Decide in planning.
- Risks: chroncal moves fast; Omarchy may add its own calendar; recurrence/timezone bugs are hard; Google may ask for a narrower scope.

## Rejected
- Read-only bar widget → ~35 Omarchy plugins.
- Google-only CLI → gcalcli.
