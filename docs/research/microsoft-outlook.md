# Research: microsoft-outlook

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

## Programmatic access to personal Outlook.com (MSA) mailboxes, as of 2026-10-09

**Bottom line:** For Outlook.com, use **Microsoft Graph** as the main transport, through your own public-client app registration. Keep IMAP/SMTP XOAUTH2 only as a fallback: both are unreliable for consumer mailboxes in 2026. EWS is not an option.

### 1. Getting a client ID
- A plain personal account can't register apps anymore. The portal says: *"The ability to create applications outside of a directory has been deprecated. You can get a new directory by joining the M365 Developer Program or signing up for Azure."* People have reported this since Aug 2024, and it still appeared in Sept 2026. Signing in with only an MSA puts you in the locked "Microsoft Services" tenant.
- **Cheapest legitimate path:** sign up for an Azure free account. It requires a phone and a non-prepaid credit or debit card (a temporary $1 hold, no charge). That gives you a Default Directory where you register the app. The M365 Developer sandbox is limited to Visual Studio Pro/Enterprise subscribers, ISV Success members, eligible partners and Premier/Unified support customers.
- **Registration settings:**
  - Audience: "Any Entra ID tenant + Personal Microsoft accounts" (`AzureADandPersonalMicrosoftAccount`). Use `/common`, or `/consumers` for personal accounts only.
  - Platform: Mobile & desktop, redirect `http://localhost`. You can add `http://127.0.0.1` through the manifest.
  - **Allow public client flows = Yes** (`isFallbackPublicClient`), which device code needs. No secret.
  - Permissions are requested at runtime (dynamic consent).
- **Risk: the tenant can be deleted.** Inactive tenants get blocked (AADSTS5000225) and are **deleted 20 days later**. Microsoft's warning emails cite more than 200 days without a purchase. If that happens, the shared client ID dies for every user. Keep billing active (pay-as-you-go) and sign in as admin from time to time.
- **Publisher verification** is free but needs a Partner Center (CPP) account and a custom domain. Apps registered with an MSA can't be verified.
- **Work/school (M365) users:** since Oct–Nov 2025, Microsoft's default consent policy stops users consenting on their own to Mail.\*, MailboxSettings.\*, IMAP/POP/EWS scopes and similar. Only listed "popular mail clients" are exempt (Apple Mail, Spark, eM Client, Android-Samsung, Android-Mail, Thunderbird). So M365 users of neomail's own ID will need admin consent. This doesn't apply to personal accounts.

### 2. IMAP/SMTP OAuth2
- **Scopes:** `https://outlook.office.com/IMAP.AccessAsUser.All`, `.../POP.AccessAsUser.All`, `.../SMTP.Send`, plus `offline_access`.
- **Auth:** SASL XOAUTH2 `base64("user="+addr+"\x01auth=Bearer "+tok+"\x01\x01")`.
- **Servers:** IMAP `outlook.office365.com:993`; SMTP `smtp-mail.outlook.com:587` STARTTLS.
- **IMAP and POP are off by default.** The user must turn them on in Settings > Mail > Forwarding and IMAP.
- **Basic auth and app passwords** stopped working for all Outlook.com accounts on **2024-09-16**.
- **Problems reported in 2026:**
  - Newly created accounts get `535 5.7.139 SmtpClientAuthentication is disabled for the Mailbox` even with OAuth. Mozilla's KB says "For new accounts, SMTP always starts disabled", and Microsoft support called it temporary.
  - IMAP logins fail intermittently with "User is authenticated but not connected" (reports from Dec 2025 through 2026-09-24, about 1 in 5 attempts succeeding in the latest one). This is unresolved.
  - Exchange IMAP doesn't support custom keywords, so **categories can't be set over IMAP**.

### 3. Graph Mail (delegated, personal accounts supported)
- **Scopes:** `User.Read Mail.ReadWrite Mail.Send MailboxSettings.ReadWrite offline_access` (+ `openid profile`).
- **Categories:**
  - `/me/outlook/masterCategories` needs MailboxSettings.Read to list and MailboxSettings.ReadWrite to create.
  - `displayName` must be unique and **can't be renamed**; only `color` (`preset0`..`preset24` or `None`) can change.
  - To apply, PATCH `message.categories` with the full array (Mail.ReadWrite).
  - Deleting a master category doesn't remove it from messages already tagged.
