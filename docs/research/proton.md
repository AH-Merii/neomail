# Research: proton

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

## Proton Mail programmatic access (checked 2026-10-09)

I didn't need a browser for this, so closing Chromium had no effect. I worked from Proton's docs, the GitHub, Codeberg, crates.io and Arch APIs, and by reading the Bridge, gluon and go-proton-api source. I briefly saved some source files to /tmp to search them, then deleted them. No project files were touched.

### 1. Proton Mail Bridge is still paid-only
- **Plans:** Bridge and IMAP/SMTP access are still paid-only. On the pricing table, the "Email client support (via IMAP/SMTP)" row is empty for Free. The "14-day free trial" listed for Free is for the desktop app, not Bridge. Free accounts get the error "Proton Mail Bridge is exclusive to our paid plans".
- **Releases:** The latest version is v3.27.1 (2026-09-29), with roughly monthly releases. FIDO2 support arrived in v3.22.0.
- **Linux packages:** DEB, RPM and PKGBUILD, x86_64 only. Arch `extra` has `protonmail-bridge-core` 3.27.1, a headless `/usr/bin/protonmail-bridge-core`. Bridge needs a keychain (Secret Service or `pass`).
- **CLI mode:**
  - `-c/--cli` gives an interactive shell. Its commands include `login`, `info`, `change imap-security|smtp-security|imap-port|smtp-port|mode`, `cert export|import` and `all-mail-visibility`.
  - `-n/--noninteractive` runs with no interface. It is documented only in BUILDS.md.
  - Typical setup: log in once with `--cli`, then run `-n` as a systemd service.
