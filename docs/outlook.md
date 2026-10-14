# Outlook with your own Entra app

neomail reads Outlook, Outlook.com and Hotmail mail through Microsoft Graph, marks it read or
unread, archives it or moves it to Deleted Items there, and signs in with OAuth, through a
Microsoft Entra app registration that **you** create. It ships with no app of its own
([decisions](decisions.md#accounts-and-providers)): an app registration is what Microsoft grants
access to, so with your own, no one else's app ever holds access to your mail.

## Add the account

```sh
neomail account add outlook
neomail sync --account outlook
```

On a terminal, `account add outlook` takes you through the setup one step at a time. Each step
says what to do, why it exists and which page to open; `<` goes back a step. It first asks
whether the account is personal (Outlook.com, Hotmail) or work or school (`--account-type`), and
how you will sign in (`--device-code`), and shows only the steps that apply:

- **Personal accounts only: a directory.** An app registration lives in a directory (tenant), and
  a personal account has none until you sign up for the free
  [Azure account](https://azure.microsoft.com/pricing/purchase-options/azure-account), which asks
  for a phone number and a card ([tenant quickstart](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-create-new-tenant)).
  Neither page says what happens to the registration when the subscription ends.
- **Register the app** at <https://entra.microsoft.com> with **Any Entra ID Tenant + Personal
  Microsoft accounts**, not the suggested single tenant: neomail signs in through Microsoft's
  `common` endpoint ([app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)).
- **Browser sign-in only: the redirect URI** `http://localhost` under **Mobile and desktop
  applications**. Microsoft ignores the port of a localhost redirect URI, so neomail can listen on
  any free port, on 127.0.0.1 and, where it can, `[::1]`
  ([redirect URI rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)).
- **Device code sign-in only: Allow public client flows.** The device code sign-in sends no
  redirect URI, so this setting alone tells Microsoft the app is a public client
  ([AADSTS7000218](https://learn.microsoft.com/en-us/troubleshoot/entra/entra-id/app-integration/confidential-client-application-authentication-error-aadsts7000218)).
- **Work or school accounts only: approval.** Microsoft's pages disagree on whether you can allow
  mail access yourself: [one](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/configure-user-consent)
  says a user can by default, [another](https://learn.microsoft.com/en-us/entra/identity/enterprise-apps/manage-app-consent-policies)
  that the default for new tenants keeps users from allowing `Mail.Read` and `Mail.ReadWrite`.
  If the sign-in asks for an admin's approval, ask your IT admin.
- **Your address and a name**, then neomail checks that the name is free, that the config file
  can take it and that the keyring opens.
- **The Application (client) ID** from the app's Overview page. It must be a GUID. There is no
  secret: neomail runs on your machine, so it cannot keep one.

neomail then asks Microsoft about the client ID before you sign in, with a device code request
that is never shown or used: an unknown ID sends you back to that step. Then you sign in
(`--no-browser` only prints the URL), and neomail reads the INBOX's message count through Graph,
once. Nothing reaches the keyring or the config file until all of that has worked, with one
exception: a sign-in left in the keyring under that name with no account in the config is deleted
right before the new sign-in (Microsoft has no endpoint to revoke it), and if the sign-in or the
Graph check then fails, nothing replaces it. `--no-check` skips the client check and the Graph
check, not the sign-in. An error Microsoft sends back names
its step; a sign-in that times out names the steps whose mistakes Microsoft shows only on its own
page (the redirect URI, AADSTS50011, and the supported account types, AADSTS50194).

Off a terminal, or with `--no-input`, the advice steps are skipped: pass `--email` and
`--client-id` (and `--account-type work` for a work or school account).

neomail asks
for `Mail.ReadWrite` (to read your mail and change it), `Mail.Send` (to send mail as you, with
[sendMail](https://learn.microsoft.com/en-us/graph/api/user-sendmail), which also keeps the copy
in Sent Items), `Mail.Read` (so an earlier version can
still refresh this sign-in; see [Where things are kept](#where-things-are-kept)),
`offline_access` (a refresh token), and `openid profile email` (your address). The consent screen
therefore asks to read and change your mail ("Read and write access to your mail") and to send
mail as you. neomail needs
the change part to mark messages read or unread (`neomail mark`), to move them
(`neomail archive`, `neomail delete`) and to set their groups as categories
([below](#groups-as-categories)): Graph allows each only with `Mail.ReadWrite`
([update message](https://learn.microsoft.com/en-us/graph/api/message-update),
[move message](https://learn.microsoft.com/en-us/graph/api/message-move)). neomail never deletes a
message for good, and never creates, renames or deletes a folder.

### Accounts added before neomail could mark mail

Earlier versions asked for `Mail.Read` only. Such an account keeps syncing exactly as before:
neomail keeps asking Microsoft for `Mail.Read` when it refreshes that sign-in, because a refresh
may ask only for the scopes the sign-in asked for or fewer
([authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow)).
`neomail mark`, `archive` or `delete` on it fails without changing anything and says to run
`neomail account login NAME`.
Do that once, accept the new permission, and the account can change mail.

neomail tells the two apart by the `mail_grant` it keeps with the refresh token in the keyring
(below): `read-write-send` for a sign-in that granted `Mail.ReadWrite` and `Mail.Send`,
`read-write` for one that granted `Mail.ReadWrite` only, `read` for one that granted only
`Mail.Read`, and no `mail_grant` at all for a sign-in saved by an earlier version, which counts as
`read`.

### Accounts added before neomail could send

A sign-in made before neomail asked for `Mail.Send` keeps reading and changing mail, and is
refreshed for no more than it granted. Sending from it fails before the question and before any
request, sends nothing, and says to run `neomail account login NAME` once. Graph takes the
recipients from the message's headers and has no envelope, so an Outlook account cannot send Bcc
yet: neomail refuses such a message rather than drop its Bcc recipients. Graph's `202 Accepted` means accepted, not delivered.

### Device code sign-in

`--device-code` (on `account add outlook` and `account login NAME`) shows a code and a URL instead:
open the URL on any device, enter the code and sign in. neomail chooses it on its own over SSH
without a display (`SSH_CONNECTION` or `SSH_TTY` set, `DISPLAY` and `WAYLAND_DISPLAY` unset or
empty) and says so.

Microsoft calls device code flow "a high-risk authentication method that can be part of a phishing
attack" and recommends that organizations block it wherever possible
([Conditional Access](https://learn.microsoft.com/en-us/entra/identity/conditional-access/concept-authentication-flows)).
A work or school tenant may therefore refuse it; use the browser sign-in then. Only enter a code
that neomail itself just printed.

## Where things are kept

- The config file holds the account's name, address and client ID, which are not secret:

  ```toml
  [[accounts]]
  name = "outlook"
  username = "you@outlook.example"
  oauth = { provider = "microsoft", client_id = "00000000-0000-0000-0000-000000000000" }
  ```

- The refresh token is in the OS keyring only (service `neomail-oauth`, the account name as its
  `username` attribute), with `mail_grant`, which says whether the sign-in lets neomail change mail
  and send it. Microsoft sends a new refresh token with each refresh; neomail saves it back with
  the same `mail_grant`.
- Going back to a version from before `mail_grant`: it still reads the entry, since it ignores
  `mail_grant`. A later version from before sending knows `read` and `read-write` but not
  `read-write-send`, the grant of a sign-in made by this version, so it cannot read that entry:
  run `neomail account login NAME` in it. An earlier version refreshes
  with `offline_access` and `Mail.Read`, a subset of what a sign-in made by this version asked
  for, which Microsoft allows: a refresh's scopes "must be equivalent to or a subset of the scopes
  requested" at sign-in
  ([authorization code flow](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow);
  not checked live). When the earlier version saves a new refresh token, the entry loses
  `mail_grant`, and this version then treats it as `read`: `neomail mark` says to run
  `neomail account login NAME`, which lets it mark mail again.
- `neomail account login NAME` signs in again. `neomail account logout NAME` deletes the sign-in
  from the keyring: Microsoft's v2 endpoints list no token revocation endpoint
  ([OpenID Connect metadata](https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc)),
  so the sign-in is only deleted here.

## How mail is marked

`neomail mark read ID...` and `neomail mark unread ID...` send
`PATCH /me/messages/{id}` with `{"isRead": true}` or `{"isRead": false}` for each message, with the
same immutable ids sync uses, at most four at once, and wait out throttling as reads do
([update message](https://learn.microsoft.com/en-us/graph/api/message-update)). The cache changes
only for the messages Graph confirmed. A message Graph no longer has (404) fails, and the next sync
removes it. Any other error stops the writes, and `mark` says the account's messages may not be
marked: Graph keeps the changes it already made, the cache is left alone for all of them, and the
next sync reads their state. There is no queue: run the command again.

## Groups as categories

Each classified message gets its group as an Outlook category: the label prefix followed by the
group `neomail list --group` shows, such as `nm-receipts-invoices`
([DEC-2026-10-13-categories](decisions.md#dec-2026-10-13-categories)). `neomail classify` writes
them after it stores its results, and, once `daemon = true` under `[classify]` switches background
classifying on, `neomail daemon` after each wake that synced
([daemon.md](daemon.md#groups-on-the-server)). There is no command of its own. Gmail and Proton
get labels the same way ([labels.md](labels.md)).

- **What is written.** The effective group, worked out when the cache is read, so a category
  changes when the group does: a `follow-up` that grows old becomes `nm-follow-up-old`, and a new
  `other_below`, `classify --again` or a new prefix rewrites what they change. Nothing else is
  written again: a category you remove or change in Outlook stays as you left it until the
  message's group changes. Spam gets `nm-spam`, and spam at or above `junk_at` then moves to
  Junk Email, once ([labels.md](labels.md#spam-to-junk)).
- **What is kept.** For each message neomail reads its categories
  (`GET /me/messages/{id}?$select=categories`), removes only its own (the prefix followed by one
  of the eleven group names, ignoring ASCII case, or the exact name it wrote on that message
  before), adds the new one, and sends the list back with `PATCH /me/messages/{id}` and
  `{"categories": [...]}` only when it changed
  ([update message](https://learn.microsoft.com/en-us/graph/api/message-update)). Your other
  categories stay, in their order, a name of yours such as `nm-project` included. The PATCH
  replaces the whole list: in a live check, a PATCH whose list left a name out removed that name
  from the message. That is why neomail reads the list first. A change you make in Outlook
  between the two requests is lost.
- **No new permission, and the master list is left alone.** Outlook applies a category "by
  assigning the **displayName** property of the category to the **categories** collection of the
  item" ([outlookCategory](https://learn.microsoft.com/en-us/graph/api/resources/outlookcategory)).
  That is a message update, which needs `Mail.ReadWrite`, part of every sign-in since neomail
  could mark mail. neomail never adds, colours or deletes entries in your master category list,
  which would need `MailboxSettings.ReadWrite` and a new sign-in for every account
  ([create category](https://learn.microsoft.com/en-us/graph/api/outlookuser-post-mastercategories)).
  A sign-in that granted only `Mail.Read` writes nothing and says, once, to run `neomail account
  login NAME`. Outlook still shows a name the master list does not hold: in a live check, Outlook
  on the web showed each `nm-` category on its message as a plain grey chip.
- **How much at once.** Up to 1,000 messages per account per `neomail classify` run and 100 per
  daemon wake, at most four requests at once, with the same throttling waits as reads; the rest
  is written next time. Writes take the account's lock, as `mark` does.
- **When it fails.** What is still to write is worked out from the cache each time, so nothing is
  queued: mail classified before neomail wrote categories is written on the next run or wake.
  neomail records what it wrote on each message by the message's immutable
  id, which stays the same when the message moves to another folder of the mailbox
  ([immutable ids](https://learn.microsoft.com/en-us/graph/outlook-immutable-id)), so two copies
  of one mail are written, and retried, each on its own. A write that fails for one message waits
  before it is tried again: 15 minutes, doubled after each failure in a row, at most a day, or not
  at all once its group or the prefix changes
  ([DEC-2026-10-14-label-retries](decisions.md#dec-2026-10-14-label-retries)). Until then
  `neomail classify` counts it, as in `outlook: 1 held back after a failure (tried again later)`,
  and exits 0 for it; the daemon says so only when its counts change, a held-back message counting
  as a failed one, so a retry that fails again is not said again. A message Graph no longer has (404) is not asked about again until
  the sync drops it. A refused token or grant (401 or 403), or Graph still throttling after its
  retries or out of reach, stops that account's writes, and the rest wait; the other accounts go
  on. In the daemon the writes also stop at a time budget (60 seconds, or the interval when that
  is shorter), the sign-in's token refresh included, so they hold up the next sync by little more
  than that. Each account gets an equal share of what is left of the budget when its turn comes,
  so a throttled or slow account cannot keep the others from theirs.
  `neomail classify` exits 1 when a category write, a move to Junk or an account's sign-in
  failed; classifying, syncing and notifying go on whatever happens. Lines about categories name
  the account only, never a subject, a sender or a category of yours.

`[classify] label_prefix` (default `nm-`, never empty) sets the prefix, and `labels = false`
turns writing off: nothing more is written, and the categories already written stay on the
messages. neomail itself never shows the prefix. The sync does not read categories back, and a
category write does not make the next sync download the message again.

## Where archive and delete put a message

`neomail archive ID...` moves each message to the folder Graph calls `archive`: the one
Outlook's one-click Archive moves to. `neomail delete ID...` moves it to `deleteditems`, the
Deleted Items folder, where a deleted message goes. Both are Graph's well-known folder names,
which work whatever the mailbox's language
([mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)). The archive is
not Exchange Online's separate Archive Mailbox feature (same page). Neither command deletes a
message for good, and neomail never empties Deleted Items.

Each command first reads that folder (`GET /me/mailFolders/archive?$select=id` or
`.../deleteditems?...`). A mailbox without it fails with "the mailbox has no "archive" folder",
and nothing moves. Then each message gets `POST /me/messages/{id}/move` with
`{"destinationId": "archive"}` or `{"destinationId": "deleteditems"}`, at most four at once, with
the same immutable ids and throttling waits as `mark`
([move message](https://learn.microsoft.com/en-us/graph/api/message-move)). `201 Created` is
moved, and that message's row leaves the cache. A 404 is a message Graph no longer has: it fails,
and the next sync removes it. Checking the folder first is what makes a 404 mean the message,
not a missing folder. Any other error stops the moves, as for `mark`: Graph keeps the moves it
already made, the cache is left alone, and the next sync finds those messages gone from the
INBOX. `--json` names the folder by its well-known name: `archive` or `deleteditems`.

Graph moves a message by its id, wherever it is now. A message you moved out of the INBOX
elsewhere since the last sync is therefore moved again, from that folder. Run `neomail sync`
first if you have been sorting mail in another app.

## Folders and move

`neomail folder list` names each folder by its path of display names from the top of the
mailbox, joined with `/`: `Inbox`, `Inbox/Receipts`, `Receipts`. That path is what
`neomail move FOLDER` takes, matched exactly. The listing is read-only and works with either
grant (Mail.Read or Mail.ReadWrite): `GET /me/mailFolders`, then
`GET /me/mailFolders/{id}/childFolders` for each folder whose `childFolderCount` is above 0, each
followed through every `@odata.nextLink`, 100 folders to a page
([list mailFolders](https://learn.microsoft.com/en-us/graph/api/user-list-mailfolders),
[list childFolders](https://learn.microsoft.com/en-us/graph/api/mailfolder-list-childfolders)).
Graph's listing leaves hidden folders out by default, and neomail leaves out search folders
(`@odata.type` `#microsoft.graph.mailSearchFolder`): their contents are a query's results, not
messages moved there. Whether Graph refuses a move into a search folder is unverified; neomail
refuses it before anything moves.

Roles come from Graph's well-known folder names, which work whatever the mailbox's language
([mailFolder](https://learn.microsoft.com/en-us/graph/api/resources/mailfolder)): neomail asks
`GET /me/mailFolders/{name}?$select=id` for `inbox`, `archive`, `deleteditems`, `junkemail`,
`sentitems` and `drafts`, and the folder with that id gets the role `inbox`, `archive`, `trash`,
`junk`, `sent` or `drafts`. A listing therefore takes six requests for the roles, one per page of
the top level, and one per page of each folder that has children.

`neomail move FOLDER ID...` finds the folder before anything moves, without walking the whole
folder tree: it lists the top of the mailbox, then the children of only those folders whose name
starts what is left of the path. `Inbox/Receipts` takes two listings (the top, then the Inbox's
children); `Receipts` takes one. Then it asks for the inbox's id, and refuses the move when the
path names the inbox: its display name follows the mailbox's language, its id does not. Each
message then gets `POST /me/messages/{id}/move` with the folder's id as `destinationId`
([move message](https://learn.microsoft.com/en-us/graph/api/message-move)), with the concurrency,
throttling waits and failures of `archive` above. So moving N messages to `Inbox/Receipts` takes
3 + N requests when each listing fits on one page.

Refused, with nothing moved and the account's one `neomail:` line:

- a path no folder has: "the mailbox has no folder named "Receipts"; nothing was moved; run
  `neomail folder list --account NAME` to see its folders";
- a path that several folders share, which happens when a display name holds a `/` ("a/b" at
  the top, and "b" inside "a"): use Outlook on the web for that folder;
- the Inbox, and a search folder.

The `folder` that `--json` prints for `move` is the path as typed; for `archive` and `delete` it
stays the well-known name. neomail never creates, renames or deletes a folder.

## How mail is read

`neomail sync` reads the INBOX through Graph, read-only: it lists messages
(`/me/mailFolders/inbox/messages`, newest first) and downloads each new one's MIME source
(`/me/messages/{id}/$value`), at most four at once, which is Outlook's limit per app and mailbox
([throttling limits](https://learn.microsoft.com/en-us/graph/throttling-limits)). Ids are Graph's
immutable ids, so they stay the same when a message moves. When Graph throttles, neomail waits as
long as `Retry-After` says, else 1, 2, 4 and 8 seconds, for at most five tries. In `--json` output,
`uid` is `null` for these messages ([json.md](json.md)).

After the first sync, a sync asks Graph only for what changed since the last one, through the
delta query ([message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)): new
messages are downloaded, read changes update the cache, and messages deleted or moved out of the
INBOX leave it. A sync with nothing new costs one request. neomail keeps Graph's delta link in the
cache, never prints it, and follows it only to `https://graph.microsoft.com/`.

The delta query filters on arrival time
([delta query for messages](https://learn.microsoft.com/en-us/graph/delta-query-messages)), so a
delta link starts at the oldest cached message: every cached message is inside it. Microsoft says a
filtered delta query can also report read changes for messages outside its filter
([message: delta](https://learn.microsoft.com/en-us/graph/api/message-delta)), but in our live
check it did not, so neomail does not count on that. With a filter, Graph returns at most 5,000
messages per round; when the cached window holds that many, neomail keeps listing the INBOX on
every sync instead of storing a delta link, so nothing is missed, but each sync then costs a full
listing of the window rather than one request. A later round can reach 5,000 too, after a change
to thousands of messages at once such as marking them all read; neomail then starts over as after
a reset (below).

Microsoft can reset a delta link: it answers `410 Gone`, or an error such as `syncStateNotFound`
when the link expired
([delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)). A stored
link Graph cannot use drew `400 BadRequest` in our live check, so neomail treats that as a reset
too. The sync then lists the INBOX as the first sync after an upgrade does, applies what changed,
and starts a new delta link. Nothing is lost and nothing needs doing. The same happens when a
message comes back without its source: the next sync lists from it and asks again.

Known limit: mail moved into the INBOX from another folder, with an arrival time older than the
oldest cached message, is never seen: the delta query and the listing both filter on arrival time
and leave it out.

## Large files

A message with a file of 3 MB or more, or one whose sendMail request would pass 4 MB, goes the
way Microsoft documents for large attachments
([Attach large files to Outlook messages](https://learn.microsoft.com/en-us/graph/outlook-large-attachments)):

1. The message without its files is saved as a draft from its MIME (`POST /me/messages`).
2. Each file under 3 MB is added with one `POST /me/messages/{id}/attachments`. Each larger one
   gets an upload session (`POST .../attachments/createUploadSession`), then its bytes in order,
   3,276,800 bytes a `PUT`, to the session's URL. That URL is on outlook.office.com and carries
   its own token, so neomail sends it no bearer token, and refuses a URL on any other host.
3. The draft is sent (`POST /me/messages/{id}/send`); Outlook keeps it in Sent Items.

Nothing is sent before step 3, so a failure of step 1 or 2 says nothing was sent. If step 1
fails with a 5xx or its answer is lost, Exchange may still have saved the draft, which neomail
cannot delete without its id: the error says a draft may be left in Drafts. If step 2 fails,
neomail deletes the draft; if it cannot, the error says the draft is left in Drafts. Steps 1 and
2 are retried only after a 429, never a 503, which may come after Graph did the work. If step 3
is refused (a 4xx), nothing was sent and the draft stays in Drafts, which the error says. If
step 3 fails with a 5xx or its answer is lost, the message may have been sent: neomail says so,
does not retry, and keeps the draft. Every sign-in that can send (`Mail.Send`) also grants
`Mail.ReadWrite`, which the draft and the sessions need. Outlook takes 25 MB of files a message,
the Outlook.com limit; neomail refuses more before any request.

## When something goes wrong

- **"access token refused":** during `neomail sync`, the token expired between the refresh and
  the request, and the next sync gets a new one. During `account add`, nothing was saved: run it
  again.
- **"Microsoft Graph refused access to the mailbox (HTTP 403)":** Graph answers 403 when the
  sign-in lacks the permission, the account has no mailbox or licence, or a Conditional Access
  policy blocks the app ([Graph errors](https://learn.microsoft.com/en-us/graph/errors)). During
  `account add`, the sign-in has already granted mail access, so it is one of the others; nothing
  was saved. For a saved account, run `neomail account login NAME` and accept the permission to
  read (and, for `neomail mark`, `archive` and `delete`, change) your email first.
- **"this account's sign-in lets neomail read mail but not change it":** the account was added
  before neomail could mark mail, or the consent granted only `Mail.Read`. Run
  `neomail account login NAME`; nothing was changed.
- **"the mailbox has no "archive" folder in Microsoft Graph":** the mailbox has no folder for
  Outlook's one-click Archive, so nothing was moved. Archive those messages in Outlook on the
  web.
- **"the app registration is not multitenant" or "is for Microsoft Entra accounts only":** the
  supported account types are wrong. Register the app again with the right type, and
  use its client ID.
- **"counts as a confidential client":** for the device code sign-in, Allow public client flows
  is off; for the browser sign-in, the redirect URI is under Web instead of Mobile and desktop
  applications.
- **"you signed in as …, not …":** the browser used another Microsoft account. Sign in with the
  address you gave, or add the other one as its own account.
- **"the saved Microsoft sign-in expired or was revoked":** Microsoft ends a sign-in unused for 90
  days, after a password change, or when access is revoked. Run `neomail account login NAME`.
