# Research: rust-ecosystem

> Collected 2026-10-09 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

## Recommended stack for neomail
Versions are the latest stable releases on crates.io as of 2026-10-09. The current Rust stable is 1.99.0.

**Runtime and app**
- Core: tokio 1.53.2 · clap 4.6.7 + clap_complete 4.6.11 + clap_mangen 0.3.3 · serde 1.0.229 / serde_json 1.0.151.
- Config: toml 1.1.8 (TOML 1.1) · toml_edit 0.25.17, so the CLI and agents can edit config without losing comments · schemars 1.2.2, for JSON Schema of config, IPC and MCP tools · etcetera 0.11.0 for directories.
- Logging and watching: tracing 0.1.44 / tracing-subscriber 0.3.23 / tracing-appender 0.2.5, writing logs to a file · notify 8.2.0.
- Networking and errors: reqwest 0.13.5 (its default TLS is now rustls + aws-lc-rs + the platform verifier) · thiserror 2 / anyhow 1 · interprocess 2.4.4.
- Avoid figment: no release since 2024-05.

**TUI**
- Framework: ratatui 0.30.2, now split into ratatui-core, ratatui-widgets and ratatui-crossterm (MSRV 1.86). Use crossterm 0.29.0 with event-stream, through `ratatui::crossterm` so the two versions can't drift apart.
- Architecture: Elm-style or Flux. One `Command`/`Action` enum serves the keymaps, the `:` prompt, the CLI, IPC and MCP.
- Vim-style input: build your own keymap trie with crokey 1.5.0, or use modalkit 0.0.31 + modalkit-ratatui (the vim engine behind iamb; pre-1.0).
- Editing: hand off to `$EDITOR` first. For in-app editing use edtui 0.11.7 (vim modes; himalaya-tui uses it) or ratatui-textarea 0.9.3. tui-textarea 0.7.0 only supports ratatui 0.29.
- Other widgets: tui-input 0.15.5, tui-tree-widget 0.24.1, tui-scrollview 0.6.8, nucleo 0.5.0 (MPL-2.0), ratatui-image 11.1.0 (turn off the default `chafa-dyn` feature), tui-markdown 0.3.10, ansi-to-tui 8.0.1, tui-term 0.3.4. Use insta 1.49 with TestBackend for snapshot tests.

**Mail**
- Parsing and building: mail-parser 0.11.9 (fuzzed, MIRI-tested, zero-copy) + mail-builder 1.0.0.
- HTML: html2text 0.17.1 with the `css` feature gives rich annotations you map to ratatui styles. ammonia 4.2.1 and lol_html 3.0.1 sanitise HTML and strip remote images before it goes to a browser.
- **IMAP: async-imap 0.12.0.** It runs on tokio and supports IDLE (29-minute timeout), CONDSTORE SELECT, MOVE, UIDPLUS, ID, COMPRESS and Gmail labels. Delta Chat uses it in production. It has no first-class ENABLE, QRESYNC or NAMESPACE.
- The alternative is io-imap 0.7.1 from pimalaya. It is sans-I/O on top of imap-codec 2.0-alpha and covers CONDSTORE/QRESYNC, ENABLE, THREAD, XOAUTH2, OAUTHBEARER and SCRAM. It has had 10 releases since June 2026, so expect breaking changes.
- **SMTP:** lettre 0.11.23 (supports XOAUTH2; turn off its default native-tls) or mail-send 0.6.2.
- **Gmail:** use the REST API through a thin reqwest client, with io-gmail 0.5.0 as a reference.
- **Outlook.com:** use Microsoft Graph through a thin client. graph-rs-sdk 3.0.1 hasn't been released since 2025-04, and io-msgraph has no masterCategories endpoints.
- **Proton:** go through Bridge's IMAP and SMTP on 127.0.0.1:1143 and :1025.
- **Threading:** use each provider's thread IDs first and fall back to JWZ. mail-threading 0.1.3 does JWZ but returns flat membership only; otherwise write your own.

