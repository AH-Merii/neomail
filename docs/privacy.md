# Privacy: what leaves your machine

neomail keeps your mail on your machine. Syncing reads it from your provider into a local cache.
Only `neomail classify`, which you run, sends anything else anywhere.

`neomail classify` sends a short text per message to jev, the classifier. That text is redacted
first, on your machine, by neomail's own rules: no model and no network call. `neomail redact
ID...` shows exactly what is sent for cached messages, so you can see what would leave before
anything does: `--json` gives it exactly, and the terminal and plain forms show it with
`Subject: ` before the subject, a `Signals:` line, a note where the body was cut, and control
characters made safe. It reads the cache only and sends nothing.

**Consent, once per backend.** Before the first request to a host, `neomail classify` says what
will be sent, to which host and under whose terms, and asks yes or no. A yes is remembered for
that host in `classify-consent` in the data directory (not in the cache, so deleting
`cache.sqlite3` does not lose it, and not in your config file); `--yes` gives it from a script.
Without a yes, nothing is sent. `neomail classify --estimate` never sends anything.

## What will be sent

For each message, `neomail classify` sends one request to the classifier's host
(`classify.url`, `api.typesafe.ai` by default), tried again up to twice on a rate limit or a
server error, with your key as a Bearer token, holding only this:

- **The subject**, redacted, whole.
- **The start of the body**: the body as `neomail show` renders it (the plain-text part, else the
  HTML part as text), redacted whole, then cut to its first 1,500 characters. Redacting before
  cutting means a value cut in half at the edge cannot slip past its rule, and a cut never splits
  a placeholder. 1,500 characters is the excerpt size the PII benchmark measured
  ([DEC-2026-10-11-pii-rules](decisions.md#dec-2026-10-11-pii-rules)). Whether it was cut is sent
  too.
- **The sender's domain**: the part of the From address after the `@`, lower-cased (`acme.example`),
  always ([DEC-2026-10-13-classify-send](decisions.md#dec-2026-10-13-classify-send)). It is sent
  only when it is a domain name; an address literal such as `[192.0.2.1]` is not. A personal
  domain, one that only you or one person uses, identifies its owner.
- **Four signals**, each a yes or no, or one word: whether the sender is a role address (the
  part before the `@` is one of `noreply`, `no-reply`, `donotreply`, `do-not-reply`, `info`,
  `news`, `newsletter`, `support`, `help`, `billing`, `invoice`, `invoices`, `receipts`,
  `orders`, `shipping`, `notifications`, `notification`, `alerts`, `marketing`, `hello`, `team`,
  `jobs`, `careers`, `recruiting`, `account`, `accounts` or `security`, compared without case),
  a person's address, or unknown (no From); whether the mail came from a mailing list
  (List-Unsubscribe or List-Id); whether it is automated (Auto-Submitted other than `no`, or
  Precedence bulk, list or junk); and whether it is a reply (In-Reply-To).

The request never holds the part of the sender's address before the `@`, any other header (display
names among them), attachments or the rest of the body. Names written in the subject and body are
masked only as [What is masked](#what-is-masked) says, so a name its rules miss is sent as written.
One type in nm-redact, `Outgoing`, holds exactly the list above, and the request is built from it
alone ([architecture](architecture.md)).

**What TypeSafe does with it.** jev is TypeSafe's model, and TypeSafe keeps and uses what it
receives as its [privacy policy](https://www.typesafe.ai/legal/privacy-policy) and its
[customer agreement](https://www.typesafe.ai/legal/mca) say. The privacy policy names no fixed
period: TypeSafe keeps personal data "for as long as reasonably necessary to provide you with the
Services, or otherwise in support of our business or commercial purposes". It "will not train or
fine tune any artificial intelligence or machine learning models on your prompts or other
Input", and "will not disclose any Input to a third party other than our service providers".
Its model page says jev is not trained on customer requests or responses, and points enterprise
customers to zero data retention
([docs.typesafe.ai/models](https://docs.typesafe.ai/models#data-handling)).
A local backend you name in `classify.url` (`http://127.0.0.1:…`) keeps everything on your
machine.

**Where answers are kept.** Each answer (the group jev chose, its confidence and probabilities,
the model, the tokens it used and when) is stored in the local cache, `cache.sqlite3` in the data
directory, keyed by account and Message-ID (a hash of the message without one), as soon as it
arrives. It outlives a mailbox rebuild (a UIDVALIDITY change, which gives the mailbox's messages
new ids), so the same mail is never sent twice unless you ask with `--again`. Deleting
`cache.sqlite3` deletes the stored answers too, and the next run sends that mail again. Nothing is
written to your mail server.

**Where the key is kept.** `neomail key set` stores the key in the OS keyring (the Secret Service
or the macOS keychain) under the service `neomail-jev`, one entry per backend host. Without one
there, `JEV_API_KEY` is used. neomail never prints the key.

