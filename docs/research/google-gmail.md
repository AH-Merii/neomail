# Research: google-gmail

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

Note: the relayed user message ("closed chromium by mistake") had no effect on this research. Everything was done with WebSearch/WebFetch plus read-only curl/openssl. No browser, no Gmail/Drive/Calendar/Docs tools.

## 1. Creating an OAuth client (brand-new personal account)
- **Requirements:** no billing account is needed for a project, an OAuth client or the Gmail API. Phone verification is only "sometimes" required, as an anti-abuse check at account creation. It is not a documented requirement for Cloud. What is required: since **2025-05-12**, personal accounts must turn on **2-Step Verification (2SV)** before they can use the Cloud console. 2SV can be enabled with only an authenticator app or security key, no phone (since May 2024). The gcloud CLI has no 2SV requirement, but there is **no API** for creating Desktop OAuth clients. The IAP OAuth Admin API shut down on 2026-03-19, so client creation is manual in the console.
- **Console flow:** Google Auth Platform → **Branding** (app name, support email, contact) → **Audience** (External; add test users) → **Data Access** (scopes) → **Clients** → *Desktop app*. Nothing else is needed to create the client.
- **Client secret:** since June 2025 it is shown/downloadable **only at creation** (hashed, last 4 characters shown afterwards). A client can have at most 2 secrets (rotation).
- **Unused clients:** auto-deleted after **6 months** inactive (policy dated 2025-10-27; 30-day restore window).
- **Redirect and PKCE:** loopback `http://127.0.0.1:<random port>` or `[::1]`. Desktop clients don't register redirect URIs. Loopback is deprecated only for iOS/Android/Chrome clients. The OOB/copy-paste flow has been fully blocked since 2023-01-31. PKCE uses S256. Embedded webviews are forbidden by policy. Incremental authorization isn't supported for installed apps, and users can uncheck individual scopes, so read `scope` from the token response.
- **The device flow can't be used:** it does not allow any Gmail scope. For headless/SSH use, fall back to loopback plus port-forwarding or pasting the redirect URL.
- **Client secret is still needed in practice:** Google calls it "obviously not treated as a secret" and the doc table marks it *Optional*. But a 2026-09-22 report still got `400 client_secret is missing` on Desktop clients. Send it.
- **Testing vs production:** in **Testing**, up to 100 test users, and authorizations/refresh tokens **expire 7 days after consent**. Moving to **In production** without verification removes the 7-day expiry. Users then see the unverified-app screen, with a 100-user lifetime cap. Since about Aug 2026 the Publish button requires a **homepage URL and privacy-policy URL** (plus an authorized domain). This is new friction for bring-your-own-client (BYO) users.
- **Other ways a production refresh token dies:** 6 months unused, a password change (tokens with Gmail scopes), revocation, or more than 100 live tokens per account per client (the oldest is silently revoked).

## 2. Scope classes and verification
- Non-sensitive: `gmail.labels`
- Sensitive: `gmail.send`
- Restricted: `mail.google.com`, `gmail.modify`, `gmail.readonly`, `gmail.compose`, `gmail.metadata`, `gmail.insert`, `gmail.settings.*`
- `gmail.labels` can create labels but **can't apply** them. `messages.modify` needs `gmail.modify` or `mail.google.com`. `gmail.modify` also allows send. IMAP/SMTP require `mail.google.com`.

For production with restricted scopes:
- **Verification:** brand verification (about 2–3 days) + restricted-scope review (about 6 weeks), with a verified domain, homepage, privacy policy, demo video and Limited Use compliance. "Email clients" are an approved use case.
- **Security assessment (CASA):** required only if restricted data is stored on or passes through servers. It is annual, done by a lab at AL1/AL2 (self-scan no longer accepted), and community-reported costs are roughly $500–$4.5k.
- **Rejection:** unapproved scopes stop working. Exceptions: personal use (<100 known users), Testing, Internal (Workspace only).
- **Advanced Protection:** blocks unverified apps and app passwords.