**Auth and security**
- OAuth: oauth2 5.0.0 supports PKCE and the device flow and uses typestates. It depends on reqwest 0.12, so disable its default features and implement `AsyncHttpClient` over reqwest 0.13. Use a 127.0.0.1 loopback redirect with PKCE S256.
- Secrets: keyring-core 1.0.0 plus the apple-native, windows-native, zbus-secret-service and linux-keyutils store crates, with a `password_command` fallback.
- Memory and TLS: secrecy 0.10.3 and zeroize 1.9.1 · rustls 0.23.45 + tokio-rustls 0.26.6 + rustls-platform-verifier 0.7.1, with one crypto provider for the whole build.
- Data at rest: rusqlite's `bundled-sqlcipher` (Delta Chat does this), and age 0.12.1 for exports.
- Untrusted text: strip CSI, OSC and DCS sequences, C0/C1 control characters and bidi overrides from every untrusted string, including in CLI output. ratatui only drops control characters on its normal text path.

**Storage**
- rusqlite 0.40.2 with `bundled`: SQLite 3.53.x with FTS5, JSON1 and RTREE compiled in. Pair it with rusqlite_migration 2.6.0 and one writer thread (or tokio-rusqlite 0.8.0), WAL mode, and FTS5 external-content tables with a trigram tokenizer.
- **Don't pull in sqlx 0.9.0 as well.** It needs libsqlite3-sys <0.38 while rusqlite needs ^0.38.2, and Cargo refuses two crates that both link `sqlite3`.
- Optional: tantivy 0.26.2 for search, sqlite-vec 0.1.9 for embeddings.

**Extensibility**
- MCP: rmcp 3.5.1, the official SDK, on spec 2026-07-28. It went from 1.0 to 3.0 between March and July 2026, so pin the version and wrap it.
- IPC: write your own JSON-RPC 2.0 over a Unix socket. jsonrpsee 0.26.1 has no IPC server.
- Scripting later: mlua 0.12.2 (Lua 5.5 or a sandboxed Luau). For WASM plugins, extism 1.41.0 (on wasmtime 49) or wasmi 2.0.0. rhai 1.26.1 is a sandboxed alternative.

## Provider facts that shape the design
- **Gmail IMAP** has CONDSTORE but not QRESYNC.
- **Outlook.com IMAP** has neither, plus no ENABLE and no custom keywords. Categories are only available through Graph `categories` and `/me/outlook/masterCategories`.
- **Proton Bridge** has IDLE, MOVE and UIDPLUS but not CONDSTORE. Labels appear as `Labels/<name>` folders, and you add one by COPYing a message there.
- QRESYNC is therefore no use for any day-one provider. The efficient paths are Gmail `history.list` (an HTTP 404 means do a full sync) and Graph's per-folder delta queries (deletions come back as `@removed`).
- Google's device flow doesn't allow Gmail scopes, and Google only accepts a loopback redirect. Gmail's read/modify scopes are Restricted. Microsoft's device code flow works with `/consumers`.

## Lessons from prior art
- **himalaya 2.2.1 and himalaya-tui:** v2 dropped built-in keyring and OAuth support, moving token handling to the `ortie` token-broker CLI and secret commands. It added Gmail REST and Graph backends.
- **pimdir / io-pimdir:** one SQLite file plus content-addressed body blobs; a three-way merge for each source; soft deletes with explicit purge; an action queue; a change feed; separate owner, producer and reader roles.
- **mxr:** this is the closest existing project to neomail. A daemon owns sync, storage, search and rules, and the TUI, CLI, web UI and MCP server are all clients over length-delimited JSON on a Unix socket. SQLite is the source of truth and the Tantivy index can be rebuilt from it. It has a fake provider with a conformance suite, plus dry-runs, permission profiles and send gates for agents.
- **aerc 0.22:** one ex-command language for both keybindings and scripting, and external filters (such as w3m) for HTML.
- **NeoMutt:** its header cache is keyed on UIDVALIDITY and UIDNEXT, and QRESYNC is marked experimental. The 2026-06-16 release fixed a path-traversal bug where mailbox names could escape the body cache directory.
- **Licences:** melib (EUPL/GPL) and the notmuch crate (GPL-3.0+) can't be reused in permissively licensed code.

