# Getting Calinator verified by Google

Until Google verifies Calinator, only test users can sign in, they see an "unverified app" page, and they must sign in again every 7 days. Verification is free and takes about 3–5 business days. `calendar.events` is "sensitive" (the console lists `calendar.calendarlist.readonly` as non-sensitive), not "restricted": no security audit is needed.

Files here:
- `privacy-policy.md`: the privacy policy page.
- `homepage.md`: the Calinator page (must link to the privacy policy).
- `scopes.md`: texts for the "why do you need these scopes" fields.
- `demo-video.md`: what to record.
- `../logo/calinator-120.png`: the app logo (120×120).

## Steps

1. **Website (done, live):**
   - Calinator page: https://stevinator.com/apps/calinator/
   - Privacy policy: https://stevinator.com/privacy/#calinator
2. **Verify the domain.** At [Search Console](https://search.google.com/search-console), add `stevinator.com` with the **same Google account that owns the Cloud project**.
3. **Branding** ([Google Auth Platform → Branding](https://console.cloud.google.com/auth/branding)):
   - App name: `Calinator`
   - User support email: `info@stevinator.com`
   - App logo: `docs/logo/calinator-120.png`
   - App home page: `https://stevinator.com/apps/calinator/`
   - Privacy policy: `https://stevinator.com/privacy/`
   - Authorized domains: `stevinator.com`
   - Developer contact email: `info@stevinator.com`
4. **Data access** ([Data access](https://console.cloud.google.com/auth/scopes)): the scopes must be exactly
   - `.../auth/calendar.events`
   - `.../auth/calendar.calendarlist.readonly`
   - `openid`, `.../auth/userinfo.email`
5. **Demo video:** https://www.youtube.com/watch?v=NaRhWAYDN7E (unlisted, recorded Nov 16 2026).
6. **Publish** ([Audience](https://console.cloud.google.com/auth/audience)): click **Publish app** (Testing → In production). Google then asks you to **prepare for verification**: paste the scope texts from `scopes.md` and the video link, and submit.
7. **Wait** for Google's email. They may ask questions, usually about the video or the privacy policy; answer by email. Tell me what they ask and I'll help.

After approval: no more warning page, no 100-user limit, no re-login every 7 days. Then we make the repository public.