- **Delta:** `GET /me/mailFolders/{id}/messages/delta` works per folder only.
  - Least privilege is Mail.ReadBasic. Supports `$select`/`$top`/`$expand`, `changeType`, and `Prefer: odata.maxpagesize`.
  - Filtering only works on `receivedDateTime ge|gt` and caps results at 5,000. Sorting only on `receivedDateTime desc`.
  - Moved and deleted items both arrive as `@removed`. Use mailFolder delta to track the folder tree.
- **Sending:** `POST /me/sendMail` accepts JSON or base64 MIME (`text/plain`) and returns 202. Attachments over 3 MB need an upload session (up to 150 MB).
- **Organising:** `move` takes well-known names (`archive`, `deleteditems`, `junkemail`, …). DELETE and `permanentDelete` are also available.
- **Junk:** reporting junk or phishing only exists in **beta** (`reportMessage`). `markAsJunk` was retired in Dec 2025.
- **IDs:** `Prefer: IdType="ImmutableId"` keeps message IDs stable across folder moves.
- **Throttling:** per app and mailbox, 10,000 requests per 10 minutes, 4 concurrent, 150 MB uploaded per 5 minutes. `$batch` takes at most 20 requests.
- **From 2026-12-31:** changing subject, body or recipients of non-draft messages needs `Mail-Advanced.ReadWrite` (admin consent). Categories, isRead and flags are not affected.
- **Where categories show up:** they're stored in the mailbox, so Outlook on the web shows them. Outlook iOS (2024) and Android (mid-2025) added mail categories, but Microsoft only announced that for M365, not Outlook.com.

### 4. EWS
- Exchange Online started turning EWS off by default on **2026-10-01** for tenants without an AppID allow-list. It is permanently shut off on **2027-04-01**.
- EWS OAuth never worked for personal accounts (the scope is rejected).

### 5. How other open-source clients get a client ID
- **Thunderbird** has its own ID `9e5f94bc-…d753` (the old `08162f7c…` was disabled).
  - Since TB 153 (July 2026) it signs in through the system browser with a loopback `http://127.0.0.1:<port>` redirect.
  - Its code also requests Graph scopes, but Mozilla only supports Graph for M365.
- **email-oauth2-proxy, mutt_oauth2.py, aerc:** you supply your own client ID. Their docs and wikis often point to Thunderbird's ID.
- **Himalaya v2:** OAuth is handled by the `ortie` helper. Its preset uses Thunderbird's ID for IMAP/SMTP and the Graph PowerShell app `14d82eec-…` for Graph. There is a native Graph backend through the Rust crate `io-msgraph` (0.5.0).
- **Evolution:** uses the legacy Live endpoints with `wl.imap` scopes and client `cc6e0693-…`.
- **Geary:** no OAuth of its own; it depends on GNOME Online Accounts, which dropped its Windows Live provider. Geary 46 doesn't use GOA's Microsoft 365 provider, so personal Outlook accounts stopped working.
- **Reusing Thunderbird's ID is common, but don't ship it.** Microsoft's identity platform terms say credentials are "non-transferable", forbid impersonation and forbid hiding where requests come from, and Microsoft can disable the app. The consent screen shows "Thunderbird", Mozilla controls its redirects, and it's unconfirmed whether it works with Graph for personal accounts.

### 6. Protocol details
- **Auth code + PKCE (S256):** recommended. Public clients must not send a secret.
- **Loopback redirect:** Entra ignores the port only for `localhost`. `[::1]` isn't supported, and personal-account apps can't use query strings in the redirect.
- **Device code:**
  - Works with `/consumers`. Codes expire after 15 minutes, and personal users are asked to sign in again.
  - Security defaults in tenants **created after 2026-07-01** block device code, which affects M365 users.
- **Tokens:**
  - Access tokens last 60–90 minutes; treat them as opaque, since personal-account tokens may be encrypted.
  - Refresh tokens expire after 90 days without use, rotate on every use (keep the newest), and can't be tuned for personal-account apps.
  - A refresh token belongs to the user and client, not one resource, so one consent can cover Graph and outlook.office.com. Each `/token` call must still target one resource.