## Key facts

- **[high]** ratatui 0.30.2 (2026-06-19) is the latest version. 0.30.0 (2025-12-26) split the crate into ratatui-core 0.1.2, ratatui-widgets 0.3.2, ratatui-crossterm 0.1.2, ratatui-termion, ratatui-termwiz and ratatui-macros. It added ratatui::run() and raised MSRV to 1.86. — <https://ratatui.rs/highlights/v030/>
- **[high]** crossterm 0.29.0 (2025-04-05) is still the latest release. ratatui-crossterm 0.1.2 supports crossterm 0.28 or 0.29 through optional features. termwiz 0.23.3 dates from 2025-03. — <https://crates.io/crates/crossterm>
- **[high]** tui-textarea 0.7.0 (2024-10-22) only supports ratatui 0.29. ratatui-textarea 0.9.3 (2026-10-05, ratatui org) is an independently maintained fork that targets ratatui-core 0.1. — <https://github.com/ratatui/ratatui-textarea>
- **[high]** edtui 0.11.7 (2026-08-16, MIT) is a vim-style editor widget with normal, insert and visual modes, an Emacs mode, syntax highlighting, mouse and clipboard support, and an optional system-editor handoff. It builds on ratatui-core 0.1 and ratatui-widgets 0.3. — <https://github.com/preiter93/edtui>
- **[high]** modalkit 0.0.31 and modalkit-ratatui 0.0.31 (2026-10-07, Apache-2.0) provide vim and Emacs editing environments, registers, history, command-bar parsing and UI components. They depend on ratatui 0.30, and iamb is the main user. — <https://docs.rs/modalkit/latest/modalkit/>
- **[high]** ratatui-image 11.1.0 (12.0.0-rc.2 is out) supports sixel, kitty, iTerm2 and halfblocks. Its default features include chafa-dyn, which links libchafa through pkg-config. mxr disables default features and limits image decoders. — <https://crates.io/crates/ratatui-image>
- **[high]** himalaya-tui is built on ratatui 0.30, edtui 0.11 (system-editor), tui-input 0.15, html2text 0.17, mail-parser 0.11 and io-imap. It shares one TOML config file with the himalaya CLI. — <https://github.com/pimalaya/himalaya-tui>
- **[high]** async-imap 0.12.0 (2026-09-30, MSRV 1.88) supports IDLE (Handle::wait uses a 29-minute timeout since 0.11.2), select_condstore, MOVE, UIDPLUS uid_expunge, ID, QUOTA, METADATA, COMPRESS and raw commands. 0.11.2 exposed Gmail labels and message IDs. — <https://github.com/async-email/async-imap/blob/main/CHANGELOG.md>
- **[high]** Delta Chat's chatmail core uses async-imap 0.12.0, async-smtp 0.10.2, mail-builder 1.0.0, mailparse, and rusqlite with SQLCipher. — <https://github.com/chatmail/core/blob/main/Cargo.toml>
- **[medium]** mxr forked async-imap as mxr-async-imap to add ENABLE and NAMESPACE support, so upstream async-imap lacks first-class ENABLE/QRESYNC. — <https://crates.io/crates/mxr-async-imap>
- **[high]** io-imap 0.7.1 (pimalaya, MIT/Apache) is sans-I/O coroutines with blocking light and full clients, built on imap-codec 2.0.0-alpha.8. It covers IDLE, ID, UNSELECT, UIDPLUS, SASL-IR, ENABLE, SORT/THREAD, MOVE, CONDSTORE/QRESYNC, XOAUTH2, OAUTHBEARER and SCRAM-SHA-256. It had 10 releases between 2026-06-03 and 2026-10-01. — <https://github.com/pimalaya/io-imap>
- **[high]** imap-codec's latest stable release is 1.0.0 (2023). 2.0.0-alpha.9 (2026-07-19) gates CONDSTORE/QRESYNC, ID, METADATA, NAMESPACE and UTF8 behind features and has quirk flags for misbehaving servers. imap-client has moved into io-imap. — <https://crates.io/crates/imap-codec>
- **[high]** io-replica was retired on 2026-09-03 and folded into io-pimdir, the reference implementation of the pimdir draft-04 store and sync engine. — <https://github.com/pimalaya/io-replica>
- **[high]** The pimdir design is one SQLite file plus a content-addressed blob directory, a three-way merge against each source, soft-delete retention, an action queue for non-owning processes, a change feed, and owner, producer and reader roles. — <https://github.com/pimalaya/pimdir>
- **[high]** himalaya 2.2.1 (2026-10-02, MIT/Apache) supports IMAP, SMTP, JMAP, the Gmail REST API, Microsoft Graph and Maildir. v2 removed native keyring support and ships no OAuth flow; tokens come from the ortie broker or secret commands. It documents Proton via Bridge at 127.0.0.1:1143/1025. — <https://github.com/pimalaya/himalaya>
- **[medium]** io-gmail 0.5.0 covers the full Gmail v1 API, including labels, history, batching and polling-based watch. io-msgraph 0.4.7 covers mail folders, messages and delta queries but does not list the outlook/masterCategories endpoints. — <https://github.com/pimalaya/io-msgraph>
- **[high]** mxr's architecture: a daemon owns sync, storage, search and rules, and the TUI, CLI, web UI and MCP server are clients using length-delimited JSON over a Unix socket. SQLite is the source of truth, Tantivy is rebuildable, and it ships a fake provider with a conformance suite. Its stack includes sqlx 0.9, tantivy 0.26, ratatui 0.30, rmcp and lettre. — <https://github.com/planetaryescape/mxr/blob/main/ARCHITECTURE.md>
- **[high]** mail-parser 0.11.9 (Apache/MIT) is zero-copy, supports 41 charsets natively (more with the full_encoding feature), and every function is fuzzed and tested with MIRI. mail-builder reached 1.0.0 on 2026-09-12. — <https://github.com/stalwartlabs/mail-parser>
- **[high]** lettre 0.11.23 supports the PLAIN, LOGIN and XOAUTH2 SMTP mechanisms. Its default features include native-tls; tokio1-rustls is opt-in. — <https://docs.rs/lettre/latest/lettre/transport/smtp/authentication/enum.Mechanism.html>
- **[medium]** mail-send 0.6.2 is tokio-only and uses rustls. It supports CRAM-MD5, DIGEST-MD5, XOAUTH2, LOGIN and PLAIN, plus DKIM signing; OAUTHBEARER is not listed. — <https://github.com/stalwartlabs/mail-send>
- **[high]** html2text 0.17.1 (MIT) can produce rich annotations with from_read_rich, and its css feature handles colours and display:none. It depends on html5ever 0.39 while ammonia 4.2.1 uses html5ever 0.40, so using both duplicates the parser. — <https://github.com/jugglerchris/rust-html2text>
- **[high]** mail-threading 0.1.3 (MIT/Apache, extracted from mxr) implements JWZ / RFC 5256 REFERENCES threading but returns flat thread membership, not a tree. — <https://github.com/planetaryescape/mail-threading>
- **[high]** oauth2 5.0.0 (2025-01) supports authorization code with PKCE, the device flow (RFC 8628), refresh, introspection and revocation, and uses typestate generics for endpoints. It depends on reqwest ^0.12 or ureq 2. — <https://github.com/ramosbugs/oauth2-rs/blob/main/UPGRADE.md>
- **[high]** reqwest 0.13 (0.13.0 released 2025-12-30, now 0.13.5) uses rustls with aws-lc-rs and rustls-platform-verifier as its default TLS. — <https://crates.io/crates/reqwest>
- **[high]** keyring 4.x moved the API into keyring-core 1.0.0, with stores in separate crates: apple-native, windows-native, dbus- or zbus-secret-service, and linux-keyutils. Applications set the default store explicitly at startup. — <https://github.com/open-source-cooperative/keyring-core>
- **[high]** Google's device authorization flow only allows openid/email/profile, two Drive scopes and YouTube scopes, so no Gmail scopes. Native apps must use a loopback redirect; OOB and custom URI schemes are no longer supported. — <https://developers.google.com/identity/protocols/oauth2/limited-input-device>
- **[high]** Gmail's https://mail.google.com/, gmail.modify, gmail.readonly and gmail.metadata scopes are Restricted, gmail.labels is non-sensitive, and gmail.send is sensitive. — <https://developers.google.com/workspace/gmail/api/auth/scopes>
- **[high]** Microsoft's IMAP OAuth scope is https://outlook.office.com/IMAP.AccessAsUser.All and its SMTP scope is SMTP.Send, both using SASL XOAUTH2. The device code flow supports the /common and /consumers tenants, so personal accounts work. — <https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth>
- **[medium]** Gmail IMAP supports CONDSTORE and X-GM-EXT-1 but not QRESYNC. — <https://bugzilla.mozilla.org/show_bug.cgi?id=1747311>
- **[medium]** outlook.office365.com IMAP advertises IDLE, MOVE, UIDPLUS, ID and UNSELECT, but not CONDSTORE, QRESYNC or ENABLE. Exchange does not support client-defined keywords, so categories can't be set over IMAP. — <https://learn.microsoft.com/en-us/openspecs/exchange_standards/ms-stanximap/a8c49281-d59a-4cb9-9b90-e091c1bc24d9>
- **[high]** Outlook categories go in the message's categories collection, matched by displayName against the master list at /me/outlook/masterCategories. The master list allows 25 colour presets and requires MailboxSettings.ReadWrite, which personal accounts can grant. — <https://learn.microsoft.com/en-us/graph/api/outlookuser-post-mastercategories?view=graph-rest-1.0>
- **[medium]** Proton Bridge (Gluon) exposes labels as folders under Labels/. COPYing a message into a label folder adds the label, and moving it out removes the label. Bridge 3.22 advertised IDLE, MOVE, UIDPLUS, UNSELECT and ID, without CONDSTORE. — <https://proton.me/support/labels-in-bridge>
- **[high]** Gmail partial sync uses history.list with startHistoryId. If that ID is outside the retained range, the API returns HTTP 404 and the client must do a full sync. History is typically kept for at least a week. — <https://developers.google.com/workspace/gmail/api/guides/sync>
- **[high]** Graph message delta queries work one folder at a time, use nextLink/deltaLink state tokens, and report deletions as @removed. — <https://learn.microsoft.com/en-us/graph/delta-query-messages>
- **[high]** rusqlite 0.40.2's bundled build compiles in FTS3, FTS5, JSON1 and RTREE and offers bundled-sqlcipher(-vendored-openssl). The master branch bundles SQLite 3.53.4. — <https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs>
- **[high]** sqlx-sqlite 0.9.0 requires libsqlite3-sys >=0.30.1,<0.38.0 while rusqlite 0.40.2 requires ^0.38.2. They can't share a build, and sqlx 0.9 also needs MSRV 1.94. — <https://crates.io/crates/sqlx-sqlite/0.9.0/dependencies>
- **[high]** rusqlite_migration 2.6.0, tokio-rusqlite 0.8.0 and deadpool-sqlite 0.14.0 all support rusqlite 0.40. — <https://crates.io/crates/rusqlite_migration>
- **[high]** rmcp 3.5.1 (2026-10-05, Apache-2.0) is the official MCP Rust SDK and implements spec 2026-07-28 while staying compatible with 2025-11-25. Major versions moved fast: 1.0.0 on 2026-03-03, 2.0.0 on 06-29, 3.0.0 on 07-28. — <https://github.com/modelcontextprotocol/rust-sdk>
- **[medium]** jsonrpsee 0.26.1 has servers for HTTP and WebSocket only, with no Unix-socket or IPC server transport. — <https://github.com/paritytech/jsonrpsee/issues/907>
- **[high]** mlua 0.12.2 supports Lua 5.5, 5.4 through 5.1, LuaJIT and Luau, with async and send features. Other scripting and plugin hosts: rhai 1.26.1, wasmtime 49.0.2 (MSRV 1.96), extism 1.41.0 (BSD-3), wasmi 2.0.0, rquickjs 0.14.0 and boa_engine 0.22.0. — <https://github.com/mlua-rs/mlua>
- **[medium]** Zellij moved its plugin runtime from wasmtime to the wasmi interpreter in v0.44.0 (March 2026) for smaller binaries and better portability. — <https://zellij.dev/news/remote-sessions-windows-cli/>
- **[medium]** ratatui filters control characters on its normal Span/Buffer path, but it doesn't strip whole escape sequences or bidi controls, and raw cell writes bypass the filter. Untrusted text has to be sanitised by the app. — <https://github.com/ratatui/ratatui/pull/1226>
- **[medium]** NeoMutt 2026-06-16 fixed a security bug where mailbox names could escape the body-cache directory. NeoMutt's header cache is reused only when UIDVALIDITY matches, and $imap_qresync is still marked experimental. — <https://docs.neomutt.org/reference/config/imap.html>
- **[medium]** aerc 0.22.0 was released on 2026-07-28 and added IMAP labels. 0.21 added read-only Gmail labels and :modify-labels for Gmail and Proton. — <https://git.sr.ht/~rjarry/aerc/blob/master/CHANGELOG.md>
- **[high]** meli/melib 0.8.13 is licensed EUPL-1.2 OR GPL-3.0-or-later, and the notmuch crate 0.8.1 is GPL-3.0+, so neither can be reused in a permissively licensed project. notmuch 0.40 was released on 2026-01-31. — <https://crates.io/crates/melib>
- **[high]** mujmap 0.2.0 (2022) syncs notmuch with JMAP. It stores state tokens, does a full resync when they expire, and lets local changes win conflicts. — <https://github.com/elizagamedev/mujmap>
- **[high]** figment's last release was 0.10.19 on 2024-05-17. config 0.15.27 is still active, and toml 1.1.8 implements TOML spec 1.1. — <https://crates.io/crates/figment>
- **[medium]** Proton has no official public Rust mail SDK. proton-core 0.1.2 is an unofficial community crate, and Proton itself publishes proton-crypto 0.14.1 and proton-srp 0.8.4. — <https://github.com/ProtonMail/proton-crypto-rs>

