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
neomail daemon: syncing 2 accounts every 5 minutes, notifications on; Ctrl-C stops
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

## Push

IMAP accounts get push: new mail is synced, and notified, seconds after it arrives instead of at
the next round. That covers every account the daemon syncs over IMAP: plain IMAP and
app-password accounts, Proton through Bridge, and Gmail, which signs in with OAuth. Outlook and
other Microsoft accounts sync through Microsoft Graph, not IMAP: they are checked every minute
instead ([Outlook accounts](#outlook-accounts)).

For each IMAP account whose server supports IDLE (RFC 2177), the daemon keeps one extra
connection open next to the syncs, with INBOX opened read-only. It never changes anything on the
server. When INBOX changes, the daemon syncs that account alone, about 2 seconds later, so a
burst of changes makes one sync, and notifies as after a round. Rounds keep their times, as a
safety net.

- The connection opens after a successful sync of the account, with the login that sync used,
  so the password or token is not asked for again. It is renewed every 29 minutes, as RFC 2177
  advises.
- A server without IDLE gets no extra connection; that account keeps syncing every round.
- When the connection drops, the daemon syncs the account to catch up and opens a new one, at
  most once a minute per account: if the last one opened less than a minute before, the daemon
  syncs the account again once the minute is up and opens it then. A connection that dies
  without a word is noticed within 30 minutes, and the next round still syncs the account in the
  meantime.
- If the connection fails before its first IDLE, the daemon prints `push failing` and tries
  again after the account's sync in the next round.
- If a round fell due while the machine slept, that round opens every connection again. After a
  shorter sleep, a connection that died meanwhile is noticed within 30 minutes, as above.
- Ctrl-C or SIGTERM closes them all at once.

Each IMAP account's push state is printed when its first connection opens or fails, and again
when it changes. Like the start line, these lines go to standard error:

```text
14:00:01 neomail daemon: work: push on
14:00:01 neomail daemon: home: push off (the server has no IDLE)
14:20:00 neomail: work: push failing: could not connect to imap.example.com:993: Connection refused (os error 111)
```

A failure that repeats is not printed again; `push on` follows when it works again.

### Outlook accounts

Microsoft Graph has no push that reaches a program on your computer: it sends change
notifications only to a publicly reachable HTTPS endpoint, or to Azure Event Hubs or Event Grid
([webhook delivery](https://learn.microsoft.com/en-us/graph/change-notifications-delivery-webhooks)).
Its pull model is the delta query
([delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)), which
every Outlook sync uses once it has a delta link ([docs/outlook.md](outlook.md)). So the daemon
checks each Outlook account every minute between the rounds, and a check with nothing new costs one
request. Two cases cost a full listing of the cached window on every check instead: a window of
5,000 messages or more, which is more than Graph's filtered delta query returns
([delta query for messages](https://learn.microsoft.com/en-us/graph/delta-query-messages)), and a
message still missing its body.

- A check syncs that account alone, as a push sync does: the same account lock, the same lines,
  and a notification when new mail came in. Rounds keep their times.
- The next check is due a minute after the account's last sync ended, a failed one included, so a
  slow or failing Graph never stacks checks. A failure that repeats is not printed again.
- When `neomail sync` holds the account, the check is skipped and tried again a minute later.
- With `--interval` of a minute or less, the rounds alone check every account that often: there
  are no extra checks and no line about them.
- After a sleep, the round that falls due syncs the account, and its checks go on from there.
  Ctrl-C or SIGTERM ends them at once.

Each Outlook account gets one line, once, after its first sync:

```text
14:00:01 neomail daemon: outlook: checked every minute (Outlook can't push to this computer)
```

## Notifications

On Linux, after each round, push sync or Outlook check, every account that got new mail in it
shows one desktop notification:

```text
work: 5 new messages
Ada Lovelace: Notes on the engine
Charles Babbage: Re: Difference engine
bob@example.com: (no subject)
and 2 more
```

It names up to three of the new messages, newest first, by sender (the display name, else the
address) and subject (cut at 80 characters). An account's first sync, or a rebuild of its cache,
never notifies: that mail is not new. Neither do the syncs that finish a first sync which was
interrupted or left a message without its body; mail that arrives before such a sync finishes
is not announced. The sender and subject can show on a lock screen or on a screen you share;
`neomail daemon --no-notify` turns notifications off. There is no config key for it yet.

Notifications need a desktop notification service (the one in GNOME or KDE Plasma, or mako,
dunst, swaync and the like) on your session's D-Bus. The daemon looks for one when it starts, and
its start line says which applies:

- `notifications on`;
- `notifications off (no notification service)`: none was running or could be started, or the
  session bus could not be reached. The daemon looks again before each round's notifications, so
  a service your desktop starts after the daemon is found then, and the daemon prints
  `notifications on`;
- `notifications off (--no-notify)`;
- `notifications off (not supported here yet)`: on macOS and other systems than Linux.

The session bus, for notifications and the keyring alike, is found through
`DBUS_SESSION_BUS_ADDRESS`, else `$XDG_RUNTIME_DIR/bus`, else `/run/user/UID/bus` (zbus's
`Address::session`, [source][zbus-session]). A service started without your session's variables
still reaches a bus at that path, while your session runs one there; a bus at any other address
needs `DBUS_SESSION_BUS_ADDRESS`.

[zbus-session]: https://docs.rs/crate/zbus/5.19.0/source/src/address/mod.rs

A notification that fails is reported once, like a repeated sync error, and never stops a sync.
Each call gives up after 5 seconds, and the first that fails ends that round's notifications, so a
service that does not answer holds a round up once, not once per account. The round in which the
daemon finds a service is the exception: looking for it and the first notification each have their
own 5 seconds, so a service that hangs can hold that round up twice.

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

`default.target` starts the daemon at login, which can be before your desktop starts its
notification service; the daemon finds the service before the first notification after that
(see [Notifications](#notifications)). An `After=graphical-session.target` line would not wait
for it: `After=` only orders units that are started together ([systemd.unit(5)][unit-before]),
and `PartOf=graphical-session.target` would stop syncing when the desktop session ends.

[unit-before]: https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Before=

`RestartPreventExitStatus=2` keeps systemd from restarting it while the config has no accounts or
is invalid; fix the config, then `systemctl --user restart neomail.service`. Passwords and sign-ins
come from the same place as for `neomail sync`: a `password_command`, or the OS keyring, which a
user service reaches through the session's D-Bus, as it reaches the notification service. The
daemon runs a `password_command` with standard input closed and stops it after 60 seconds, so a
command that would prompt (gpg asking for a passphrase) fails that account instead of stalling the
others; keep its agent's cache warm, or use the keyring. A locked keyring is different: reading it
shows the keyring's unlock prompt, and the round waits until that prompt is answered or cancelled,
so keep the keyring unlocked while the daemon runs.

## Without systemd

These examples follow each init system's own documentation; they were checked against the
documentation only, not run under OpenRC or runit.

### OpenRC (Alpine, Gentoo)

OpenRC 0.60 and later run services per user (`openrc --user`, out of experimental in 0.62:
[NEWS](https://github.com/OpenRC/openrc/blob/master/NEWS.md)). Their scripts live in
`~/.config/rc/init.d`, every `rc-*` tool takes `--user`, and `XDG_RUNTIME_DIR` must be set; add
`pam_openrc.so` to your login's PAM configuration to start them with your session
([user guide](https://github.com/OpenRC/openrc/blob/master/user-guide.md), "User services"). Save
this as `~/.config/rc/init.d/neomail`:

```sh
#!/sbin/openrc-run
description="neomail background sync"
supervisor=supervise-daemon
command="${HOME}/.local/bin/neomail"
command_args="daemon"
output_log="${HOME}/.local/state/neomail-daemon.log"
error_log="${HOME}/.local/state/neomail-daemon.log"
# Exit 2 (no accounts, invalid config) would come back at once: give up after three tries.
respawn_delay=5
respawn_max=3
respawn_period=60
```

Then make it executable, add it to your default runlevel and start it:

```sh
mkdir -p ~/.local/state
chmod +x ~/.config/rc/init.d/neomail
rc-update --user add neomail default
rc-service --user neomail start
```

`supervisor`, `output_log`, `error_log` and the `respawn_*` settings are openrc-run's
([openrc-run(8)](https://github.com/OpenRC/openrc/blob/master/man/openrc-run.8),
[supervise-daemon(8)](https://github.com/OpenRC/openrc/blob/master/man/supervise-daemon.8)).
OpenRC filters the environment of the services it starts. A user service keeps a fixed list
(`LANG`, `LC_*`, `TERM` and OpenRC's own, plus `USER`, `LOGNAME`, `HOME`, `SHELL`, `XDG_*`,
`EDITOR`, `VISUAL`, `PAGER`, `LESS` and `DIFFTOOL`) and the names in `rc_env_allow`
([env_filter][openrc-env]). `DBUS_SESSION_BUS_ADDRESS` is not on the list, so by default the
daemon finds the session bus only at `$XDG_RUNTIME_DIR/bus` (see [Notifications](#notifications)).
To pass it through, add it to `rc_env_allow` in `~/.config/rc/rc.conf`, which overrides
`/etc/rc.conf` for user services ([rc.conf][openrc-conf]; user guide, "User services"):

```sh
rc_env_allow="DBUS_SESSION_BUS_ADDRESS"
```

An OpenRC older than 0.60 has no user services: start `neomail daemon` from your session there.

[openrc-env]: https://github.com/OpenRC/openrc/blob/14e11b7f5b09072aa1433e790bbf832800858fec/src/shared/misc.c#L60-L105
[openrc-conf]: https://github.com/OpenRC/openrc/blob/14e11b7f5b09072aa1433e790bbf832800858fec/etc/rc.conf#L66-L68

### runit (Void)

Void's handbook describes two ways to run services as your user
([Per-User Services](https://docs.voidlinux.org/config/services/user-services.html)):

- **turnstile**, which starts them with your session from `~/.config/service/`, and passes your
  session's variables through `chpst -e "$TURNSTILE_ENV_DIR"`. `DBUS_SESSION_BUS_ADDRESS` is
  among them only if turnstile runs D-Bus as one of your services too. Otherwise (with
  `dbus-run-session`, say) the service reaches a session bus only at the path zbus falls back
  to (see [Notifications](#notifications)); with none there, notifications are off and the
  keyring cannot be reached. The handbook's steps for that service:

  ```sh
  mkdir -p ~/.config/service/dbus
  ln -s /usr/share/examples/turnstile/dbus.run ~/.config/service/dbus/run
  ln -s /usr/share/examples/turnstile/dbus.check ~/.config/service/dbus/check
  ```

  and `core_services="dbus"` in `~/.config/service/turnstile-ready/conf`, so it starts before
  your other services.
- **runsvdir** as your user, a system service started at boot that watches `~/service/`. Its
  services do not get your session's variables, so they reach a session bus only at the path
  zbus falls back to, and only while your session runs one there (see
  [Notifications](#notifications)). Without one, notifications are off and the keyring cannot be
  reached: use a `password_command`.

With turnstile, save this as `~/.config/service/neomail/run` and make it executable:

```sh
#!/bin/sh
exec chpst -e "$TURNSTILE_ENV_DIR" "$HOME/.local/bin/neomail" daemon 2>&1
```

For runsvdir, put it in `~/service/neomail/run` without `chpst -e "$TURNSTILE_ENV_DIR"`. runsv
restarts a service whose `run` exits, a second later; to keep it down on exit 2 (no accounts,
invalid config) until you fix the config, add an executable `finish` beside `run`, which runsv
starts with `run`'s exit code
([runsv(8)](https://man.voidlinux.org/runsv.8), [sv(8)](https://man.voidlinux.org/sv.8)):

```sh
#!/bin/sh
[ "$1" = 2 ] && exec sv down .
exit 0
```

Then `SVDIR=~/.config/service sv up neomail` (or `SVDIR=~/service`) starts it again, and
`sv status` shows it.

## What the output means

Each account's first successful sync after the daemon starts prints a line, even with nothing
new, so the log shows the daemon working. After that the daemon is quiet: it prints a line only
when an account changed or failed, or its push state changed (see [Push](#push)). Each sync line
starts with the local time, then the line `neomail sync` prints:

```text
14:00:00 work: 0 new, 0 updated, 0 removed
14:00:00 home: 0 new, 0 updated, 0 removed
14:05:00 work: 2 new, 0 updated, 0 removed
14:10:00 neomail: home: could not connect to imap.example.com:993: Connection refused (os error 111)
14:35:00 home: 0 new, 0 updated, 0 removed
```

Changes go to standard output; failures, warnings and the push lines go to standard error. One
account failing never stops the others. A failure or warning that repeats the account's last one is
not printed again; the account's next success is printed even with nothing new, so you can see it
recovered (the last line above). Messages whose body could not be fetched (`N without a body`) are
retried every round but printed only when their number changes. There is no progress counter.

An OAuth account's access token is kept in memory, never on disk, until 5 minutes before it
expires, and only while the account (its server and signed-in user) stays as it was; then it is
refreshed, and a new refresh token is saved as `neomail sync` saves it.

## `neomail sync` alongside

`neomail sync` still works while the daemon runs. Each account is synced by one process at a time:
when the daemon is syncing an account, `neomail sync` prints
`neomail: waiting for neomail daemon to finish syncing ACCOUNT` once and goes on when it is done;
when `neomail sync` holds an account, the daemon skips it for that round.

## Why Gmail stays on IMAP

Gmail syncs and pushes over IMAP. The Gmail API allows about 300 message downloads a minute per
user ([usage limits](https://developers.google.com/workspace/gmail/api/reference/quota)), and
its own push needs a Google Cloud Pub/Sub topic
([push notifications](https://developers.google.com/workspace/gmail/api/guides/push)), so IMAP
is faster and needs nothing more from you
([DEC-2026-10-12-gmail-imap](decisions.md#dec-2026-10-12-gmail-imap)).
