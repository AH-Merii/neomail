# Background sync

`neomail daemon` keeps every account's cache current: it syncs them all at once, then again every
interval, so new mail shows up in `neomail list` without running `neomail sync`.

## In a terminal

```sh
neomail daemon                  # every 5 minutes
neomail daemon --interval 1m    # digits followed by s, m or h, from 30s to 24h
```

It prints one line when it starts, then stays in the foreground:

```text
neomail daemon: syncing 2 accounts every 5 minutes; Ctrl-C stops
```

Ctrl-C (SIGINT) or SIGTERM lets the account being synced finish, then exits 0. A second Ctrl-C
stops it at once. One daemon runs per data directory; a second one exits 1 with
`neomail daemon is already running for DATA_DIR`.

A round starts one interval after the previous one started, or at once if that one took longer;
missed rounds are not made up. The interval counts on the wall clock too, so after the machine
wakes from sleep the overdue round starts within 30 seconds rather than at the end of the
interval.

The config is read again before each round, so an account you add or remove takes effect at the
next round. If the config becomes invalid, the daemon says so once and keeps the last good one.
Without a config file or accounts it prints the start lines and exits 2.

## As a systemd user service

Save this as `~/.config/systemd/user/neomail.service` (it expects `neomail` in `~/.local/bin`,
where `install.sh` puts it):

```ini
[Unit]
Description=neomail background sync
Documentation=https://github.com/AH-Merii/neomail/blob/main/docs/daemon.md

[Service]
ExecStart=%h/.local/bin/neomail daemon
Restart=on-failure
RestartPreventExitStatus=2

[Install]
WantedBy=default.target
```

Then load, enable and start it, and follow its output:

```sh
systemctl --user daemon-reload
systemctl --user enable --now neomail.service
journalctl --user -u neomail.service -f
```

`RestartPreventExitStatus=2` keeps systemd from restarting it while the config has no accounts or
is invalid; fix the config, then `systemctl --user restart neomail.service`. Passwords and sign-ins
come from the same place as for `neomail sync`: a `password_command`, or the OS keyring, which a
user service reaches through the session's D-Bus. The daemon runs a `password_command` with
standard input closed and stops it after 60 seconds, so a command that would prompt (gpg asking
for a passphrase) fails that account instead of stalling the others; keep its agent's cache warm,
or use the keyring. A locked keyring is different: reading it shows the keyring's unlock prompt,
and the round waits until that prompt is answered or cancelled, so keep the keyring unlocked while
the daemon runs.

## What the output means

Each account's first successful sync after the daemon starts prints a line, even with nothing
new, so the log shows the daemon working. After that the daemon is quiet: it prints a line only
when an account changed or failed. Each line starts with the local time, then the line
`neomail sync` prints:

```text
14:00:00 work: 0 new, 0 updated, 0 removed
14:00:00 home: 0 new, 0 updated, 0 removed
14:05:00 work: 2 new, 0 updated, 0 removed
14:10:00 neomail: home: could not connect to imap.example.com:993: Connection refused (os error 111)
14:35:00 home: 0 new, 0 updated, 0 removed
```

Changes go to standard output; failures and warnings go to standard error. One account failing
never stops the others. A failure or warning that repeats the account's last one is not printed
again; the account's next success is printed even with nothing new, so you can see it recovered
(the last line above). Messages whose body could not be fetched (`N without a body`) are retried
every round but printed only when their number changes. There is no progress counter.

An OAuth account's access token is kept in memory, never on disk, until 5 minutes before it
expires, and only while the account (its server and signed-in user) stays as it was; then it is
refreshed, and a new refresh token is saved as `neomail sync` saves it.

## `neomail sync` alongside

`neomail sync` still works while the daemon runs. Each account is synced by one process at a time:
when the daemon is syncing an account, `neomail sync` prints
`neomail: waiting for neomail daemon to finish syncing ACCOUNT` once and goes on when it is done;
when `neomail sync` holds an account, the daemon skips it for that round.

## Coming next

For now the daemon polls. Later parts of the same slice add push, so new mail arrives without
waiting for the interval: IMAP IDLE, Microsoft Graph's delta query, and Gmail through the Gmail
API with its history.