## Recommendations

- Use a daemon plus clients: an `neomail` daemon owns sync, SQLite, search, rules and AI, and the TUI, CLI, MCP server and scripts all talk to it over JSON-RPC 2.0 on a Unix socket (interprocess). One Command enum backs the keymaps, the `:` prompt, CLI subcommands and MCP tools.
- Build the TUI on ratatui 0.30.2 + crossterm 0.29 (EventStream) with Elm-style state. Write your own modal keymap trie (crokey for parsing keys from config), and evaluate modalkit before you adopt it. Make $EDITOR handoff the main compose path, with edtui 0.11.7 for inline editing.
- Use a native API for each provider. For Gmail, call the REST API with history.list for incremental sync and labels; IMAP with X-GM-EXT-1 is the fallback. For Outlook.com, use Graph delta queries plus categories and masterCategories, since IMAP can't store categories. For Proton, use Bridge IMAP/SMTP with Labels/ COPY semantics. Write thin reqwest/serde clients rather than heavy generated SDKs.
- Use async-imap 0.12.0 for the IMAP path (Proton Bridge and generic servers) behind your own trait. Plan for no CONDSTORE: run UID SEARCH ALL / UID FETCH FLAGS diffs, keep (UIDVALIDITY, UIDNEXT, HIGHESTMODSEQ) per mailbox, discard the cache when UIDVALIDITY changes, and run IDLE on a dedicated connection renewed every 29 minutes or less. Look at io-imap or imap-codec later for QRESYNC on generic servers.
- Store data in rusqlite 0.40.2 (bundled) with rusqlite_migration, one writer thread, WAL and FTS5. Model it on pimdir: content-addressed body blobs, per-source sync bases, soft deletes, and an intent queue for offline mutations. Avoid sqlx in the same build.
- Run OAuth with the oauth2 5.0 crate using a loopback 127.0.0.1 random port and PKCE S256 for Google and Microsoft. Use the device code flow only for Microsoft. Turn off oauth2's default features to avoid pulling in reqwest 0.12. Let users bring their own Google client ID, because Gmail scopes are Restricted.
- Keep secrets in keyring-core plus per-platform stores (zbus Secret Service on Linux, Keychain on macOS, Credential Manager on Windows), with a `*_command` fallback (pass, secret-tool, op), and hold them in secrecy/zeroize types. Consider SQLCipher (rusqlite bundled-sqlcipher) for the cache, with its key kept in the keyring.
- Treat all mail content as hostile. Strip ANSI, OSC and DCS sequences, C0/C1 control characters and bidi controls before rendering or CLI output. Never use server-supplied names as filesystem paths. Set image decode limits and restrict decoders. Block remote content by default, and use ammonia/lol_html before opening anything in a browser. In agent and MCP contexts, treat mail as data, never as instructions.
- Pick one rustls crypto provider for the whole build (aws-lc-rs, the reqwest 0.13 default, or ring) and use rustls-platform-verifier. Turn off native-tls defaults in lettre, ratatui-image and similar crates.
- Ship an rmcp 3.5.1 stdio server (`neomail mcp serve`) with the version pinned and wrapped behind your own types. Add dry-run, allowlists and send gates for agents, as mxr does. Leave the embedded scripting VM until later: start with CLI + JSON + shell hooks, then add mlua (sandboxed Luau) for configuration logic and possibly WASM via extism or wasmi for third-party plugins.
- Use toml + toml_edit (preserves comments when the CLI or agents edit config) plus schemars-generated JSON Schema for config and IPC. Use etcetera so paths follow XDG on Linux and macOS.
- Use provider thread IDs (Gmail threadId, Graph conversationId) with your own JWZ fallback that produces a tree, using mail-threading 0.1.3 as a reference or a conformance corpus.
- Test with insta snapshots of the ratatui TestBackend, a fake provider with a conformance suite (as in mxr), and integration tests against Dovecot/GreenMail and a Proton Bridge instance.