## Key facts

- **[high]** Personal Microsoft accounts can no longer register apps outside a directory. The portal error says: 'The ability to create applications outside of a directory has been deprecated. You can get a new directory by joining the M365 Developer Program or signing up for Azure.' It was first reported Aug 2024 and was still in effect in May and Sept 2026. — <https://learn.microsoft.com/en-us/answers/questions/5360121/cannot-access-my-files-using-rclone-anymore-the-ab>
- **[high]** Microsoft's Entra quickstart (ms.date 2026-05-14) requires an Azure account with an active subscription and a workforce tenant (the Default Directory works). The supported account type labels are now 'Any Entra ID Tenant + Personal Microsoft accounts' and 'Personal accounts only'. — <https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app>
- **[high]** If your account has no tenant, you see a GUID under your account name and 'won't be able to do actions like registering apps until you create a Microsoft Entra tenant'. — <https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-create-new-tenant>
- **[high]** Only paid customers can create an additional workforce tenant from the admin center. Free or trial customers are told to sign up for a free Azure account instead (doc dated 2026-07-29). — <https://learn.microsoft.com/en-us/entra/fundamentals/create-new-tenant>
- **[high]** An Azure free account requires a phone number and a non-prepaid credit or debit card for identity verification. There may be a temporary $1 hold, and 'Azure free account customers will not incur any charges'. After the trial you either move to pay-as-you-go or your account and services are disabled. — <https://azure.microsoft.com/en-us/pricing/purchase-options/azure-account>
- **[high]** The free M365 E5 developer sandbox is only for Visual Studio Pro/Enterprise standard subscribers, ISV Success members, eligible Microsoft AI Cloud Partner Program partners, and Premier/Unified Support customers (doc dated 2026-09-02). — <https://learn.microsoft.com/en-us/office/developer-program/microsoft-365-developer-program>
- **[medium]** Tenants blocked for inactivity (AADSTS5000225) are deleted after 20 days and can't be recovered. Microsoft's warning emails cite more than 200 days of inactivity and ask for a purchase; the exact criteria aren't published. — <https://learn.microsoft.com/en-us/entra/fundamentals/inaccessible-tenant>
- **[high]** Publisher verification needs a verified Partner Center (CPP) account and an app registered with a work or school account. Apps registered with a personal Microsoft account can't be verified, and the publisher domain can't be *.onmicrosoft.com. Verification is free. — <https://learn.microsoft.com/en-us/entra/identity-platform/publisher-verification-overview>
- **[high]** Microsoft's default user-consent policy for work/school tenants blocks users from consenting on their own to Mail.*, MailboxSettings.*, MailBoxFolder.*, Calendars.*, Contacts.*, IMAP/POP/EWS/EAS.AccessAsUser.All and similar scopes. Only the 'popular mail clients' are exempt: Apple Mail, Spark, eM Client, Android-Samsung, Android-Mail and Thunderbird (9e5f94bc-e8a4-4e73-b8be-63364c29d753). Announced in MC1163922 (Oct–Nov 2025) and extended by MC1304287 (Jun–Jul 2026). — <https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies>
- **[high]** The IMAP/POP/SMTP OAuth scopes are https://outlook.office.com/IMAP.AccessAsUser.All, .../POP.AccessAsUser.All and .../SMTP.Send, plus offline_access. Microsoft's doc says this OAuth support is available for both Microsoft 365 and Outlook.com users, using the SASL XOAUTH2 format. — <https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth>
- **[high]** Outlook.com server settings: IMAP outlook.office365.com:993 SSL, POP outlook.office365.com:995, SMTP smtp-mail.outlook.com:587 STARTTLS, OAuth2 required. Microsoft's page says 'POP & IMAP access is disabled by default'; users enable it under Settings > Mail > Forwarding and IMAP. — <https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040>
- **[high]** Basic authentication, including app passwords, stopped working for all Outlook.com, Hotmail and Live accounts on September 16, 2024. — <https://support.microsoft.com/en-us/office/modern-authentication-methods-now-needed-to-continue-syncing-outlook-email-in-non-microsoft-email-apps-c5d65390-9676-4763-b41f-d7986499a90d>
- **[medium]** Newly created personal Outlook.com accounts fail SMTP even with OAuth: '535 5.7.139 SmtpClientAuthentication is disabled for the Mailbox'. Reports run Feb–Aug 2026 while IMAP works with the same token. Microsoft support called it a temporary restriction on new accounts, and Mozilla's KB says 'For new accounts, SMTP always starts disabled'. — <https://learn.microsoft.com/en-us/answers/questions/5963371/thunderbird-can-receive-but-not-send-email-via-smt>
- **[medium]** Personal-account IMAP logins intermittently fail with 'User is authenticated but not connected' and POP fails with 'bad password'. This happens with any client or client ID; the latest report (2026-09-24) saw about 1 in 5 IMAP attempts succeed. It remains unresolved. — <https://github.com/simonrob/email-oauth2-proxy/issues/428>
- **[high]** Exchange's IMAP server does not support client-defined keywords (only $MDNSent), so Outlook categories can't be read or set over IMAP. — <https://learn.microsoft.com/en-us/openspecs/exchange_standards/ms-stanximap/a8c49281-d59a-4cb9-9b90-e091c1bc24d9>
- **[medium]** Exchange Online (Microsoft 365, not Outlook.com) basic-auth SMTP AUTH is turned off by default for existing tenants at the end of December 2026, but admins can turn it back on. The final removal date is to be announced in H2 2027. — <https://office365itpros.com/2026/01/29/smtp-auth-basic-retirement/>
- **[high]** Graph mail works for personal accounts ('Outlook mail data in a personal or organization account'). It does not cover in-place archive mailboxes. — <https://learn.microsoft.com/en-us/graph/api/resources/mail-api-overview?view=graph-rest-1.0>
- **[high]** For masterCategories, listing needs MailboxSettings.Read and creating needs MailboxSettings.ReadWrite; personal accounts are supported. displayName must be unique, is read-only after creation and can't be renamed; only color (None or preset0..preset24) can change. You apply a category by adding its displayName to the message's categories collection. — <https://learn.microsoft.com/en-us/graph/api/resources/outlookcategory>
- **[high]** Updating a message (PATCH) needs Mail.ReadWrite, and personal accounts are supported. categories, flag, isRead, importance and inferenceClassification can be updated; subject, body and recipients only on drafts. — <https://learn.microsoft.com/en-us/graph/api/message-update>
- **[high]** Message delta sync works one folder at a time. Least privilege is Mail.ReadBasic (personal supported). $filter only accepts receivedDateTime ge/gt and returns at most 5,000 messages; $orderby only receivedDateTime desc; changeType can be created, updated or deleted. Deletions and moves come back as @removed with reason 'deleted'. — <https://learn.microsoft.com/en-us/graph/delta-query-messages>
- **[high]** POST /me/sendMail needs Mail.Send (personal supported), accepts JSON or base64 MIME with Content-Type text/plain, and returns 202 Accepted. Attachments over 3 MB need an upload session, up to 150 MB. — <https://learn.microsoft.com/en-us/graph/api/user-sendmail>
- **[high]** Moving a message takes a folder ID or a well-known name such as archive, deleteditems, junkemail or inbox. It returns 201 with a new message object, so the ID changes unless you request immutable IDs. — <https://learn.microsoft.com/en-us/graph/api/message-move>
- **[high]** Reporting junk, not-junk or phishing (reportMessage) only exists in beta (Mail.ReadWrite, personal supported). markAsJunk and markAsNotJunk are deprecated. Microsoft's EWS parity roadmap lists 'Report Message' for Q4 CY2026. — <https://learn.microsoft.com/en-us/graph/api/message-reportmessage?view=graph-rest-beta>
- **[high]** Graph throttling for Outlook is per app and mailbox: 10,000 requests per 10 minutes, 4 concurrent requests, and 150 MB uploaded per 5 minutes. — <https://learn.microsoft.com/en-us/graph/throttling-limits>
- **[medium]** With the header Prefer: IdType="ImmutableId", message IDs stay the same across folder moves within the mailbox, and delta responses honor the header. Microsoft doesn't say explicitly whether this works for personal accounts. — <https://learn.microsoft.com/en-us/graph/outlook-immutable-id>
- **[medium]** From 2026-12-31, Graph updates to subject, body or recipients on non-draft messages require Mail-Advanced.ReadWrite (admin consent). Drafts and other properties stay editable with Mail.ReadWrite. Microsoft doesn't say whether personal accounts are in scope. — <https://devblogs.microsoft.com/microsoft365dev/graph-api-updates-to-sensitive-email-properties/>
- **[high]** EWS in Exchange Online has been disabled by default since 2026-10-01 for tenants without an AppID allow-list, and is fully and permanently disabled on 2027-04-01 with no exceptions. — <https://techcommunity.microsoft.com/blog/exchange/exchange-online-ews-your-time-is-almost-up/4492361>
- **[medium]** EWS OAuth (EWS.AccessAsUser.All) isn't supported for personal Microsoft accounts. Requests are rejected with AADSTS70011 or 'The given token is invalid'. — <https://learn.microsoft.com/en-us/answers/questions/917011/the-given-token-is-invalid-error-in-ews-oauth-auth>
- **[high]** Thunderbird's source uses client ID 9e5f94bc-e8a4-4e73-b8be-63364c29d753 on the /common endpoints with outlook.office.com IMAP/POP/SMTP scopes plus offline_access. It also has an EWS scope and Graph scopes (User.Read, MailboxFolder.ReadWrite, Mail.ReadWrite, Mail.Send). It signs in through the system browser with a loopback http://127.0.0.1 redirect on an ephemeral port. — <https://github.com/mozilla/releases-comm-central/blob/master/mailnews/base/src/OAuth2Providers.sys.mjs>
- **[medium]** Thunderbird 153 (2026-07-22) moved OAuth sign-in to the default browser. Mozilla says Microsoft changed how it classifies clients, so Thunderbird's old OAuth setup no longer counted as a desktop client. Thunderbird's Graph support is only for M365. — <https://blog.thunderbird.net/2026/07/thunderbird-153-meadow-is-out-now/>
- **[high]** Thunderbird's old client ID 08162f7c-0fd2-4200-a84a-f25a4db0b584 returned 'AADSTS7000112 ... is disabled' by Aug 2024. Configs that reuse the new ID must send the redirect URI Thunderbird registered (https://localhost); http://localhost failed with AADSTS50011. — <https://github.com/simonrob/email-oauth2-proxy/issues/267>
- **[high]** email-oauth2-proxy's README says you can reuse another email client's client ID 'with care and restraint', because access will be attributed to that client's owner. For personal Outlook accounts it says you need an M365 Developer or Azure account to register your own, or else reuse an existing ID. — <https://github.com/simonrob/email-oauth2-proxy/blob/main/README.md>
- **[high]** Himalaya v2 hands OAuth to the 'ortie' helper. Ortie's Outlook IMAP/SMTP preset uses Thunderbird's ID with https://localhost, and its Graph preset uses the Microsoft Graph PowerShell app 14d82eec-204b-4c2f-b7e8-296a70dab67e. Himalaya has a native Graph backend via the io-msgraph crate (Rust, Apache-2.0/MIT, v0.5.0 on crates.io). — <https://github.com/pimalaya/ortie>
- **[high]** Evolution's Outlook.com support uses the legacy Live Connect endpoints (login.live.com/oauth20_authorize.srf) with scopes 'wl.offline_access wl.emails wl.imap' and client cc6e0693-0e26-4220-8322-9d363e308fc6. evolution-ews uses 20460e5d-ce91-49af-a3a5-70b6be7486d1 with the nativeclient redirect. — <https://gitlab.gnome.org/GNOME/evolution-data-server/-/blob/master/CMakeLists.txt>
- **[medium]** GNOME Online Accounts removed its Windows Live provider in 3.55.0. Its Microsoft 365 (Graph) provider uses client b155a604-3c31-4079-93b0-6bb6aa9d5464 with redirect goa-oauth2://localhost/<client_id>. Geary has no OAuth of its own and doesn't use that provider, so personal Outlook accounts broke for Geary on GNOME 49. — <https://gitlab.gnome.org/GNOME/gnome-online-accounts/-/blob/master/meson_options.txt>
- **[high]** Microsoft identity platform terms: Access Credentials are 'non-transferable and non-assignable'; registered display names may not impersonate others; you must not 'obscure or alter the source of queries'; and Microsoft may disable your Application at its discretion. — <https://learn.microsoft.com/en-us/legal/microsoft-identity-platform/terms-of-use>
- **[high]** For loopback redirect URIs, the port is ignored only for localhost. The docs say prefer 127.0.0.1, but http://127.0.0.1 has to be added through the manifest, [::1] isn't supported, query parameters aren't allowed for apps that support personal accounts, and such apps can have at most 100 redirect URIs. — <https://learn.microsoft.com/en-us/entra/identity-platform/reply-url>
- **[high]** For auth code flow, PKCE is recommended for all app types and S256 is supported. Public clients must not send secrets. The /authorize request can cover scopes for several resources, but each /token request must be for one resource. — <https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow>
- **[high]** Device code flow works with /common, /consumers and /organizations. Codes expire after 15 minutes (expires_in) and the client polls at the given interval. Personal-account users must sign in again on the verification page. The app needs 'Allow public client flows' enabled. — <https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code>
- **[high]** Since 2026-07-01, security defaults in new Entra tenants block device code flow. This affects M365 users, not personal accounts. — <https://learn.microsoft.com/en-us/entra/fundamentals/security-defaults>
- **[high]** Refresh tokens last 90 days by default (24 hours for SPAs) and replace themselves on each use. They are bound to the user and client, not a resource, so one can be used for several APIs. The 90-day max inactive time can't be configured, and token lifetime policies aren't supported for apps with personal-account audiences. Access tokens last 60–90 minutes. — <https://learn.microsoft.com/en-us/entra/identity-platform/refresh-tokens>
- **[medium]** Outlook for Android gained the ability to apply categories to emails in Jun–Jul 2025 (MC1085132). The announcement only mentions organizational (M365) environments, not Outlook.com. — <https://mc.merill.net/message/MC1085132>

