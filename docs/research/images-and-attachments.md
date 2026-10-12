# Research: images and attachments

> Collected 2026-10-09/10 by research agents for neomail's design. Facts carry a confidence
> level and a source; re-verify anything time-sensitive before relying on it.

Method: crate sources were downloaded from crates.io and read directly (ratatui-image 11.1.0,
image 0.25.10, mail-parser 0.11.9, mail-builder 1.0.0, sanitize-filename 0.6.0, open 5.4.4,
dirs 7.0.0, arboard 3.6.1, reqwest 0.13.5, async-imap 0.12.0). The RustSec advisory DB and
ProtonMail/gluon were cloned to /tmp. The Gmail discovery document was parsed for upload limits,
and Outlook's SMTP `SIZE` was checked live with `EHLO`. No project files other than this note were
touched.

## Summary

- **Rendering.** Use **ratatui-image 11.1.0** (ratatui `^0.30.1`, MSRV 1.86) with
  `default-features = false, features = ["crossterm"]`. Leave out `chafa-dyn` (it links libchafa)
  and `image-defaults` (it pulls every `image` format, including the `ravif`/rav1e AVIF *encoder*).
  - It covers kitty (unicode placeholders, so it works through tmux passthrough), Sixel, iTerm2
    and half-blocks.
  - It detects the protocol and the cell pixel size with one stdio query (2 s timeout).
  - Resize and encode run off the UI thread through `ThreadProtocol`.
  - 12.0.0-rc.2 is out. Pin 11.1 and move to 12 once it is stable.
- **Terminals.** Ghostty and kitty use kitty graphics. WezTerm and iTerm2 use iTerm2 inline images.
  foot and Windows Terminal ≥ 1.22 (WSL) use Sixel. Konsole is mixed. Alacritty and stock
  GNOME Terminal/VTE get **half-blocks** only. tmux needs `allow-passthrough` for kitty and iTerm2,
  or a `--enable-sixel` build for Sixel. zellij 0.45 re-implements kitty graphics but has no
  passthrough.
- **Decoding.** Enable only `png`, `jpeg` (zune-jpeg), `gif` and `webp` in
  `image` 0.25.10. png, gif and image-webp are `#![forbid(unsafe_code)]`. zune-jpeg is too, except
  for its SIMD paths.
  - Set `image::Limits` explicitly: the default is only `max_alloc = 512 MiB`, with no dimension
    caps. Add a byte cap before decoding and a wall-clock budget.
  - Decode on a blocking pool, never on the UI thread. Do not render SVG.
- **HTML images.** mail-parser puts every non-text part, including `cid:` images inside
  `multipart/related`, into `attachments()`. Related images come out as `PartType::Binary`, not
  `InlineBinary` (tested against 0.11.9; see 3.1), so its inline flag is useless for cid images.
  neomail builds a `Content-ID → part` map and treats a part as "inline" only if the HTML actually
  references it.
  - Remote images are blocked by default, in line with Thunderbird, aerc and neomutt.
  - Fetching is done by the daemon: no cookies, `referer(false)`, no auth, a size and time cap,
    and an **SSRF guard** that refuses loopback and private IPs, so a mail can't poke Bridge on
    127.0.0.1:1143. The guard must check IP-literal URLs too, because hyper skips the resolver
    for them (see 3.3).
  - Known trackers come from a vendored BSD-3 list plus heuristics.
- **Providers.**

  | Provider | Read | Send |
  |---|---|---|
  | Gmail | `attachments.get` returns base64url JSON; no ranges | ≤ 35 MiB request (36,700,160 B) on `messages.send` media upload; 25 MB user-facing |
  | Graph | `/$value` returns raw bytes | < 3 MB inline `POST`; 3–150 MB via `createUploadSession`; MIME `sendMail` ≈ 4 MB (unverified) |
  | Proton | Bridge serves `BODY.PEEK[n]<o.l>` from its local store (no `BINARY`) | 25 MB send / 50 MB receive |
  | SMTP | n/a | the server's `SIZE` (Gmail 35,882,577; Outlook 157,286,400) |

- **Saving.** Write a small in-house sanitiser. `sanitize-filename` does not strip bidi or
  zero-width characters, and it pulls in `regex`.
  - Sniff the type with `infer` 0.22.
  - Create files with mode 0o600/0o644, never executable.
  - On macOS, **set `com.apple.quarantine` ourselves**: CLI binaries without
    `LSFileQuarantineEnabled` do not get it automatically.
  - Use `dirs::download_dir()` (etcetera has no Downloads dir). Use `open` 5.4.4, which already
    goes through PowerShell on WSL.
- **Drafting.** Don't rely on mail-builder's `.inline()`: it puts inline parts into
  `multipart/mixed`. Build
  `mixed{ alternative{ text/plain, related{ text/html, img… } }, attachments… }` by hand with
  `MimePart`.

## 1. Terminal image rendering

### 1.1 ratatui-image

- **Versions.** 11.1.0 (2026-09-17) is the latest stable. 12.0.0-rc.0..rc.2 followed (rc.2 on
  2026-10-05: "remove fontsize fallback except for halfblocks"). It depends on
  `ratatui ^0.30.1` (default-features off), `image ^0.25.6` (png only), `icy_sixel ^0.5`,
  `base64-simd`, `flate2` and `self_cell`. MSRV is 1.86, which matches ratatui 0.30.
- **Features.** The default is `image-defaults + crossterm + chafa-dyn`.
  - `chafa-dyn` links libchafa through pkg-config at build *and* run time.
  - `chafa-static` embeds it.
  - To avoid libchafa, the README suggests `--no-default-features --features
    image-defaults,crossterm`. neomail goes further and also drops `image-defaults`, adding `image`
    itself with the formats it wants. Feature unification then gives ratatui-image exactly those
    decoders (plus anything another dependency enables; see arboard in 6.3).
- **Protocols** (`src/protocol/`):
  - **kitty:** transmits once, then places the image with *unicode placeholders* (U+10EEEE plus
    diacritics). The placeholders are ordinary text cells, so they survive tmux and redraws.
  - **Sixel:** encoded with icy_sixel. Immediate-mode: the app must not overwrite the cells.
  - **iTerm2:** base64 PNG.
  - **Halfblocks:** `▀`/`▄` with fg/bg colours. Works everywhere and assumes a 4:8 cell when the
    font size is unknown.
- **Detection** (`Picker::from_query_stdio`):
  1. Env hints first. `WEZTERM_EXECUTABLE` and `KONSOLE_VERSION` blacklist **both kitty and
     Sixel** (Konsole's Sixel is buggy, and neither implements kitty unicode placeholders
     correctly). Inside tmux, `ITERM_SESSION_ID` and `WEZTERM_EXECUTABLE` mean iTerm2; outside
     tmux, `TERM_PROGRAM` (iTerm, WezTerm, mintty, vscode, rio, …) or `LC_TERMINAL=iTerm` do.
  2. Then one stdio query: kitty `_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA`, an optional kitty compression
     probe, DA1 (`CSI c`, where parameter 4 means Sixel), `CSI 16 t` for the cell size in pixels,
     plus colour and cursor reports.
  3. The query times out after **2000 ms** (`STDIN_READ_TIMEOUT_MILLIS`).
  4. It must run *before* crossterm's `EventStream` starts reading stdin.
- **tmux side effect.** When `TERM` or `TERM_PROGRAM` says tmux, the picker runs
  `tmux set -p allow-passthrough on` by itself and wraps queries in `ESC Ptmux;`. neomail should
  make this opt-in, or at least document it: passthrough turns off tmux's escape filtering for that
  pane.
- **Widgets.**
  - `Image` (stateless): fixed cells; resized and encoded once when the `Protocol` is created.
  - `StatefulImage`: fits, crops or scales at render time, which is *blocking*. Pair it with
    `thread::ThreadProtocol`, which sends `ResizeRequest`s to a worker and swaps in the
    `ResizeResponse`. There is also a `tokio` feature.
  - The `sliced` example shows images partly scrolled out of view, which a pager-style message
    buffer needs.
- **Performance model.**
  - The expensive work is decode → resize → encode. Sixel encoding (palette quantisation) is the
    slowest; kitty/iTerm2 are a PNG/zlib plus base64 pass; half-blocks are cheapest.
  - After encoding, a render is cheap. kitty only re-emits placeholders; Sixel and iTerm2 re-send
    the image when the area changes.
  - neomail should cache the *encoded* protocol keyed by (content hash, cell size, protocol), and
    the decoded `DynamicImage` in an LRU bounded by bytes.
  - Known issue #57: a Sixel image on the last terminal line scrolls the screen.

### 1.2 Terminal support matrix