- **Ports and TLS:**
  - Bridge listens on 127.0.0.1: IMAP 1143, SMTP 1025. If a port is taken it uses the next free one.
  - STARTTLS is the default. You can switch to implicit TLS ("SSL").
  - The certificate is self-signed RSA-2048 with `CA:TRUE`, and 127.0.0.1 is its only subject alternative name.
  - rustls refuses this certificate (`CaUsedAsEndEntity`, see himalaya #633). A Rust client needs a custom verifier that pins the certificate's fingerprint, and must connect to 127.0.0.1, not `localhost`.
- **Bridge password:**
  - It is 16 random bytes per Proton account, and the IMAP/SMTP username is the email address.
  - In split mode each address is listed separately but uses the same password.
  - The password is reused if the account logs in again.
- **Mailbox layout:**
  - System mailboxes have SPECIAL-USE attributes: INBOX, Drafts, Sent, Starred, Archive, Spam, Trash, All Mail and Scheduled.
  - Proton folders appear under `Folders/…` and can be nested.
  - Proton labels appear as `Labels/<name>`. They are flat, so the name can't contain `/`.
  - You can't create mailboxes at the top level. `CREATE Labels/X` creates a Proton label with the fixed color #f66.
- **Label semantics (from the source code):**
  - `COPY` calls Proton's `LabelMessages`, so copying into `Labels/X` adds the label without moving the message.
  - `MOVE` from INBOX to `Labels/X` also only adds the label; the message stays in INBOX.
  - `EXPUNGE` inside `Labels/X` removes the label.
  - `EXPUNGE` in Trash or Drafts permanently deletes the message if it has no other labels.
- **Flags:**
  - PERMANENTFLAGS are `\Seen \Flagged \Deleted $Forwarded Forwarded`.
  - Only read status, `\Flagged` (which maps to Starred) and forwarded are written back to Proton.
  - Any other keyword is stored only in Bridge's local database and is not synced.
- **Capabilities:**
  - Supported: IMAP4rev1, IDLE, MOVE, UIDPLUS, UNSELECT, ID, AUTH=PLAIN and STARTTLS.
  - Not supported: CONDSTORE, QRESYNC and X-GM-EXT-1. X-GM-EXT-1 has open community pull requests (gluon #450, bridge #521), opt-in and not merged.
- **Useful details for neomail:**
  - Messages carry `X-Pm-Internal-Id`, a stable Proton message ID that stays the same across mailboxes and UIDVALIDITY resets.
  - The IMAP `ID` response name is "Proton Mail Bridge", which allows auto-detection.
  - The client's `ID` name and version are forwarded to Proton as the User-Agent.
  - If the client appends a message to Sent within about 90 seconds of sending it, Bridge recognises it and doesn't create a duplicate.

### 2. Options for free accounts
- **hydroxide:** I would not depend on it.
  - The GitHub repo was archived on 2026-08-02 and moved to Codeberg. The latest release is v0.2.32 (2026-05-10) and it is now "casually maintained".
  - IMAP is still a work in progress and only works without encryption. Labels show up as IMAP keywords, not folders. The default app version string is "Other".
  - Logins still fail with error 9001 (CAPTCHA required). Issue #328 has a report from 2026-10-07.
  - The maintainer will not merge automatic CAPTCHA solving. Forks that let the user solve the CAPTCHA exist but haven't been reviewed.
- **ProtonMail/go-proton-api:** Proton's official Go library (MIT), active as of 2026-10-09.
  - It covers only part of the API but supports human verification, FIDO2 and two-password mode.
  - It includes a fake API server for testing.
  - The README says Proton is "not actively looking for contributors". It is not a public API.
- **Rust:**
  - `proton-api-rs` was archived in 2024.
  - `proton-crypto-rs` is Proton's code (MIT), but its README says it is "not intended or vetted for general usage outside Proton". The crates.io copies (for example `proton-srp`) were published by a third-party account, "tirbofish".
  - `rust-mail` (AGPL-3.0) was archived on 2026-06-12 and moved to the ProtonMail/clients repo. It depends on Proton's private registry and on `muon`.
  - `muon` is Proton's Rust API client, version 4.1.0. It can be downloaded from rust-registry.proton.me, but it has no license and its README calls the registry internal.
- **Proton Mail Export Tool:** official and available on all plans, including Free. It exports EML plus JSON metadata and can restore those exports. Latest is v1.0.7 (2026-09-29). Exports can't be paused or resumed.
- **SMTP submission tokens:** paid plans with a custom domain only, and send-only (`smtp.protonmail.ch:587`).
- **Official API or OAuth for Mail:** none. The UserVoice requests have had no official response.

### 3. How authentication works for API clients
- **Password login (SRP-6a):**
  - The server uses a 2048-bit modulus signed by Proton, with bcrypt for password hashing.
  - Flow: `/auth/v4/info`, then `/auth/v4`, then the client checks the server's proof.
- **2FA:** `/auth/v4/2fa` accepts either a TOTP code or a FIDO2 assertion. A security key requires TOTP to be set up first. Bridge's CLI supports both.
- **Key unlock:**
  - The key passphrase is the last 31 bytes of bcrypt(password, key salt).
  - The salt comes from `/core/v4/keys/salts`.
  - In two-password mode the second (mailbox) password is used. Proton calls this mode legacy.
- **Human verification:**
  - Error 9001 returns a token and a list of methods.
  - The user completes the challenge at `https://verify.proton.me/?methods=…&token=…`.
  - The client then retries with the `x-pm-human-verification-token` and `x-pm-human-verification-token-type` headers. Bridge does exactly this.
- **Sessions:**
  - Requests send `x-pm-uid` plus `Authorization: Bearer`.
  - On a 401 the client calls `/auth/v4/refresh`, and the refresh token changes each time. Error 10013 means a full login is needed again.
- **App version header (`x-pm-appversion`):**
  - The header is required: error 5001 means it is missing and 5003 means the version is outdated or bad.
  - The format is `<platform>-<product>@semver`, for example `linux-bridge@3.27.1`.
  - The value "Other" is reported to trigger anti-bot checks, and muon deprecated it in 2.3.0.
  - Proton's only published rule for third parties is in the Drive SDK: `external-drive-{name}@…`, and no pretending to be an official client.
- **Terms of Service §2.10:** automated access is allowed only if it is indistinguishable from normal human client use, and must not try to get around security controls.

### 4. Recommendation
- **Paid accounts: Bridge only.**
  - neomail never sees the Proton password.
  - Use STARTTLS with a pinned certificate on 127.0.0.1, and keep the Bridge password in the OS keyring.
  - Store AI groups as `Labels/<name>`: `CREATE` the label, `UID COPY` to apply it, and expunge it from the label mailbox to remove it.
  - Never use IMAP keywords for groups, because they don't sync.
  - Treat a message's identity as `X-Pm-Internal-Id`.
- **Free accounts: don't build a native API client in v1.**
  - Offer importing a Proton Export Tool archive, read-only with groups kept local.
  - Optionally, let users connect their own hydroxide-style bridge through a generic IMAP profile marked experimental.
  - Proton Free also allows only 3 labels and 3 folders, so syncing AI groups as labels would barely work anyway.
- **CI testing:**
  - **Every PR:** run without network access. Use gluon's demo server for IMAP wire compatibility, plus a fake server that mimics Bridge's folder and label rules.
  - **Nightly:** build the real Bridge with `-tags build_qa` and point `BRIDGE_HOST_URL` at go-proton-api's fake server, so no real account or CAPTCHA is involved. Log in through the CLI using expect.
  - **Optional, manual:** test against a real paid account on a self-hosted runner, using a Bridge that a person has already logged in. Never do fresh logins from CI.

## Key facts

- **[high]** Proton Mail Bridge is still paid-only. The Bridge page says it is 'available only with a paid plan that includes Proton Mail', the pricing-page FAQ says Bridge is 'currently available for anyone with a paid Proton plan', and the 'Email client support (via IMAP/SMTP)' row is empty for Free. The 'Desktop app' row shows '14-day free trial' for Free, which is a different product. — <https://proton.me/mail/pricing>
- **[high]** Free accounts that try Bridge get: 'Proton Mail Bridge is exclusive to our paid plans. Upgrade your account to use Bridge.' — <https://proton.me/support/bridge-linux-login-error>
- **[high]** The latest stable Bridge is v3.27.1 (2026-09-29). 2026 releases: v3.22.0 (Jan 28, added FIDO2), 3.23.x (Mar), 3.24.x (Mar/Apr), 3.25.0 (Jun 1), 3.26.0 (Aug 17), 3.27.0 (Sep 9). — <https://github.com/ProtonMail/proton-bridge/releases>
- **[high]** Bridge on Linux ships as 64-bit DEB, RPM and PKGBUILD packages and is officially supported on the latest Ubuntu LTS and Fedora Workstation. ARM is unsupported apart from Apple silicon Macs. It needs a Secret Service password manager (GNOME Keyring etc.) or pass. — <https://proton.me/support/operating-systems-supported-bridge>
- **[high]** Arch Linux extra has protonmail-bridge-core 3.27.1-1 (updated 2026-10-08), a headless binary at /usr/bin/protonmail-bridge-core. It depends on libfido2 and libsecret, with gnome-keyring, Secret Service or pass as optional keychains. — <https://archlinux.org/packages/extra/x86_64/protonmail-bridge-core/>
- **[high]** The Bridge CLI starts with `protonmail-bridge -c` (--cli). `--noninteractive`/`-n` runs Bridge with no interface (you can't add accounts or read the bridge password) and still needs a keychain. The CLI has commands for login/info, imap-port/smtp-port, imap-security/smtp-security (STARTTLS or SSL), split/combined mode, cert export/import/install, all-mail-visibility, telemetry and repair. — <https://github.com/ProtonMail/proton-bridge/blob/master/BUILDS.md>
- **[high]** Default settings: IMAP on 1143 and SMTP on 1025, each moving to the next free port if taken, bound to 127.0.0.1, with SSL off (i.e. STARTTLS). Show All Mail is on by default. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/vault/types_settings.go>
- **[high]** Bridge's TLS certificate is self-signed RSA-2048 with IsCA=true, CN and IP SAN 127.0.0.1 only, valid for 20 years. rustls/webpki rejects it with CaUsedAsEndEntity even when it is added as a trusted root, as reported in himalaya issue #633 against Bridge 3.22.0. — <https://github.com/pimalaya/himalaya/issues/633>
- **[high]** The Bridge password is 16 random bytes per Proton account, and the IMAP/SMTP username is the email address. In split mode the CLI `info` command lists every address with the same bridge password. The password is archived and reused if the same primary email logs in again. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/frontend/cli/accounts.go>
- **[high]** Over IMAP, Proton folders appear under `Folders/` and labels under `Labels/`. Creating at the top level is refused. Labels can't have children. CREATE Labels/X creates a Proton label with color #f66. The hierarchy delimiter is '/'. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/connector.go>
- **[high]** IMAP COPY (AddMessagesToMailbox) calls Proton's LabelMessages, so copying into Labels/X adds the label without removing the message from INBOX. MOVE from INBOX to Labels/X also leaves the message in its source mailbox. Expunging from a Labels/ mailbox removes the label. Expunging from Trash or Drafts permanently deletes the message if it has no other labels. APPEND to All Mail and INBOX<->Sent moves are not allowed. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/connector.go>
- **[high]** Proton's documentation says moving a message into a label folder applies the label, moving it out of the label folder removes it, and you should copy to keep the label. Client tags such as Thunderbird's 'are not synchronized to Proton Mail servers' (page modified 2024-01-26). — <https://proton.me/support/labels-in-bridge>
- **[high]** Bridge mailbox FLAGS/PERMANENTFLAGS are \Seen \Flagged \Deleted $Forwarded Forwarded. Gluon sends only Seen, Flagged (which maps to Starred) and forwarded changes to Proton. Other keywords are stored only in Bridge's local gluon database. \Answered and \Draft are derived from Proton metadata. — <https://github.com/ProtonMail/gluon/blob/dev/internal/state/updates.go>
- **[high]** Gluon (Bridge's IMAP server) advertises IMAP4rev1, UNSELECT, UIDPLUS, MOVE, ID, IDLE (behind a feature-flag kill switch), AUTH=PLAIN and STARTTLS. CONDSTORE, QRESYNC and X-GM-EXT-1 are not supported. System mailboxes carry SPECIAL-USE attributes. — <https://github.com/ProtonMail/gluon/blob/dev/internal/session/session.go>
- **[medium]** Community pull requests that add X-GM-EXT-1 (X-GM-LABELS STORE/FETCH/SEARCH), opt-in via --enable-gmail-labels, were opened on 2026-02-22 and are still open and unmerged. — <https://github.com/ProtonMail/proton-bridge/pull/521>
- **[high]** Bridge adds headers X-Pm-Internal-Id (the Proton message ID), X-Pm-External-Id and X-Pm-Date, and appends <ID@protonmail.internalid> to References. Its IMAP ID response name is 'Proton Mail Bridge'. The client's IMAP ID name/version becomes the API User-Agent. — <https://github.com/ProtonMail/proton-bridge/blob/master/pkg/message/build.go>
- **[high]** The upstream hydroxide GitHub repo was archived on Aug 2, 2026 and moved to Codeberg. The latest tag is v0.2.32 (2026-05-10) and the last commit was 2026-08-03. It is now marked 'casually maintained', and its README still says IMAP is 'work-in-progress' and supports only unencrypted local connections. — <https://codeberg.org/emersion/hydroxide>
- **[high]** hydroxide logins still often fail with '[9001] For security reasons, please complete CAPTCHA' (issue #328, latest report 2026-10-07). Whether it happens appears to depend on the account. The maintainer said he would not merge automatic CAPTCHA solving but is open to showing the challenge to the user. hydroxide sends x-pm-appversion 'Other' by default, with an -app-version override. — <https://codeberg.org/emersion/hydroxide/issues/328>
- **[high]** hydroxide shows Proton folders (Exclusive=1) as mailboxes and labels as IMAP keywords, and can't create labels. This differs from Bridge's Labels/ folders. — <https://codeberg.org/emersion/hydroxide/src/branch/master/imap/user.go>
- **[high]** ProtonMail/go-proton-api is Proton's MIT Go client and fake server for a subset of the API, and is actively maintained (commits 2026-10-09). It supports HV-token login, TOTP/FIDO2 and two-password mode. Its default host is https://mail.proton.me/api and its default app version is 'go-proton-api'. The README says Proton is not actively looking for contributors. — <https://github.com/ProtonMail/go-proton-api>
- **[high]** proton-api-rs (unofficial) was archived on 2024-07-06; its last crate was 0.14.0 (2023-09-13). — <https://github.com/LeanderBB/proton-api-rs>
- **[high]** ProtonMail/proton-crypto-rs (MIT) holds Proton's Rust crypto and SRP crates. Its README says they are 'Not intended or vetted for general usage outside Proton' and it doesn't take contributions. The crates.io crates with these names (proton-srp, proton-crypto, proton-rpgp, gopenpgp-sys) were published by user 'tirbofish', not Proton, with a 'non-standard' license field. — <https://crates.io/crates/proton-srp>
- **[high]** ProtonMail/rust-mail (AGPL-3.0-only) was archived on 2026-06-12 and moved to ProtonMail/clients. It depends on crates from Proton's 'proton' registry (rust-registry.proton.me) and on muon from an internal GitLab. — <https://github.com/ProtonMail/rust-mail>
- **[high]** muon, Proton's Rust API client (latest 4.1.0), can be downloaded from https://rust-registry.proton.me but its Cargo.toml has no license field and it ships no LICENSE file. Its README says it is 'published to the internal Proton registry'. Changelog 2.3.0 (2026-04-15) deprecated AppVersion::Other. — <https://rust-registry.proton.me/index/mu/on/muon>
- **[high]** The Proton Mail Export Tool 'is available for everyone' (Free included). It exports EML and JSON, can restore its own exports, and exports can't be paused or resumed. Latest release v1.0.7 (2026-09-29), GPL-3.0. — <https://proton.me/support/proton-mail-export-tool>
- **[high]** SMTP submission tokens are send-only, for paid plans with custom-domain addresses: smtp.protonmail.ch:587, STARTTLS, PLAIN/LOGIN. — <https://proton.me/support/smtp-submission>
- **[medium]** Proton has no public Mail API or OAuth for third parties. The UserVoice 'Proton API' request (2015, 308 votes, latest comment 2026-07-12) and an OAuth request (2026-03-20) have no official status. The Spring/Summer 2026 roadmap announces a Lumo API and Drive work, but no Mail API. — <https://protonmail.uservoice.com/forums/945460-general-ideas/suggestions/7179569-proton-api>
- **[high]** API auth: SRP-6a with a 2048-bit Proton-signed modulus and bcrypt via /auth/v4/info and /auth/v4, then a ServerProof check. 2FA goes to /auth/v4/2fa (TOTP code or FIDO2 assertion). PasswordMode is 1 or 2. Key passphrase = last 31 bytes of bcrypt(password, KeySalt) from /core/v4/keys/salts. Sessions use x-pm-uid plus Bearer; the refresh token changes on every /auth/v4/refresh; code 10013 means log in again. — <https://github.com/ProtonMail/go-proton-api/blob/master/manager_auth.go>
- **[high]** Human verification: code 9001 returns HumanVerificationMethods and a HumanVerificationToken. Bridge prints https://verify.proton.me/?methods=…&token=…, waits for the user, then retries with the x-pm-human-verification-token and x-pm-human-verification-token-type headers. Code 12087 means an invalid HV token. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/hv/hv_default.go>
- **[high]** x-pm-appversion is required: code 5001 means missing, 5003 means bad or outdated. Bridge sends `<os>-bridge@<version>` (e.g. linux-bridge@3.27.1) to https://mail-api.proton.me. The only published third-party rule is the Drive SDK's `external-drive-{name}@{semver}-{channel}` and 'Do not spoof… masquerade as official Proton first-party clients'. — <https://github.com/ProtonDriveApps/sdk>
- **[medium]** Proton Terms §2.10 (page shows 'Last modified: October 09, 2026') forbid automated access that is distinguishable from standard human client behavior or that tries to get around security controls. Automation is allowed only if its traffic 'remains indistinguishable from the standard client behavior of human users'. — <https://proton.me/legal/terms>
- **[medium]** Two-password mode is a 'legacy feature', and passkeys work only as a second factor, not as a password replacement. FIDO2 keys require TOTP 2FA to be set up first. — <https://proton.me/support/switch-two-password-mode>
- **[high]** The Free plan allows 3 folders, 3 labels, 1 filter and 150 messages a day. Mail Plus and Unlimited have unlimited folders and labels. — <https://proton.me/mail/pricing>
- **[medium]** Bridge's API connection can be redirected for testing: builds with `-tags build_qa` read BRIDGE_HOST_URL and BRIDGE_API_HOST, and BRIDGE_API_SKIP_VERIFY skips SRP proof checks. go-proton-api ships a standalone fake server (server/cmd/server, gRPC CreateUser/CreateAddress/CreateLabel). Bridge's own integration tests run against this fake API. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/constants/host_qa.go>
- **[high]** Gluon has a demo IMAP server (`go run demo/demo.go`, 127.0.0.1:1143, demo users with password 'pass') that uses the same IMAP engine as Bridge. — <https://github.com/ProtonMail/gluon>

## Recommendations

- Support Proton only through the official Bridge (paid plans), as an IMAP/SMTP provider profile. Auto-detect Bridge from the IMAP ID response name 'Proton Mail Bridge' or the ports 1143/1025, send our own IMAP ID (name=neomail, version=X), and never ask for or store the Proton account password.
- Use a custom rustls ServerCertVerifier for Bridge that pins the exported certificate by SHA-256 fingerprint. Get the certificate from `protonmail-bridge -c` → `cert export`, from Settings → Export TLS certificates, or by trust-on-first-use with user confirmation. Connect to 127.0.0.1, support both STARTTLS (default) and implicit TLS, and refuse plaintext unless explicitly configured.
- Keep the Bridge password in the OS keyring (Secret Service via the keyring/oo7 crates) or a password_command, never in the TOML/SQLite config.
- Store AI groups as Proton labels: `CREATE "Labels/<name>"` (flat names, no '/'), `UID COPY <uids> "Labels/<name>"` to apply (it does not move), and in the label mailbox `UID STORE +FLAGS (\Deleted)` plus `UID EXPUNGE` to remove. Never use IMAP keywords for groups on Proton (they stay local to Bridge). Expect labels created via IMAP to get the default color #f66.
- In the SQLite cache, key Proton messages by the X-Pm-Internal-Id header so one row covers all the (mailbox, uid) places a message appears (INBOX, Labels/*, All Mail). Plan for full UID resyncs because Bridge has no CONDSTORE/QRESYNC; use IDLE for push.
- Use MOVE to Trash and MOVE to Archive for delete/archive rather than EXPUNGE from folders, and don't APPEND to Sent after sending through Bridge (Bridge already files the message and de-duplicates within about 90 seconds).
- Document a headless setup: install protonmail-bridge-core (Arch) or the official DEB/RPM, set up pass or gnome-keyring, run `--cli` once to log in and read `info`, then run `--noninteractive` as a systemd user service. Add `nmail doctor proton` to check ports, connection mode, certificate pin and login.
- For free accounts in v1, don't build a native Proton API client. Offer `nmail import proton-export <dir>` for the official Export Tool's EML+JSON (read-only, groups kept locally) and an experimental 'external bridge' IMAP profile for user-run hydroxide or forks (labels as keywords, plaintext, CAPTCHA may block login). Say clearly that this is unsupported.
- If a native API path is ever built, write it as a sidecar on the official go-proton-api rather than on unlicensed or third-party-republished Rust crates (muon, crates.io proton-srp copies). Use an honest app version (never copy an official client string), show human verification to the user via verify.proton.me (never solve it automatically), keep request rates human-like to stay within ToS §2.10, and ask Proton for guidance on third-party Mail clients first.
- CI on every PR, with no network: unit tests for the Proton quirks (mailbox-name→role/folder/label mapping, flag policy, label-name validation) and IMAP compatibility tests against gluon's demo server plus a fake server that mimics Bridge's Folders/ and Labels/ semantics.
- Nightly CI, still with no real account: build Bridge from source with `make build-nogui BUILD_TAGS=build_qa`, start go-proton-api's server/cmd/server, create users and labels over gRPC, start Bridge with BRIDGE_HOST_URL pointing at the fake API and a passphrase-less GPG `pass` store, log in through `--cli` using expect, then run neomail's IMAP/SMTP end-to-end tests.
- Optional manual or weekly real-account test: a dedicated Mail Plus test account on a self-hosted runner with a Bridge vault a person has already logged in. Never do fresh logins from cloud CI (CAPTCHA), and treat `--log-imap`/`--log-smtp` output as sensitive (it contains decrypted mail).

## Open questions

- I can't confirm whether Proton will ever publish an official Mail SDK or OAuth for third parties. ProtonMail/clients (the new home of rust-mail) is AGPL-3.0, and its dependencies come from Proton's registry. Licensing and distribution of muon and the 'proton' registry crates is undocumented.
- There is no official x-pm-appversion rule for third-party Mail clients; the only published one is the Drive SDK's 'external-drive-…'. Whether something like 'external-mail-neomail@x.y.z' would be accepted, or blocked or challenged, is unverified.
- Driving Bridge's `--cli` login non-interactively (expect) and running a build_qa Bridge against go-proton-api's standalone fake server are inferred from source code and Bridge's own tests. Neither is a documented third-party workflow and both need a hands-on proof of concept.
- COPY into a folder (Folders/X, Archive) behaving as a server-side move follows from Proton folders being exclusive, but I didn't test it. Likewise, whether EXPUNGE from INBOX without moving to Trash leaves the message only in All Mail needs testing.
- Whether community pull requests gluon #450 and proton-bridge #521 (X-GM-EXT-1, opt-in) will be merged is unknown; until they are, Labels/ COPY is the only way to label without moving.
- The proton.me Terms page showed 'Last modified: October 09, 2026' (today), which may be a page artifact. The exact §2.10 wording should be re-checked before relying on it.

## Fact-check (independent verification pass)

Overall reliability: **high**

- **unverifiable:** labels-in-bridge support page last modified 2024-01-26
  - Correction: The page shows no modified date. '2024-01-26' appears only in an embedded screenshot's filename. Proton's support sitemap gives lastmod 2026-07-02, which looks like a site-wide rebuild. Don't cite a modification date. Confidence: high that the date is unsupported. — <https://proton.me/support/sitemap.xml>
- **refuted:** System mailboxes have SPECIAL-USE attributes: INBOX, Drafts, Sent, Starred, Archive, Spam, Trash, All Mail and Scheduled.
  - Correction: Partly wrong. Only Trash (\Trash), Spam (\Junk), All Mail (\All), Archive (\Archive), Sent (\Sent), Drafts (\Drafts) and Starred (\Flagged) carry special-use attributes. INBOX (special by name) and Scheduled (renamed from the API's 'All Scheduled') have only \Noinferiors. Map Scheduled by name. Confidence: high. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/imap_updates.go>
- **outdated:** FIDO2 keys require TOTP 2FA to be set up first (a security key requires TOTP).
  - Correction: The current security-key page lists no TOTP prerequisite. It says 'If you have another 2FA method enabled, you'll need to verify with that too'. The 2FA overview says you 'can also have authenticator app 2FA and security key 2FA enabled at the same time'. The API has a FIDO2-only state (TwoFAStatus HasFIDO2, separate from HasFIDO2AndTOTP), and the Bridge CLI handles it. Passkey sign-in to Proton Accounts is still not supported; security keys work only as a second factor. Confidence: medium. — <https://proton.me/support/2fa-security-key>
- **refuted:** If the client appends a message to Sent within about 90 seconds of sending it, Bridge recognises it and doesn't create a duplicate (also in the recommendation: 'de-duplicates within about 90 seconds').
  - Correction: The window is wrong. The send recorder keeps message hashes for SendEntryExpiry = 30 minutes. On APPEND to any mailbox, Bridge matches the hash and recipients, and if that send is still in progress it waits up to 90 seconds for it to finish. A matching APPEND within about 30 minutes returns the server copy. The advice not to APPEND to Sent is still right. Confidence: high. — <https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/sendrecorder/recorder.go>

### Missed facts

- Bridge goes offline when Proton is unreachable. On a ConnStatusDown event it stops the IMAP listener and closes the SMTP server, and it restarts them when the connection returns. neomail will see 'connection refused' on the Bridge ports while offline, so it must read from its own SQLite cache and report 'Bridge offline'. Confidence: high. Source: https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapsmtpserver/service.go
- Bridge behaviour can change without a new release. Remote Unleash feature flags control the IDLE kill switch, which of two EXPUNGE/MOVE code paths is used (FolderUnlabelCallDisabled), FIDO2 login (u2fLoginEnabled) and more. Pinning a Bridge version in CI does not freeze behaviour. Confidence: high. Source: https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/connector.go
- Proton Mail now has its own automatic Categories, which overlap with neomail's AI groups. There are six preset categories (Primary, Social, Promotions, Newsletters, Transactions, Updates). They are sorted from metadata, available to all users on web and mobile, and were rolled out by the end of September 2026. Bridge does not appear to expose them: there is no category label type in go-proton-api, Bridge only shows a fixed list of system labels, and the changelog through 3.27.1 doesn't mention them. neomail can't read or set them over IMAP. Confidence: high that Categories exist, medium that Bridge doesn't expose them. Source: https://proton.me/support/mail-categories
- neomail's cache would weaken Proton's at-rest encryption. Bridge does a full initial sync into its own encrypted on-disk message store (gluon, passphrase-protected). A plain SQLite cache in neomail would be an extra unencrypted copy of the mail. For a security-focused client, encrypt the Proton cache with a key kept in the OS keyring, or cache metadata only. Confidence: high for the Bridge store. Source: https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapsmtpserver/imap.go
- Failed APPENDs land in a local-only 'Recovered Messages' folder. Common causes are a message that is too large, badly formatted or unparseable. The APPEND size limit is 55 MB since 3.22.0 (BRIDGE-409). Messages in that folder have no real Proton ID, so identity keyed on X-Pm-Internal-Id needs a fallback. Confidence: high. Source: https://proton.me/support/what-is-the-recovered-messages-folder-in-bridge
- Bridge's 3.27.1 rollout is staged. Proton's official stable channel lists 3.27.1 from 2026-10-08 at 25% rollout (EarlyAccess 2026-10-05), so many users are still on 3.27.0. Check compatibility against at least the last two versions. Confidence: high. Source: https://proton.me/download/bridge/linux/x86/v1/version.json
- Arch's package already includes a systemd user service. It ships protonmail-bridge.service (ExecStart=/usr/bin/protonmail-bridge-core --noninteractive, Restart=always), so the headless guide can just say 'systemctl --user enable --now protonmail-bridge' after logging in once with --cli. Confidence: high. Source: https://gitlab.archlinux.org/archlinux/packaging/packages/protonmail-bridge/-/blob/main/protonmail-bridge.service
- There is a ready-made Rust answer to the certificate problem. pimalaya-stream 0.1.1+ (MIT OR Apache-2.0) has a PinnedServerCertVerifier that pins Bridge's leaf certificate, which neomail can reuse or copy. Alternatively, Bridge's 'cert import' accepts a custom cert and key, so a normal leaf signed by a local CA would pass stock webpki verification. Confidence: high. Source: https://github.com/pimalaya/stream/blob/master/CHANGELOG.md
- Bridge does not require TLS before login. gluon accepts LOGIN and AUTHENTICATE PLAIN on loopback without STARTTLS (there is no LOGINDISABLED), so neomail must enforce TLS on its own side. Confidence: medium-high. Source: https://github.com/ProtonMail/gluon/blob/dev/internal/session/handle_login.go
- Bridge's IMAP MOVE does not follow RFC 6851, and All Mail and Scheduled are restricted. MOVE into Labels/* keeps the source copy. COPY, APPEND or MOVE into All Mail or Scheduled is refused. All Mail has no \Deleted flag. Code that assumes standard MOVE or EXPUNGE behaviour must special-case Bridge. Confidence: high. Source: https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/connector.go
- The Export Tool can be scripted and keeps label information. It runs non-interactively with flags or ET_* environment variables, and writes labels.json plus a per-message .metadata.json with LabelIDs, so 'nmail import proton-export' can rebuild folder and label membership. If neomail ran the tool with ET_USER_PASSWORD it would handle the Proton password, so let the user run the tool themselves. Confidence: high. Source: https://github.com/ProtonMail/proton-mail-export
- Proton's own CLI (Pass) shows the auth patterns Proton supports, but none of them exist for third-party Mail clients. pass-cli's default is a web login: the CLI prints a URL and polls a session fork until the user finishes in a browser. It also supports personal access tokens with a 2-hour lifetime. Hardware keys and SSO only work through web login. muon gained session-fork support in 2.5–2.6.2. Confidence: high. Source: https://protonpass.github.io/pass-cli/commands/login/
- Proton's Terms were genuinely updated today (2026-10-09). The change is in section 4 (service modification and discontinuation). The §2.10 automation clause is unchanged from the June 23, 2026 version. Confidence: high. Source: https://web.archive.org/web/20261003105539/https://proton.me/legal/terms
- The Drive SDK's third-party rules go further than the app-version header. Third-party use is limited to personal, non-commercial projects, with no Proton branding and a mandatory third-party disclosure when asking for credentials. Rate limits are the same as first-party clients, and non-compliant clients may be blocked. Follow these if neomail ever builds a native API path. Confidence: high. Source: https://github.com/ProtonDriveApps/sdk
- The Bridge greeting banner identifies Bridge before login. It reads '<FullAppName> <version> - gluon session ID n', so auto-detection works without sending ID. The ID vendor and support-url fields are literally 'TODO'. Confidence: high. Source: https://github.com/ProtonMail/gluon/blob/dev/internal/session/session.go
- Bridge can't move into a label without copying, and EXPUNGE from INBOX removes the INBOX label. In the default code path, EXPUNGE from INBOX or any folder unlabels the message in that mailbox; where it ends up afterwards is server-side and still untested. Use explicit MOVE to Archive or Trash, as the researcher recommended. Confidence: high for the code. Source: https://github.com/ProtonMail/proton-bridge/blob/master/internal/services/imapservice/connector.go
