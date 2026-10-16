# Gmail with an app password

`neomail account add gmail-app-password` adds a Gmail account that reads mail over IMAP
(imap.gmail.com, port 993) and sends over SMTP (smtp.gmail.com, port 465), both with TLS
([Google's settings](https://developers.google.com/workspace/gmail/imap/imap-smtp)). It logs in
with an app password, never your Google Account password. To sign in through your own OAuth
client instead, see [gmail.md](gmail.md).

## Can your account have one?

Google: "App passwords can only be used with accounts that have 2-Step Verification turned on."
Even then, Google may offer none when 2-Step Verification is set up only for security keys, for a
work, school or other organisation account, or with Advanced Protection
([Google's help](https://support.google.com/accounts/answer/185833)). The wizard asks which case
fits first, before any other step:

- **Security keys only:** add another second step, such as Google prompts or an authenticator
  app, then run the wizard again.
- **Work or school:** the wizard goes on. Google Workspace users can sign in with an app password,
  but not where the administrator requires security keys
  ([Google's help](https://knowledge.workspace.google.com/admin/security/how-2-step-verification-works-with-legacy-apps)).
  If <https://myaccount.google.com/apppasswords> has no option to add one, ask the administrator,
  or use `neomail account add gmail` with your own OAuth client.
- **Advanced Protection:** apps that use app passwords are blocked, and only verified third-party
  apps can reach the account's data
  ([Google's help](https://support.google.com/accounts/answer/7539956)), so neomail has no way in.

## The steps

```sh
neomail account add gmail-app-password
```

1. Whether the account can have an app password, as above.
2. Turn on 2-Step Verification at
   <https://myaccount.google.com/signinoptions/two-step-verification>, if it is off.
3. The account's name in neomail (`--name`, default `gmail`).
4. neomail checks that no account has that name yet.
5. Your Gmail address (`--username`).
6. neomail checks that the config file can take the account and the OS keyring opens, before it
   asks for the app password.
7. Create an app password at <https://myaccount.google.com/apppasswords> and copy it. If the
   page has no option to add one, see the cases above.
   This step comes right before the one that asks for it, since Google says "Every app password
   can only be checked once" ([Google's help](https://support.google.com/accounts/answer/185833)).
8. Paste the app password. It is not shown, and spaces and no-break spaces are removed. A value
   that is not 16 letters or digits gets a warning but is still checked: Google calls it "a
   16-digit passcode" ([Google's help](https://support.google.com/accounts/answer/185833)).
9. neomail logs in to IMAP (it opens INBOX read-only) and to SMTP (it logs in and leaves, sending
   nothing). `--no-check` skips this step; the name, the config and the keyring are still checked.
10. The app password goes to the OS keyring and the account to the config file.

This list numbers neomail's checks and the save too. The screen numbers only the steps you see,
so with no flags the app password is step 6 of 6 there.

Press Esc to go back a step. A failed check says which step to change, and nothing is written
until the save. On a terminal, a taken name asks for another name, and a refused login asks for
the app password again and repeats the cases with no app password. Any other failure, or one on a
value given as a flag, ends the run.

## In scripts

Off a terminal, or with `--no-input`, steps 1, 2 and 7 are skipped and `--username` is needed. The
app password is read as one line of standard input, or comes from `--password-command`, which
must print it without spaces: they are removed only from a pasted or piped app password.

```sh
pass show mail/gmail | neomail account add gmail-app-password --no-input \
    --username you@example.com
```

A failure prints `step N of 10 (title): what to change` and exits 1; a missing flag exits 2
([exit codes](json.md#exit-codes)).

## Later

Google revokes app passwords when you change your Google Account password
([Google's help](https://support.google.com/accounts/answer/185833)). Create a new one, then run
`neomail account set-password gmail`.
