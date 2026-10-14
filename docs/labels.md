# Groups as Gmail and Proton labels, and spam to Junk

Each classified Gmail or Proton message gets its group as a label: the label prefix followed by
the group `neomail list --group` shows, such as `nm-receipts-invoices`
([DEC-2026-10-14-labels-junk](decisions.md#dec-2026-10-14-labels-junk)). Spam at or above
`junk_at` then moves to the account's Junk folder. Outlook gets the same as categories, with its
spam moved to Junk Email ([outlook.md](outlook.md#groups-as-categories)). `neomail classify`
writes them after it stores its results, and, once `daemon = true` under `[classify]` switches
background classifying on, `neomail daemon` after each wake that synced
([daemon.md](daemon.md#groups-on-the-server)). There is no command of its own.

## What is written

The effective group, worked out when the cache is read, as for categories: a `follow-up` that
grows old becomes `nm-follow-up-old`, and a new `other_below`, `classify --again` or a new prefix
rewrites what they change. Nothing else is written again: a label you remove stays removed until
the message's group changes. neomail removes only its own label (the prefix followed by one of the
eleven group names, ignoring ASCII case, or the exact name it wrote on that message before) and
never touches yours.

- **Gmail** is an account on `imap.gmail.com` whose server has the `X-GM-EXT-1` capability.
  neomail reads the message's labels, then sends `UID STORE` with `-X-GM-LABELS` and
  `+X-GM-LABELS` and one name each, so the message's set of labels is never replaced. A missing
  label is created first: Google's page documents adding a label with STORE, not that STORE
  creates one
  ([IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)).
- **Proton** is an account with Bridge's shape in the config (this machine, a pinned certificate)
  whose server lists Bridge's `Labels` and `Folders`. A label is a copy of the INBOX message into
  `Labels/<name>`, created when missing; the message stays in the INBOX. neomail's earlier label
  is removed by marking and expunging exactly that message's UIDs in that label's mailbox, which
  Bridge applies as removing the label
  ([research/proton.md](research/proton.md), from Bridge's IMAP connector), and only on a server
  that advertises UIDPLUS, whose `UID EXPUNGE` touches no other message
  ([RFC 4315, section 2.1](https://www.rfc-editor.org/rfc/rfc4315#section-2.1)). neomail finds
  the message there by its Message-ID. Without UIDPLUS or a Message-ID the old label stays beside
  the new one, and a warning says how many.
- **Other IMAP servers** get no labels, only the move to Junk, so neomail signs in to them only
  when spam is due a move.

## Spam to Junk

Spam at or above `junk_at` (0.9 unless set) moves to the account's Junk folder once its label or
category is written: over IMAP, the mailbox LIST marks `\Junk`
([RFC 6154](https://www.rfc-editor.org/rfc/rfc6154.html#section-2)), with `UID MOVE`
([RFC 6851](https://www.rfc-editor.org/rfc/rfc6851)); on Outlook, Junk Email (`junkemail`)
([move message](https://learn.microsoft.com/en-us/graph/api/message-move)). Lower-confidence
spam keeps its label and stays where it is.

- **At most once.** neomail records the move before it sends it, so a message is never moved
  twice, even when neomail stops between the server's answer and the record. A message you move
  back to the INBOX stays there.
- **The backlog moves too.** Spam classified before neomail moved spam moves under the same caps.
- **A confirmed move takes the message out of the cache**, as archiving does.
- **No Junk folder, or no MOVE:** nothing moves, and a warning says why, for example
  `neomail: work: spam is not moved to Junk: the server has no Junk folder: no mailbox has the
  \Junk attribute (RFC 6154)`.

## How much at once, and when it fails

As for categories: up to 1,000 messages per account per `neomail classify` run and 100 per
daemon wake, under the account's lock; over IMAP, 50 messages per session. What is still to write
or move is worked out from the cache each time, so nothing is queued. A write or move that failed
for one message waits before it is tried again: 15 minutes, doubled after each failure in a row,
at most a day, or not at all once the message's group or the prefix changes
([DEC-2026-10-14-label-retries](decisions.md#dec-2026-10-14-label-retries)). A message no longer
in the INBOX is not asked about again until the sync drops it. A line such as `neomail: work: 37 labels written, 2 moved to
Junk` says what was done, and `1 held back after a failure (tried again later)` counts the
messages still waiting after a failure; those alone do not make `neomail classify` exit 1. A failed sign-in or session stops that account's writes, and says
`neomail: work: labels not written: WHY` (`spam not moved to Junk` when only moves were due); the
other accounts go on. `neomail classify` exits 1 when a write, a move or an account failed; a
server without Junk and an earlier Proton label that stays are warnings. Classifying, syncing and
notifying go on whatever happens. The lines name the account only, never a subject, a sender or
a label of yours.

## Settings

Under `[classify]`: `label_prefix` (default `nm-`; never empty, and never holding `/`, which
nests a label under another on Gmail and Proton), `labels = false` to write no groups, `junk =
false` to move nothing, and `junk_at` (above 0, at most 1) for the confidence spam needs to move.
Labels already written stay on the messages when writing is turned off, and neomail never renames
or deletes a label.
