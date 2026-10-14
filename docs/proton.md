# Proton Mail through Proton Mail Bridge

neomail reads and sends Proton Mail through
[Proton Mail Bridge](https://proton.me/mail/bridge), which runs on your computer and serves your
mailbox over IMAP and SMTP on 127.0.0.1. Bridge comes only with a paid Proton plan that includes
Mail. neomail never asks for your Proton password: it goes only into Bridge.

```sh
neomail account add proton
```

The wizard carries the steps, each with why it exists and where to go:

1. A paid Proton plan.
2. Install Bridge and a keyring ([Bridge for Linux](https://proton.me/support/bridge-for-linux)).
   When a port refuses the connection and neomail finds neither `protonmail-bridge` nor
   `protonmail-bridge-core` on PATH, it adds a warning, but packages may name it otherwise, so
   it goes on.
3. Sign in to Bridge, and keep it running (its window, its systemd --user unit, or
   `protonmail-bridge --noninteractive`).
4. The account's name in neomail (`--name`, default `proton`).
5. Bridge's mailbox details: the username (`--username`), the IMAP port (`--port`, default
   1143) and security (`--security`, default `starttls`; Bridge's SSL is `tls`), and the SMTP
   port (`--smtp-port`, default 1025) and security (`--smtp-security`). With split addresses
   mode, add one neomail account per address.
6. Is Bridge ready? neomail connects to both ports as answered. Nothing listening points to
   starting Bridge or the port; a server that speaks the other security points to the security
   answer.
7. The certificate. Bridge made its own, which no authority vouches for, so neomail pins its
   SHA-256. To check it against Bridge itself, export it (Settings > Advanced settings > Export
   TLS certificates, or `cert export` in `protonmail-bridge --cli`) and give the cert.pem path
   (`--bridge-cert PATH`), or pass the SHA-256 (`--cert-sha256`). A mismatch is refused and
   nothing is written. The export also writes key.pem, Bridge's private key: delete it. If the
   SMTP port presents another certificate, it is shown and pinned on its own.
8. The Bridge password, from the mailbox details (prompted, standard input, or
   `--password-command`). neomail logs in to IMAP (read-only) and SMTP (sending nothing) before
   it writes the keyring and the config. A refused login means the Bridge password, not the
   Proton one, or no account signed in to Bridge.

Then `neomail sync --account proton`.

Off a terminal or with `--no-input`, the advice steps are skipped and the flags answer the rest:

```sh
neomail account add proton --no-input --username you@example.com \
    --bridge-cert ~/bridge-cert/cert.pem --password-command 'pass show bridge'
```

If Bridge is reset or installed again, it makes a new certificate and the pin no longer matches:
delete its `[[accounts]]` block from the config (`neomail config path`
shows where) and add it again.