## Open questions

- Is QRESYNC/CONDSTORE support needed for generic IMAP accounts beyond the three launch providers? That decides between async-imap with raw commands and the more complete but fast-changing io-imap / imap-codec 2.0-alpha stack.
- For Gmail, should sync use the REST API (efficient history and native labels) or IMAP with X-GM-LABELS? Both need Restricted scopes. Will the project go through Google verification or ask users to bring their own OAuth client?
- For Outlook.com, should everything go through Graph (mail, categories, sendMail with MIME), or should IMAP/SMTP be used for mail with Graph only for categories?
- Proton: is relying on Bridge acceptable long term? Bridge has no CONDSTORE and represents labels as folders. The unofficial direct-API crate (proton-core) carries ToS and CAPTCHA risk.
- Should the SQLite cache be encrypted at rest with SQLCipher (more build complexity, needs OpenSSL or vendored crypto), or rely on OS disk encryption?
- Search: is FTS5 enough, or should a Tantivy index (as mxr uses) and sqlite-vec embeddings be added for AI grouping and semantic search?
- Embedded scripting language choice (Lua via mlua vs Rhai vs WASM), and whether the MCP server plus CLI already covers 'agent-driven customization' well enough.
- The Outlook IMAP and Proton Bridge CAPABILITY lists above come from third-party captures dated 2026-03 and 2026-02. Re-check them live after login before committing to a sync strategy.
