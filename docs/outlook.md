# Outlook with your own Entra app

neomail reads Outlook, Outlook.com and Hotmail mail through Microsoft Graph, marks it read or
unread, archives it or moves it to Deleted Items there, and signs in with OAuth, through a
Microsoft Entra app registration that **you** create. It ships with no app of its own
([decisions](decisions.md#accounts-and-providers)): an app registration is what Microsoft grants
access to, so with your own, no one else's app ever holds access to your mail.

The setup takes about ten minutes, once. `neomail account add outlook` prints the same steps in
short. The wording below follows Microsoft's
[app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)
and [redirect URI guide](https://learn.microsoft.com/en-us/entra/identity-platform/how-to-add-redirect-uri).

## 1. A directory for a personal account

A work or school account already belongs to a directory (tenant): skip this step. A personal
account (Outlook.com, Hotmail) needs one first: create a free
[Azure account](https://azure.microsoft.com/pricing/purchase-options/azure-account) with it.
Microsoft asks for a phone number and a credit or debit card "for identity verification"; it
charges nothing at sign-up, but may place a small verification hold on the card (same page).

*Why:* an app registration lives in a directory. Microsoft lists "an Azure account that has an
active subscription" as the prerequisite and suggests the account's **Default Directory**
([guide](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)).
For a new tenant, Microsoft points customers on a free tenant to the same
[free account](https://learn.microsoft.com/en-us/entra/fundamentals/create-new-tenant). Neither
page says what happens to the directory or the registration when the subscription ends.

## 2. Register the app

Sign in at <https://entra.microsoft.com>, go to **Entra ID > App registrations** and select
**New registration**. Give it any name, for example "neomail". Under **Supported account types**
choose **Any Entra ID Tenant + Personal Microsoft accounts**, then **Register**.

*Why:* this account type lets both work or school and personal addresses sign in with the one
registration.

## 3. Add the redirect URI

In the app, under **Manage**, select **Authentication**, then **Add Redirect URI**, and the
**Mobile and desktop applications** tile. Under **Custom redirect URIs** enter
`http://localhost` and select **Configure**.

*Why:* after you sign in, Microsoft sends the browser back to `http://localhost`, where neomail
waits. Microsoft ignores the port of a localhost redirect URI, so neomail can listen on any free
port ([redirect URI rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url)).
neomail listens on 127.0.0.1 and, where the machine has it, on `[::1]` on the same port, since a
browser may resolve `localhost` to either. Microsoft prefers `127.0.0.1`, but its portal refuses
`http://127.0.0.1`, so `localhost` keeps the setup to the portal.

## 4. Allow public client flows

On the same **Authentication** page, under **Advanced settings**, set **Allow public client
flows** to **Yes**, then select **Save**
([desktop app configuration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration)).

*Why:* the device code sign-in (below) is a public client flow. Microsoft recommends leaving the
setting off unless the app is a public client that uses such a flow, as neomail is
([public client flows](https://learn.microsoft.com/en-us/entra/identity-platform/msal-client-applications)).

## 5. Copy the Application (client) ID

The app's **Overview** page shows the **Application (client) ID**. Copy it. There is no client
secret to create.

*Why:* neomail runs on your machine, so it cannot keep a secret; Microsoft calls such an app a
public client, and a public client has no secret.

## Add the account

```sh
neomail account add outlook        # asks for the address and the client ID, then signs in
neomail sync --account outlook
```

Off a terminal, or with `--no-input`, pass `--email` and `--client-id`. Before anything is saved,
neomail reads the INBOX's message count through Graph, once, to check the sign-in can read mail;
`--no-check` skips that. A failed sign-in or check saves nothing.

The browser opens on Microsoft's sign-in page; `--no-browser` only prints the URL. neomail asks
for `Mail.ReadWrite` (to read your mail and change it), `Mail.Read` (so an earlier version can
still refresh this sign-in; see [Where things are kept](#where-things-are-kept)),
`offline_access` (a refresh token), and `openid profile email` (your address). The consent screen
therefore asks to read and change your mail ("Read and write access to your mail"). neomail needs
the change part to mark messages read or unread (`neomail mark`) and to move them
(`neomail archive`, `neomail delete`): Graph allows both only with `Mail.ReadWrite`
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
(below): `read-write` for a sign-in that granted `Mail.ReadWrite`, `read` for one that granted only
`Mail.Read`, and no `mail_grant` at all for a sign-in saved by an earlier version, which counts as
`read`.

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
  `username` attribute), with `mail_grant`, which says whether the sign-in lets neomail change mail.
  Microsoft sends a new refresh token with each refresh; neomail saves it back with the same
  `mail_grant`.
- Going back to an earlier version: it still reads the entry, since it ignores `mail_grant`. It
  refreshes with `offline_access` and `Mail.Read`, a subset of what a sign-in made by this
  version asked for, which Microsoft allows: a refresh's scopes "must be equivalent to or a
  subset of the scopes requested" at sign-in
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
  supported account types in step 2 are wrong. Register the app again with the right type, and
  use its client ID.
- **"public client flows are off":** step 4.
- **"you signed in as …, not …":** the browser used another Microsoft account. Sign in with the
  address you gave, or add the other one as its own account.
- **"the saved Microsoft sign-in expired or was revoked":** Microsoft ends a sign-in unused for 90
  days, after a password change, or when access is revoked. Run `neomail account login NAME`.
