# Gmail with your own OAuth client

neomail reads Gmail over IMAP and signs in with OAuth, through a Google OAuth client that **you**
create. It ships with no client of its own
([decisions](decisions.md#accounts-and-providers)): an app's client is what Google grants access
to, so with your own, no one else's app ever holds access to your mail, and no shared client can
be blocked or abused for everyone at once.

The setup takes about five minutes, once. `neomail account add gmail` prints the same steps in
short.

## 1. Create a Google Cloud project

Open <https://console.cloud.google.com/projectcreate> and create a project, for example
"neomail".

*Why:* every OAuth client belongs to a project. A project of its own keeps neomail's client apart
from anything else you run. No billing account is needed.

## 2. Enable the Gmail API

Open <https://console.cloud.google.com/apis/library/gmail.googleapis.com> with the project
selected and click **Enable**.

*Why:* it makes Gmail's scopes available to the project in step 4. neomail reads your mail over
IMAP with one of them.

## 3. Set up the Google Auth Platform: Branding and Audience

In the console, go to **Google Auth platform > Branding**. In a new project it says the platform
is not configured yet: click **Get started** and fill in its steps: **App information** (an app
name only you will see, and your address as the support email), **Audience** (**External**),
**Contact information** (your address) and **Finish** (agree to the user data policy), then
click **Create** ([Google's guide](https://developers.google.com/workspace/guides/configure-oauth-consent)).

Then open **Audience** and, under **Test users**, click **Add users** and add the Gmail address
you will read.

*Why:* *External* lets a personal `@gmail.com` account sign in (*Internal* is only for a Google
Workspace organisation's own users). While the app is in *Testing*, only the test users listed
here can sign in.

## 4. Data Access: add the mail scope

Go to **Data Access > Add or Remove Scopes**, add `https://mail.google.com/` and save.

*Why:* this is the scope Gmail's IMAP requires for OAuth sign-in
([IMAP with OAuth](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol)).
neomail also asks for `openid email`, which needs no setup: it tells neomail which address you
signed in with.

## 5. Clients: create a Desktop app client

Go to **Clients > Create Client**, choose **Application type > Desktop app**, give it a name and
click **Create**
([Google's guide](https://developers.google.com/workspace/guides/create-credentials)). Copy the
**client ID** and the **client secret** now: Google shows the secret only once, when the client
is created, and afterwards only its last four characters
([Google: manage OAuth clients](https://support.google.com/cloud/answer/15549257)). If you lost
it, open the client and click **Add secret** to get a new one.

*Why:* a *Desktop app* client may redirect the browser back to `http://127.0.0.1` on any port
([OAuth for desktop apps](https://developers.google.com/identity/protocols/oauth2/native-app)).
That is where `neomail` listens for the sign-in to come back.

## 6. Add the account

```sh
neomail account add gmail
```

It asks for a name for the account (default `gmail`), your address, the client ID and the client
secret (not shown as you type), then opens Google's sign-in in your browser. Use `--no-browser` to
only print the URL, for example over SSH. Google may first show a warning that it hasn't
verified the app, in Testing mode too
([Google: publishing status](https://support.google.com/cloud/answer/15549945)); it may also go
straight to consent. If the warning shows, it is your own app, so click **Continue**. Then:

```sh
neomail sync --account gmail
```

Off a terminal, pass everything as flags and the client secret as one line of standard input:

```sh
secret-tool lookup service my-google-client | \
  neomail account add gmail --email you@example.com --client-id 1234-abc.apps.googleusercontent.com
```

## Testing mode: sign-ins that last 7 days

While the app's publishing status is *Testing*, Google gives it refresh tokens that expire after
7 days ([Google: refresh token expiration](https://developers.google.com/identity/protocols/oauth2#expiration)).
`neomail sync` then says the sign-in expired; run `neomail account login gmail` to sign in again.

To avoid that, publish the app: **Audience > Publish app**. The unverified-app warning may
still show; its way through is then **Advanced**, then **Go to** your app's name
([Google's troubleshooting guide](https://developers.google.com/apps-script/api/troubleshoot-authentication-authorization)),
and an app for your own use (fewer than 100 users) needs no verification
([Google: verification exceptions](https://support.google.com/cloud/answer/13464323)).

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
  not a test user (step 3), or you signed in with another account.
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
  client under **Clients** and click **Add secret** (step 5).
