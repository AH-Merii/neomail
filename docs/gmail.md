# Gmail with your own OAuth client

neomail reads Gmail over IMAP and signs in with OAuth, through a Google OAuth client that **you**
create. It ships with no client of its own
([decisions](decisions.md#accounts-and-providers)): an app's client is what Google grants access
to, so with your own, no one else's app ever holds access to your mail, and no shared client can
be blocked or abused for everyone at once.

The setup takes about five minutes, once. `nmail account add gmail` prints the same steps in
short.

## 1. Create a Google Cloud project

Open <https://console.cloud.google.com/projectcreate> and create a project, for example
"neomail".

*Why:* every OAuth client belongs to a project. A project of its own keeps neomail's client apart
from anything else you run. No billing account is needed.

## 2. Enable the Gmail API

Open <https://console.cloud.google.com/apis/library/gmail.googleapis.com> with the project
selected and click **Enable**.

*Why:* it makes Gmail's scopes available to the project in step 4. neomail uses IMAP today and
will use the API in a later version.

## 3. Set up the Google Auth Platform: Branding and Audience

In the console, go to **Google Auth platform > Branding**, enter an app name (only you will see
it) and your address as the support and contact email, and agree to the user data policy
([Google's guide](https://developers.google.com/workspace/guides/configure-oauth-consent)).

Then, under **Audience**, choose **External**, and under **Test users** click **Add users** and add
the Gmail address you will read.

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
That is where `nmail` listens for the sign-in to come back.

## 6. Add the account

```sh
nmail account add gmail
```

It asks for a name for the account (default `gmail`), your address, the client ID and the client
secret (not shown as you type), then opens Google's sign-in in your browser. Use `--no-browser` to
only print the URL, for example over SSH. Then:

```sh
nmail sync --account gmail
```

Off a terminal, pass everything as flags and the client secret as one line of standard input:

```sh
secret-tool lookup service my-google-client | \
  nmail account add gmail --email you@example.com --client-id 1234-abc.apps.googleusercontent.com
```

## Testing mode: sign-ins that last 7 days

While the app's publishing status is *Testing*, Google gives it refresh tokens that expire after
7 days ([Google: refresh token expiration](https://developers.google.com/identity/protocols/oauth2#expiration)).
`nmail sync` then says the sign-in expired; run `nmail account login gmail` to sign in again.

To avoid that, publish the app: **Audience > Publish app**. Google then shows "Google hasn't
verified this app" when you sign in; click **Advanced** and continue. An app for your own use
(fewer than 100 users) needs no verification
([Google: verification exceptions](https://support.google.com/cloud/answer/13464323)).

## Where things are kept

- The config file holds the account's name, address and client ID, which are not secret:

  ```toml
  [[accounts]]
  name = "gmail"
  username = "you@example.com"
  oauth = { provider = "google", client_id = "1234-abc.apps.googleusercontent.com" }
  ```

- The client secret and the refresh token are in the OS keyring only, in one entry with the
  service `neomail-oauth` and the account name as the user. They are never written to the config,
  the cache or a log, and never printed. Without a keyring (often on WSL) OAuth accounts cannot
  be used yet.
- `nmail account login NAME` signs in again and replaces the refresh token.
  `nmail account logout NAME` revokes it at Google and deletes the entry.

## When something goes wrong

- **"did not grant mail access":** Google's consent screen lets you untick permissions. Sign in
  again and leave mail access ticked.
- **"Access blocked" or "has not completed the Google verification process":** the address is
  not a test user (step 3), or you signed in with another account.
- **"you signed in as …, not …":** the browser used another Google account. Sign in with the
  address you gave, or add the other one as its own account.
- **"invalid_client":** the client ID or secret is wrong. Run `nmail account logout NAME`, then
  `nmail account login NAME`, which asks for the secret again. Google shows a secret only when it
  is created; if you no longer have it, open the client under **Clients** and click
  **Add secret** (step 5).
