# Privacy: what leaves your machine

neomail keeps your mail on your machine. Syncing reads it from your provider into a local cache,
and nothing else sends it anywhere today.

Classification into groups (planned) will send a short text per message to jev, the classifier.
That text is redacted first, on your machine, by neomail's own rules: no model and no network
call. `neomail redact ID...` shows that text for cached messages now, so you can see what would
leave before anything does: `--json` gives it exactly, and the terminal and plain forms show it
with `Subject: ` before the subject, a note where the body was cut, and control characters made
safe. It reads the cache only and sends nothing.

## What will be sent

For each message, once classification lands:

- **The subject**, redacted, whole.
- **The start of the body**: the body as `neomail show` renders it (the plain-text part, else the
  HTML part as text), redacted whole, then cut to its first 1,500 characters. Redacting before
  cutting means a value cut in half at the edge cannot slip past its rule, and a cut never splits
  a placeholder. 1,500 characters is the excerpt size the PII benchmark measured
  ([DEC-2026-10-11-pii-rules](decisions.md#dec-2026-10-11-pii-rules)).

Headers, addresses, attachments and the rest of the body are not part of this text. Slice 10 will
decide which other fields go with it, and this page will say so when it does.

## What is masked

| What | Becomes | Notes |
| ---- | ------- | ----- |
| Email addresses | `<email>@acme.example` | The local part goes, quoted (`"jo doe"@…`) or not; the domain stays. |
| Your own addresses | `<email>` | Each account's login in the config, when it is an email address, matched without case, masked whole. Without a config file at the default path there are none; a `--config` or `NEOMAIL_CONFIG` file that is missing or invalid is an error. |
| Links | `<link host=acme.example>` or `<link>` | Scheme, user, port, path, query and fragment go; the host stays. A link without a scheme (`acme.example/reset/…`) counts too, in any case. In a link with a scheme or `www.`, a backslash starts a path as `/` does, as browsers read it (`https://acme.example\u\…`). The host is only ASCII letters, digits, dots, hyphens and underscores (or an `[…]` IPv6 address of hex digits, `:` and `.`). When punctuation or a symbol follows it with no space (`https://acme.example—call …`, a comma, `。`) and no `/`, `\`, `?` or `#` comes later in the link, the link ends at the host and the rest goes through the other rules, except after `;`, `%`, `=`, `&`, `+` or `~`, which keep the rest in the link. The whole link becomes a bare `<link>` when the host goes on in letters that aren't ASCII (an internationalised host written in Unicode, such as `https://bücher.example`), when anything else that can't be part of a host comes before a `/`, `\`, `?` or `#` (a `%` in the host, an IPv6 zone), when the user part isn't plain ASCII, when there is no host, when the host is longer than 253 characters, or when it carries personal data: 7 or more digits (a phone or card number) or a known name as one of its words between dots, hyphens and underscores. |
| IBANs | `<iban>` | Only when the ISO 13616 check digits are valid. |
| Card numbers | `<card>` | 13 to 19 digits, optionally grouped by spaces or hyphens, passing the Luhn check. |
| Amounts | `<amount>` | A number with a currency symbol (`$`, `€`, `£`, `﷼` …) or code (`EUR`, `USD`, `EGP`, `KWD` …, and `SR` for the Saudi riyal) before or after it, or followed by a currency written in Arabic: the dotted symbols (`ر.س`, `د.إ`, `ج.م`, `د.ك` …), `جم` when it ends its word (`120 جم`, but not `3 جمل`, "3 sentences"), or `ريال`, `درهم`, `دينار`, `جنيه`, `ليرة`, `دولار`, `يورو`, their plurals and the accusative ending (`٥٠ ريالاً`), with or without a space (`460ريال`). `ريال` before a club's name is a team, not a currency (`٢-١ ريال مدريد`). |
| One-time codes | `<code>` | A 4 to 16 character token holding a digit, or 4 to 16 digits in groups split by spaces, dots or hyphens (`482 917`, `4 8 2 9`), right after a word such as code, OTP, PIN, passcode, password, verification or 2FA. After password, passcode or PIN, a token of 4 to 64 characters, symbols included (`Xk#92!pq7`): any token when a separator comes first (`:`, `#`, `=`, a dash, an en or em dash, an arrow, or several of these, with or without "is" or "was", as in `password: hunterxyz`), and otherwise only one that looks like a secret: holding a digit or a symbol, or a capital letter after its first letter beside a lower-case one. So "Reset your password here", "Pin this message", "Your password was changed" and "YOUR PASSWORD WAS CHANGED" stay, and "Your password is hunter2" does not. The Arabic keywords work the same way: `رمز` ("code"), `رمز التحقق`, `رمز التفعيل`, `رمز الأمان`, `رمز أحادي الاستخدام` and `كود` before a code; `كلمة المرور`, `كلمة السر`, `الرقم السري`, `الرمز السري` and `رمز الدخول` before a password, also with "your" joined to them (`كلمة مرورك`, `رمزك`, `رقمكم السري`), with `هو` or `هي` for "is", and words such as `المؤقتة` ("temporary") or `الخاص بك` ("your") between (`رمز التحقق الخاص بك هو 482913`). One or two other words may stand between a password keyword and a colon or `هو`/`هي` after them (`كلمة سر الواي فاي: sunshine2026`, "the Wi-Fi password:"), but a token after such words is masked only when it looks like a secret, so `كلمة المرور منتهية الصلاحية: يرجى تحديثها` ("the password has expired: please renew it") stays. A dash joining two words is no separator, so "password-protected" stays too. A token that is itself one of these words is never masked. Sentence punctuation after it stays, Arabic `،` and `؟` among it, except a `!` or `?`, which is taken as part of it. A code right after its word wins over a date (`sort code 12-03-45`). |
| Phone numbers | `<phone>` | International (`+` or `00`) and national forms. Dates and times are never phone numbers. |
| Other numbers | `<number>` | Any other run of 4 or more digits: order numbers, account numbers, a year on its own. |
| Known names | `<name>` | Each capitalised word of 2 or more characters of a display name in From, To, Cc or Reply-To, as a whole word, as written and in capitals; a word written in capitals with 3 or more letters (`SMITH`) also in title case (`Smith`). Apostrophes written `'`, `’`, `‘`, `ʼ`, `´`, a backtick or a full-width `＇` match one another: `O'Brien` in a header masks `O’Brien` or `O´Brien` in the body. In scripts without capital letters (Arabic, Hebrew, Hindi, Korean …), every word of 2 or more letters; see [Arabic mail](#arabic-mail). |

Names are matched with case: for someone called Will or May, "Will" and "WILL" are masked, but
"will" and "may" are not. Lower-case particles such as "de" or "van" are not masked on their own.
Single-letter initials are not masked. A sender written `MAY` also masks "May" at the start of a
sentence, the cost of catching `Smith` for `SMITH John`. Your own name is masked when the message's
headers carry it as a display name, such as `Ana Lopez <you@outlook.example>` on the To line.

An organisation's display name is treated like a person's. Mail from "The Acme Billing Team" masks
every "The", "Acme", "Billing" and "Team" (and their capitals) in its subject and body, the company
name among them, so a subject can read `<name> <name> receipt`. Arabic has no capitals, so the
Arabic words of an organisation's display name would mask those words everywhere in its mail; a
display name holding a word such as `بنك` or `مصرف` ("bank"), `شركة` ("company"), `خدمة`
("service"), `العملاء` ("customers"), `متجر` ("store"), `فريق` ("team"), `دعم` ("support"),
`وزارة` ("ministry"), `هيئة` ("authority"), `مركز` ("centre"), `منصة` ("platform"), `صحيفة`
("newspaper"), `نادي` ("club") or `موقع` ("website") is taken for an organisation, and none of
its Arabic words is masked: mail from `البنك الأهلي` keeps `البنك` and `الأهلي`.

Digits from other scripts, such as Arabic-Indic `٠٥٠ ١٢٣ ٤٥٦٧` or full-width `０９０ １２３ ４５６`, count as
digits for every rule, and the Arabic separators `٫` and `٬` as `.` and `,`. Direction marks
(U+200E, U+200F, U+061C, U+202A to U+202E, U+2066 to U+2069), which Arabic formats set inside
numbers and dates
([CLDR 48](https://github.com/unicode-org/cldr/blob/release-48/common/main/ar.xml)'s `ar` date
pattern `d‏/M‏/y` holds U+200F), are passed over, so a number split by them is masked whole.

### Arabic mail

- **Names.** A word of an Arabic display name is matched without short vowels (`مُحَمَّد`), the
  stretching tatweel (`محـمد`) or a hamza on an alef (`احمد` for `أحمد`), and with `و`, `ب` or `ل`
  joined to its front, `و` before one of them (`ولمحمد`, `وبسارة`, `وللعتيبي`), `لل` before a family
  name's article, or the vocative `يا` (`يامحمد`): `محمد العتيبي` in a header masks `لمحمد`,
  `والعتيبي`, `بالعتيبي` and `للعتيبي`. A name in the accusative matches when written with its
  tanween (`خالداً`), but not without it (`خالدا`), so a sender called `سعيد` leaves `سعيدا`
  ("happy") alone. The whole word is masked, its prefix too. The article itself is never taken
  off, as no given name carries it: a sender called `كريم` leaves `عميلنا الكريم` ("our valued
  customer") alone. `ي` and `ى` stay
  different letters, as do a final `ة` and `ه`, and `آ` and `ا`, so a sender called `علي` never
  masks `على` ("on"), nor `أمل` the word `آمل` ("I hope"). `لعلي` ("perhaps I"), `لدينا` ("we have")
  and `وجود` ("existence") are never read as a prefix and a name. The particles `بن`, `ابن`, `بنت`,
  `أبو`, `أم` and `آل` are not masked alone, nor `الله` or `الدين`; `عبد الله` (or `عبدالله`) is
  masked as one name. Names that are ordinary words cost more than in English: Arabic has no
  capitals, so a sender called `أمل` masks every `أمل` ("hope"), `بأمل` and `وأمل` too, where Will
  masks "Will" but not "will".
- **Codes, passwords and amounts**: see the table.
- **Dates** with an Arabic month name are kept as `16 January 2026` is: the Gregorian names (`يناير`
  … `ديسمبر`), the Levantine and Iraqi names (`كانون الثاني`, `شباط` …), the Maghreb names (`جانفي`,
  `غشت` …) and the Hijri names (`رمضان`, `ذو الحجة` …), from
  [CLDR 48](https://github.com/unicode-org/cldr/tree/release-48/common/main)'s `ar` and regional
  Arabic files, with a Hijri year written `١٤٤٧هـ` or `١٤٤٧ هـ`, and two names paired by a slash
  (`أكتوبر/تشرين الأول`).
  `ذي القعدة` and `ذي الحجة` (the form after a day) and `ربيع الثاني` count too. A numeric date may
  put a Hijri year first (`١٤٤٧/٠٥/١٢هـ`). A month name must start a word or follow one joined
  letter (`وأغسطس`), so `الحساب 2045` ("the account", ending in `اب`) is no date.

When two rules match the same text, the merged mask takes the kind listed first in the table, so a
valid IBAN or card number is never reported as a phone number.

## What is kept, and why

- **Domains and link hosts** (`acme.example`): they say which company a message is from, which is
  most of what a classifier needs. A personal domain does identify you, though: see the gaps. A bare
  domain counts only when it ends in a real top-level domain, and it is kept from the name rule
  only: a run of 4 or more digits inside one is still masked (`shop<number>.example`), so a card or
  phone number written straight before a full stop (`4111….co`) never slips through as a "domain".
- **Dates and times** (`2026-01-16`, `16/01/2026`, `16 January 2026`, `١٦ يناير ٢٠٢٦`, `١٤٤٧/٠٥/١٢`,
  `10:30`): they help tell a bill from a reminder, and are not phone numbers even when they look
  like one.
- **Numbers of 1 to 3 digits**, and numbers with thousands separators such as `1,500`, `1.250.000`
  or `١٬٥٠٠`: quantities and short counts carry meaning and rarely identify anyone. A large number
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
- **Month names in languages other than English and Arabic** are not recognised as dates, so
  the day or year next to them may be masked as a number.
- **Some links without a scheme.** Without `http://`, `https://` or `www.`, a link counts only when
  its top-level domain is on a built-in list and a `/` follows the host. One with a newer ending
  (`acme.travel/reset/…`), with only a query (`acme.example?token=…`) or with an IP address for a
  host (`192.0.2.10/u/…`) keeps its path and query, apart from what the other rules mask in them.
- **A link's host is kept as written** when it holds fewer than 7 digits and no known name:
  `https://shop12345.example/x` becomes `<link host=shop12345.example>`, and a name in no header
  stays in a host too.
- **A password with no digit, symbol or inner capital, or in capitals only, after "is" or "was"**
  (or `هو`, `هي`) with no colon or dash: "Your password is hunterxyz" and "Your password is
  HUNTERXYZ" keep it, the price of keeping "Your password was changed" and "YOUR PASSWORD WAS
  CHANGED".
- **A link whose host isn't plain ASCII** becomes a bare `<link>`, so the classifier does not learn
  which company an internationalised domain belongs to.
- **Text written against a link with no space**, as Chinese and Japanese often are, is masked with
  the link up to the next space when it starts with a letter or a path follows the host:
  `詳細はhttps://acme.exampleです` becomes `詳細は<link>`.
- **Names in Chinese and Japanese**, written without spaces, are masked only when a header's
  name stands alone in the body, which it rarely does. Hebrew and Korean names are masked as whole
  words only, so a name with a joined Hebrew prefix (`ודנה`) or a Korean ending (`민수님`) stays.
- **An Arabic name spelled differently** from the header beyond vowels, tatweel and hamza: a final
  `ى` written for `ي`, `ه` for a final `ة`, `ا` for `آ` (`اية` for `آية`), `ف`
  or `ك` joined to its front, the article on a name the header gives without it, a suffix
  (`محمدنا`), or an Arabic name the header gives only in Latin letters (`Mohammed`), or the other
  way round.
- **Keywords and currencies in other languages** besides English and Arabic: a password after
  `Passwort:` or `mot de passe :` keeps it, and an amount with a currency word the table does not
  list (`120 lire`) is not an amount: its digits are masked only when there are 4 or more.
- **A person's name in an organisation's display name**: `سارة من البنك الأهلي` holds `البنك`,
  so `سارة` is not masked from it. A sender whose name holds only a brand (`أرامكو السعودية`)
  masks its words everywhere, `السعودية` among them.
- **Arabic keywords after the code** (`482913 هو رمز التحقق`) are not read, so a code of 1 to 3
  digits or with letters stays.
- **Unusual address forms** keep their local part: an invisible character or a full-width `＠`
  before the `@`, an escaped quote inside a quoted local part, a domain with no dot
  (`jo@localhost`) or an IP address for a domain, two dots in a row, and a percent-encoded `@` in a
  `mailto:` link.
- **Digits hidden in other ways**: groups split by a soft hyphen or a zero-width space, or
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