## 3. IMAP+XOAUTH2 vs the Gmail API
- **IMAP** (live check today): `X-GM-EXT-1`, IDLE, SASL-IR, `AUTH=XOAUTH2`/`OAUTHBEARER`. SMTP offers XOAUTH2/OAUTHBEARER with SIZE 35,882,577 bytes.
  - Extensions: X-GM-MSGID/THRID are decimal forms of the Gmail API hex IDs; X-GM-LABELS supports FETCH/STORE/SEARCH; X-GM-RAW accepts Gmail search syntax.
  - Community reports: CONDSTORE yes, QRESYNC no.
  - Limits: 2,500 MB/day download, 500 MB upload, up to 15 clients, sessions about 24 h.
  - Does not use API quota. Requires the most powerful scope (`mail.google.com`).
- **Gmail API:**
  - Sync: full sync = `messages.list` (≤500/page) + batched `get`. Partial sync = `history.list` (404 → full resync; history kept at least about a week).
  - Batches: ≤100 calls per batch, ≤50 recommended; each call still counts.
  - Labels: `labels.create`. Apply with `messages.modify` (≤100 labels) or `batchModify` (≤1000 ids). Fixed color palette, 10k label max (Help Center says 5k; Google recommends ≤500).
  - Push via `watch` needs Pub/Sub, which needs billing, so poll `history.list` instead (2 units).
  - **Quotas (projects created after 2026-05-01):** 1.2M units/min/project, **6,000/min/user**. Costs: `messages.get` 20, `threads.get` 40, `history.list` 2, `send` 100. Daily threshold 80M units/project. Overage billing is "planned later in 2026" with 90 days' notice. Standard use is free. The new per-user limit means about 300 `messages.get` calls per minute.

## 4. App passwords
- They still exist (no official deprecation found). They require 2SV and can be created when 2SV uses an authenticator app.
- Unavailable for accounts whose only 2SV method is security keys, for Advanced Protection accounts, and for work/school accounts. Revoked on password change. Google says they're "not recommended."
- Plain-password ("less secure apps") access ended for personal accounts on 2022-05-30, and for Workspace on 2025-05-01.
- IMAP has been always-on since January 2025.
- An app password gives IMAP/SMTP only (no Gmail API) with full mailbox access.

## 5. What open-source clients do about client IDs
- **Ship their own ID/secret:** Thunderbird (source says "Don't copy these values"), GNOME Online Accounts and Evolution (build-time override options), lieer (shared ID; recommends your own).
- **Bring your own:** aerc, neomutt `mutt_oauth2.py`, himalaya v2 via ortie/oama/pizauth, email-oauth2-proxy, MishMail.
- **Dropped Gmail OAuth:** Delta Chat (2024, citing yearly CASA cost; uses app passwords).
- **Risks:**
  - The Google APIs ToS says "Developer credentials may not be embedded in open source projects", and forbids misrepresenting your client's identity.
  - Shared IDs can be blocked: KDE's Drive scope was blocked in 2024; Penguin Mail's built-in Gmail client was "This app is blocked" on 2026-10-08.
  - Shared projects pool quota (and future overage billing).
  - BYO hits the 7-day and publish friction, and the new-project quotas.

## 6. 2025–2026 changes (summary)
Cloud console 2SV required (May 2025); secrets shown only at creation (June 2025); unused-client deletion (Oct 2025); IAP OAuth API shut down (Mar 2026); Workspace less-secure-apps ended (May 2025); IMAP toggle removed (Jan 2025); quota/tiering change (May 2026, billing TBA); Gmail MCP server preview (Apr 2026); optional DPoP (2026 docs); publish now needs homepage/privacy URLs (about Aug 2026); Gmailify/POP fetch ends Jan 2027.

## Key facts