## Recommendations

- Make Microsoft Graph the main Outlook.com backend: use delta sync for each folder, sendMail for sending (with a MIME body), PATCH message.categories plus masterCategories for AI labels, and move to archive/deleteditems/junkemail. Keep IMAP/SMTP XOAUTH2 only as an optional fallback, given the 2026 failures on new accounts and the intermittent 'authenticated but not connected' errors.
- Register neomail's own public-client app in an Entra tenant created through an Azure free account (it needs a card). Use audience AzureADandPersonalMicrosoftAccount, Mobile & desktop redirect http://localhost (optionally http://127.0.0.1 via the manifest), 'Allow public client flows' = Yes, and no secret. Request scopes: openid profile offline_access User.Read Mail.ReadWrite Mail.Send MailboxSettings.ReadWrite. Add the outlook.office.com IMAP.AccessAsUser.All / SMTP.Send scopes only if you ship the IMAP fallback.
- Guard against tenant deletion: keep a billing relationship on the tenant (pay-as-you-go with $0 spend), have at least two admins sign in regularly, watch for inactivity emails, and let users override the client ID in config (--client-id, tenant) so they can bring their own registration.
- Don't ship Thunderbird's client ID. It goes against Microsoft's terms on credential transfer and impersonation, it is fragile, and it's unverified with Graph for personal accounts. At most, document a user-supplied client-ID override.
- Auth implementation (Rust): auth code + PKCE S256 via the system browser with a loopback listener on an ephemeral port, and device code flow for headless use. Use /consumers for personal-only sign-in, or /common. Persist the rotated refresh token after every refresh in the OS keyring (keyring crate), and treat invalid_grant or AADSTS700082 as 'sign in again'. Never parse access tokens.
- With a localhost redirect, the browser may resolve to ::1. Have the loopback server listen on both 127.0.0.1 and ::1, or test the http://127.0.0.1 redirect registered through the manifest.
- Send Prefer: IdType="ImmutableId" on every Graph request (including the inner requests of a $batch) so SQLite rows stay stable across moves. Test this on a real Outlook.com account before relying on it.
- Respect throttling: at most 4 concurrent requests per mailbox, $batch of no more than 20, honor Retry-After on 429/503, and use Prefer: odata.maxpagesize together with $select for delta.
- For categories, create the master category first (pick a preset color), never try to rename it (delete and recreate instead), and read-modify-write the whole categories array. Plan a cleanup step if neomail deletes a group, because deleting a master category doesn't untag messages.
- Onboarding UX: tell users to turn on IMAP in Outlook.com settings only if they use the IMAP fallback. Warn that new accounts may have SMTP AUTH disabled, and default to Graph sendMail.
- For M365 (work) users, expect 'admin approval required' under the default consent policy, because neomail isn't on the approved mail-client list. Document the admin-consent URL, and consider publisher verification (needs a Partner Center/CPP account and a custom domain) once there are M365 users.
- Look at pimalaya's io-msgraph (Rust, Apache-2.0/MIT) and io-oauth crates, or oauth2-rs 5.x, rather than writing Graph and OAuth plumbing from scratch.