## What is masked

| What | Becomes | Notes |
| ---- | ------- | ----- |
| Email addresses | `<email>@acme.example` | The local part goes, quoted (`"jo doe"@…`) or not; the domain stays. |
| Your own addresses | `<email>` | Each account's login in the config, when it is an email address, matched without case, masked whole. Without a config file at the default path there are none; a `--config` or `NEOMAIL_CONFIG` file that is missing or invalid is an error. |
| Links | `<link host=acme.example>` or `<link>` | Scheme, user, port, path, query and fragment go; the host stays. A link without a scheme (`acme.example/reset/…`) counts too, in any case. In a link with a scheme or `www.`, a backslash starts a path as `/` does, as browsers read it (`https://acme.example\u\…`). The host is only ASCII letters, digits, dots, hyphens and underscores (or an `[…]` IPv6 address of hex digits, `:` and `.`). When punctuation or a symbol follows it with no space (`https://acme.example—call …`, a comma, `。`) and no `/`, `\`, `?` or `#` comes later in the link, the link ends at the host and the rest goes through the other rules, except after `;`, `%`, `=`, `&`, `+` or `~`, which keep the rest in the link. The whole link becomes a bare `<link>` when the host goes on in letters that aren't ASCII (an internationalised host written in Unicode, such as `https://bücher.example`), when anything else that can't be part of a host comes before a `/`, `\`, `?` or `#` (a `%` in the host, an IPv6 zone), when the user part isn't plain ASCII, when there is no host, when the host is longer than 253 characters, or when it carries personal data: 7 or more digits (a phone or card number) or a known name as one of its words between dots, hyphens and underscores. |
| IBANs | `<iban>` | Only when the ISO 13616 check digits are valid. |
| Card numbers | `<card>` | 13 to 19 digits, optionally grouped by spaces or hyphens, passing the Luhn check. |
| Amounts | `<amount>` | A number with a currency symbol (`$`, `€`, `£` …) or code (`EUR`, `USD` …) before or after it. |
| One-time codes | `<code>` | A 4 to 16 character token holding a digit, or 4 to 16 digits in groups split by spaces, dots or hyphens (`482 917`, `4 8 2 9`), right after a word such as code, OTP, PIN, passcode, password, verification or 2FA. After password, passcode or PIN, a token of 4 to 64 characters, symbols included (`Xk#92!pq7`): any token when a separator comes first (`:`, `#`, `=`, `is`, `was`, a dash, an en or em dash, an arrow, or several of these, as in `password: hunterxyz`), and otherwise only one holding a digit or a symbol, so "Reset your password here" and "Pin this message" stay. A dash joining two words is no separator, so "password-protected" stays too. A token that is itself one of these words is never masked. Sentence punctuation after it stays, except a `!` or `?`, which is taken as part of it. A code right after its word wins over a date (`sort code 12-03-45`). |
| Phone numbers | `<phone>` | International (`+` or `00`) and national forms. Dates and times are never phone numbers. |
| Other numbers | `<number>` | Any other run of 4 or more digits: order numbers, account numbers, a year on its own. |
| Known names | `<name>` | Each capitalised word of 2 or more characters of a display name in From, To, Cc or Reply-To, as a whole word, as written and in capitals; a word written in capitals with 3 or more letters (`SMITH`) also in title case (`Smith`). Apostrophes written `'`, `’`, `‘`, `ʼ`, `´`, a backtick or a full-width `＇` match one another: `O'Brien` in a header masks `O’Brien` or `O´Brien` in the body. |

Names are matched with case: for someone called Will or May, "Will" and "WILL" are masked, but
"will" and "may" are not. Lower-case particles such as "de" or "van" are not masked on their own.
Single-letter initials are not masked. A sender written `MAY` also masks "May" at the start of a
sentence, the cost of catching `Smith` for `SMITH John`. Your own name is masked when the message's
headers carry it as a display name, such as `Ana Lopez <you@outlook.example>` on the To line.

An organisation's display name is treated like a person's. Mail from "The Acme Billing Team" masks
every "The", "Acme", "Billing" and "Team" (and their capitals) in its subject and body, the company
name among them, so a subject can read `<name> <name> receipt`.

Digits from other scripts, such as full-width `０９０ １２３ ４５６`, count as digits for every rule.

When two rules match the same text, the merged mask takes the kind listed first in the table, so a
valid IBAN or card number is never reported as a phone number.

## What is kept, and why

- **Domains and link hosts** (`acme.example`): they say which company a message is from, which is
  most of what a classifier needs. A personal domain does identify you, though: see the gaps. A bare
  domain counts only when it ends in a real top-level domain, and it is kept from the name rule
  only: a run of 4 or more digits inside one is still masked (`shop<number>.example`), so a card or
  phone number written straight before a full stop (`4111….co`) never slips through as a "domain".