- **[high]** Desktop app OAuth clients use loopback redirect http://127.0.0.1:<random port> or http://[::1]:port (localhost allowed but may hit firewall issues); loopback deprecation applies only to iOS/Android/Chrome client types; Desktop clients remain supported. Page updated 2026-09-14. — <https://developers.google.com/identity/protocols/oauth2/native-app>
- **[high]** PKCE: code_verifier 43-128 chars; S256 recommended (plain also accepted). Refresh tokens are always returned for installed applications. Incremental authorization is not supported for installed apps. — <https://developers.google.com/identity/protocols/oauth2/native-app>
- **[high]** Google: installed apps get a client ID and sometimes a secret embedded in source; 'the client secret is obviously not treated as a secret.' The native-app doc marks client_secret 'Optional' for code exchange and refresh. — <https://developers.google.com/identity/protocols/oauth2>
- **[medium]** In practice Desktop-app clients still return '400 client_secret is missing' without the secret even with PKCE (report dated 2026-09-22), so send it. — <https://github.com/olalaurao/quartzo-obsidian-companion/pull/59>
- **[high]** OOB (copy/paste) flow fully blocked for all clients since 2023-01-31. — <https://developers.google.com/identity/protocols/oauth2/resources/oob-migration>
- **[high]** Device authorization flow (TVs/limited input) supports only openid/email/profile, drive.appdata, drive.file and YouTube scopes; no Gmail scopes. — <https://developers.google.com/identity/protocols/oauth2/limited-input-device>
- **[high]** Google Auth Platform setup: Branding (app name, support email, contact, agree to User Data Policy) -> Audience (Internal/External, test users) -> Data Access (scopes) -> Clients. Desktop clients need no extra info. — <https://developers.google.com/workspace/guides/configure-oauth-consent>
- **[high]** Client secrets are hashed: visible/downloadable only at creation (announced April 2025; new clients from June 2025; existing clients from Nov 2025); max two secrets per client for rotation. — <https://support.google.com/cloud/answer/15549257?hl=en>
- **[high]** OAuth clients inactive for 6+ months may be deleted (policy changelog 2025-10-27); 30-day email warning; restorable within 30 days. — <https://developers.google.com/identity/protocols/oauth2/policies>
- **[high]** External + Testing: max 100 test users; authorizations (incl. refresh tokens) expire 7 days after consent unless only openid/email/profile are requested. — <https://support.google.com/cloud/answer/15549945?hl=en>
- **[high]** Refresh tokens also stop working after 6 months unused, on password change if they contain Gmail scopes, on revocation, or when the account exceeds 100 refresh tokens per client (oldest silently invalidated). — <https://developers.google.com/identity/protocols/oauth2>
- **[high]** Unverified apps requesting sensitive/restricted scopes show the unverified-app screen and get a 100-new-user cap over the project's lifetime that cannot be reset; personal use (<100 users) is an exception to verification. — <https://support.google.com/cloud/answer/13464323?hl=en>
- **[medium]** Since about Aug 2026 the Publish button tooltip says a valid app name, support email, homepage URL and privacy policy URL are required to switch to External production. Several unresolved forum reports from Aug-Oct 2026. — <https://discuss.google.dev/t/cannot-publish-due-to-error-message-your-apps-oauth-configuration-is-incomplete-you-must-enter-the-missing-information-to-proceed-please-visit-the-branding-page-to-finish-configuring-your-app-even-though-all-required-fields-are-complete/392229>
- **[high]** Branding help: homepage/privacy/ToS links 'are required for all external production apps'; their domains must be authorized domains; homepage must be on a verified domain for verification. — <https://support.google.com/cloud/answer/15549049?hl=en>
- **[high]** Google Cloud console requires 2SV for personal Google Accounts on or after 2025-05-12; gcloud CLI has no 2SV requirement; passkey-only accounts must still enable 2SV. — <https://docs.cloud.google.com/docs/authentication/mfa-requirement>
- **[high]** Since 2024-05-06, personal and Workspace users can add an authenticator app or security key before turning on 2SV, so no phone number is required. — <https://workspaceupdates.googleblog.com/2024/05/updates-for-configuring-two-step-verification-for-your-google-account.html>
- **[medium]** Phone verification at account creation/sign-in is 'sometimes' required as anti-abuse; no doc makes it mandatory for Cloud project creation. Billing is not required to create a project. — <https://support.google.com/accounts/answer/114129?hl=en>
- **[high]** IAP OAuth Admin API (the only programmatic OAuth-client API, IAP-only) lost support 2026-01-19 and shut down 2026-03-19; Desktop clients must be created in the console. — <https://docs.cloud.google.com/sdk/gcloud/reference/iap/oauth-brands/create>
- **[high]** Gmail scope tiers: gmail.labels non-sensitive; gmail.send sensitive; mail.google.com, gmail.modify, gmail.readonly, gmail.compose, gmail.insert, gmail.metadata and gmail.settings.* are restricted. Request mail.google.com only for immediate permanent deletion. — <https://developers.google.com/workspace/gmail/api/auth/scopes>
- **[high]** messages.modify/batchModify accept only mail.google.com, gmail.modify or gmail.modify.restricted (admin domain-wide delegation); labels.create accepts gmail.labels; messages.send accepts gmail.send/compose/modify/mail.google.com. — <https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/modify>
- **[high]** IMAP/POP/SMTP XOAUTH2 requires scope https://mail.google.com/; SASL string base64('user='+email+'^Aauth=Bearer '+token+'^A^A'). — <https://developers.google.com/workspace/gmail/imap/xoauth2-protocol>
- **[high]** Live pre-auth CAPABILITY on imap.gmail.com (2026-10-09): IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 XYZZY SASL-IR AUTH=XOAUTH2 AUTH=PLAIN AUTH=PLAIN-CLIENTTOKEN AUTH=OAUTHBEARER. SMTP EHLO: AUTH LOGIN PLAIN XOAUTH2 PLAIN-CLIENTTOKEN OAUTHBEARER XOAUTH; SIZE 35882577. — <imaps://imap.gmail.com:993>
- **[high]** Gmail IMAP extensions: X-GM-RAW (Gmail search), X-GM-MSGID/X-GM-THRID (64-bit decimal equivalents of API hex IDs), X-GM-LABELS (FETCH/STORE/SEARCH), RFC 6154 special-use (\All, \Important, etc.); XLIST deprecated. — <https://developers.google.com/workspace/gmail/imap/imap-extensions>
- **[medium]** Gmail supports CONDSTORE but not QRESYNC (community reports 2018-2024). — <https://bugzilla.mozilla.org/show_bug.cgi?id=1747311>
- **[medium]** IMAP limits: 2,500 MB/day download, 500 MB/day upload (Workspace doc); up to 15 email clients per account; IMAP sessions about 24h. — <https://support.google.com/mail/answer/7126229?hl=en>
- **[high]** Gmail API sync: full sync via messages.list + batched messages.get; partial via history.list(startHistoryId); out-of-range ID returns HTTP 404 meaning full sync; history 'typically available for at least one week'. — <https://developers.google.com/workspace/gmail/api/guides/sync>
- **[high]** Batch: max 100 calls per batch, recommend at most 50; n batched calls count as n requests. — <https://developers.google.com/workspace/gmail/api/guides/batch>
- **[high]** Gmail API quotas updated 2026-05-01: 1,200,000 units/min/project, 6,000 units/min/user (was 15,000); messages.get 20, threads.get 40, history.list 2, messages.list 5, messages.modify 5, batchModify 50, send 100, watch 100. Older projects keep previous quotas for at least 60 days. — <https://developers.google.com/workspace/gmail/api/reference/quota>
- **[high]** Gmail API standard use is free; 80,000,000 units/day/project threshold; exceeding it 'is planned to incur charges later in 2026' with at least 90 days' notice; quota increases will require billing. — <https://developers.google.com/workspace/tools-safety>
- **[medium]** users.watch push requires a Cloud Pub/Sub topic (publisher role for gmail-api-push@system.gserviceaccount.com) and renewal at least every 7 days; Pub/Sub needs billing enabled. — <https://developers.google.com/workspace/gmail/api/guides/push>
- **[high]** Labels: max 10,000 per mailbox (API ref); colors restricted to a fixed palette; messages.modify adds/removes up to 100 labels; batchModify up to 1000 ids; labels can't be applied to drafts. — <https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.labels>
- **[high]** Restricted-scope production apps need verification; a security assessment (CASA, annual, Letter of Assessment, revalidated every 12 months) is needed if the app stores or transmits restricted data on or through servers. Approximate review times: brand 2-3 business days, sensitive 10 business days, restricted 6 weeks. — <https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification>
- **[medium]** CASA uses assurance levels AL1/AL2 via ADA-authorized labs; Google decides the level; self-assessment not accepted; annual revalidation. Costs roughly $500-$4.5k (third-party reports). — <https://appdefensealliance.dev/casa/casa-tiering>
- **[high]** Approved Gmail restricted-scope use cases include 'Built-in and web email clients' and productivity apps 'providing generative AI summaries'; Limited Use forbids using user data to train AI/ML beyond that user's personalized model. — <https://developers.google.com/workspace/workspace-api-user-data-developer-policy>
- **[high]** Google APIs ToS: 'Developer credentials (such as passwords, keys, and client IDs)... may not be embedded in open source projects'; must not misrepresent your API client's identity. — <https://developers.google.com/terms>
- **[high]** Thunderbird hard-codes its Google client ID/secret (PKCE on, scope mail.google.com) with the comment 'Don't copy these values for your own application - register one for yourself!' — <https://hg-edge.mozilla.org/comm-central/raw-file/tip/mailnews/base/src/OAuth2Providers.sys.mjs>
- **[high]** App passwords: require 2SV; unavailable for security-key-only 2SV, work/school, and Advanced Protection accounts; revoked on password change; 'not recommended'. No official 2026 deprecation found. — <https://support.google.com/accounts/answer/185833?hl=en>
- **[high]** Less secure apps: personal accounts lost password-only sign-in from 2022-05-30; Workspace ended 2025-05-01; IMAP enable/disable toggle removed January 2025 (IMAP always on). — <https://workspaceupdates.googleblog.com/2023/09/winding-down-google-sync-and-less-secure-apps-support.html>
- **[medium]** Advanced Protection allows only Google apps and verified third-party apps to access Gmail data and revokes/blocks app passwords. — <https://support.google.com/accounts/answer/7539956?hl=en>
- **[high]** Delta Chat removed Gmail OAuth2 (2024-08-14) citing a yearly CASA Tier 2/3 assessment; uses 2FA plus app passwords. — <https://github.com/deltachat/deltachat-android/issues/3233>
- **[high]** Google blocked KDE's Drive scope in June 2024 when KDE couldn't justify usage, which broke all Google sign-in via KAccounts until the scope was removed (24.12.2). — <https://bugs.kde.org/show_bug.cgi?id=480779>
- **[medium]** Penguin Mail's built-in Gmail client shows 'This app is blocked' pending verification (2026-10-08); the project is adding a BYO client option. — <https://github.com/c9dev/penguin-mail/issues/21>
- **[high]** Granular consent: users may grant a subset of requested scopes; check the scope field in the token response. — <https://developers.google.com/identity/protocols/oauth2/resources/granular-permissions>
- **[medium]** Optional DPoP (RFC 9449) for refresh-token binding is documented (adoption guide updated 2026-07-31); access tokens remain Bearer. — <https://developers.google.com/identity/protocols/oauth2/resources/dpop-adoption>
- **[medium]** Gmailify and POP fetching of other accounts, plus third-party 'Send as', are fully removed January 2027 (IMAP access to Gmail unaffected). — <https://support.google.com/mail/answer/17101213?hl=en>
- **[high]** Rust crates (crates.io, 2026-10-09): oauth2 5.0.0, yup-oauth2 12.1.2, google-gmail1 7.0.0+20251215, async-imap 0.12.0, lettre 0.11.23, mail-parser 0.11.9, keyring 4.2.0. — <https://crates.io/api/v1/crates/oauth2>