| Terminal | Protocol ratatui-image uses | Notes |
|---|---|---|
| Ghostty | kitty (unicode placeholders) | QA-tested upstream. No Sixel. |
| kitty ≥ 0.28 | kitty | Reference implementation. |
| WezTerm | iTerm2 | Also has Sixel and kitty, but upstream says only iTerm2 is bug-free. |
| iTerm2 (macOS) | iTerm2 | Sixel since 3.3.0 (decoding sandboxed since 3.3.8). |
| foot | Sixel | QA-tested. |
| Alacritty | none → half-blocks | Sixel only in an unmerged fork. |
| GNOME Terminal / Ptyxis (VTE) | none → half-blocks | The Sixel pref exists since 3.38, but distro VTE builds ship without `+SIXEL` (Fedora VTE 0.84.1 confirmed). |
| Konsole | none → half-blocks | Konsole has partial kitty and buggy Sixel, but ratatui-image's `KONSOLE_VERSION` check blacklists both, so it gets half-blocks. |
| Windows Terminal (WSL) | Sixel | Since 1.22.10352.0 stable (2025-02-06). Also answers `CSI 16 t`/`14 t`. |
| tmux | passthrough → outer terminal | kitty and iTerm2 need `allow-passthrough on` (≥ 3.3; `all` in 3.4). Sixel needs tmux ≥ 3.4 built with `--enable-sixel`. Native kitty support is PR #5445, work in progress. |
| zellij | Sixel (buggy); kitty since 0.45.0 | Re-emits placements; no passthrough, so unicode-placeholder users like yazi still fail. |
| Over SSH | whatever the *local* terminal supports | The query goes through the pty, so it works; bandwidth matters (prefer kitty with compression or half-blocks). |

## 2. Decoding untrusted images safely

- **Formats.** `image` 0.25.10's `default-formats` turns on avif, bmp, dds, exr, ff, gif, hdr, ico,
  jpeg, png, pnm, qoi, tga, tiff and webp, plus `rayon`.
  - The `avif` feature means *encoding* through `ravif` (rav1e). AVIF decoding needs `avif-native`
    (dav1d, C).
  - So use `default-features = false, features = ["png", "jpeg", "gif", "webp"]`. Mail images are
    almost all PNG, JPEG or GIF, with some WebP. Add `ico`/`bmp` only if needed.
  - Show TIFF, HEIC, AVIF and SVG as attachments, with no inline preview.
  - Feature unification can switch extra decoders on behind neomail's back (arboard turns on
    `image/tiff` on macOS). So also allowlist the *detected* `ImageFormat` before decoding,
    rather than trusting the feature list.
- **Memory safety.** `png` 0.18.1, `gif` 0.14.2, `image-webp` 0.2.4, `infer` and
  `mail-builder` are `#![forbid(unsafe_code)]`. `mail-parser` 0.11.9 contains no `unsafe`, but its
  `#[forbid(unsafe_code)]` is an outer attribute on `mod core` only, not crate-wide.
  - `zune-jpeg` 0.5.15 forbids unsafe *unless* its default `x86`/`neon` SIMD features are on.
    `image` depends on it with default features, so they are on: roughly 90 lines with `unsafe`.
  - Accept this (it is fuzzed) or patch the features off. Expect roughly a 2× slower JPEG decode
    without SIMD; this is not measured here.
  - The RustSec DB has only two old (2019/2020) advisories for `image` and none for png, gif or
    zune-jpeg.
- **Limits.** `image::Limits` has `max_image_width` and `max_image_height` (strict) and
  `max_alloc` (non-strict, "some decoders may ignore it"). The default is
  `{ None, None, Some(512 MiB) }`, so set all three. neomail defaults:
  - **≤ 16 MiB of encoded input** (checked before decoding);
  - **≤ 8192 × 8192 px**;
  - **`max_alloc` 128 MiB**.
  - Read the header with `ImageReader::with_guessed_format()?.into_dimensions()` first and reject
    oversized images without allocating.
  - GIF: decode the first frame only; animation is off by default.
- **Decompression bombs.** A 1 KB PNG can declare 100k × 100k px. The dimension limits catch this
  before allocation, and `max_alloc` covers the rest.
- **Time.** Decoding is synchronous and can't be cancelled in-thread. Use
  `tokio::task::spawn_blocking` (or a small dedicated pool) with at most 2 concurrent decodes.
  After a 2 s budget, *abandon* the result: the thread finishes but its output is dropped. Also
  wrap the decode in `catch_unwind`.
- **Out of process?** Not for v1. The chosen decoders are memory-safe Rust, the limits are strict,
  and a subprocess adds about 5–20 ms of spawn time per image. Keep a seam (`ImageDecoder` trait)
  so a later "paranoid" mode can run `neomail __decode` under `setrlimit(RLIMIT_AS, RLIMIT_CPU)` plus
  seccomp/landlock, returning RGBA over a pipe. That is the only way to truly kill a runaway
  decode.
- **SVG is not rendered.**
  - SVG is an XML document, not a bitmap. It can reference external URLs (`<image href>`, CSS
    `@import`, fonts), which defeats remote-content blocking, and it can embed script and
    animation.
  - ammonia has had four SVG/MathML XSS advisories, two of them in 2026 (RUSTSEC-2026-0193,
    MathML `annotation-xml`; RUSTSEC-2026-0213, SVG `animate`/`set`).
  - resvg 0.48.1 would add tiny-skia, usvg and fontdb, plus filter-driven CPU blowups.
  - Major clients barely support it: Gmail rasterises or drops it, and Outlook for Windows (Word
    engine) shows nothing.
  - Show SVG as an attachment ("SVG image, not rendered").

## 3. HTML mail images

### 3.1 `cid:` references and `multipart/related` with mail-parser 0.11.9

- **How parts are classified** (`src/parsers/message.rs`):
  - Every non-text leaf part is pushed to `message.attachments`. It becomes
    `PartType::InlineBinary` when it is "inline", otherwise `PartType::Binary`.
  - **Corrected by the fact-check.** Inside `multipart/related`, every part after the root is
    *not* inline: the condition is `state.parts == 1 || parent != MultipartRelated && …`. So cid
    images in `related` come out as `PartType::Binary`. An image/audio/video part sitting
    directly in `multipart/mixed` without `Content-Disposition: attachment` is `InlineBinary`,
    and its index is also added to `html_body` and `text_body` (Apple-Mail-style inline parts).
    Verified by parsing a sample message with 0.11.9.
  - `content_id()` already strips the `<…>` brackets.
  - `html_body`/`text_body` hold the indices of the chosen alternatives, which can include
    non-text inline parts, so the renderer must handle binary indices there.
  - Nested `message/rfc822` is parsed recursively up to `MAX_NESTED_ENCODED = 3`.
- **What neomail must do.**
  1. Walk `attachments()` and build `HashMap<ContentId, PartId>` from `content_id()` (brackets
     already stripped), and percent-decode the `cid:` URL (RFC 2392) before looking it up.
  2. While converting the HTML, rewrite `src="cid:X"` to an internal reference.
  3. A part is shown *inline* if it is referenced. It is listed as an *attachment* if it is
     unreferenced or has `Content-Disposition: attachment`.
  - Thunderbird and Apple Mail behave the same way: unreferenced related images still appear as
    attachments, so nothing is ever hidden.
- **Lazy sync.** mail-parser needs the whole message, but neomail's sync should store the
  *structure* without the bytes:
  - Gmail `format=full` gives `parts[].body.attachmentId` and `size`.
  - Graph gives `attachments?$select=id,name,contentType,size,isInline,contentId` (no
    `contentBytes`).
  - IMAP gives `BODYSTRUCTURE`.
  - Inline images under a small cap (for example 256 KiB each, 2 MiB per message) can be prefetched
    with the body. Everything else is fetched on demand.

### 3.2 Remote images and tracking pixels

- **What other clients do.**
  - **Thunderbird:** `mailnews.message_display.disable_remote_image = true` by default. A
    notification bar offers "Show remote content in this message" or "Allow remote content from
    <sender>", and the permission is stored per sender address.
  - **Apple Mail Privacy Protection:** goes the other way. It *preloads* all remote content in the
    background through two relays (the IP is hidden, and every mail looks "opened"). It does not
    cover links or attachments.
  - **aerc:** its stock `html` filter runs w3m with `-o use_cookie=false -o no_cache=true` inside
    `unshare --map-root-user --net`, so no network. It falls back to `socksify` with a dead SOCKS
    server, then to dead `http_proxy` settings. Named `html-unsafe`, it is allowed out.
  - **neomutt:** renders HTML through mailcap `w3m/lynx -dump`, which never fetches images.
  - **Gmail web:** proxies images through googleusercontent, which hides the IP but still
    confirms the open.
- **Tracker lists.** All the open lists are stale (last pushes 2021–2022):
  - leggett/simplify-trackers (**BSD-3-Clause**, regexes per service; used by Simplify,
    MailTrackerBlocker, Twobird);
  - OneClickLab/ugly-email-trackers (no licence);
  - leavemealone-app/email-trackers (GPL-3.0);
  - DHH's HEY gist (49 services).

  Domain-only blocking misses trackers on shared hosts (`list-manage.com/track`, `rs6.net/on.jsp`),
  so match full URL regexes. Vendor the BSD-3 list as data (`trackers.toml`), keep it
  user-extensible, and add **heuristics** that also catch unknown trackers:
  - `width`/`height` ≤ 2 or CSS `display:none`/`opacity:0` on an `<img>`;
  - a query string or path segment that looks like a per-recipient token (≥ 16 base64/hex chars);
  - an `<img>` whose host matches the sender's ESP click-tracking domain;
  - images placed after the footer/unsubscribe block.

  Anything flagged is *never* fetched, even when the sender is allowed.

### 3.3 A privacy-safe fetch design (daemon)