- **Dates and times** (`2026-01-16`, `16/01/2026`, `16 January 2026`, `10:30`): they help tell a
  bill from a reminder, and are not phone numbers even when they look like one.
- **Numbers of 1 to 3 digits**, and numbers with thousands separators such as `1,500` or
  `1.250.000`: quantities and short counts carry meaning and rarely identify anyone. A large number
  grouped with spaces, such as `12 500 000`, looks like a phone number and is masked as `<phone>`.
- **Everything else in the text**, so the classifier can read it.

## Gaps

The rules are deliberately simple and fast. Said plainly, they miss some personal data, and mask
some text that could have stayed:

- **Names that appear in no header.** Other people and organisations named only in the body still go
  to jev. On the benchmark sample, the prototype these rules were ported from caught almost no names
  (0.3%) and left 58.6% of PII characters unmasked
  ([DEC-2026-10-11-pii-rules](decisions.md#dec-2026-10-11-pii-rules)); the shipped rules have not
  been measured again. Masking the names from the headers is the first step to close that gap
  ([DEC-2026-10-11-redaction-gaps](decisions.md#dec-2026-10-11-redaction-gaps)).
- **Your own name, when no header carries it.** neomail does not add your name from the config, so
  mail sent to your bare address (newsletters, automated mail) still sends your name if the body
  uses it ("Hi Ana,").
- **Postal addresses** are not masked.
- **Numbers of 1 to 3 digits** stay, a house number among them.
- **Identifier formats the rules do not know**, such as some national ID or passport numbers
  written with letters. The prototype caught 73.1% of IDs on the benchmark sample.
- **Phone numbers** are matched by pattern, not checked against numbering plans: the prototype
  caught 71.4% of them on the benchmark sample, and a number written in an unusual way, such as
  single digits split by spaces with no code word before them, can slip through.
- **Your own address, when it is not your login.** Only each account's login is masked whole. An
  alias, a plus-address (`you+news@…`), a login that is not an address, or no config file at all
  leaves your address as `<email>@yourdomain`, and a personal domain identifies you.
- **Month names in languages other than English** are not recognised as dates, so the day or year
  next to them may be masked as a number.
- **Some links without a scheme.** Without `http://`, `https://` or `www.`, a link counts only when
  its top-level domain is on a built-in list and a `/` follows the host. One with a newer ending
  (`acme.travel/reset/…`), with only a query (`acme.example?token=…`) or with an IP address for a
  host (`192.0.2.10/u/…`) keeps its path and query, apart from what the other rules mask in them.
- **A link's host is kept as written** when it holds fewer than 7 digits and no known name:
  `https://shop12345.example/x` becomes `<link host=shop12345.example>`, and a name in no header
  stays in a host too.
- **An ordinary word after "is" or "was"** following password, passcode or PIN is masked as if it
  were a secret: "Your password was changed yesterday" becomes "Your password was `<code>`
  yesterday", which takes a word from the text a classifier reads.
- **A link whose host isn't plain ASCII** becomes a bare `<link>`, so the classifier does not learn
  which company an internationalised domain belongs to.
- **Text written against a link with no space**, as Chinese and Japanese often are, is masked with
  the link up to the next space when it starts with a letter or a path follows the host:
  `詳細はhttps://acme.exampleです` becomes `詳細は<link>`.
- **Names in scripts without capital letters**, such as Arabic, Hebrew, Chinese, Japanese, Korean or
  Hindi, are never known names, since only capitalised words of a display name are: `سارة أحمد` in
  a header leaves `سارة` in the body.
- **Keywords and currencies in other languages.** The words before a code or password are English
  only, so `كلمة المرور: hunterxyz` keeps its password, and an amount written with `ر.س` or `د.إ` is
  not an amount: its digits are masked only when there are 4 or more.
- **Unusual address forms** keep their local part: an invisible character or a full-width `＠`
  before the `@`, an escaped quote inside a quoted local part, a domain with no dot
  (`jo@localhost`) or an IP address for a domain, two dots in a row, and a percent-encoded `@` in a
  `mailto:` link.
- **Digits hidden in other ways**: groups split by a soft hyphen, a zero-width character or a direction mark, or
  glued to a letter (`x555-1234`), can partly slip through, and superscript or circled digits
  (`⁴¹¹¹`, `①②③④`) are not read as digits.
- **A name written in decomposed Unicode** in the body (an `e` followed by a combining diaeresis)
  doesn't match the same name written composed (`ë`) in a header.

Models that catch names exist, but they cost far more time and memory than the rules
([DEC-2026-10-11-pii-rules](decisions.md#dec-2026-10-11-pii-rules)); one may come later as an
opt-in.

## Try it

```sh
neomail list                       # find a message's id
neomail redact 42                  # its subject and body excerpt, masked
neomail redact 42 --json           # the same, with the mask counts per kind
neomail list | fzf -m | neomail redact -
```

The JSON shape is in [json.md](json.md#neomail-redact-id).
