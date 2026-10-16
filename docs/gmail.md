# Gmail with your own OAuth client

neomail reads Gmail over IMAP and signs in with OAuth, through a Google OAuth client that **you**
create. It ships with no client of its own
([decisions](decisions.md#accounts-and-providers)): an app's client is what Google grants access
to, so with your own, no one else's app ever holds access to your mail, and no shared client can
be blocked or abused for everyone at once.

## Add the account

```sh
neomail account add gmail
neomail sync --account gmail
```

On a terminal, `account add gmail` takes you through the setup one step at a time, from no
Google Cloud project to a signed-in Desktop client. Each step says what to do, why it exists and
which console page to open; Esc goes back a step. Give your project's ID when asked (or
`--project ID`) and the links open that project. The steps are:

First, choose the route: the console steps below, or
[commands](#set-up-the-project-with-google-cloud-cli-commands) for steps 1 and 2.

1. Create a Google Cloud project, then give its ID (optional).
2. Enable the Gmail API, so its scope is offered under Data Access.
3. Set up the Google Auth Platform with Audience **External**, so a personal `@gmail.com`
   account can sign in ([Google's guide](https://developers.google.com/workspace/guides/configure-oauth-consent)).
4. Your address, then add it as a test user: while the app is in *Testing*, only test users can
   sign in.
5. Data Access: add `https://mail.google.com/`, the scope Gmail's IMAP and SMTP need for OAuth
   ([IMAP with OAuth](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol)).
6. Choose Testing or published ([below](#testing-mode-sign-ins-that-last-7-days)).
7. Name the account. neomail checks that the name is free, that the config file can take it and
   that the keyring opens, before it asks for anything secret.
8. Create a **Desktop app** client and download its JSON. Google shows the secret in full
   only once, when the client is created
   ([manage OAuth clients](https://support.google.com/cloud/answer/15549257)); if you lost it,
   open the client and click **Add secret**. A Desktop app client may send the sign-in back to
   `http://127.0.0.1` on any port, where neomail waits
   ([OAuth for desktop apps](https://developers.google.com/identity/protocols/oauth2/native-app)).
   The wizard reads the ID and secret from the JSON
   ([below](#the-client-id-and-secret-are-picked-up)), or you paste them.

neomail then checks the client ID and secret with Google before you sign in: it sends a token
request with a made-up code, which Google refuses differently for an unknown client, a wrong
secret and a good pair. A wrong value sends you back to the step that asks for it. Then it opens
Google's sign-in (`--no-browser` only prints the URL), and logs in to Gmail's IMAP (opening INBOX
read-only) and SMTP (sending nothing) with the new sign-in. Nothing reaches the keyring or the
config file until all of that has worked, with one exception: a sign-in left in the keyring under
that name with no account in the config is revoked and deleted right before the new sign-in, since
revoking it afterwards would end the new sign-in too. If the sign-in or the mail logins then fail,
nothing replaces it. `--no-check` skips the client check and the mail logins, not the sign-in. A sign-in that times out names the steps whose mistakes Google shows
only on its own page: the test user, the client type and the mail scope.

Off a terminal, or with `--no-input`, the console steps are skipped: pass `--email` and
`--client-id`, and the client secret as one line of standard input:

```sh
secret-tool lookup service my-google-client | \
  neomail account add gmail --email you@example.com --client-id 1234-abc.apps.googleusercontent.com
```

### Set up the project with Google Cloud CLI commands

The wizard's first question is how to set up the project: **console**, the browser steps above,
or **commands**, a script you run in a terminal with the
[Google Cloud CLI](https://cloud.google.com/sdk/docs/install) (`gcloud`) installed. Off a
terminal and with `--no-input`, console answers it and the flags work as before.

- **The paste:** one short line, such as `sh '/home/you/.local/share/neomail/runs/12345/setup.sh'`,
  the same in fish, zsh, bash and sh. The wizard writes `setup.sh` into a folder of its own in
  the data directory, readable only by you, and removes it when the step ends. The script is
  plain sh with a comment for each part and holds no secret: open it and read it first if you
  like. This is the same paste and pick-up as Outlook's
  ([the decision](decisions.md#dec-2026-10-16-commands-route)).
- **The script** signs in with `gcloud auth login` in your browser (or with a code when there is
  none), finds or makes the project, runs `gcloud services enable gmail.googleapis.com` in it,
  prints `ok` and the project ID, and hands the same to the wizard. The step moves on by itself
  and shows the project ID it got; Esc goes back to change it. No browser here (over SSH)? The
  sign-in prints a link to open in any browser and asks for the code it shows. Or run the
  script on another machine with `sh setup.sh` and type the project ID it prints.
- **The project ID** is `neomail-` and the first 10 hex digits of the SHA-256 of
  `neomail-gmail:` and the address you signed in with, so every run finds the same project and
  the address itself never shows in the ID. The script looks it up with
  `gcloud projects describe` and makes it only when it is not there. To use a project of your
  own, made in the browser or by hand, give its ID: `--project ID`, or after the line
  (`sh '.../setup.sh' my-project-123`). The script then only enables the Gmail API in it, and
  stops if it cannot open it.
- **Every later step opens in that project:** each page link carries `?project=ID`.

A second run changes nothing and makes no second project. A project pending deletion is not
replaced: the script stops and names `gcloud projects undelete ID`.

**Only three steps are commands.** The project, its ID and the Gmail API have commands. The Google
Auth Platform pages (branding, the test user, the mail scope, publishing) and the Desktop client
have none: Google shut down the IAP OAuth Admin API (`gcloud iap oauth-brands`) on 2026-03-19,
and it only ever made Internal brands; `gcloud iam oauth-clients` makes Workforce Identity
clients, not Desktop app clients. So they stay browser steps on both routes, each opened in your
project.

**Why a throwaway sign-in.** The Google Cloud CLI keeps its signed-in accounts and active project in
`~/.config/gcloud` or `CLOUDSDK_CONFIG`, and reads any setting from a `CLOUDSDK_<SECTION>_<NAME>`
variable. The script clears those variables (keeping only where Python is, the proxy and
certificates), signs in with a `CLOUDSDK_CONFIG` of its own from `mktemp -d`, and at the end
revokes that sign-in (`gcloud auth revoke --all`) and removes the folder, also after a failure
or Ctrl-C. So an earlier gcloud session never decides the result, and your own account and
active project are the same before and after. For its own runs it also sets
`CLOUDSDK_CORE_DISABLE_PROMPTS=1`, so no question waits for a key (gcloud takes each one's
default; the code at a sign-in without a browser is still asked),
`CLOUDSDK_CORE_DISABLE_USAGE_REPORTING=true`, so no reporting process gcloud leaves running
outlives the script, and `CLOUDSDK_COMPONENT_MANAGER_DISABLE_UPDATE_CHECK=true`.

### The client ID and secret are picked up

At **Create a Desktop app client**, click **Download JSON** in the box that shows the new client:
Google offers the file only there. While the step shows, the wizard watches your download folder
(`XDG_DOWNLOAD_DIR`, from the environment or `user-dirs.dirs`, else `~/Downloads`) for a
`client_secret_*.json` that arrives after the step showed. It reads the client ID and secret from
it, checks that the file names your project, and moves on by itself, showing the client ID, never
the secret. A file of another project or of another kind of client is ignored, and the step says
why. No download? Press Enter and paste the client ID and secret, as before.

The file holds the secret in plain text. Once the account is saved and the secret is in the
keyring, the wizard offers to delete it.

## Testing mode: sign-ins that last 7 days

While the app's publishing status is *Testing*, Google gives it refresh tokens that expire after
7 days ([Google: refresh token expiration](https://developers.google.com/identity/protocols/oauth2#expiration)).
`neomail sync` then says the sign-in expired; run `neomail account login gmail` to sign in again.

To avoid that, publish the app: **Audience > Publish app**. The unverified-app warning may
still show; its way through is then **Advanced**, then **Go to** your app's name
([Google's troubleshooting guide](https://developers.google.com/apps-script/api/troubleshoot-authentication-authorization)),
and an app for your own use (fewer than 100 users) needs no verification
([Google: verification exceptions](https://support.google.com/cloud/answer/13464323)).

Neither works with Advanced Protection: Google lets "only verified third-party apps" reach such an
account's sensitive data ([Advanced Protection](https://support.google.com/accounts/answer/7539956)),
and your own client is unverified.

## Archive and delete

neomail reads Gmail over IMAP, where labels are folders and Gmail marks its own folders with
special-use attributes on every LIST
([IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions#special-use_extension_of_the_list_command)).
Gmail has no `\Archive` folder, so:

- `neomail archive` moves a message from INBOX to `[Gmail]/All Mail`, the folder with `\All`, with
  `UID MOVE`, which takes it out of the source mailbox
  ([RFC 6851 section 3.3](https://www.rfc-editor.org/rfc/rfc6851#section-3.3)), so it leaves the
  INBOX. Google does not document `MOVE` to All Mail: its
  [IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions) page covers
  reading and adding labels with `X-GM-LABELS` but not moving a message or removing a label, and
  its [IMAP client settings](https://support.google.com/mail/answer/78892) page says only "You can
  find messages you delete from an IMAP folder in your 'All Mail' label in Gmail". On a test
  message, a live check saw its `X-GM-LABELS` go from `(\Inbox)` to `()`. That a message keeps its
  other labels is unverified: that message had none. Removing the label with
  `STORE -X-GM-LABELS (\Inbox)` is no better documented, and a move works the same on every IMAP
  server.
- `neomail delete` moves it to `[Gmail]/Trash`, the folder with `\Trash`. Gmail deletes a
  message in the Trash for good after 30 days
  ([Delete messages in Gmail](https://support.google.com/mail/answer/7401)); neomail never does.

Gmail shows a label to IMAP apps only when its **Show in IMAP** box is checked under
**Settings**, **Labels**
([Tips to read & send email in Gmail](https://support.google.com/a/users/answer/11339703)). If
All Mail or Trash is not shown to IMAP, LIST does not name it, and `archive` or `delete` fails for
the account without moving anything.

## Move to a folder or label

`neomail folder list --account NAME` lists the labels shown to IMAP, which Gmail "treats ... as
folders for the purposes of IMAP"
([IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)), and
Gmail's own folders under `[Gmail]/`, with their roles from the special-use attributes:
`[Gmail]/All Mail` is `all`, `[Gmail]/Spam` `junk`, `[Gmail]/Trash` `trash` (the same page's LIST
example shows `* LIST (\HasNoChildren \Junk) "/" "[Gmail]/Spam"`). A label's name is
its IMAP mailbox name, decoded from modified UTF-7, with `/` between a nested label and its
parent as LIST reports it. A label hidden from IMAP (its **Show in IMAP** box unchecked) is not
listed and cannot be moved to.

`neomail move LABEL ID...` moves a message from INBOX to the label's mailbox with `UID MOVE`, as
`archive` does. That this does what Gmail's own "Move to" does, adding the label and taking the
message out of the INBOX, is unverified: Google's IMAP pages above and its
[IMAP client settings](https://support.google.com/mail/answer/78892) do not say what a MOVE into a
label's mailbox does, and no live check has tried it yet. neomail confirms the move by UID: if a
message were still in the INBOX afterwards, it would report it as not confirmed and keep it in the
cache. `neomail move "[Gmail]/Spam" ID...` moves a message to Spam the same way: a live check on a
test account moved one, and it was then in `[Gmail]/Spam` and no longer in the INBOX.

## Groups as labels

`neomail classify` gives each classified message its group as a Gmail label, such as
`nm-receipts-invoices`, next to your own labels, and moves confident spam to Spam, the mailbox
Gmail marks `\Junk`: see [labels.md](labels.md).

## Sending

neomail sends through smtp.gmail.com on port 465 with TLS, signed in with the same OAuth scope
(SASL XOAUTH2) or the app password
([IMAP, POP and SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp)). Gmail keeps
the copy in Sent itself ("Sent messages are automatically copied to the Gmail/Sent folder if your
email client uses SMTP", [IMAP client settings](https://support.google.com/mail/answer/78892)), so
neomail adds none. No setting is needed: an account on imap.gmail.com sends this way.

## Where things are kept

- The config file holds the account's name, address and client ID, which are not secret:

  ```toml
  [[accounts]]
  name = "gmail"
  username = "you@example.com"
  oauth = { provider = "google", client_id = "1234-abc.apps.googleusercontent.com" }
  ```

- The client secret and the refresh token are in the OS keyring only, in one entry with the
  service `neomail-oauth` and the account name as its `username` attribute. They are never written
  to the config, the cache or a log, and never printed. Without a keyring (often on WSL) OAuth
  accounts cannot be used yet.
- `neomail account login NAME` signs in again and replaces the refresh token.
  `neomail account logout NAME` revokes it at Google and deletes the entry.

## When something goes wrong

- **"did not grant mail access":** when Google's consent screen shows a checkbox per
  permission, mail access may have been left unticked. Sign in again and tick it.
- **"Access blocked" or "has not completed the Google verification process":** the address is
  not a test user, or you signed in with another account.
- **"you signed in as …, not …":** the browser used another Google account. Sign in with the
  address you gave, or add the other one as its own account.
- **"the server has no archive folder" or "the server has no trash folder":** All Mail or Trash
  is hidden from IMAP, so nothing was moved. In Gmail, open **Settings**, **Labels**, check
  **Show in IMAP** next to All Mail or Trash
  ([Tips to read & send email in Gmail](https://support.google.com/a/users/answer/11339703)), and
  run the command again.
- **"invalid_client":** the client ID or secret is wrong. During `neomail account add`, run it
  again with the right ones. For an account already added, run `neomail account logout NAME`, then
  `neomail account login NAME`, which asks for the secret again (a wrong client ID is fixed in the
  config file). Google shows a secret only when it is created; if you no longer have it, open the
  client under **Clients** and click **Add secret**.
