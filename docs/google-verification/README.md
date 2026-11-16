# Getting Calinator verified by Google

Until Google verifies Calinator, only test users can sign in, they see an "unverified app" page, and they must sign in again every 7 days. Verification is free and takes about 3–5 business days. Calendar scopes are "sensitive", not "restricted": no security audit is needed.

Files here:
- `privacy-policy.md`: the privacy policy page.
- `homepage.md`: the Calinator page (must link to the privacy policy).
- `scopes.md`: texts for the "why do you need these scopes" fields.
- `demo-video.md`: what to record.
- `../logo/calinator-120.png`: the app logo (120×120).

## Steps

1. **Website.** Put two pages on stevinator.com, for example:
   - `https://stevinator.com/calinator` from `homepage.md`
   - `https://stevinator.com/calinator/privacy` from `privacy-policy.md`

   Replace `info@stevinator.com` and `PRIVACY_POLICY_URL` in both. Add a screenshot (`docs/screenshots/week-catppuccin.png`). The homepage must be public, not behind a login, and must name the app "Calinator" like the consent screen.
2. **Verify the domain.** At [Search Console](https://search.google.com/search-console), add `stevinator.com` with the **same Google account that owns the Cloud project**.
3. **Branding** ([Google Auth Platform → Branding](https://console.cloud.google.com/auth/branding)):
   - App name: `Calinator`
   - User support email: your support email
   - App logo: `docs/logo/calinator-120.png`
   - App home page: `https://stevinator.com/calinator`
   - Privacy policy: `https://stevinator.com/calinator/privacy`
   - Authorized domains: `stevinator.com`
   - Developer contact email: your email
4. **Data access** ([Data access](https://console.cloud.google.com/auth/scopes)): the scopes must be exactly
   - `.../auth/calendar.events`
   - `.../auth/calendar.calendarlist.readonly`
   - `openid`, `.../auth/userinfo.email`
5. **Demo video.** Record it as in `demo-video.md`, upload to YouTube as **Unlisted**.
6. **Publish** ([Audience](https://console.cloud.google.com/auth/audience)): click **Publish app** (Testing → In production). Google then asks you to **prepare for verification**: paste the scope texts from `scopes.md` and the video link, and submit.
7. **Wait** for Google's email. They may ask questions, usually about the video or the privacy policy; answer by email. Tell me what they ask and I'll help.

After approval: no more warning page, no 100-user limit, no re-login every 7 days. Then we make the repository public.
