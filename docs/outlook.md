# Outlook with your own Entra app

neomail reads Outlook, Outlook.com and Hotmail mail through Microsoft Graph and signs in with
OAuth, through a Microsoft Entra app registration that **you** create. It ships with no app of its
own ([decisions](decisions.md#accounts-and-providers)): an app registration is what Microsoft
grants access to, so with your own, no one else's app ever holds access to your mail.

The setup takes about ten minutes, once. `neomail account add outlook` prints the same steps in
short. The wording below follows Microsoft's
[app registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app)
and [redirect URI guide](https://learn.microsoft.com/en-us/entra/identity-platform/how-to-add-redirect-uri).

## 1. A directory for a personal account

A work or school account already belongs to a directory (tenant): skip this step. A personal
account (Outlook.com, Hotmail) needs one first: create a free
[Azure account](https://azure.microsoft.com/pricing/purchase-options/azure-account) with it.

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
([desktop app registration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-registration)).

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
for `Mail.Read` (read-only), `offline_access` (a refresh token), and `openid profile email` (your
address).

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
  `username` attribute). Microsoft sends a new refresh token with each refresh; neomail saves it
  back.
- `neomail account login NAME` signs in again. `neomail account logout NAME` deletes the sign-in
  from the keyring: Microsoft's v2 endpoints list no token revocation endpoint
  ([OpenID Connect metadata](https://learn.microsoft.com/en-us/entra/identity-platform/v2-protocols-oidc)),
  so the sign-in is only deleted here.

## How mail is read

`neomail sync` reads the INBOX through Graph, read-only: it lists messages
(`/me/mailFolders/inbox/messages`, newest first) and downloads each new one's MIME source
(`/me/messages/{id}/$value`), at most four at once, which is Outlook's limit per app and mailbox
([throttling limits](https://learn.microsoft.com/en-us/graph/throttling-limits)). Ids are Graph's
immutable ids, so they stay the same when a message moves. When Graph throttles, neomail waits as
long as `Retry-After` says, else 1, 2, 4 and 8 seconds, for at most five tries. In `--json` output,
`uid` is `null` for these messages ([json.md](json.md)).

Known limit: mail moved into the INBOX from another folder, with an arrival time older than the
oldest cached message, is not seen. Graph's delta query, in the
[background sync slice](roadmap.md), lifts it.

## When something goes wrong

- **"access token refused":** during `neomail sync`, the token expired between the refresh and
  the request, and the next sync gets a new one. During `account add`, nothing was saved: run it
  again.
- **"Mail.Read not granted":** the sign-in lacks mail access. During `account add`, nothing was
  saved: run it again and accept the permission to read your email. For a saved account, run
  `neomail account login NAME` and accept it.
- **"the app registration is not multitenant" or "is for Microsoft Entra accounts only":** the
  supported account types in step 2 are wrong. Register the app again with the right type, and
  use its client ID.
- **"public client flows are off":** step 4.
- **"you signed in as …, not …":** the browser used another Microsoft account. Sign in with the
  address you gave, or add the other one as its own account.
- **"the saved Microsoft sign-in expired or was revoked":** Microsoft ends a sign-in unused for 90
  days, after a password change, or when access is revoked. Run `neomail account login NAME`.