## Open questions

- Does Prefer: IdType="ImmutableId" work for consumer Outlook.com mailboxes? It isn't documented; test it.
- Exactly what inactivity criteria trigger tenant blocking (AADSTS5000225)? Do app sign-ins count as activity, and does a pay-as-you-go subscription with $0 spend prevent deletion?
- Is SMTP AUTH on new Outlook.com accounts disabled only temporarily or permanently? Microsoft has published nothing.
- Will the IMAP 'User is authenticated but not connected' regression for personal accounts be fixed? It was still open as of 2026-09-24.
- Does the 2026-12-31 Mail-Advanced.ReadWrite requirement apply to personal accounts? It doesn't affect categories, but it would affect editing non-draft messages.
- Will reportMessage (junk/not junk/phishing) reach Graph v1.0 in Q4 2026 as the EWS parity roadmap says, and does the beta version actually work for Outlook.com?
- Do Outlook iOS/Android show categories for personal Outlook.com accounts? The announcements only covered M365.
- Does an http://127.0.0.1 redirect with an ephemeral port match reliably for personal-account sign-ins (Thunderbird 153+ appears to do this), or is http://localhost required?
- Does Graph accept tokens from Thunderbird's client ID for personal accounts? Not verified, and not needed if neomail uses its own ID.
- Could not fetch Mozilla's KB article 'Microsoft OAuth Authentication and Thunderbird in 2026' directly (bot challenge). Its claims here come from search snippets only.