## Recommendations

- Support three Gmail auth modes from day one: (1) BYO Desktop OAuth client (default for open-source builds), (2) an optional nmail-owned client compiled in from CI secrets at build time (never committed to the repo, per Google APIs ToS), used only once verified, (3) app password with IMAP/SMTP as a no-Google-Cloud fallback.
- Make the Gmail API with gmail.modify the default backend: it covers read, label create/apply and send, avoids mail.google.com (permanent delete), and is the more approvable scope if nmail later seeks verification. Offer an IMAP+XOAUTH2 backend (mail.google.com) as opt-in for BYO users who want IDLE and no API quota usage.
- OAuth implementation: system browser only, loopback 127.0.0.1 on a random port, PKCE S256, state, and always send client_secret. Parse the granted 'scope' and degrade features if a scope was unchecked. Treat invalid_grant as a normal re-auth path (7-day Testing expiry, password change, 6-month idle, 100-token cap). Store refresh tokens in the OS keyring via the keyring crate.
- Headless/SSH: the device flow can't be used for Gmail, so provide a 'paste the redirected URL' fallback that extracts the code while keeping the PKCE verifier local, and document SSH -L port forwarding.
- Ship an 'nmail auth google setup' wizard covering: enable 2SV (authenticator works, no phone needed), create project, Branding, Audience External + add yourself as test user, enable the Gmail API, create a Desktop client and download the JSON immediately (secret shown once), then optionally publish to production to escape the 7-day expiry. Warn about the 2026 homepage/privacy URL requirement for publishing.
- Host a generic project homepage and privacy-policy page that BYO users can reference when publishing, and test whether Google accepts third-party authorized domains for unverified personal-use apps.
- Design sync for post-May-2026 quotas (6,000 units/min/user; messages.get = 20). Use messages.list + batches of 50 or fewer, store historyId, poll history.list (2 units) instead of watch/Pub/Sub, back off on 403/429, and cache bodies in SQLite so each message is fetched once.
- Map AI classification groups to user labels prefixed under one parent (e.g. nmail/...) using the fixed color palette, apply them via batchModify (up to 1000 ids), and stay well under 500 labels.
- Keep AI classification local or 'user brings their own LLM key' with explicit consent. Never train generalized models on Gmail data (Limited Use). Avoid routing Gmail data through any nmail-operated server, which would trigger a CASA assessment if nmail is ever verified.
- Do not reuse Thunderbird's or any other app's client ID: it violates Google APIs ToS and Thunderbird's explicit request, and risks sudden blocking. Detect Advanced Protection or blocked-app errors and point users to the app-password or verified-client paths.