- **One dedicated `reqwest::Client`:**
  - no `cookies` feature or `cookie_store`;
  - **`.referer(false)`**: reqwest's default is `true`, which sets `Referer` on redirects;
  - a generic `User-Agent`;
  - `https_only(true)` by default, with plain HTTP opt-in per sender;
  - `redirect::Policy::limited(3)`, re-checking each hop against the SSRF rules;
  - `connect_timeout(3 s)`, `timeout(10 s)`;
  - no auth headers, no proxy credentials;
  - ETag/If-None-Match only against neomail's own cache (no conditional requests that link
    sessions).
- **Size and type caps.**
  - Reject `Content-Length` > 5 MiB and stop reading at 5 MiB when streaming.
  - Accept only `image/*`, and sniff the bytes with `infer` before decoding.
  - At most 20 images per message and 4 concurrent fetches.
- **SSRF guard.**
  - Resolve DNS through a custom `reqwest::dns::Resolve` that drops loopback, RFC 1918, link-local
    (169.254/16, fe80::/10), ULA (fc00::/7), CGNAT (100.64/10) and multicast addresses.
  - Without this, an `<img src="http://127.0.0.1:1143/…">` makes the daemon talk to Proton Bridge,
    and other images could hit the router or cloud metadata at 169.254.169.254.
  - Doing the check in the resolver also defeats DNS rebinding.
  - **The resolver is not enough on its own.** hyper-util's `HttpConnector` skips the resolver
    when the host is already an IP literal ("skip resolving the dns and start connecting right
    away"), so `<img src="https://127.0.0.1:1143/">` never reaches the custom `Resolve`. Also
    check `Url::host()` for `Host::Ipv4`/`Host::Ipv6` against the same deny-list, on the first
    request *and* in the redirect policy.
  - Call `.no_proxy()`: with a system proxy (`HTTP(S)_PROXY`) set, reqwest hands the target to
    the proxy and the local resolver never sees it.
- **Permissions.**
  - `remote_images = "block" | "ask" | "allow"` globally.
  - Per-sender and per-domain allow-lists in SQLite, keyed by the *authenticated* From:
    require DMARC/DKIM `pass` from the `Authentication-Results` header before honouring a sender
    allow-list entry, otherwise spoofed mail inherits trust.
  - Gmail/Graph spam verdicts override every allow-list.
- **Cache.**
  - Content-addressed under `$XDG_CACHE_HOME/neomail/remote/` (sha256 of the bytes), with the URL
    → hash map in SQLite, an LRU, a total budget (for example 200 MiB) and an expiry.
  - Remote images are fetched once per URL, never on every view. That limits repeat-open signals.
- **Browser view** (decisions: "open in a browser"):
  - Write a sanitised copy with lol_html/ammonia ≥ 4.1.4 (patched for RUSTSEC-2026-0213).
  - `cid:` parts are rewritten to `data:` URIs or sibling files.
  - Blocked `<img>`s point to nothing.
  - A strict CSP `<meta>` (`default-src 'none'; img-src data: file:; style-src 'unsafe-inline'`)
    blocks anything that slips through.
  - The file goes in a 0700 temp dir.

## 4. Provider attachment APIs and limits

| Provider / path | Download | Upload / send limit | Source |
|---|---|---|---|
| Gmail API | `GET …/messages/{id}/attachments/{attachmentId}` returns `MessagePartBody{attachmentId,size,data}`, with `data` as **base64url in JSON** and no Range support. Small parts arrive inline in `format=full` (no `attachmentId`). | `messages.send`, `drafts.create/update/send` media upload `maxSize` **36,700,160 B (35 MiB)**, `accept: message/*`, protocols simple and resumable. `messages.insert/import` 150 MiB. User-facing personal limit **25 MB** (larger files become Drive links in the web UI). | discovery doc; support.google.com/mail/answer/6584 |
| Gmail resumable upload | n/a | Session URI valid **1 week**. Chunks are multiples of **256 KiB**. Resume with an empty `PUT` plus `Content-Range: bytes */N` and read the `Range` header; 404/410 means restart. | developers.google.com/…/guides/uploads |
| Gmail quotas | `messages.attachments.get` costs **20 units** since May 2026 (was 5) | n/a | docs/research/google-gmail.md |
| Graph | `GET …/attachments/{id}/$value` returns **raw bytes**. `$select` without `contentBytes` gives metadata (`name, contentType, size, isInline`, plus `contentId` on fileAttachment). | `POST …/attachments` **< 3 MB**. `createUploadSession` for **3–150 MB**: PUT ranges in order, with **no Authorization header** (pre-authenticated `uploadUrl`, valid until `expirationDateTime`). ≤ 4 MB per PUT recommended. Resume from `nextExpectedRanges`. Final 201 returns `Location`. Below 3 MB the session fails with `ErrorAttachmentSizeShouldNotBeLessThanMinimumSize`. MIME `sendMail` ≈ 4 MB (not found in the sendMail or MIME docs; unverified). Throttle: 150 MB uploaded per 5 min per mailbox. | learn.microsoft.com/graph/outlook-large-attachments; docs/research/microsoft-outlook.md |
| Graph types | `fileAttachment` (bytes), `itemAttachment` (an embedded message/event/contact, ≤ 3 MB to add), `referenceAttachment` (a link to OneDrive/SharePoint; no bytes, `sourceUrl`) | Large-file links (OneDrive) would be a referenceAttachment; out of scope for v1 | learn.microsoft.com/graph/api/resources/attachment |
| Outlook.com limits | n/a | Web UI **25 MB** attachments (OneDrive links up to 2 GB). Outlook desktop 20 MB for internet accounts (not on the cited page; unverified). SMTP `SIZE 157286400` (150 MiB) on smtp-mail.outlook.com and smtp.office365.com (live `EHLO`, 2026-10-10). The mailbox policy, not `SIZE`, decides. | support.microsoft.com "Sending limits in Outlook.com" |
| IMAP (generic) | `UID FETCH n (BODYSTRUCTURE)`, then `BODY.PEEK[2.1]<0.1048576>` chunks (RFC 3501 partial). `BINARY.PEEK[n]` (RFC 3516) skips base64 when advertised; Gmail and Gluon don't advertise it. | APPEND to Sent; the server's `APPENDLIMIT` if advertised | RFC 3501 §6.4.5; RFC 3516 |
| Proton Bridge (Gluon) | Parses `BODY[section]<offset.count>` and serves it **from its local decrypted literal** (`getLiteral`, then slice); `BODYSTRUCTURE` comes from its DB without the literal. No `BINARY`. A partial fetch saves socket bytes, not Proton downloads. | Proton: **25 MB** outgoing, 50 MB incoming, 100 files. Bridge APPEND limit 55 MB (3.22+). | github.com/ProtonMail/gluon/…/mailbox_fetch.go; proton.me/support/attaching-multiple-documents-to-a-message |
| SMTP | n/a | Respect the EHLO `SIZE` (RFC 1870). Gmail SMTP `SIZE 35882577`. Base64 adds about 37% (4/3 plus CRLF every 76 chars), so a 25 MB cap holds about 18 MB of files. | google-gmail.md; RFC 1870 |

**Downloading lazily, resumably and with progress.**

- **The cache is content-addressed:** `$XDG_CACHE_HOME/neomail/att/<account>/<sha256>`. SQLite
  keeps `(account, message_id, part_id) → {name, declared_type, sniffed_type, size, sha256?,
  state}`.
  - Write to a `.part` file, then `fsync` and rename.
  - On restart, a `.part` file is resumed where the transport allows it.
- **Resumability per transport:**
  - IMAP: chunked `BODY.PEEK[sec]<offset.1MiB>`, so the next offset equals the `.part` length
    (encoded bytes; decode at the end or with a streaming base64 decoder).
  - Graph `$value`: try `Range: bytes=N-`. Whether Graph honours it is unverified; otherwise
    restart.
  - Gmail: no ranges, so restart. Stream the JSON body and base64url-decode incrementally to avoid
    holding twice the size in RAM.
- **Progress.** The expected size is `size` (Gmail/Graph) or the BODYSTRUCTURE octets × ~0.75.
  The daemon emits `Attachment.Progress{bytes, total}` events, which the TUI shows as virtual text
  on the attachment row and the CLI shows on stderr (or as NDJSON with `--json`).
- **Concurrency.** Use per-provider semaphores: Graph allows 4 concurrent requests per mailbox,
  and Gmail's quota units cost 20 per `attachments.get`. Cancel when the user cancels or the
  buffer is wiped.

## 5. Saving files safely

- **Filename sanitising.** `sanitize-filename` 0.6.0 removes `/ ? < > \ : * | "`, C0/C1
  controls, `.`/`..`, and Windows reserved names (only on Windows by default), and truncates to
  255 bytes. It does **not** handle:
  - bidi controls (U+202A–202E, U+2066–2069, U+200E/200F, U+061C), e.g. `invoice\u{202E}fdp.exe`
    shown as `invoiceexe.pdf`;
  - zero-width characters (U+200B–200D, U+FEFF) and other format-class (Cf) characters;
  - leading `-` (option injection when passed to `open`/`xdg-open`);
  - leading `.` (hidden files);
  - Unicode normalisation.

  It also pulls in `regex`. Write about 60 lines in-house:
  1. Decode RFC 2231/2047 (mail-parser already does this).
  2. NFC-normalise.
  3. Drop Cc, Cf, Zl and Zp characters plus the illegal set.
  4. Collapse whitespace.
  5. Strip leading `.`, `-` and spaces and trailing `.` and spaces.
  6. **Always** apply the Windows rules (reserved names `CON|PRN|AUX|NUL|COM\d|LPT\d`, trailing
     dot/space), because on WSL the target is often `/mnt/c/...`.
  7. Cap at 200 bytes on a char boundary while keeping the extension.
  8. If the result is empty, use `attachment-<n>.<ext from sniffed type>`.
  9. Never join a path without checking `file_name() == candidate`.
  10. Open with `O_CREAT|O_EXCL`; on a clash, add ` (1)`.
  11. Never follow symlinks at the target (`O_NOFOLLOW`).
- **Type sniffing.** `infer` 0.22.0 (MIT, `forbid(unsafe)`, magic numbers for images, archives,
  OOXML/ODF, PDF, ELF/Mach-O/PE executables, scripts with a shebang via text checks, and more).
  - Compare the result with the declared `Content-Type` and the extension (`mime_guess` 2.0.5 for
    extension → MIME).
  - On a mismatch, warn ("named .pdf, contains a Windows executable").
  - Classify as *dangerous*: executables, scripts (`#!`), `.desktop`, `.lnk`, `.app`/`.pkg`/`.dmg`,
    `.jar`, `.msi`, `.bat/.cmd/.ps1/.vbs/.js/.hta`, Office macro formats (`.docm/.xlsm`), `.iso`,
    and archives that contain any of these.
- **Never executable.** Create files with `OpenOptions::mode(0o644)` (or 0o600 under a
  private-mode config), never 0o755. Clear the executable bits even if the umask allows them.
- **macOS quarantine.**
  - A CLI binary has no `Info.plist`, so `LSFileQuarantineEnabled` does not apply. Files that
    `neomail` writes are **not** quarantined automatically; curl and wget behave the same, and
    malware abuses this.
  - neomail must set `com.apple.quarantine = "0081;<hex unix time>;neomail;<UUID>"` itself (the
    `xattr` 1.6.1 crate) on every saved attachment. Then Gatekeeper checks it on first open.
  - "Kept" (decisions) therefore means: never remove it, and copy it when re-saving.
- **Downloads directory.**
  - `dirs` 7.0.0 `download_dir()` reads `XDG_DOWNLOAD_DIR` from `user-dirs.dirs` on Linux and
    `~/Downloads` on macOS. etcetera 0.11 only covers config, data, cache, state and runtime, so
    keep etcetera for those and add `dirs` (or a 20-line `user-dirs.dirs` parser) for Downloads.
  - On **WSL**, default to the *Linux* `~/Downloads`, with a one-key option to use the Windows one.
    Resolve it with `powershell.exe -NoProfile -Command "(New-Object -ComObject
    Shell.Application).NameSpace('shell:Downloads').Self.Path"`, then `wslpath -u`.
  - Fall back to `$HOME`. Create the directory 0700 if it is missing.
- **Opening.** `open` 5.4.4:
  - Linux: tries `xdg-open`, `gio open`, `gnome-open`, then `kde-open --`. It passes the path
    through `option_safe_path`, which guards against a leading `-`. It never calls `wslview`.
  - WSL: first `powershell.exe -NoProfile -NonInteractive` with the target passed through
    `WSLENV`, then the Linux openers. It converts `/mnt/c/...` to `C:\...` and other paths to
    `\\wsl$\<distro>\...`.
  - macOS: `/usr/bin/open`.
  - Use `open::that_detached`.
  - The open action warns for dangerous types and always opens the *saved* copy, never a temp file
    under the cache, so quarantine applies.
  - Never auto-open. On WSL, opening a Linux path on the Windows side crosses into Windows'
    default handler, and the file carries no Mark-of-the-Web (Zone.Identifier), so SmartScreen probably doesn't
    check it (unverified). Warn louder there.

## 6. Drafting with files and images

### 6.1 MIME structure with mail-builder 1.0.0

- **`MessageBuilder` shortcuts are not enough.** `.inline(ct, cid, bytes)` adds a part with
  `Content-Disposition: inline` and a `Content-ID`, but puts it in the *same list as attachments*.
  `write_body_parts` then emits `multipart/mixed{ alternative{text, html}, inline…, attachment… }`.
  There is no `multipart/related`, so many clients (Outlook, Apple Mail) show the logo as a
  paperclip attachment and may not resolve the `cid:`.
- **Build the tree by hand** with `MessageBuilder::body(MimePart)`:

  ```text
  multipart/mixed                       (only if there are real attachments)
  ├── multipart/alternative
  │   ├── text/plain; charset=utf-8     (the Markdown source, see 6.2)
  │   └── multipart/related             (only if there are inline images)
  │       ├── text/html; charset=utf-8
  │       ├── image/png  Content-ID: <…>  Content-Disposition: inline; filename=logo.png
  │       └── …
  ├── application/pdf  Content-Disposition: attachment; filename*=UTF-8''…
  └── …
  ```

  This is the layout Thunderbird and Apple Mail produce. mail-builder writes RFC 2231
  `filename*=UTF-8''…` and continuation parameters itself, and its tests cover control characters
  in filenames.
- **Content-IDs.** Use `<{sha256(bytes)[..16]}@neomail.invalid>`, or the sending domain instead of
  `.invalid`. The same bytes get the same cid, so a signature logo used twice is sent once. The
  local hostname is never leaked.
- **Markdown images.**
  - `![alt](path)` with a local path (resolved relative to the draft and `~`, symlinks resolved,
    must be a regular file) is read, sniffed and limited (inline image ≤ 5 MiB, decodable by the
    same safe decoder), then rewritten in the HTML to `<img src="cid:…" alt="…">`. Add `width` and
    `height` from the decoded size, clamped to a configurable max display width such as 600 px.
  - An `https://` URL stays a remote `<img>`. Warn that most recipients block remote images.
  - `data:` URIs become cid parts. Gmail strips `data:` images.
- **Privacy on send.**
  - Offer (default on for pasted screenshots and camera JPEGs) to strip EXIF/GPS by re-encoding
    inline images with `image` (PNG/JPEG encoders).
  - Never touch regular attachments.
  - Warn if a local path would appear in either body part.

### 6.2 The plain-text alternative

- The decisions say the text part is the Markdown source. For images, rewrite `![alt](path)` to
  `[image: alt]`, or `[image: alt <https://…>]` for remote ones, so recipients don't see local
  filesystem paths such as `/home/alice/Clients/…`.
- The signature separator is `-- \n`. A signature logo becomes nothing in text/plain.
- With the "plain text only" toggle, inline images become ordinary attachments (or are dropped,
  after asking). There is no `multipart/related`.

### 6.3 Attaching: picker, paste, drop, clipboard

- **`:attach`** opens a file `PickerSource` (TUI-side). It walks from `$PWD` or `~` with the
  `ignore` crate, and the preview uses the same previewers as attachments. `:attach <path>` with
  completion also works.
- **Bracketed paste and drag-and-drop.**
  - Enable `crossterm::event::EnableBracketedPaste`. A drop arrives as `Event::Paste(String)`,
    because terminals turn a drop into pasted text (medium confidence; formats vary):
    - GNOME Terminal and VTE: single-quoted paths, space-separated;
    - kitty and foot: plain path or `file://` URI;
    - macOS Terminal and iTerm2: backslash-escaped spaces;
    - Windows Terminal: `C:\…`, so on WSL convert with `wslpath -u`.
  - In compose, if *every* token of a paste parses as an existing regular file (after `shlex`
    unquoting and `file://` decoding), offer "Attach 3 files? [Y/n]" instead of inserting text.
  - Never attach on paste without that confirmation, so pasting a path into prose stays possible.
- **Clipboard images.** Read only on an explicit key (`<C-v>` in a "paste image" mapping,
  `:attach clipboard`). Per platform:

  | Platform | Command | Notes |
  |---|---|---|
  | Wayland | `wl-paste --list-types`, then `wl-paste --no-newline --type image/png` | Needs wl-clipboard. arboard's `wayland-data-control` feature needs a compositor with (ext/wlr) data-control. |
  | X11 | `xclip -selection clipboard -t TARGETS -o`, then `-t image/png -o` | `xsel` has no image support. |
  | macOS | arboard 3.6.1 `get_image()` (NSPasteboard, TIFF, then RGBA; re-encode to PNG), or `osascript -e 'the clipboard as «class PNGf»'` | No extra binary needed with arboard. |
  | WSL | `powershell.exe -NoProfile -STA -Command "Add-Type -A System.Windows.Forms; $i=[Windows.Forms.Clipboard]::GetImage(); if($i){$i.Save($env:TEMP+'\\nm.png')}"`, then `wslpath -u` | `Get-Clipboard -Format Image` exists only in Windows PowerShell 5.1; PS 7 dropped `-Format`. Needs STA. |
  | SSH | not possible | OSC 52 is text-only and reads are usually disabled. Say so in the error. |

  - Sniff the result. If it is a raw bitmap, encode it to PNG and name it
    `pasted-<yyyymmdd-hhmmss>.png`.
  - Then ask whether it goes inline (with a cid in the Markdown at the cursor) or as an attachment.
  - Recommendation: shell out on Linux and WSL (no X11 or Wayland client libraries in the binary),
    and use arboard only on macOS, behind a cfg.

### 6.4 Forgotten attachments and size limits

- **Prior art.** Thunderbird's `mail.compose.attachment_reminder_keywords` is a *localized* list
  in `composeMsgs.properties` (en: "attachment, resume, cover letter, certificate, white paper,
  ecard, letterhead, enclosure"; de adds "Anhang" etc.).
  - It ignores quoted text in both HTML and plain-text compose (bugs 520076 and 495943).
  - The keyword language follows the UI locale, not the spellcheck language, which users
    complained about.
  - Wrapped phrases defeat it.
- **neomail.**
  - A `DiagnosticSource` in namespace `attach` scans only the *user-authored* text: it skips `>`
    quotes, forwarded blocks, the signature after `-- `, code spans and fences, and link URLs.
  - It works on normalised, unwrapped paragraphs, so wrapping doesn't defeat it.
  - Keyword sets per language follow the draft's spellcheck language(s): en, de, fr, es, it, pt,
    nl, ar, …. Ship them as TOML so users and Lua can extend them.
    - en: `attach(ed|ment)?`, `enclosed`, `see the file`, `\.pdf\b`, `CV`, `résumé`
    - de: `Anhang`, `angehängt`, `anbei`
    - fr: `pièce jointe`, `ci-joint`
    - es: `adjunto`
  - It shows a hint sign while typing. At send time, keywords with zero attachments trigger a
    blocking confirm, with "don't ask for this draft".
- **Size check at send.**
  - Encoded size = Σ(⌈n/3⌉·4 · 78/76) + headers + bodies.
  - Compare it with the identity's provider limit: Gmail 25 MB policy / 35 MiB API; Outlook.com
    25 MB; Proton 25 MB; SMTP `SIZE`.
  - Options:
    - warn at 80%;
    - block over 100% with choices (remove, shrink images, or for Graph-backed identities
      draft + `createUploadSession` for 3–150 MB files);
    - a later option to upload to the user's own cloud and link.
- **Large files on Graph.** Upload sessions apply to *draft attachments*. The flow is
  `POST /me/messages` (draft with the body), then a `createUploadSession` per file over 3 MB, a
  plain `POST /attachments` per file under 3 MB, then `POST /me/messages/{id}/send`. Gmail's
  resumable `messages.send` upload covers up to 35 MiB of raw message, with the upload session
  saved in SQLite so a crash can resume within a week.

### 6.5 Per-identity signatures with a logo

- The config holds `[identity.<name>] signature = "path/to/sig.md"`. The signature is Markdown, and
  `![Acme](logo.png)` is resolved relative to the signature file.
- The logo becomes a related inline part with a content-hash cid. Cap it (warn above 100 KB) and
  pre-scale it to 2× its display size.
- Replies and forwards must not re-attach logos found in *quoted* content. Only the user's own
  signature is added.
- Trade-off to document: Outlook and some mobile clients show cid logos as attachments, and remote
  logos are blocked by most privacy-respecting clients. Offer `logo_mode = "inline" | "remote" |
  "none"`, with inline as the default.

## 7. Proposed design on neomail's building blocks

### 7.1 Where things live

| Daemon (data, network, secrets) | TUI (pixels, latency) |
|---|---|
| `AttachmentStore`: structure from sync, on-demand fetch per provider, content-addressed cache, `.part` resume, progress events, a sniff + classify step (`infer` → `AttachmentKind{Image, Text, Pdf, Archive, Dangerous, Other}`) | `GraphicsCaps` from `Picker::from_query_stdio` at startup, before `EventStream`, overridable by `images.protocol = "auto" \| "kitty" \| "sixel" \| "iterm2" \| "halfblocks" \| "off"` |
| `RemoteImageFetcher`: the safe client from 3.3, tracker matcher, per-sender permissions (DMARC-gated), remote cache | `ImageService`: blocking pool (2 workers) doing `bytes → Limits-checked decode → DynamicImage`, then ratatui-image `ThreadProtocol` for resize and encode, with abandon-on-timeout |
| `PartRef{account, message, part}` → cache *path*. The TUI reads the file itself (same user, 0700 cache dir), so megabytes never cross the socket. | Two-level LRU: decoded RGBA (≤ 64 MiB) and encoded protocol keyed by `(sha256, cells, protocol)` (≤ 32 MiB) |
| Text extraction for search and agents: text/*, HTML → text, PDF → text (separate crate, see open questions), archive listing. Stored in FTS5 next to the message. | Previewers and decorations (below) |

### 7.2 Images in buffers: a block decoration

- **Reserve rows, then draw.** Extend the `Mark` from extensibility-building-blocks §5.4 with a
  block variant rather than inventing a widget:
  `virt_block: Option<VirtBlock { rows: u16, cols: u16, content: BlockContent::Image(ImageKey) }>`.
  - The text layout reserves `rows` empty lines under the anchor. This is Neovim's
    `virt_lines`, sized from the image's pixel size ÷ the cell size, clamped to
    `images.max_rows` (default 20) and the window width.
  - The compositor draws images **after** text, in a separate pass over the visible block marks.
  - kitty writes placeholder cells (cheap, survives scrolling). Sixel and iTerm2 re-emit when the
    block moves, and nothing must paint over those cells (ratatui-image's skip handling).
  - Partly visible blocks use the `sliced` approach. Fully off-screen blocks are not drawn, and
    kitty placements are deleted.
- **Who creates the marks.** The HTML→text step (html2text rich annotations) emits an
  `Image{src, alt, w, h}` annotation per `<img>`. A `DecorationProvider` in namespace `image` then
  turns each annotation into either:
  - a **block mark** when the image is allowed, decodable and images are on; or
  - a **one-line virt_text placeholder** otherwise, styled with highlight groups `NeomailImageBlocked`
    and `NeomailImageTracker`, for example
    `▣ logo.png 240×80 · remote blocked · gi load · gI always for acme.com`.

  Tracking pixels collapse to a single sign-column marker (`NeomailTracker`) with a hover that
  explains it.
- **Text-only fallback.** When `images = off`, on half-blocks with `images.halfblocks = false`, on
  a slow or SSH link, or under `MotionPref::Reduced`-style budgets, only placeholders are shown.
  Lua can replace the provider, so the same primitive serves extensions.
- **Composer preview.** The draft buffer runs the same provider over Markdown `![…](…)` spans, so
  images appear as block marks under the line. The paths are TUI-local, so no daemon round trip is
  needed. Remote URLs in drafts are never fetched.

### 7.3 Pickers, previewers, hover

- **`attachments` PickerSource** (daemon, live):
  - columns: name, sniffed type, size, sender, date, account;
  - scopes: message, thread, folder/group, account, all;
  - `find` runs FTS5 over name, type, sender and extracted text;
  - actions: save, save-all, open, preview, copy path, forward-with-attachment, jump to message.
- **`files` PickerSource** (TUI) for `:attach`. **`images`** is a filtered view of `attachments`
  (kind = Image) with a thumbnail previewer. Optional, but it falls out of the config-only picker
  composition for free.
- **Previewers** (`Previewer` trait, cancel-aware, sized to the preview window):
  - `image`: the same `ImageService`;
  - `text`: SafeText, first 2k lines, with charset detection;
  - `pdf_text`: extracted text plus page count;
  - `archive`: entry list, with dangerous entries flagged;
  - `dangerous`: shows *why* ("ELF executable named invoice.pdf"), never content;
  - `fallback`: the metadata card.
- **Hover** on an attachment row or an image placeholder shows the type mismatch warning, the
  real type, the size, the cid/URL with its tracker verdict, and the permission state.

### 7.4 Actions (one registry: TUI keys, CLI, MCP, Lua)

| Action | CLI | Notes |
|---|---|---|
| `attachment.list` | `neomail attachment list <msg\|query> --json` | Metadata only, including `sniffed_type`, `declared_type`, `mismatch`, `dangerous`, `inline` |
| `attachment.fetch` | `neomail attachment fetch <id…> [--progress ndjson]` | Downloads into the cache; returns the cache path |
| `attachment.save` | `neomail attachment save <id…\|--all> [--dir D] [--overwrite=never]` | Sanitised names, `O_EXCL`, quarantine xattr; prints final paths |
| `attachment.open` | `neomail attachment open <id> [--yes]` | Saves first, then opens. Dangerous kinds need `--yes` or an interactive confirm. |
| `attachment.text` | `neomail attachment cat <id> --text` | Extracted text wrapped as data: `{"provenance":"attachment","untrusted":true,"text":…}` |
| `image.load_remote` | `neomail image allow --message M \| --sender S \| --domain D` | Persists the permission; never overrides the tracker list |
| `image.toggle` | n/a (TUI) | Per buffer; `gi`/`gI` default keys |
| `draft.attach` | `neomail draft attach <draft> <path…\|--clipboard> [--inline]` | Same validation as the composer |
| `draft.detach` / `draft.inline` | `neomail draft detach …` | n/a |

- **MCP.**
  - Expose `attachment_list`, `attachment_text` and `attachment_save` (restricted to the
    configured downloads dir; the name always comes from the sanitiser, never from the agent).
  - Do **not** expose `attachment_open` or `image.load_remote` to agents by default: both cause
    side effects outside neomail (launching handlers, network beacons). Make them opt-in
    capabilities.
  - Attachment text is returned as data with provenance, per the decisions.
- **Events:** `Attachment.Progress`, `Attachment.Fetched`, `Attachment.Saved`, `Image.Blocked`,
  `Image.TrackerDetected`, `Draft.AttachmentAdded` and `Draft.SizeWarning`. Lua hooks can, for
  example, auto-save invoices from a sender to a folder, using the same save action and therefore
  the same sanitiser.

### 7.5 Security limits (defaults, all configurable within hard ceilings)

| Limit | Default | Hard ceiling |
|---|---|---|
| Encoded image bytes before decode | 16 MiB | 64 MiB |
| Image dimensions | 8192 × 8192 | 16384 × 16384 |
| `image::Limits::max_alloc` | 128 MiB | 512 MiB |
| Decode wall-clock budget | 2 s, then abandoned | 10 s |
| Concurrent decodes | 2 | cores / 2 |
| GIF / animation | first frame only | n/a |
| Formats decoded | png, jpeg, gif, webp | + bmp, ico (opt-in) |
| SVG, TIFF, HEIC, AVIF | never rendered; attachment only | n/a |
| Remote image fetch | blocked; per message, sender or domain allow | trackers are never fetched |
| Remote fetch size and time | 5 MiB; 3 s connect, 10 s total; ≤ 20 per message; 4 concurrent | 20 MiB |
| Remote fetch network | HTTPS only, no cookies or referrer, ≤ 3 redirects, SSRF filter | n/a |
| Inline image prefetch at sync | ≤ 256 KiB each, ≤ 2 MiB per message | n/a |
| Attachment cache | 1 GiB LRU, per account | n/a |
| Saved file mode | 0644 (0600 in private mode), never +x | n/a |
| Filename | NFC; no Cc, Cf or bidi; Windows rules; ≤ 200 bytes | n/a |
| Inline image in a draft | 5 MiB, EXIF stripped by default | n/a |

### 7.6 Crates

| Crate | Version (2026-10-10) | Use | Features / notes |
|---|---|---|---|
| ratatui-image | 11.1.0 (12.0.0-rc.2 out) | protocol detection, encoding, widgets | `default-features = false, features = ["crossterm"]`; no chafa |
| image | 0.25.10 | decode, resize, PNG/JPEG re-encode | `default-features = false, features = ["png", "jpeg", "gif", "webp"]`; no `rayon` needed |
| infer | 0.22.0 | magic-byte sniffing | `forbid(unsafe)` |
| mime_guess | 2.0.5 | extension → MIME for the mismatch check | stale (2024) but stable |
| mail-parser / mail-builder | 0.11.9 / 1.0.0 | parse; build the hand-made MIME tree | no `unsafe` in either; only mail-builder forbids it crate-wide |
| html2text | 0.17.1 | HTML → styled text plus image annotations | already chosen |
| lol_html / ammonia | 3.0.1 / 4.2.1 | sanitised browser copy | ammonia ≥ 4.1.4 for the 2026 advisories |
| reqwest | 0.13.5 | remote images, Graph/Gmail transfers | a separate client with `referer(false)`, a custom DNS resolver and no cookies |
| async-imap | 0.12.0 | `BODYSTRUCTURE` plus partial `BODY.PEEK` | `uid_fetch` with a raw query string; `Fetch::section()` |
| dirs | 7.0.0 | `download_dir()` | etcetera has no Downloads |
| open | 5.4.4 | default app | `that_detached`; WSL goes through PowerShell |
| xattr | 1.6.1 | macOS `com.apple.quarantine` | cfg(macos) |
| unicode-normalization | 0.1.25 | NFC for filenames | n/a |
| shlex | 2.0.1 | unquoting dropped paths | n/a |
| ignore | 0.4.33 | `files` picker walker | n/a |
| arboard | 3.6.1 | clipboard image on macOS only | `image-data` (default); on macOS it enables `image/tiff`, so allowlist formats at decode time; elsewhere shell out |
| zip | 9.0.0 | archive listing previewer | list only, never extract automatically |
| pdf-extract / lopdf | 0.12.1 / 0.45.0 | PDF text previewer and search | not evaluated; consider running it in the daemon with a time budget or out of process |

## Key facts

- **[high]** ratatui-image 11.1.0 depends on ratatui ^0.30.1 and image ^0.25.6 (png only), has MSRV 1.86, and defaults to `image-defaults + crossterm + chafa-dyn`. To avoid libchafa the README recommends `--no-default-features --features image-defaults,crossterm`; dropping `image-defaults` too is neomail's choice. — <https://github.com/ratatui/ratatui-image>
- **[high]** ratatui-image's picker reads env hints: WEZTERM_EXECUTABLE and KONSOLE_VERSION blacklist both kitty and Sixel; ITERM_SESSION_ID/WEZTERM_EXECUTABLE (in tmux) or TERM_PROGRAM/LC_TERMINAL (outside) suggest iTerm2. It then queries kitty support (`_Gi=31…a=q`), DA1 and `CSI 16 t` with a 2000 ms stdin timeout, and inside tmux it runs `tmux set -p allow-passthrough on` itself. — <https://github.com/ratatui/ratatui-image/blob/master/src/picker.rs>
- **[high]** ratatui-image renders kitty images with unicode placeholders (U+10EEEE). `StatefulImage` resizes and encodes at render time and is blocking, so the README recommends `ThreadProtocol` (thread or tokio). — <https://docs.rs/ratatui-image/latest/ratatui_image/>
- **[high]** The ratatui-image compatibility matrix: Xterm, foot and mlterm use Sixel; kitty (≥ 0.28) and Ghostty use kitty; WezTerm, iTerm2 and Rio use iTerm2; Alacritty, Konsole, Contour and Warp are marked not working. — <https://github.com/ratatui/ratatui-image#compatibility-matrix>
- **[high]** Windows Terminal supports Sixel since v1.22 (Preview 1.22.2362.0 in August 2024, stable 1.22.10352.0 on 2025-02-06), and answers `CSI 16 t`/`14 t` with pixel sizes. — <https://github.com/microsoft/terminal/releases/tag/v1.22.2362.0>
- **[high]** GNOME Terminal's "Enable Sixel" pref (3.38+) only works when VTE is built with `-Dsixel=true`; stable distro builds such as Fedora's VTE 0.84.1 lack `+SIXEL`. — <https://www.arewesixelyet.com/>
- **[medium]** tmux 3.4 added Sixel behind `--enable-sixel` and `allow-passthrough all`. kitty graphics in tmux need passthrough plus unicode placeholders. Native kitty support is an open PR (#5445). — <https://github.com/tmux/tmux/pull/5445>
- **[medium]** zellij 0.45.0 (August 2026) implements the kitty graphics protocol alongside Sixel, but has no passthrough mode (issue #4500). — <https://zellij.dev/news/nested-sessions-kitty-graphics-new-ui/>
- **[high]** iTerm2 has supported Sixel since 3.3.0, with sandboxed Sixel decoding since 3.3.8. — <https://iterm2.com/downloads.html>
- **[high]** `image` 0.25.10's `Limits::default()` is `{max_image_width: None, max_image_height: None, max_alloc: Some(512 MiB)}`. Width and height are strict limits; `max_alloc` is best-effort and some decoders ignore it. — <https://docs.rs/image/0.25.10/image/struct.Limits.html>
- **[high]** `image`'s `default-formats` include avif (the ravif/rav1e encoder), exr, tiff and others. AVIF *decoding* needs `avif-native` (dav1d). — <https://github.com/image-rs/image/blob/main/Cargo.toml>
- **[high]** png 0.18.1, gif 0.14.2, image-webp 0.2.4, infer 0.22.0 and mail-builder 1.0.0 are `#![forbid(unsafe_code)]`. mail-parser 0.11.9 has no `unsafe` but forbids it only on `mod core`. zune-jpeg 0.5.15 forbids unsafe only without its default x86/neon SIMD features (roughly 90 `unsafe` lines otherwise). — <https://crates.io/crates/zune-jpeg>
- **[high]** ammonia has had SVG/MathML XSS advisories RUSTSEC-2021-0074, RUSTSEC-2025-0071, RUSTSEC-2026-0193 (MathML `annotation-xml`, fixed in 4.1.3) and RUSTSEC-2026-0213 (SVG `animate`/`set`, fixed in 4.1.4). — <https://rustsec.org/packages/ammonia.html>
- **[high]** mail-parser 0.11.9 files every non-text part, including related inline images, under `attachments`. Parts after the root of a `multipart/related` are never inline, so cid images are `PartType::Binary`; images directly in `multipart/mixed` without `Content-Disposition: attachment` are `InlineBinary` and also listed in `html_body`/`text_body`. Nested messages are parsed to depth 3. (Corrected by the fact-check; verified by parsing a sample message.) — <https://github.com/stalwartlabs/mail-parser/blob/main/src/parsers/message.rs>
- **[high]** mail-builder 1.0.0's `MessageBuilder::inline()` puts inline parts next to attachments in `multipart/mixed` and never emits `multipart/related`. A custom tree goes through `.body(MimePart)`. — <https://github.com/stalwartlabs/mail-builder/blob/main/src/lib.rs>
- **[high]** Thunderbird blocks remote content by default (`mailnews.message_display.disable_remote_image = true`) and offers per-message and per-sender allowing. — <https://support.mozilla.org/en-US/kb/remote-content-in-messages>
- **[high]** Apple Mail Privacy Protection downloads remote content in the background, whether or not the mail is opened, through two relays that hide the IP. It does not cover links or attachments. — <https://www.apple.com/legal/privacy/data/en/mail-privacy-protection/>
- **[high]** aerc's stock `html` filter runs w3m without cookies or cache inside `unshare --map-root-user --net`, falling back to socksify or dead proxies; the `html-unsafe` name allows network. — <https://git.sr.ht/~rjarry/aerc/tree/master/item/filters/html>
- **[medium]** The public tracking-pixel lists are stale. leggett/simplify-trackers (BSD-3-Clause, regexes) was last pushed 2022-02; ugly-email-trackers has no licence; leavemealone-app/email-trackers is GPL-3.0. — <https://github.com/leggett/simplify-trackers>
- **[high]** hyper-util 0.1.21's `HttpConnector` skips DNS resolution when the URL host is an IP literal, so a custom `reqwest::dns::Resolve` cannot filter `https://127.0.0.1/…`; IP-literal hosts need their own check. — <https://github.com/hyperium/hyper-util/blob/master/src/client/legacy/connect/http.rs>
- **[high]** reqwest 0.13.5 `ClientBuilder::referer` defaults to `true`, so `Referer` is set on redirects; cookies need an opt-in feature or `cookie_store`. — <https://docs.rs/reqwest/0.13.5/reqwest/struct.ClientBuilder.html#method.referer>
- **[high]** The Gmail discovery doc gives `messages.send`, `drafts.create`, `drafts.update` and `drafts.send` a media upload `maxSize` of 36,700,160 bytes (simple and resumable, `message/*`); `messages.insert` and `messages.import` allow 157,286,400. — <https://gmail.googleapis.com/$discovery/rest?version=v1>
- **[high]** Gmail `messages.attachments.get` returns a `MessagePartBody` whose `data` is base64url; there is no documented range download. — <https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages.attachments/get>
- **[high]** Google resumable uploads use session URIs that expire after one week and chunks that are multiples of 256 KiB, resumed with `Content-Range: bytes */N`. — <https://developers.google.com/workspace/gmail/api/guides/uploads>
- **[high]** Personal Gmail accounts can send 25 MB of attachments per message; above that, the web UI converts them to Drive links. — <https://support.google.com/mail/answer/6584>
- **[high]** Graph attachments under 3 MB are added with a single POST. 3–150 MB files need `createUploadSession`, uploaded in order to a pre-authenticated `uploadUrl` with no Authorization header (≤ 4 MB per PUT recommended), resumed through `nextExpectedRanges`. `/$value` returns raw bytes. — <https://learn.microsoft.com/en-us/graph/outlook-large-attachments>
- **[high]** Graph attachment types are fileAttachment, itemAttachment and referenceAttachment. Adding an item attachment, or any attachment to a post, is limited to 3 MB. — <https://learn.microsoft.com/en-us/graph/api/resources/attachment>
- **[medium]** Outlook.com's web attachment limit is 25 MB (OneDrive links up to 2 GB). The claim that Outlook desktop defaults to 20 MB for internet accounts is not on this page [low]. smtp-mail.outlook.com and smtp.office365.com advertised `SIZE 157286400` in a live EHLO on 2026-10-10. — <https://support.microsoft.com/en-us/outlook/sending-limits-in-outlook-com>
- **[high]** Gluon (Proton Bridge) parses partial `BODY[section]<o.n>` fetches and serves them by slicing the full local literal. `BODYSTRUCTURE` comes from its DB, and `BINARY` is not supported. — <https://github.com/ProtonMail/gluon/blob/dev/internal/state/mailbox_fetch.go>
- **[high]** Proton Mail allows 25 MB of attachments on outgoing mail, 50 MB on incoming mail, and 100 files per message. — <https://proton.me/support/attaching-multiple-documents-to-a-message>
- **[high]** sanitize-filename 0.6.0 strips `/?<>\:*|"`, C0/C1 controls and dot-only names, applies Windows reserved names only on Windows by default, and truncates to 255 bytes. It does not strip bidi or zero-width characters. — <https://github.com/kardeiz/sanitize-filename/blob/master/src/lib.rs>
- **[high]** Non-`LSFileQuarantineEnabled` binaries such as curl and wget don't add `com.apple.quarantine` to the files they write. Its value is `flags;hex-time;agent;uuid`, for example `0081;…;Chrome;`. — <https://eclecticlight.co/2020/10/29/quarantine-and-the-quarantine-flag/>
- **[high]** `dirs` 7.0.0 `download_dir()` reads XDG user-dirs on Linux. etcetera 0.11.0 has no Downloads directory. — <https://docs.rs/dirs/7.0.0/dirs/fn.download_dir.html>
- **[high]** `open` 5.4.4 tries, on WSL, `powershell.exe` with the path mapped to `C:\…` or `\\wsl$\<distro>\…` first, then xdg-open, gio, gnome-open and kde-open, with a leading-dash guard. — <https://github.com/Byron/open-rs/blob/main/src/unix.rs>
- **[high]** `Get-Clipboard -Format Image` exists only in Windows PowerShell 5.1. PowerShell 7 reads only text, so the workaround is `[System.Windows.Forms.Clipboard]::GetImage()`, which needs STA. — <https://github.com/PowerShell/PowerShell/issues/12290>
- **[high]** Thunderbird's attachment-reminder keywords are a localized pref (`composeMsgs.properties`), and quoted text is ignored in HTML and plain-text compose. — <https://bugzilla.mozilla.org/show_bug.cgi?id=520076>
- **[medium]** caniemail shows inline `<svg>` unsupported in Gmail, Outlook and Yahoo, and linked SVG only partly supported in Gmail (rasterised, non-Google accounts only). — <https://www.caniemail.com/features/image-svg/>

## Recommendations

1. **Rendering stack.**
   - Use ratatui-image 11.1 (no chafa, no image defaults) plus `image` with png, jpeg, gif and
     webp only.
   - Run detection once at startup, before `EventStream`, and store a `GraphicsCaps`. Don't let
     the picker flip tmux options silently: set `allow-passthrough` only when
     `images.tmux_passthrough = true`, and otherwise use half-blocks inside tmux.
   - Images default **on** (per the provisional decision) for local cid images, with half-blocks
     wherever graphics are missing. Over SSH, default to half-blocks unless
     `images.protocol` is set.
2. **Decode safely in-process for v1.** Use strict `Limits`, a pre-decode byte and dimension
   check, a 2-worker blocking pool, abandon-on-timeout and `catch_unwind`, behind an
   `ImageDecoder` trait so an out-of-process sandbox can be added later. Never render SVG.
3. **Inline images = referenced cids.** Show referenced cid parts inline. List everything else,
   including unreferenced related parts, as attachments. Prefetch only small inline images during
   sync.
4. **Remote images** are blocked by default. The daemon fetches them with the hardened client:
   HTTPS, no cookies or referrer, no proxy, an SSRF-filtering resolver *plus* an IP-literal host
   check on every request and redirect, size and time caps, a content-addressed cache. Use per-message, sender and domain permissions gated on DMARC/DKIM
   pass. Vendor simplify-trackers (BSD-3) as data, add heuristics, and never fetch trackers.
5. **Attachments.**
   - Sync stores structure only.
   - Fetches are on demand per provider: IMAP chunked partial fetch (resumable), Graph `$value`,
     Gmail `attachments.get` streamed and base64url-decoded.
   - Downloads go to a `.part` file and are renamed when complete, with progress events.
   - Sniff with `infer`, warn on mismatches, and classify dangerous kinds.
6. **Saving.**
   - An in-house sanitiser (NFC; strip Cc, Cf and bidi; Windows rules always; length cap; never
     an empty name).
   - `O_EXCL | O_NOFOLLOW`, mode 0644 (never +x).
   - Explicitly set `com.apple.quarantine` on macOS.
   - `dirs::download_dir()`, with WSL's Windows Downloads as an opt-in.
   - `open::that_detached` only after an explicit action, plus a confirm for dangerous kinds.
7. **Drafting.**
   - Hand-build `mixed{alternative{text, related{html, imgs}}, attachments}` with mail-builder
     `MimePart` and content-hash cids.
   - Rewrite Markdown images to `[image: alt]` in text/plain.
   - Strip EXIF on inline images by default.
   - Paste: bracketed-paste file detection with a confirm. Clipboard images through wl-paste,
     xclip, PowerShell (WSL) or arboard (macOS).
   - Pre-send checks: the localized forgotten-attachment `DiagnosticSource` and per-provider
     encoded-size limits.
   - Large files: Graph upload sessions (3–150 MB), and the Gmail resumable `messages.send` up to
     35 MiB.
8. **Building blocks.**
   - Images in buffers are a `virt_block` mark drawn in a compositor image pass, with one-line
     placeholders when off or blocked.
   - Attachments, files and images are `PickerSource`s. Image, text, PDF, archive and dangerous
     views are `Previewer`s.
   - All operations are registry actions shared by keys, CLI (`neomail attachment …`, `neomail image
     allow …`, `neomail draft attach …`) and MCP. Agents get list, text and save, but not open or
     remote-load, by default.

## Open questions

- Should images be **on by default** over SSH? Sixel and kitty payloads on slow links can stall
  redraws. The proposal is half-blocks over SSH unless configured; the owner should confirm.
- Should neomail ever auto-enable tmux `allow-passthrough`? It weakens tmux's escape filtering for
  the pane. Proposed: opt-in only.
- PDF text extraction: pdf-extract 0.12.1 and lopdf 0.45.0 were not evaluated for robustness
  against hostile PDFs (deep recursion, huge streams). Run them in-process with a budget, out of
  process, or by shelling out to `pdftotext` when present?
- Does Graph's `/attachments/{id}/$value` honour HTTP `Range`? This was not verified. It decides
  whether Outlook downloads can be resumed.
- Should sender allow-lists require DMARC/DKIM pass? That is safer, but some legitimate small
  senders fail it, which would confuse users ("I allowed this sender and it still blocks").
- How should the WSL Downloads default work: Linux `~/Downloads` (safer, invisible in Explorer) or
  the Windows Downloads folder (what users expect, but it crosses into Windows' handlers without a
  Mark-of-the-Web)? Should neomail write a `Zone.Identifier` alternate stream through PowerShell
  when saving to `/mnt/c`?
- Logo mode for signatures: inline cid (shows as a paperclip in Outlook) or remote (blocked by
  privacy-respecting clients)? The proposed default is inline.
- Should zune-jpeg's SIMD (unsafe) be kept for speed, or turned off for a fully `forbid(unsafe)`
  decode path? Benchmark on typical newsletter JPEGs before deciding.
- Should neomail offer an optional, Apple-style "prefetch all remote images through a proxy" mode
  for users who want images without revealing open times? It needs a relay that neomail would not
  operate, so probably never.

## Fact-check (independent)

Checked on 2026-10-10 against crates.io (API and `.crate` sources), the RustSec advisory DB
(cloned 2026-10-09), ProtonMail/gluon (dev, 2026-10-09), the Gmail discovery document, Microsoft
Learn, Google and Proton support pages, GitHub release and PR metadata, tmux `CHANGES`, iTerm2
release notes, Fedora's `vte291.spec`, and live SMTP `EHLO`. Corrections are already made in place
above.

**Confirmed.**

- All crate versions in 7.6 are the latest stable on crates.io (ratatui-image 11.1.0 with
  12.0.0-rc.2 out; zune-jpeg 0.5.16-rc2 and sanitize-filename 0.7.0-beta are pre-releases only).
- ratatui-image 11.1.0: `ratatui ^0.30.1`, `image ^0.25.6` (png only), MSRV 1.86, default
  features, the chafa build script, `STDIN_READ_TIMEOUT_MILLIS = 2000`, the kitty query string,
  `CSI 16 t`, the `tmux set -p allow-passthrough on` side effect, the U+10EEEE placeholder,
  `ThreadProtocol`/`ResizeRequest`/`ResizeResponse`, the `tokio` feature, the `sliced` example,
  known issue #57, and the README compatibility matrix.
- `image` 0.25.10: `default-formats` list, `avif` = ravif encoder, `avif-native` = dav1d, and
  `Limits::default()` = `{None, None, Some(512 MiB)}`.
- png, gif, image-webp, infer and mail-builder are `#![forbid(unsafe_code)]`; zune-jpeg's forbid
  is conditional on SIMD being off, and `image` keeps its defaults on.
- mail-builder 1.0.0: `.inline()` parts land in `multipart/mixed` and nothing emits
  `multipart/related`; RFC 2231 `filename*` and continuations are written, with a control-char
  test. mail-parser nesting depth 3.
- reqwest 0.13.5 `referer` defaults to `true`. async-imap `Fetch::section()` ignores the partial
  origin, so partial fetches are readable.
- Gmail: `messages.send` and `drafts.*` media `maxSize` 36,700,160 B, `insert`/`import`
  157,286,400 B, `message/*`, simple and resumable; `MessagePartBody.data` is base64url; resumable
  URIs expire after one week, chunks are multiples of 256 KB, 404/410 means restart;
  `messages.attachments.get` costs 20 units (May 2026 change); personal limit 25 MB with Drive
  links above it.
- Graph: < 3 MB single POST, 3–150 MB upload session, no `Authorization` on the `PUT`, < 4 MB per
  range for performance, in-order upload, `nextExpectedRanges`, final 201 with `Location`,
  `ErrorAttachmentSizeShouldNotBeLessThanMinimumSize`, `$value` raw bytes; the 3 MB cap for item
  attachments; Outlook per-mailbox limits (10,000 per 10 min, 4 concurrent, 150 MB upload per
  5 min).
- Live `EHLO` (2026-10-10): smtp.gmail.com `SIZE 35882577`; smtp-mail.outlook.com and
  smtp.office365.com `SIZE 157286400`.
- Proton: 25 MB outgoing, 50 MB incoming, 100 files. Bridge v3.22.0 (2026-01-28) release notes:
  "BRIDGE-409: Increased the import size limit to 55MB". Gluon serves partial fetches by slicing
  `getLiteral`, and its capability list (IMAP4rev1, STARTTLS, IDLE, UNSELECT, UIDPLUS, MOVE, ID,
  AUTH=PLAIN) has no `BINARY` and no `APPENDLIMIT`.
- Terminals: Windows Terminal v1.22.2362.0 preview (2024-08-27) and v1.22.10352.0 stable
  (2025-02-06) both mention Sixel. tmux 3.3 added `allow-passthrough`; 3.4 added `all` and Sixel
  behind `--enable-sixel`. tmux PR #5445 ("Support both kitty and sixel images") is open. zellij
  0.45.0 (2026-08-20) notes mention kitty, and passthrough issue #4500 is open. iTerm2 shows Sixel
  in the 3.3.0 betas and "Sandbox sixel image decoding" in 3.3.8. VTE's `sixel` meson option
  defaults to `false`, and Fedora's 0.84.1 spec doesn't set it.
- sanitize-filename 0.6.0 regexes and its `regex` dependency; `open` 5.4.4 WSL PowerShell path
  with `WSLENV` and `\\wsl$\` mapping, plus the leading-dash guard; `dirs` 7.0.0
  `download_dir()`; etcetera 0.11.0 has no Downloads; `infer` shebang, ELF, PE and Mach-O
  matchers; arboard 3.6.1 features and its macOS TIFF path.
- RustSec: the ammonia advisory IDs and patched versions; only two `image` advisories (2019, 2020)
  and none for png, gif or zune-jpeg.
- Tracker lists: simplify-trackers BSD-3-Clause, last pushed 2022-02-11; leavemealone GPL-3.0;
  ugly-email-trackers has no licence.

**Corrected.**

1. **mail-parser classification was backwards.** cid images inside `multipart/related` are
   `PartType::Binary`, not `InlineBinary`. `InlineBinary` is for media directly in `mixed`, which
   are also indexed in `html_body`/`text_body`. `content_id()` already strips brackets. Fixed in
   the Summary, 3.1 and Key facts. The design (referenced-cid map) still stands, and needs this
   fix even more.
2. **ratatui-image env blacklist.** `WEZTERM_EXECUTABLE`/`KONSOLE_VERSION` blacklist kitty *and*
   Sixel, not only Sixel, so Konsole gets half-blocks. Fixed in 1.1, 1.2 and Key facts.
3. **README feature advice.** It is `--no-default-features --features image-defaults,crossterm`.
   Dropping `image-defaults` is neomail's own choice.
4. **mail-parser is not crate-wide `forbid(unsafe)`.** The attribute covers only `mod core`
   (though there is no `unsafe` anywhere). zune-jpeg has about 90 `unsafe` lines, not 85.
5. **SSRF gap.** hyper-util skips the resolver for IP-literal hosts, and a system proxy bypasses
   it too. Added an IP-literal check and `.no_proxy()` to 3.3, the Summary and Recommendation 4.
6. **Feature unification.** arboard enables `image/tiff` on macOS, so the decoder must allowlist
   formats at runtime.
7. ammonia RUSTSEC-2026-0193 is a MathML (`annotation-xml`) issue, not SVG.

**Still uncertain (confidence lowered or flagged).**

- Graph MIME `sendMail` ≈ 4 MB: not stated on the sendMail or MIME pages. It is probably the
  general Graph request-size limit; marked unverified.
- Outlook desktop's 20 MB default for internet accounts: not on the cited Outlook.com page;
  marked [low].
- Whether Graph `$value` honours `Range` (still an open question).
- caniemail SVG details and Gmail's rasterising: page structure not machine-readable; left
  [medium].
- Drag-and-drop paste formats per terminal and the clipboard commands per platform were not
  re-tested.
- Thunderbird's pref name and keyword list, Apple MPP's behaviour, and aerc's filter script were
  not re-fetched; they are left at their stated confidence.
- zune-jpeg's SIMD speed-up (about 2×) is still unmeasured.