## Fact-check (independent verification pass)

Overall reliability: **high**

- **unverifiable:** Deleting a master category doesn't remove it from messages already tagged.
  - Correction: The Graph delete doc says nothing either way. Slipstick and a Q&A thread on Graph deletion say items keep the name with no colour. Some guides for new Outlook say deletion removes it from items, so sources conflict. Test it. — <https://learn.microsoft.com/en-us/graph/api/outlookcategory-delete?view=graph-rest-1.0>
- **outdated:** EWS has been disabled by default since 2026-10-01 for tenants without an AppID allow-list; permanently off 2027-04-01.
  - Correction: The 2027-04-01 shutdown with no exceptions is confirmed, but the rollout is phased. Per Microsoft's 2026-10-01 post, phase 1 enforces the allow-list for EWSEnabled=True tenants from Oct 10. Phase 2 then flips Null tenants to False in batches with 7 days' Message Center notice. As of 2026-10-09 most Null tenants aren't blocked yet. — <https://techcommunity.microsoft.com/blog/exchange/ews-deprecation-is-here-%E2%80%93-what-this-means-to-you/4561431>
- **refuted:** Thunderbird's source uses client 9e5f94bc-... on /common with IMAP/POP/SMTP, EWS and Graph scopes, and since TB 153 signs in via the system browser with an http://127.0.0.1 loopback on an ephemeral port.
  - Correction: The client ID, /common endpoints and scopes are confirmed. But TB 153 ESR defaults to http://localhost:<ephemeral port>. The switch to http://127.0.0.1 is Bug 2045029, landed in comm-central on 2026-08-29 and targeting TB 157. Microsoft has no override in that change, which suggests 127.0.0.1 plus ephemeral ports works with Entra in practice. — <https://bugzilla.mozilla.org/show_bug.cgi?id=2045029>