## Open questions

- Is the Aug-Oct 2026 'Publish app requires homepage + privacy policy URL' behavior intended policy or a console bug, and does Google accept an unverified third-party domain (e.g., nmail's site) for personal-use BYO apps?
- Does the Desktop-client token endpoint still require client_secret despite docs marking it Optional? Needs a test with a real BYO client.
- Will an unverified, In-production BYO app requesting restricted Gmail scopes keep getting the bypassable unverified screen, or 'This app is blocked', for new projects in late 2026?
- Exact Gmail API overage pricing and start date (Google says later in 2026 with at least 90 days' notice). Does billing affect BYO personal projects that stay under 80M units/day?
- Would sending Gmail content to a user-chosen cloud LLM count as 'transmitting restricted data through a third-party server' (triggering CASA) if nmail pursues verification?
- Does Advanced Protection block a user's own unverified BYO client in Testing mode?
- Post-auth IMAP CAPABILITY for Gmail (CONDSTORE, MOVE, COMPRESS, APPENDLIMIT) could not be verified without logging in.
- Default Google Cloud project quota for brand-new personal accounts is undocumented and varies per account.

## Fact-check (independent verification pass)

Overall reliability: **high**

- **outdated:** Unverified apps get a 100-new-user cap over the project's lifetime that cannot be reset.
  - Correction: The current docs say '100 new users in total, after the app presents the unverified app screen'. They also say quotas 'might be adjusted for specific apps based on the app history, developer reputation, and riskiness'. The 'cannot be reset' wording no longer appears. The personal-use exception (<100 users, click-through warning) is confirmed on 13464323. Confidence: high. — <https://support.google.com/cloud/answer/7454865?hl=en>
- **refuted:** IMAP limits: 2,500 MB/day download, 500 MB upload, 15 clients, sessions about 24h (source 7126229).
  - Correction: Partly wrong. Google says 'Gmail IMAP sessions last up to approximately 24 hours. If you authenticated the session using OAuth credentials, it lasts approximately the validity period of the access token used (usually 1 hour).' So an XOAUTH2 client must reconnect about every hour. The bandwidth figures come from the Workspace page support.google.com/a/answer/1071518, not 7126229. The 15-client limit is correct and is from 7126229. Confidence: high. — <https://developers.google.com/workspace/gmail/imap/imap-smtp>
- **outdated:** Older projects keep previous quotas for at least 60 days.
  - Correction: The tools-safety page says existing projects' quotas 'will remain unchanged for at least 60 days'. It also says new standard quotas 'will apply unless an increased quota has already been approved and billing is enabled'. The blog promises owners at least 60 days' notice. The 60-day minimum ended around 2026-06-30, so grandfathering is temporary. Whether existing projects have been migrated as of 2026-10-09 could not be checked. — <https://developers.google.com/workspace/tools-safety>
- **refuted:** BYO clients: aerc, mutt_oauth2.py, himalaya v2 via ortie/oama/pizauth, email-oauth2-proxy, MishMail. Shipped IDs: GOA and Evolution with build-time overrides, and lieer.
  - Correction: Mostly correct. Correction: pimalaya's ortie offers a 'public application' option that embeds Thunderbird's Google client ID 406964657835-... It is the second preference after dynamic registration, so himalaya users are not purely BYO. Confirmed: mutt_oauth2.py prompts for an ID and secret. GOA's meson google_client_id/secret and EDS's WITH_GOOGLE_CLIENT_ID are confirmed. lieer ships shared credentials. MishMail is a Mac GUI app using BYO with gmail.modify. — <https://github.com/pimalaya/ortie>
- **unverifiable:** Recommendation: compile an nmail-owned client from CI secrets at build time 'per Google APIs ToS'.
  - Correction: Whether this complies is unclear. The ToS bars embedding credentials, including client IDs, 'in open source projects', and a binary the project distributes still embeds an extractable secret. Distro builds would lack the secret. All users would also share one project's 80M units/day threshold, about 4M messages.get per day. That threshold can't be raised, and increases will require billing. Under the Help Center wording, a verified restricted-scope client also faces annual CASA. — <https://developers.google.com/terms>
- **unverifiable:** Recommendation: host a generic nmail homepage and privacy page for BYO users to reference when publishing.
  - Correction: This is risky. The OAuth policy requires every production app's homepage on 'a verified domain under your ownership', and the URL domains must be the project's authorized domains. Google does not check this unless the app is submitted for verification, so it may work, but it goes against the written policy. A safer option is a site the user owns, such as user.github.io, which can be verified in Search Console. — <https://support.google.com/cloud/answer/13807376?hl=en>
- **unverifiable:** Recommendation: make the Gmail API with gmail.modify the default backend.
  - Correction: The trade-off has shifted. A new project gets about 300 messages.get per minute per user (6,000/20), down from 3,000 before May 2026. A 100k-message initial sync would take about 5.5 hours. There is also a per-user concurrent-request limit (429 'Too many concurrent requests for user'), and per-user limits 'can't be increased'. IMAP is bandwidth-capped but, as far as the docs indicate, not metered in API units (not stated explicitly). For BYO unverified clients, scope approvability doesn't matter. — <https://developers.google.com/workspace/gmail/api/guides/handle-errors>

### Missed facts

- Agentic OAuth clients (Google Auth Platform help): there is a 'Use this client for an AI-powered agent' checkbox. Apps with both user-interactive and AI-agent workflows 'must create and maintain separate OAuth 2.0 client IDs'. Some Google MCP endpoints accept only agentic client IDs. This is directly relevant to nmail's agent-driven CLI and customization (https://support.google.com/cloud/answer/15549257).
- OAuth 2.0 Policies require every production app to have a public homepage on a verified domain the developer owns, and forbid committing client credentials to public repos. The policy changelog also added 'continuous improvements' (2025-12-15) and 'security signals handling' (2026-08-05).
- An XOAUTH2-authenticated Gmail IMAP session lasts about as long as the access token (usually 1 hour). Password-authenticated sessions last about 24 hours. nmail's IMAP backend must expect a BYE and reconnect with a fresh token about every hour.
- The May 2026 quota change raised per-call costs as well as lowering the per-user rate. Changes versus Dec 2025: messages.get/messages.attachments.get/drafts.get/messages.trash 5->20, threads.get 10->40, threads.trash 10->20. Attachment downloads and trash operations are now 4x pricier.
- The Gmail API has a per-user concurrent-request limit shared by all clients of a mailbox (429 'Too many concurrent requests for user'). Per-user limits cannot be increased, and the 80M/day threshold 'can't' be raised. The API caps a message at 500 recipients. Consumer Gmail limits sending to 500 recipients per message or 500 emails per day.
- Grandfathered quotas for projects active Nov 2025-Apr 2026 are temporary. The guaranteed period was only 'at least 60 days', and new standard quotas apply unless an increase is approved and billing is enabled.
- Post-auth Gmail IMAP capabilities (community logs, May 2025) answer an open question. They include CONDSTORE, MOVE, COMPRESS=DEFLATE, UIDPLUS, ENABLE, ESEARCH, UTF8=ACCEPT, LIST-EXTENDED, LIST-STATUS, LITERAL-, SPECIAL-USE and APPENDLIMIT=35651584. QRESYNC is absent.
- Labels added to a thread are not inherited by messages that arrive later, so AI classification must label each new message.
- Advanced Protection accounts may block unverified apps even for listed test users (Audience help). This partly answers the open question about BYO Testing clients.
- For a verified client using IMAP, the XOAUTH2 doc requires showing 'full utilization' of mail.google.com, or else migrating to the Gmail API's narrower scopes.
- Google's wording on CASA conflicts. The Help Center says all restricted-scope apps need an annual assessment, while the developer docs require it only for data handled on or through third-party servers.
- Ortie (the token broker for himalaya v2) offers Thunderbird's Google client ID as a built-in 'public application'. Reusing another app's client is common in this ecosystem despite the ToS.
- Time-based access: when it is granted, token responses include refresh_token_expires_in (currently only in select products such as the Data Portability API). nmail should honour this field if present.
- The Workspace user-data policy reminds developers that the ToS bars building databases or permanent copies of user data unless the content owner permits it. This is relevant to Limited Use disclosures for the SQLite cache if nmail seeks verification.
- The Gmail MCP server (developer preview April 22, 2026) has no send tool, needs the Developer Preview Program, and has its own quota table. Google also announced a Workspace CLI at Cloud Next '26. Both are prior art for nmail's agent features.
