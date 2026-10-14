# Background sync

`neomail daemon` keeps every account's cache current: it syncs them all at once, then again every
interval, so new mail shows up in `neomail list` without running `neomail sync`. Once you have
agreed to classification through `neomail classify`, it also sorts new mail into groups as it
arrives (see [Classifying](#classifying)).

## In a terminal

```sh
neomail daemon                  # every 5 minutes
neomail daemon --interval 1m    # digits followed by s, m or h, from 30s to 24h
```

It prints one line when it starts, then stays in the foreground:

```text
neomail daemon: syncing 2 accounts every 5 minutes, notifications on, classifying off (`daemon = true` under [classify] in the config turns it on); Ctrl-C stops
```

The notifications part is explained under [Notifications](#notifications), the classifying part
under [Classifying](#classifying).

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

## Classifying

Classifying in the background is off until you switch it on in the config:

```toml
[classify]
daemon = true
```

While it is off, the daemon sends nothing to the classifier, does not read the jev key for it, and
writes no groups to the server; `neomail classify` still classifies what you ask it to and writes
the groups in its own run. A yes to `neomail classify`'s consent question, or `--yes`, does not
switch it on.

Once it is on, after each sync's notifications (a round, a push sync or an Outlook check), the
daemon sends that sync's new mail to the classifier, one request per message, and stores each
answer, so `neomail list --group` and `neomail list --json` show the group without running
`neomail classify`. The request is the one `neomail classify` sends, built by the same code:
`neomail redact ID` shows exactly what leaves for a message, and [privacy.md](privacy.md) says
what is sent and what never is.

The daemon never asks for anything. It sends only when all of these hold, and its start line says
which applies:

- `classifying new mail with HOST`: the switch is on, the `[classify]` backend's host has your
  consent, given once to `neomail classify` (a yes from any version of neomail counts), and
  there is a key (`neomail key set`, or `JEV_API_KEY`);
- ``classifying off (`daemon = true` under [classify] in the config turns it on)``: the switch is
  off, as it is by default;
- ``classifying off: no jev key (run `neomail key set` or set JEV_API_KEY)``;
- ``classifying off: cannot read the jev key from the keyring: WHY``: the keyring failed to
  answer (the Secret Service restarting, say) and `JEV_API_KEY` is not set. Unlike the other
  states, this keeps what waits for the next time it syncs, as it may pass;
- ``classifying off: HOST needs your consent once; run `neomail classify` on a terminal``.
  `neomail classify` asks before its first request to HOST, then classifies up to 1000 messages
  from the last 365 days, one request each; `neomail classify --limit 1` gives the consent for
  one request. When every message already has a group, `neomail classify` sends nothing and does
  not ask: `neomail classify --again --limit 1` asks, and classifies the newest message again.

While classifying is off, the daemon looks again each time it syncs new mail. When the key or the
consent appears it prints `neomail daemon: classifying on (HOST)` and classifies from that sync
on. Mail that arrived while it was off is not sent then; `neomail classify` still finds it. When
classifying turns off or pauses while messages wait for the next time it syncs, they are dropped
too (unless a 402 paused it, see below), and the line says how many:
``…; N waiting messages are left to `neomail classify` ``. The key and the consent are read again
each time it syncs with mail to classify, and the config at every round: switching
`classify.daemon` on or off takes effect from the next round (5 minutes by default), without a
restart. A push sync or an Outlook check before that round uses the config read at the last one.
`neomail key delete` and `neomail key set` take effect the next time it syncs with mail to
classify, unless `JEV_API_KEY` is set in the daemon's environment: the daemon then falls back
to it. The keyring was unlocked when the daemon started, so this rarely asks you to unlock it
again.

Which mail is sent:

- Only mail that is new in a round. An account's first sync and a rebuilt cache are not new mail
  and are not classified; `neomail classify` does that. A message already classified, in any
  mailbox, is never sent again.
- Only mail received in the last 365 days, as `neomail classify` takes; `neomail classify --all`
  covers older mail moved into a synced mailbox.
- At most 100 messages each time it syncs (a round, a push sync or an Outlook check), each
  tried up to three times, four at a time, and none started after a minute (or after the
  interval, when it is shorter), so a slow or rate-limited jev cannot hold up the next sync. A
  request already sent still gets its answer. The rest wait in memory for the next time it syncs,
  which sends them even if it brings no new mail. A watcher's other news (push on, no IDLE, a
  watcher that failed or ended) syncs nothing, so it sends nothing either. What waits is
  forgotten when the daemon stops, and `neomail classify` picks it up.

Each account that got answers prints one line, with the groups as `neomail list --group` shows
them. It never names a subject, a sender or the key:

```text
14:05:00 work: classified 3 (newsletters 2, main 1), 1 failed, 20 wait for the next sync
```

When classifying fails, the sync and the notifications go on as before:

- An outage, a status that keeps asking for a retry (after three tries), or a sync's minute
  running out (`out of time for this sync`, also when jev asks for a wait that would end past it)
  stops that sync's classifying. Its messages wait for the next time it syncs, which sends them
  again, and standard error says once
  `neomail daemon: classifying stopped: WHY; N messages wait for the next sync`.
- An answer that is not jev's (a 2xx it cannot read), or a status every request would get (a
  400 or a 404, say: any but 401, 402, 403, 408, 422, 429 and 5xx), would come back the same at
  every sync, so it pauses classifying until `classify.url` or `classify.model` changes, or the
  daemon restarts. Nothing waits, the message with the unreadable answer counts as failed, and
  standard error says once `neomail daemon: classifying paused: WHY; change classify.url or
  classify.model, or restart the daemon`.
- A 402 (the jev account is out of credit) pauses classifying for an hour, or until the key
  changes, whichever comes first. Nothing is sent meanwhile; what waits stays queued, with the
  mail that arrives during the pause, and is sent once the pause lifts. Standard error says once
  per pause `neomail daemon: classifying paused: jev answered 402: BODY; the jev account is out
  of credit; trying again at HH:MM, or sooner if the key changes`. If jev still answers 402
  then, it pauses for another hour.
- A message jev refuses as invalid (422) counts as failed and is not sent again by the daemon.
- A refused key (401 or 403) pauses classifying until the key changes:
  ``neomail daemon: classifying paused: jev refused the key (401): run `neomail key set` or set
  JEV_API_KEY``. While paused, on a refused key or a 402, the daemon keeps only a digest of the
  key, not the key itself, and reads the key again each time it syncs new mail.

Ctrl-C or SIGTERM starts no new request: the requests on their way are abandoned, a retry's wait
ends, and the answers already stored stay. An abandoned request, or one that ended in an outage
or a timeout, has no stored answer, so its message is asked about again later and may be paid
for twice. If `neomail classify` runs at the same time as the daemon, both may ask about the same
message, and it is paid for twice.

### Groups on the server

While the switch is on, after classifying, each wake that synced writes the groups of classified
mail to each account's server, as `neomail classify` does: Outlook categories
([outlook.md](outlook.md#groups-as-categories)) and Gmail and Proton labels
([labels.md](labels.md)), then moves confident spam to Junk
([labels.md](labels.md#spam-to-junk)). It writes whatever is due, not only this wake's new mail,
so mail classified earlier and a `follow-up` that grows old get theirs too: at most 100 per
account per wake, and the rest at the next. With `[classify] daemon` off (the default) the daemon
writes nothing; with it on, `labels = false` stops the writes and `junk = false` the moves. An
account the daemon writes to prints one line on standard output when it wrote or moved some, or
when its failed or waiting counts changed:

```text
14:05:00 outlook: 12 categories written, 1 failed (tried again next wake), 30 wait for the next wake
14:05:02 work: 3 labels written, 1 moved to Junk
```

Writes take the account's lock: an account `neomail sync` or `mark` holds is skipped for that
wake. A sign-in that grants only `Mail.Read` sends nothing; standard error says once
``neomail: outlook: groups are not written to Outlook: run `neomail account login outlook` to let
neomail change mail``, and again only after the account wrote without it or the reason changed;
a wake that skipped the account for its lock changes nothing. A refused token or grant, a failed
sign-in, or Graph still throttling after its retries or out of reach, ends that account's writes
for the wake and says once `neomail: outlook: categories not written: WHY`; on Gmail and Proton, a
failed sign-in or session says `labels not written: WHY`, or `spam not moved to Junk: WHY` where
only moves were due. The rest wait for the next wake. A refused token is fetched afresh at the
next wake, with nothing to run. A server without a Junk folder, and Proton labels that stay
beside the new ones, are warnings said once, and again only when they change. A warning
from the sign-in, such as a new refresh token that could not be saved, is printed like the sync's
and only when it differs from the account's last one. The writes stop starting when their
budget is spent, as classifying does (60 seconds, or the interval when that is shorter, counted
from when the writes start), and writes still in flight then are dropped and tried again next
wake. So a wake's writes can delay the next sync by little more than that budget, and accounts
not reached by then wait for the next wake. However a write fails, classifying, the sync and the
notifications go on as before. `[classify] labels = false` turns writing off and `junk = false`
moving from the next round, and `label_prefix` sets the prefix. On Outlook the writes use their
own copy of the account's access token, refreshed as the sync's is, and only while something is
due; on Gmail and Proton each IMAP session signs in on its own, one per 50 messages, after one
that asks the server what it takes, which the daemon remembers until a session fails. Ctrl-C or SIGTERM ends the writes after the four Outlook requests
or the IMAP session in flight.

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

Changes, the classifying results and the categories written go to standard output; failures, warnings, the push lines and
the classifying state lines go to standard error. One
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

Gmail syncs and pushes over IMAP, which needs nothing more from you. The Gmail API allows about
300 message downloads a minute per user
([usage limits](https://developers.google.com/workspace/gmail/api/reference/quota)), and its own
push needs a Google Cloud Pub/Sub topic
([push notifications](https://developers.google.com/workspace/gmail/api/guides/push)), so Gmail
stays on IMAP ([DEC-2026-10-12-gmail-imap](decisions.md#dec-2026-10-12-gmail-imap)).