- **refuted:** The TB 153 blog says Microsoft changed how it classifies clients, and Thunderbird's Graph support is M365-only.
  - Correction: The blog confirms the 2026-07-22 release and 'OAuth in Browser' only. The client-classification text comes from the Mozilla KB (search snippet only). The blog says Graph support is 'experimental... temporarily disabled behind a preference'. I could not verify an M365-only scope. — <https://blog.thunderbird.net/2026/07/thunderbird-153-meadow-is-out-now/>
- **outdated:** The old TB client ID 08162f7c returned AADSTS7000112 by Aug 2024; reusing 9e5f94bc requires the https://localhost redirect (http://localhost gave AADSTS50011).
  - Correction: This was accurate for Aug 2024 to Jan 2025. TB now uses http://localhost and http://127.0.0.1 loopback redirects, so the registration has changed and the https://localhost requirement may be stale. The aerc O365 wiki still points to the dead 08162f7c ID. — <https://github.com/simonrob/email-oauth2-proxy/issues/267>

### Missed facts

- Graph change notifications (webhooks) only deliver to a publicly reachable HTTPS endpoint, so a local TUI/CLI gets no push from Graph. Polling delta within the 10k requests per 10 minutes budget is the main way to stay current (high; https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks).
- Outlook delta tokens have no fixed lifetime and are evicted when an internal cache fills. Expect 410 Gone or syncStateNotFound and build a full-resync path for the SQLite cache (high; https://learn.microsoft.com/en-us/graph/delta-query-overview).
- Raw RFC 822 content is available from GET /me/messages/{id}/$value, which the local cache and offline rendering will need (high; https://learn.microsoft.com/en-us/graph/outlook-get-mime-message).
- A MIME sendMail request is capped at about 4 MB: a 413 limit is documented for S/MIME and reported for plain MIME. Bigger messages need a draft created via POST /me/messages, then createUploadSession, then send (medium; https://learn.microsoft.com/en-us/graph/api/user-post-messages).
- Outlook.com sending limits: Microsoft 365 subscribers get 5,000 recipients per day, 500 per message and 1,000 non-relationship recipients per day. Free accounts are lower, and new accounts start low. This matters for CLI and scripting (medium; https://support.microsoft.com/en-us/office/sending-limits-in-outlook-com-279ee200-594c-40f0-9ec8-bb6af7735c2e).
- If neomail ships its own client ID, the identity platform terms require a EULA that disclaims Microsoft warranties, a public privacy statement, and least-privilege scopes (high; https://learn.microsoft.com/en-us/legal/microsoft-identity-platform/terms-of-use).
- Under the Microsoft-managed consent policy, Mail.Send stays user-consentable, but MailboxSettings.ReadWrite (needed for categories) and Mail.ReadWrite need admin consent. Tenants with custom policies are unaffected (high; https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies).
- The EWS parity roadmap targets 'Mark All Items As Read' and 'Non-draft MIME update/creation' for Q4 CY2026. Until those ship, bulk read state needs per-message PATCHes (batched) (high; https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/deprecation-of-ews-exchange-online).
- Mozilla Bug 2045029 switched Thunderbird's loopback redirect from http://localhost to http://127.0.0.1 (landed 2026-08-29, TB 157). Microsoft needed no override, which is evidence that 127.0.0.1 with ephemeral ports works in practice despite the doc's 'only localhost' wording (medium; https://bugzilla.mozilla.org/show_bug.cgi?id=2045029).
- Microsoft's desktop-app registration guidance says system-browser apps should register exactly http://localhost under Mobile and desktop (high; https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration).
- Ortie's docs warn that the Graph CLI client (14d82eec) can return personal accounts an opaque token that Graph rejects (InvalidAuthenticationToken). Another reason not to piggyback on other apps' IDs (medium; https://github.com/pimalaya/ortie).
- The Azure free subscription is disabled after 30 days or $200 unless upgraded. The Entra tenant itself remains, but the billing-based inactivity check (200+ days, then a 20-day block before deletion) still applies, so treat the tenant as a single point of failure (medium).
- Mailbird attributes Outlook.com's 'authenticated but not connected' error to too many concurrent IMAP sessions. If the IMAP fallback ships, limit it to 1–2 connections per account (low; https://nextsupport.getmailbird.com/hc/en-us/articles/38155660874007).
- io-msgraph 0.5.0 was published on 2026-10-09, the day of this check, so expect API churn. Pin versions or vendor the code (high; https://crates.io/crates/io-msgraph).
