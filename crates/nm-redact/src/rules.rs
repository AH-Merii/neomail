//! The rules. Each detector works in byte offsets and adds spans to [`Found`]; [`redact`] merges
//! them and writes the placeholders.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::{Kind, Known, Mask, Redacted};

/// ISO 4217 codes, with the Arab currencies' (`EGP`, `KWD` …) and `SR`, the Latin abbreviation
/// Saudi shops print for the riyal. Sudan's `SDG` is left out: "SDG 5", one of the UN's goals, is
/// commoner in mail.
const CUR_CODE: &str = "USD|EUR|GBP|CHF|JPY|CNY|INR|CAD|AUD|NZD|SEK|NOK|DKK|PLN|CZK|HUF|BRL|MXN|ZAR|SGD|HKD\
     |AED|SAR|EGP|KWD|QAR|OMR|BHD|JOD|MAD|TND|DZD|IQD|LBP|LYD|YER|SYP|SR";
const CUR_SYM: &str = r"[$€£¥₹₩₽\u{fdfc}\u{20c1}]|(?:US|CA|AU|NZ|HK|SG|A|C|R)\$";
// Thousands separators: comma, dot, apostrophe, no-break and narrow no-break space (not a plain
// space, which would join neighbouring numbers).
const NUM: &str = r"\d{1,3}(?:[,.'\u{a0}\u{202f}]\d{3})+(?:[.,]\d{1,2})?|\d+(?:[.,]\d{1,2})?";
const MAG: &str = r"(?:\s?(?:[kKmM]|bn|mn|million|billion|thousand)(?-u:\b))?";
const CUR_WORD: &str = "euros?|dollars?|pounds?|francs?|kr|kronor|kroner|zł";
const MONTH: &str = "jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?\
                     |sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?";
const CODE_WORD: &str = "code|otp|pin|passcode|password|verification|2fa";
/// Keywords whose next token is a secret whatever it holds: a password need not hold a digit.
const SECRET_WORD: &str = "pin|passcode|password";
/// Currencies written in Arabic, after the number as CLDR's Arabic formats put them: the dotted
/// symbols (`ر.س`, `د.إ`, `ج.م` …, from CLDR's `ar` currency data), the words (`ريال`, `درهم`,
/// `دينار`, `جنيه`, `ليرة`, `دولار`, `يورو`, with their plurals and the accusative ending `اً`, `ًا`
/// or `ً` that follows the numbers 11 to 99: `٥٠ ريالاً`) and the rial signs U+FDFC and U+20C1.
/// Longer forms first, as the regex takes the first alternative that matches.
const CUR_AR: &str = "(?:ر\\.\\s?[سقعي]|د\\.\\s?[إكبأمجتعل]|ج\\.\\s?[مس]|ل\\.\\s?[سل])\\.?\
                      |(?:ريالات|ريال|دراهم|درهم|دنانير|دينار|جنيهات|جنيه|ليرات|ليرة|دولارات|دولار\
                      |يورو)(?:ا\u{64b}?|\u{64b}ا?)?|\u{fdfc}|\u{20c1}";
/// Month names written in Arabic, from CLDR 48: the Gregorian names of `ar`, the Levantine and
/// Iraqi names of `ar_SY`, `ar_LB`, `ar_IQ` and `ar_JO`, the Maghreb names of `ar_DZ`, `ar_TN` and
/// `ar_MA`, and the Hijri names of `ar`. A hamza or madda on an alef may be left off. `ذي` is the
/// form of `ذو` after a day (`٢٠ ذي القعدة`), and `ربيع الثاني` a common name for `ربيع الآخر`.
const MONTH_AR: &str = "يناير|فبراير|مارس|[أا]بريل|مايو|يونيو|يوليوز|يوليو|[أا]غسطس|سبتمبر\
                        |[أا]كتوبر|نوفمبر|ديسمبر|كانون\\s+الثاني|كانون\\s+ال[أا]ول|شباط|[آا]ذار\
                        |نيسان|[أا]يار|حزيران|تموز|[آا]ب|[أا]يلول|تشرين\\s+ال[أا]ول\
                        |تشرين\\s+الثاني|جانفي|فيفري|[أا]فريل|ماي|جوان|جويلية|[أا]وت|غشت|شتنبر\
                        |نونبر|دجنبر|محرم|صفر|ربيع\\s+ال[أا]ول|ربيع\\s+ال[آا]خر\
                        |جمادى\\s+ال[أا]ولى|جمادى\\s+ال[آا]خرة|رجب|شعبان|رمضان|شوال\
                        |ربيع\\s+الثاني|ذ[وي]\\s+القعدة|ذ[وي]\\s+الحجة";
/// Arabic words for a one-time code: `رمز` ("code") alone, as `رمزك` or `رمزكم` ("your code"), or
/// with the kind of code, and `كود`. `الخاص بك` ("your") may stand before `هو` ("is").
const CODE_AR: &str = "(?:ال)?رمز(?:كم|ك)?(?:\\s+(?:التحقق|التفعيل|الدخول|ال[أا]مان|المرور|[أا]حادي\\s+الاستخدام))?\
                       |كود(?:\\s+التحقق|\\s+التفعيل)?";
/// Arabic words for a password or PIN, with or without `ك` or `كم` ("your": `كلمة مرورك`), then
/// words that may stand between them and the secret: "temporary", "new", "current", "your", "for
/// one time".
const SECRET_AR: &str = "(?:كلمة\\s+(?:ال)?(?:مرور|سر)(?:كم|ك)?|(?:ال)?رقم(?:كم|ك)?\\s+(?:ال)?سري\
                         |(?:ال)?رمز(?:كم|ك)?\\s+(?:ال)?سري\
                         |رمز\\s+الدخول)(?:\\s+(?:المؤقتة|المؤقت|الجديدة|الجديد|الحالية|الحالي\
                         |الخاصة\\s+بك|الخاص\\s+بك|لمرة\\s+واحدة))*";
/// A separator between an Arabic password keyword and the secret: a symbol, or `هو` or `هي` ("is").
const AR_SEP: &str = r"(?:\s*(?:[:#=>–—→-]|(?:هو|هي)(?:[\s:]|$)))";
/// The separators allowed after other words between the keyword and the secret: only a colon or
/// "is", so "your password expires soon - renew it" is no password. Even then only a token that
/// looks like a secret is one: `كلمة المرور منتهية الصلاحية: يرجى تحديثها` ("the password has
/// expired: please renew it") keeps its last words.
const AR_SEP_AFTER_WORDS: &str = r"(?:\s*(?::|(?:هو|هي)(?:[\s:]|$)))";
/// Direction marks (U+200E, U+200F, U+061C), embeddings and overrides (U+202A to U+202E) and
/// isolates (U+2066 to U+2069): invisible, and set inside numbers and around currency symbols by
/// CLDR's Arabic formats.
const MARKS: &str = "\u{200e}\u{200f}\u{61c}\u{202a}-\u{202e}\u{2066}-\u{2069}";
/// File extensions that look like a top-level domain: `report.pdf` is not a host.
const FILE_EXT: &[&str] = &[
    "pdf", "txt", "doc", "docx", "xls", "xlsx", "csv", "png", "jpg", "jpeg", "gif", "zip", "htm",
    "html", "eml", "ics", "json", "xml", "md",
];

/// The 2-letter country-code top-level domains (ISO 3166-1 alpha-2, plus `ac`, `eu`, `su`, `uk`),
/// sorted for a binary search.
const CC_TLD: &[&str] = &[
    "ac", "ad", "ae", "af", "ag", "ai", "al", "am", "ao", "aq", "ar", "as", "at", "au", "aw", "ax",
    "az", "ba", "bb", "bd", "be", "bf", "bg", "bh", "bi", "bj", "bl", "bm", "bn", "bo", "bq", "br",
    "bs", "bt", "bv", "bw", "by", "bz", "ca", "cc", "cd", "cf", "cg", "ch", "ci", "ck", "cl", "cm",
    "cn", "co", "cr", "cu", "cv", "cw", "cx", "cy", "cz", "de", "dj", "dk", "dm", "do", "dz", "ec",
    "ee", "eg", "eh", "er", "es", "et", "eu", "fi", "fj", "fk", "fm", "fo", "fr", "ga", "gb", "gd",
    "ge", "gf", "gg", "gh", "gi", "gl", "gm", "gn", "gp", "gq", "gr", "gs", "gt", "gu", "gw", "gy",
    "hk", "hm", "hn", "hr", "ht", "hu", "id", "ie", "il", "im", "in", "io", "iq", "ir", "is", "it",
    "je", "jm", "jo", "jp", "ke", "kg", "kh", "ki", "km", "kn", "kp", "kr", "kw", "ky", "kz", "la",
    "lb", "lc", "li", "lk", "lr", "ls", "lt", "lu", "lv", "ly", "ma", "mc", "md", "me", "mf", "mg",
    "mh", "mk", "ml", "mm", "mn", "mo", "mp", "mq", "mr", "ms", "mt", "mu", "mv", "mw", "mx", "my",
    "mz", "na", "nc", "ne", "nf", "ng", "ni", "nl", "no", "np", "nr", "nu", "nz", "om", "pa", "pe",
    "pf", "pg", "ph", "pk", "pl", "pm", "pn", "pr", "ps", "pt", "pw", "py", "qa", "re", "ro", "rs",
    "ru", "rw", "sa", "sb", "sc", "sd", "se", "sg", "sh", "si", "sj", "sk", "sl", "sm", "sn", "so",
    "sr", "ss", "st", "su", "sv", "sx", "sy", "sz", "tc", "td", "tf", "tg", "th", "tj", "tk", "tl",
    "tm", "tn", "to", "tr", "tt", "tv", "tw", "tz", "ua", "ug", "uk", "um", "us", "uy", "uz", "va",
    "vc", "ve", "vg", "vi", "vn", "vu", "wf", "ws", "ye", "yt", "za", "zm", "zw",
];
/// Generic top-level domains common in mail, and names from IANA's Special-Use Domain Names
/// registry (RFC 6761) for examples, tests and local use.
const GENERIC_TLD: &[&str] = &[
    "com", "net", "org", "edu", "gov", "mil", "int", "info", "biz", "pro", "aero", "coop",
    "museum", "mobi", "app", "dev", "page", "xyz", "online", "site", "shop", "store", "tech",
    "cloud", "blog", "email", "example", "test", "invalid", "local", "arpa", "onion",
];

/// `host`'s top-level domain, lower-cased: `ACME.EXAMPLE` and `Acme.Com` are hosts too.
fn tld(host: &str) -> String {
    host.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

/// Whether `host` ends in a top-level domain from [`CC_TLD`] or [`GENERIC_TLD`], in any case, so
/// `x.thanks` or `4111….ok` (a full stop with no space after it) is not taken for a host.
fn real_tld(host: &str) -> bool {
    let tld = tld(host);
    CC_TLD.binary_search(&tld.as_str()).is_ok() || GENERIC_TLD.contains(&tld.as_str())
}

/// Characters RFC 5322 allows in an unquoted local part (`atext`, section 3.2.3), besides letters
/// and digits.
const ATEXT: &str = "!#$%&'*+/=?^_`{|}~-";

struct Patterns {
    url: Regex,
    email: Regex,
    url_schemeless: Regex,
    host_goes_on: Regex,
    domain: Regex,
    date_iso: Regex,
    date_numeric: Regex,
    date_month: Regex,
    date_month_ar: Regex,
    iban: Regex,
    card: Regex,
    amount: Regex,
    code: Regex,
    secret: Regex,
    code_ar: Regex,
    secret_ar: Regex,
    phone_intl: Regex,
    phone_national: Regex,
    digits: Regex,
    digit: Regex,
    to_fold: Regex,
    word: Regex,
}

#[expect(
    clippy::expect_used,
    reason = "constant patterns, compiled by every test"
)]
fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a constant pattern compiles")
}

/// Compiled once per process. Word boundaries are ASCII (`(?-u:\b)`): a Unicode `\b` stops the
/// regex crate's lazy DFA on non-ASCII text and made an excerpt about six times slower. The cost is
/// that a non-ASCII letter counts as a boundary, so `ü4111…` can match: that masks more, not less.
static P: LazyLock<Patterns> = LazyLock::new(|| {
    let labels = r"(?:[a-zA-Z0-9](?:[a-zA-Z0-9-]*[a-zA-Z0-9])?\.)+";
    // A link's path makes it a host in any case (`Acme.Com/u/x`). A bare domain's top-level domain
    // is all lower or all upper case, so a missing space in `Ask Will.It works` keeps the name rule.
    let link_host = format!("{labels}[a-zA-Z]{{2,24}}");
    let domain_host = format!("{labels}(?:[a-z]{{2,24}}|[A-Z]{{2,24}})");
    let atext = r"[\w!#$%&'*+/=?^`{|}~-]";
    Patterns {
        url: re(r#"(?i)(?-u:\b)(?:(?:https?|ftps?|wss?)://|www\.)[^\s<>"`]+"#),
        // RFC 5322 section 3.4.1: a dot-atom or a quoted string before the `@`.
        email: re(&format!(
            r#"(?:"[^"\r\n]{{1,64}}"|{atext}+(?:\.{atext}+)*)@(?:[\w-]+\.)+\p{{L}}{{2,}}"#
        )),
        url_schemeless: re(&format!(r#"(?-u:\b){link_host}(?::\d{{1,5}})?/[^\s<>"`]*"#)),
        // A letter or digit straight after a link's ASCII host, past any marks and invisible
        // format characters: the host goes on in Unicode (`bücher.example`), or text is written
        // against it. Invisible characters alone, such as a zero-width space, don't count.
        host_goes_on: re(r"^[\p{M}\p{Cf}]*[\p{L}\p{N}]"),
        domain: re(&format!(r"(?-u:\b){domain_host}(?-u:\b)")),
        date_iso: re(
            r"(?-u:\b)(?:19|20|14)\d{2}[-/.]\d{1,2}[-/.]\d{1,2}(?:[T ]\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?",
        ),
        date_numeric: re(r"(?-u:\b)\d{1,2}[-/.]\d{1,2}[-/.](?:\d{4}|\d{2})(?-u:\b)"),
        date_month: re(&format!(
            r"(?i)(?-u:\b)(?:\d{{1,2}}(?:st|nd|rd|th)?\s+)?(?:{MONTH})\.?(?:\s+\d{{1,2}}(?:st|nd|rd|th)?)?,?\s+(?:19|20)\d{{2}}(?-u:\b)"
        )),
        // The year may carry the Hijri era, joined or after a space (`١٤٤٧هـ`, `١٤٤٧ هـ`): the era is
        // letters, which no rule masks. A slash may pair two names (`أكتوبر/تشرين الأول`).
        date_month_ar: re(&format!(
            r"(?:(?-u:\b)\d{{1,2}}\s+)?(?:{MONTH_AR})(?:\s*/\s*(?:{MONTH_AR}))?\s*[،,]?\s+(?:19|20|14)\d{{2}}(?-u:\b)"
        )),
        iban: re(r"(?-u:\b)[A-Z]{2}[0-9]{2}"),
        card: re(r"(?-u:\b)\d(?:[ -]?\d){12,18}(?-u:\b)"),
        amount: re(&format!(
            r"(?:{CUR_SYM}|(?-u:\b)(?:{CUR_CODE}))\s?(?:{NUM}){MAG}|(?-u:\b)(?:{NUM}){MAG}\s?(?:[€£$¥₹]|(?:{CUR_CODE}|{CUR_WORD})(?-u:\b)|(?:{CUR_AR}))"
        )),
        code: re(&format!(
            r"(?i)(?-u:\b)(?:{CODE_WORD})(?:\s+(?:{CODE_WORD}))*(?-u:\b)(?:\s+(?:is|was))?\s*[:#=\-]?\s*(\d{{2,4}}(?:[ .\-]\d{{2,4}}){{1,3}}|\d(?:[ .\-]\d){{3,15}}|[a-z0-9][a-z0-9\-]{{2,14}}[a-z0-9])(?-u:\b)"
        )),
        // The keyword and any separators after it; the token is read after the match, so a
        // keyword inside it (`password password: x`) is found again.
        secret: re(&format!(
            r"(?i)(?-u:\b)(?:{SECRET_WORD})(?-u:\b)((?:\s*(?:[:#=>–—→-]|(?-u:\b)(?:is|was)(?-u:\b)))*)\s*"
        )),
        // Arabic keywords take no ASCII word boundary, which never holds before an Arabic letter.
        code_ar: re(&format!(
            r"(?:{CODE_AR})(?:\s+الخاص\s+بك)?(?:\s+(?:هو|هي))?\s*[:#=\-]?\s*(\d{{2,4}}(?:[ .\-]\d{{2,4}}){{1,3}}|\d(?:[ .\-]\d){{3,15}}|[a-zA-Z0-9][a-zA-Z0-9\-]{{2,14}}[a-zA-Z0-9])(?-u:\b)"
        )),
        // One or two other words may stand between the keyword and a separator after them: "the
        // password of your account:", `كلمة سر الواي فاي:` ("the Wi-Fi password:"). That separator
        // is not captured, so [`codes`] masks only a token that looks like a secret after it.
        // `هو` or `هي` ("is") before a colon is no other word: tried first, it is captured.
        secret_ar: re(&format!(
            r"(?:{SECRET_AR})(?:((?:\s*(?:هو|هي))?\s*:{AR_SEP}*)|(?:\s+[\p{{L}}\p{{M}}]+){{1,2}}{AR_SEP_AFTER_WORDS}+|({AR_SEP}*))\s*"
        )),
        phone_intl: re(r"(?:\+|(?-u:\b)00)\d[\d \-.()/]{5,20}\d(?-u:\b)"),
        phone_national: re(
            r"(?:\(\d{2,5}\)\s?|(?-u:\b)\d{2,5}[ .\-/])\d{2,4}(?:[ .\-/]\d{2,5}){0,3}(?-u:\b)",
        ),
        digits: re(r"\d{4,}"),
        digit: re(r"^\d$"),
        to_fold: re(&format!(r"[\d--0-9]|[\u{{66b}}\u{{66c}}{MARKS}]")),
        word: re(r"[\p{L}\p{M}\p{N}]+(?:['’‘´`＇][\p{L}\p{M}\p{N}]+)*"),
    }
});

/// The words of `text` as the name rule sees them: letters, marks and digits, with inner
/// apostrophes (`O'Brien`). Hyphens, dots and spaces separate words (`Jean-Luc` is two).
pub(crate) fn words(text: &str) -> impl Iterator<Item = &str> + '_ {
    P.word.find_iter(text).map(|m| m.as_str())
}

/// The characters written for an apostrophe besides `'`: `’` (U+2019, as HTML mail is usually
/// rendered), `‘` (U+2018), `ʼ` (U+02BC), the acute accent `´` (U+00B4, from Spanish and Portuguese
/// keyboards), a backtick and the full-width `＇` (U+FF07).
const APOSTROPHES: [char; 6] = ['’', '‘', 'ʼ', '´', '`', '＇'];

/// `word` with each other form of apostrophe ([`APOSTROPHES`]) written as `'`, so a header's
/// `O'Brien` and a body's `O’Brien` or `O´Brien` are one name.
pub(crate) fn fold_apostrophes(word: &str) -> Cow<'_, str> {
    if word.contains(APOSTROPHES) {
        Cow::Owned(word.replace(APOSTROPHES, "'"))
    } else {
        Cow::Borrowed(word)
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    kind: Kind,
    /// For [`Kind::Link`]: the host's byte range in the input.
    host: Option<(usize, usize)>,
}

/// Disjoint byte ranges, merged as they are added, so an overlap check is one lookup however many
/// ranges a long body holds.
#[derive(Default)]
struct Ranges(BTreeMap<usize, usize>);

impl Ranges {
    /// Whether `start..end` overlaps a range. The ranges are disjoint, so the one starting last
    /// before `end` also ends last.
    fn overlaps(&self, start: usize, end: usize) -> bool {
        self.0
            .range(..end)
            .next_back()
            .is_some_and(|(_, &e)| start < e)
    }

    /// Whether one range holds all of `start..end`.
    fn contains(&self, start: usize, end: usize) -> bool {
        self.0
            .range(..=start)
            .next_back()
            .is_some_and(|(_, &e)| end <= e)
    }

    fn insert(&mut self, mut start: usize, mut end: usize) {
        if start >= end {
            return;
        }
        if let Some((&s, &e)) = self.0.range(..=start).next_back()
            && start <= e
        {
            self.0.remove(&s);
            start = s;
            end = end.max(e);
        }
        while let Some((&s, &e)) = self.0.range(start..=end).next() {
            self.0.remove(&s);
            end = end.max(e);
        }
        self.0.insert(start, end);
    }
}

/// Spans found so far, plus the ranges that later rules must leave alone.
#[derive(Default)]
struct Found {
    spans: Vec<Span>,
    /// Links, addresses and dates: no later rule masks inside them.
    kept: Ranges,
    /// Bare domains: kept from the name rule only. A card, IBAN, phone or digit run written
    /// straight before a full stop (`4111….co`) is still masked: `<number>.co` costs less than a
    /// leaked card.
    domains: Ranges,
    /// Codes: an amount wholly inside one adds nothing, so it is dropped and the code keeps its
    /// kind (`password: Ab$12345!` is a `<code>`, not an `<amount>`). An amount reaching past a
    /// code still counts, so nothing outside the code goes out; a card or IBAN still names the mask.
    codes: Ranges,
}

impl Found {
    fn is_kept(&self, start: usize, end: usize) -> bool {
        self.kept.overlaps(start, end)
    }

    /// Adds a span unless it touches a kept range (or a bare domain, for a name, or lies inside a
    /// code, for an amount); whether it was added.
    fn push(&mut self, start: usize, end: usize, kind: Kind) -> bool {
        let blocked = self.is_kept(start, end)
            || (kind == Kind::Name && self.domains.overlaps(start, end))
            || (kind == Kind::Amount && self.codes.contains(start, end));
        if !blocked {
            self.spans.push(Span {
                start,
                end,
                kind,
                host: None,
            });
        }
        !blocked
    }

    fn keep(&mut self, start: usize, end: usize) {
        self.kept.insert(start, end);
    }
}

/// Masks every match in `text`. Digits from other scripts (`٠٥٠ ١٢٣ ٤٥٦٧`, `０９０`) and Arabic
/// number separators are folded to ASCII first, and direction marks dropped, so every rule's digit
/// counts, checksums and word boundaries see them as digits (see [`fold_digits`]).
pub(crate) fn redact(text: &str, known: &Known) -> Redacted {
    let Some(fold) = fold_digits(text) else {
        return write(text, find(text, known));
    };
    let spans = find(&fold.text, known)
        .into_iter()
        .map(|s| Span {
            start: fold.start(s.start),
            end: fold.end(s.end),
            kind: s.kind,
            host: s.host.map(|(hs, he)| (fold.start(hs), fold.end(he))),
        })
        .collect();
    write(text, spans)
}

/// The value of a decimal digit in any script, as the patterns' Unicode `\d` (`\p{Nd}`) matches
/// it. Unicode encodes each set of decimal digits as one contiguous run from 0 to 9 (the Unicode
/// Character Encoding Stability Policy), so a digit's value is its distance from the start of its
/// run of digits, modulo 10.
pub(crate) fn digit(c: char) -> Option<u8> {
    if c.is_ascii_digit() {
        return u8::try_from(c).ok().map(|b| b - b'0');
    }
    let is_digit = |c: char| P.digit.is_match(c.encode_utf8(&mut [0; 4]));
    if !is_digit(c) {
        return None;
    }
    let mut start = u32::from(c);
    while let Some(p) = start
        .checked_sub(1)
        .and_then(char::from_u32)
        .filter(|&p| is_digit(p))
    {
        start = u32::from(p);
    }
    u8::try_from((u32::from(c) - start) % 10).ok()
}

/// Whether `c` is a direction mark, embedding, override or isolate ([`MARKS`]).
fn is_mark(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{61c}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A text as the rules see it, and where each of its bytes came from.
struct Folded {
    text: String,
    /// For each byte of `text`, the offset in the input of the character it came from.
    starts: Vec<usize>,
    /// For each byte of `text`, the offset in the input just after that character.
    ends: Vec<usize>,
    len: usize,
}

impl Folded {
    fn start(&self, at: usize) -> usize {
        self.starts.get(at).copied().unwrap_or(self.len)
    }

    fn end(&self, at: usize) -> usize {
        at.checked_sub(1).map_or(0, |i| self.ends[i])
    }
}

/// `text` as the rules see it: every non-ASCII decimal digit as its ASCII digit, the Arabic
/// decimal and thousands separators (U+066B, U+066C) as `.` and `,`, and the direction marks
/// ([`is_mark`]) dropped, so `٠٥٠‎-١٢٣‎-٤٥٦٧` is one phone number and `١٬٢٥٠٫٥٠` reads as
/// `1,250.50`. A mark between two letters stays, so it still splits two words written against
/// each other. `None` when there is nothing to fold.
fn fold_digits(text: &str) -> Option<Folded> {
    if !P.to_fold.is_match(text) {
        return None;
    }
    let mut f = Folded {
        text: String::with_capacity(text.len()),
        starts: Vec::with_capacity(text.len()),
        ends: Vec::with_capacity(text.len()),
        len: text.len(),
    };
    // Where the run of marks being read ends, and whether a letter follows it: read once per run,
    // so a long run of marks costs linear time.
    let mut run: Option<(usize, bool)> = None;
    for (i, c) in text.char_indices() {
        let end = i + c.len_utf8();
        let ascii = match c {
            '\u{66b}' => Some(b'.'),
            '\u{66c}' => Some(b','),
            c if is_mark(c) => {
                let before = f.text.chars().next_back().is_some_and(char::is_alphabetic);
                let after = match run {
                    Some((run_end, letter)) if i < run_end => letter,
                    _ => {
                        let rest = &text[end..];
                        let next = rest.char_indices().find(|&(_, c)| !is_mark(c));
                        let run_end = next.map_or(text.len(), |(j, _)| end + j);
                        let letter = next.is_some_and(|(_, c)| c.is_alphabetic());
                        run = Some((run_end, letter));
                        letter
                    }
                };
                if !(before && after) {
                    continue;
                }
                None
            }
            c if !c.is_ascii() => digit(c).map(|d| b'0' + d),
            _ => None,
        };
        match ascii {
            Some(b) => {
                f.text.push(char::from(b));
                f.starts.push(i);
                f.ends.push(end);
            }
            None => {
                f.text.push(c);
                f.starts.extend(i..end);
                f.ends.extend(std::iter::repeat_n(end, c.len_utf8()));
            }
        }
    }
    Some(f)
}

/// Every span to mask in `text`, merged.
fn find(text: &str, known: &Known) -> Vec<Span> {
    let mut f = Found::default();
    links(text, known, &mut f);
    emails(text, known, &mut f);
    links_schemeless(text, known, &mut f);
    domains(text, &mut f);
    // A code follows its keyword, so it wins over a date's keep (`sort code 12-03-45`).
    codes(text, &mut f);
    dates(text, &mut f);
    ibans(text, &mut f);
    cards(text, &mut f);
    for m in P.amount.find_iter(text) {
        if !real_club(m.as_str(), &text[m.end()..]) {
            f.push(m.start(), m.end(), Kind::Amount);
        }
    }
    phones(text, &mut f);
    for m in P.digits.find_iter(text) {
        f.push(m.start(), m.end(), Kind::Number);
    }
    names(text, known, &mut f);
    merge(f.spans)
}

/// Spanish clubs whose name starts with `ريال` ("Real"), as sports news writes them.
const REAL_CLUBS: [&str; 6] = [
    "مدريد",
    "سوسيداد",
    "بيتيس",
    "مايوركا",
    "سرقسطة",
    "بلد الوليد",
];

/// Whether an amount match ending in `ريال` is a score before a club's name: `٢-١ ريال مدريد`.
fn real_club(amount: &str, after: &str) -> bool {
    amount.ends_with("ريال") && {
        let next = after.trim_start();
        next.len() < after.len() && REAL_CLUBS.iter().any(|c| next.starts_with(c))
    }
}

/// How each kind is spelled; [`Kind::Link`] adds its host.
fn placeholder(kind: Kind) -> &'static str {
    match kind {
        Kind::Email => "<email>",
        Kind::Link => "<link>",
        Kind::Iban => "<iban>",
        Kind::Card => "<card>",
        Kind::Amount => "<amount>",
        Kind::Code => "<code>",
        Kind::Phone => "<phone>",
        Kind::Number => "<number>",
        Kind::Name => "<name>",
    }
}

fn write(text: &str, spans: Vec<Span>) -> Redacted {
    let mut out = String::with_capacity(text.len());
    let mut masks = Vec::with_capacity(spans.len());
    let mut last = 0;
    for s in spans {
        out.push_str(&text[last..s.start]);
        let start = out.len();
        match (s.kind, s.host) {
            (Kind::Link, Some((hs, he))) => {
                out.push_str("<link host=");
                out.push_str(&text[hs..he]);
                out.push('>');
            }
            (kind, _) => out.push_str(placeholder(kind)),
        }
        masks.push(Mask {
            range: start..out.len(),
            kind: s.kind,
        });
        last = s.end;
    }
    out.push_str(&text[last..]);
    Redacted { text: out, masks }
}

/// Union of overlapping spans; a merged span takes the earliest kind (and its host).
fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.retain(|s| s.start < s.end);
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end), s.kind));
    let mut out: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match out.last_mut() {
            Some(l) if s.start < l.end => {
                l.end = l.end.max(s.end);
                if s.kind < l.kind {
                    l.kind = s.kind;
                    l.host = s.host;
                }
            }
            _ => out.push(s),
        }
    }
    out
}

fn char_before(text: &str, at: usize) -> Option<char> {
    text[..at].chars().next_back()
}

fn char_after(text: &str, at: usize) -> Option<char> {
    text[at..].chars().next()
}

/// Drops trailing punctuation that ends the sentence rather than the URL, and a closing bracket
/// that has no opening one inside the URL. The brackets are counted once, so a URL ending in a
/// long run of `)` costs linear time.
fn trim_url(u: &str) -> &str {
    let count = |b: u8| u.bytes().filter(|&x| x == b).count();
    let (round, square) = (count(b'('), count(b'['));
    let (mut round_close, mut square_close) = (count(b')'), count(b']'));
    let mut end = u.len();
    while let Some(c) = u[..end].chars().next_back() {
        let strip = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '"' | '*' => true,
            ')' => round < round_close,
            ']' => square < square_close,
            _ => false,
        };
        if !strip {
            break;
        }
        match c {
            ')' => round_close -= 1,
            ']' => square_close -= 1,
            _ => {}
        }
        end -= c.len_utf8();
    }
    &u[..end]
}

/// Where a URL's parts are, as byte offsets into it.
struct LinkParts {
    /// The host: a run of ASCII letters, digits, dots, hyphens and underscores, or one `[…]` IPv6
    /// literal. `None` when the host can't be read that way, and the whole URL is a bare `<link>`.
    host: Option<(usize, usize)>,
    /// Where the link ends: the end of the URL when a path, query or fragment, [`URL_GOES_ON`] or
    /// nothing follows the host and port; else straight after them, so the rest of the text
    /// (`https://a.example—call …`, `https://a.example。電話…`) goes through the other rules instead
    /// of into the host.
    end: usize,
}

/// What, straight after a host (and port), keeps the rest in the link: a path, query or fragment,
/// path parameters (`;jsessionid=…`) and percent-encoding.
const URL_GOES_ON: [char; 9] = ['/', '?', '#', ';', '%', '=', '&', '+', '~'];

/// What ends a URL's host and starts its path, query or fragment. A backslash counts as `/` does:
/// the URL Standard reads it so in http, https, ws, wss and ftp URLs
/// (<https://url.spec.whatwg.org/#authority-state>), and browsers follow it.
const PATH: [char; 4] = ['/', '\\', '?', '#'];

/// Characters RFC 3986 (section 3.2.1) allows in a URL's userinfo, besides letters and digits.
const USERINFO: &str = "-._~%!$&'()*+,;=:";

/// The host and end of the URL `u`, which starts with a scheme or `www.`.
///
/// It fails closed: the whole URL is masked with no host, so none of its path or query goes out,
/// when the host goes on in letters that aren't ASCII (`https://bücher.example`, `https://例え.jp`),
/// when something else that can't be part of a plain host (`%61cme.example`, an IPv6 zone, user
/// info with a non-ASCII letter) comes before a [`PATH`] character, or when there is no host at
/// all. Only when punctuation or a symbol other than [`URL_GOES_ON`] follows the host, and nothing
/// of a path, query or fragment comes later, does the link end at the host and the rest go
/// through the other rules.
fn link_parts(u: &str) -> LinkParts {
    let whole = LinkParts {
        host: None,
        end: u.len(),
    };
    let scheme_end = u.find("://").map_or(0, |i| i + 3);
    let rest = &u[scheme_end..];
    let auth = &rest[..rest.find(PATH).unwrap_or(rest.len())];
    let start = match auth.rfind('@') {
        None => 0,
        Some(at)
            if auth[..at]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || USERINFO.contains(c)) =>
        {
            at + 1
        }
        // User info that isn't plain ASCII: where the host starts can't be told.
        Some(_) => return whole,
    };
    let host = &auth[start..];
    let mut len = if host.starts_with('[') {
        let Some(close) = host.find(']') else {
            return whole;
        };
        let ipv6 = |c: char| c.is_ascii_hexdigit() || matches!(c, ':' | '.');
        if close == 1 || !host[1..close].chars().all(ipv6) {
            return whole;
        }
        close + 1
    } else {
        host.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')))
            .unwrap_or(host.len())
    };
    let host_len = len;
    if let Some(port) = host[len..].strip_prefix(':') {
        let digits = port.bytes().take_while(u8::is_ascii_digit).count();
        if (1..=5).contains(&digits) {
            len += 1 + digits;
        }
    }
    let host_start = scheme_end + start;
    let after = &u[host_start + len..];
    let (host_len, end) = if after.is_empty() || after.starts_with(PATH) {
        (host_len, u.len())
    } else if after.contains(PATH) || P.host_goes_on.is_match(after) {
        // Something that can't be part of a plain host comes before a path, query or fragment,
        // or the host goes on in letters that aren't ASCII.
        return whole;
    } else if after.starts_with(URL_GOES_ON) {
        (host_len, u.len())
    } else if len == host_len {
        // Only the host is a link here: a full stop straight after it ends the sentence.
        let trimmed = host[..host_len].trim_end_matches('.').len();
        (trimmed, host_start + trimmed)
    } else {
        (host_len, host_start + len)
    };
    if host_len == 0 {
        return whole;
    }
    LinkParts {
        host: Some((host_start, host_start + host_len)),
        end,
    }
}

/// A host longer than DNS allows (RFC 1035 section 2.3.4: 255 bytes on the wire, so 253 written) is
/// no host: the link is masked as a bare `<link>`.
const HOST_MAX: usize = 253;

/// Whether a host carries personal data, so the link is masked as a bare `<link>`: 7 or more
/// digits in all (a phone number, or a 13 to 19 digit card), or a known name as one of its words
/// (between dots, hyphens and underscores).
fn personal_host(host: &str, known: &Known) -> bool {
    host.len() > HOST_MAX
        || host.bytes().filter(u8::is_ascii_digit).count() >= 7
        || host.split(['.', '-', '_']).any(|w| known.is_name(w))
}

fn push_link(f: &mut Found, start: usize, u: &str, known: &Known) {
    let parts = link_parts(u);
    let end = start + parts.end;
    if !f.is_kept(start, end) {
        f.spans.push(Span {
            start,
            end,
            kind: Kind::Link,
            host: parts
                .host
                .filter(|&(hs, he)| !personal_host(&u[hs..he], known))
                .map(|(hs, he)| (start + hs, start + he)),
        });
        f.keep(start, end);
    }
}

/// URLs with a scheme, or starting with `www.`: the whole URL becomes `<link host=…>`.
fn links(text: &str, known: &Known, f: &mut Found) {
    for m in P.url.find_iter(text) {
        if in_address(text, m.start(), m.as_str()) {
            continue;
        }
        push_link(f, m.start(), trim_url(m.as_str()), known);
    }
}

/// Whether a URL match starting with `www.` is part of an address, which the email rule masks: the
/// domain after an `@` or a `.` (`jo@www.acme.example`, `jo@mail.www.acme.example`), or the rest of
/// a local part (`jo-www.x@acme.example`).
fn in_address(text: &str, start: usize, u: &str) -> bool {
    if !u.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("www.")) {
        return false;
    }
    match char_before(text, start) {
        Some('@' | '.') => true,
        Some(c) if c.is_alphanumeric() || ATEXT.contains(c) => {
            u[..u.find(['/', '?', '#']).unwrap_or(u.len())].contains('@')
        }
        _ => false,
    }
}

/// `acme.example/path` without a scheme.
fn links_schemeless(text: &str, known: &Known, f: &mut Found) {
    for m in P.url_schemeless.find_iter(text) {
        if char_before(text, m.start()) == Some('@') {
            continue;
        }
        let u = trim_url(m.as_str());
        if link_parts(u).host.is_some_and(|(s, e)| real_tld(&u[s..e])) {
            push_link(f, m.start(), u, known);
        }
    }
}

/// An address's local part, or the whole address when it is the owner's. The domain is kept.
fn emails(text: &str, known: &Known, f: &mut Found) {
    for m in P.email.find_iter(text) {
        // A leading `'` or `/` is legal but far more often a quote or a path: it stays.
        let lead = m.as_str().len() - m.as_str().trim_start_matches(['\'', '/']).len();
        let (s, e) = (m.start() + lead, m.end());
        if f.is_kept(s, e) {
            continue;
        }
        let at = m.start() + m.as_str().rfind('@').unwrap_or(0);
        if s >= at {
            continue;
        }
        let end = if known.is_owner_address(&text[s..e]) {
            e
        } else {
            at
        };
        f.spans.push(Span {
            start: s,
            end,
            kind: Kind::Email,
            host: None,
        });
        f.keep(s, e);
    }
}

/// Bare domains (`acme.example`) ending in a real top-level domain are kept from the name rule.
fn domains(text: &str, f: &mut Found) {
    for m in P.domain.find_iter(text) {
        if real_tld(m.as_str())
            && !FILE_EXT.contains(&tld(m.as_str()).as_str())
            && char_after(text, m.end()) != Some('@')
        {
            f.domains.insert(m.start(), m.end());
        }
    }
}

/// Whether a numeric match continues into more digit groups (`01.23.45.67`), which makes it a
/// phone number or an ID rather than a date.
fn continues(text: &str, s: usize, e: usize) -> bool {
    let after = text[e..].chars().take(2).collect::<Vec<_>>();
    let before = text[..s].chars().rev().take(2).collect::<Vec<_>>();
    let sep = |c: char| matches!(c, '-' | '/' | '.');
    matches!(after.as_slice(), [c, d, ..] if sep(*c) && d.is_ascii_digit())
        || matches!(after.as_slice(), [d, ..] if d.is_ascii_digit())
        || matches!(before.as_slice(), [c, d, ..] if sep(*c) && d.is_ascii_digit())
}

fn numbers_in(s: &str) -> Vec<u32> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// Dates and date-times are kept: ISO (`2026-01-16`, with an optional time), day-month-year or
/// month-day-year with one separator, and dates with an English or Arabic month name
/// (`16 January 2026`, `١٦ يناير ٢٠٢٦`, `١٥ رمضان ١٤٤٧هـ`).
fn dates(text: &str, f: &mut Found) {
    let day_month = |d: u32, m: u32| (1..=31).contains(&d) && (1..=12).contains(&m);
    for m in P.date_iso.find_iter(text) {
        let n = numbers_in(m.as_str());
        if n.len() >= 3 && day_month(n[2], n[1]) && !continues(text, m.start(), m.end()) {
            f.keep(m.start(), m.end());
        }
    }
    for m in P.date_numeric.find_iter(text) {
        let n = numbers_in(m.as_str());
        let seps: Vec<char> = m.as_str().chars().filter(|c| !c.is_ascii_digit()).collect();
        let valid = n.len() == 3 && (day_month(n[0], n[1]) || day_month(n[1], n[0]));
        if valid && seps.first() == seps.last() && !continues(text, m.start(), m.end()) {
            f.keep(m.start(), m.end());
        }
    }
    for m in P.date_month.find_iter(text) {
        f.keep(m.start(), m.end());
    }
    // An Arabic month name starts a word or follows one joined letter ("and", "in", "to", "so"):
    // `وأغسطس 2026`, but not the end of `الحساب 2045` ("the account"), which ends in `اب` (August).
    // Short vowels and the tatweel count as letters (`char::is_alphabetic`).
    for m in P.date_month_ar.find_iter(text) {
        let mut before = text[..m.start()].chars().rev().skip_while(|&c| is_mark(c));
        let starts_word = m.as_str().starts_with(|c: char| c.is_ascii_digit())
            || match before.next() {
                Some('و' | 'ب' | 'ل' | 'ف') => !before.next().is_some_and(char::is_alphabetic),
                Some(c) => !c.is_alphabetic(),
                None => true,
            };
        if starts_word {
            f.keep(m.start(), m.end());
        }
    }
}

/// IBAN length per country (ISO 13616): release 99 (December 2024) of the SWIFT IBAN registry, as
/// listed on <https://en.wikipedia.org/wiki/International_Bank_Account_Number>.
const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24),
    ("AE", 23),
    ("AL", 28),
    ("AT", 20),
    ("AZ", 28),
    ("BA", 20),
    ("BE", 16),
    ("BG", 22),
    ("BH", 22),
    ("BI", 27),
    ("BR", 29),
    ("BY", 28),
    ("CH", 21),
    ("CR", 22),
    ("CY", 28),
    ("CZ", 24),
    ("DE", 22),
    ("DJ", 27),
    ("DK", 18),
    ("DO", 28),
    ("EE", 20),
    ("EG", 29),
    ("ES", 24),
    ("FI", 18),
    ("FK", 18),
    ("FO", 18),
    ("FR", 27),
    ("GB", 22),
    ("GE", 22),
    ("GI", 23),
    ("GL", 18),
    ("GR", 27),
    ("GT", 28),
    ("HN", 28),
    ("HR", 21),
    ("HU", 28),
    ("IE", 22),
    ("IL", 23),
    ("IQ", 23),
    ("IS", 26),
    ("IT", 27),
    ("JO", 30),
    ("KW", 30),
    ("KZ", 20),
    ("LB", 28),
    ("LC", 32),
    ("LI", 21),
    ("LT", 20),
    ("LU", 20),
    ("LV", 21),
    ("LY", 25),
    ("MC", 27),
    ("MD", 24),
    ("ME", 22),
    ("MK", 19),
    ("MN", 20),
    ("MR", 27),
    ("MT", 31),
    ("MU", 30),
    ("NI", 28),
    ("NL", 18),
    ("NO", 15),
    ("OM", 23),
    ("PK", 24),
    ("PL", 28),
    ("PS", 29),
    ("PT", 25),
    ("QA", 29),
    ("RO", 24),
    ("RS", 22),
    ("RU", 33),
    ("SA", 24),
    ("SC", 31),
    ("SD", 18),
    ("SE", 24),
    ("SI", 19),
    ("SK", 24),
    ("SM", 27),
    ("SO", 23),
    ("ST", 25),
    ("SV", 28),
    ("TL", 23),
    ("TN", 24),
    ("TR", 26),
    ("UA", 29),
    ("VA", 22),
    ("VG", 24),
    ("XK", 20),
    ("YE", 30),
];

/// ISO 13616 check digits (ISO 7064 MOD 97-10): the first four characters move to the end,
/// letters become 10 to 35, and the number must leave 1 when divided by 97.
pub(crate) fn iban_checksum_ok(compact: &[u8]) -> bool {
    if compact.len() < 5 {
        return false;
    }
    let mut r: u32 = 0;
    for &b in compact[4..].iter().chain(&compact[..4]) {
        r = match b {
            b'0'..=b'9' => (r * 10 + u32::from(b - b'0')) % 97,
            b'A'..=b'Z' => (r * 100 + u32::from(b - b'A') + 10) % 97,
            _ => return false,
        };
    }
    r == 1
}

/// IBANs: the country's registered length of characters (spaces between groups allowed) whose
/// check digits hold.
fn ibans(text: &str, f: &mut Found) {
    // Each `XX00` is tried from its own start, so a valid IBAN after an `AB12` token or after
    // another IBAN on the same line is still found.
    let bytes = text.as_bytes();
    let mut from = 0;
    for m in P.iban.find_iter(text) {
        let s = m.start();
        if s < from {
            continue;
        }
        let Some(&(_, len)) = IBAN_LENGTHS
            .iter()
            .find(|(cc, _)| cc.as_bytes() == &bytes[s..s + 2])
        else {
            continue;
        };
        let mut compact = Vec::with_capacity(len);
        let mut end = s;
        for (i, &b) in bytes[s..].iter().enumerate() {
            if compact.len() == len {
                break;
            }
            match b {
                b' ' => {}
                b'A'..=b'Z' | b'0'..=b'9' => {
                    compact.push(b);
                    end = s + i + 1;
                }
                _ => break,
            }
        }
        let glued = char_after(text, end).is_some_and(|c| c.is_alphanumeric());
        if compact.len() == len && !glued && iban_checksum_ok(&compact) {
            f.push(s, end, Kind::Iban);
            from = end;
        }
    }
}

pub(crate) fn luhn_ok(digits: &[u8]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            let d = u32::from(d);
            if i % 2 == 1 {
                if d * 2 > 9 { d * 2 - 9 } else { d * 2 }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Card numbers: 13 to 19 digits, optionally grouped by spaces or hyphens, passing Luhn.
fn cards(text: &str, f: &mut Found) {
    for m in P.card.find_iter(text) {
        let d: Vec<u8> = m
            .as_str()
            .bytes()
            .filter(u8::is_ascii_digit)
            .map(|b| b - b'0')
            .collect();
        if (13..=19).contains(&d.len()) && luhn_ok(&d) {
            f.push(m.start(), m.end(), Kind::Card);
        }
    }
}

/// One-time codes: a 4 to 16 character token holding a digit, right after a keyword, or digit
/// groups joined by single spaces, dots or hyphens (`482 917`, `123.456`, `4 8 2 9`) holding 4 to
/// 16 digits. After a password, passcode or PIN keyword, a token of 4 to 64 characters, symbols
/// and all (`Xk#92!pq7`): any token after a separator (`:`, `#`, `=`, a dash or an arrow, one or
/// several, with or without "is" or "was"), else only one that looks like a secret: holding a
/// digit or a symbol, or a capital letter after its first letter. So "Reset your password here",
/// "Pin this message" and "Your password was changed" stay. A dash joining two words
/// (`password-protected`) is no separator. A token that is itself a keyword is never masked. The
/// Arabic keywords ([`CODE_AR`], [`SECRET_AR`], with `هو` or `هي` for "is") work the same way.
/// After one or two other words and a colon or "is" ([`AR_SEP_AFTER_WORDS`]), an Arabic password
/// keyword's token is masked only when it looks like a secret.
fn codes(text: &str, f: &mut Found) {
    let ok = |g: &str| {
        let digits = g.bytes().filter(u8::is_ascii_digit).count();
        if g.contains([' ', '.']) {
            (4..=16).contains(&digits)
        } else {
            digits > 0
        }
    };
    let mut found = Vec::new();
    for c in P
        .code
        .captures_iter(text)
        .chain(P.code_ar.captures_iter(text))
    {
        if let Some(g) = c.get(1).filter(|g| ok(g.as_str())) {
            found.push((g.start(), g.end()));
        }
    }
    for c in P
        .secret
        .captures_iter(text)
        .chain(P.secret_ar.captures_iter(text))
    {
        let start = c.get(0).map_or(0, |m| m.end());
        // A separator other than a word: "is", "was", هو and هي alone only say a secret may follow.
        // `secret_ar` leaves a separator after other words uncaptured, so it counts as none.
        let separated = c.get(1).or_else(|| c.get(2)).is_some_and(|g| {
            let sep = g.as_str();
            let joins_words = matches!(sep, "-" | "–" | "—") && g.end() == start;
            sep.contains(|c: char| !c.is_alphabetic() && !c.is_whitespace()) && !joins_words
        });
        let rest = &text[start..];
        let token = secret_token(&rest[..rest.find(char::is_whitespace).unwrap_or(rest.len())]);
        let core = token.trim_matches(|c: char| !c.is_alphanumeric());
        let keyword = CODE_WORD.split('|').any(|k| k.eq_ignore_ascii_case(core));
        let secretive = core.chars().any(|c| !c.is_alphabetic())
            || (core.chars().skip(1).any(char::is_uppercase)
                && core.chars().any(char::is_lowercase));
        if (4..=64).contains(&token.chars().count()) && !keyword && (separated || secretive) {
            found.push((start, start + token.len()));
        }
    }
    for (s, e) in found {
        if f.push(s, e, Kind::Code) {
            f.codes.insert(s, e);
        }
    }
}

/// Arabic punctuation that ends a sentence or clause: comma, semicolon, question mark, full stop.
const AR_PUNCT: [char; 4] = ['،', '؛', '؟', '۔'];

/// A secret without the punctuation that ends its sentence, as [`trim_url`] trims it, or Arabic
/// punctuation ([`AR_PUNCT`]), except a `!` or `?`, which a password more often ends in than a
/// sentence does after one.
fn secret_token(token: &str) -> &str {
    let token = token.trim_end_matches(AR_PUNCT);
    let trimmed = trim_url(token).len();
    let back = token[trimmed..]
        .bytes()
        .take_while(|b| matches!(b, b'!' | b'?'))
        .count();
    &token[..trimmed + back]
}

/// `1,500` or `1.250.000`: groups of exactly three digits after a separator are thousands, not a
/// phone number.
fn thousands(c: &str) -> bool {
    let groups: Vec<&str> = c.split(['.', ',', '\'']).collect();
    groups.len() > 1
        && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()))
        && (1..=3).contains(&groups[0].len())
        && groups[1..].iter().all(|g| g.len() == 3)
}

/// Phone numbers. International (`+`, `00`): 7 to 15 digits. National: digit groups joined by
/// spaces, dots, hyphens, slashes or an area code in brackets, 7 to 12 digits. Dates were kept
/// before this rule runs, and times never match (a colon is no separator); thousands are skipped.
fn phones(text: &str, f: &mut Found) {
    let digits = |s: &str| s.bytes().filter(u8::is_ascii_digit).count();
    let glued = |s: usize, e: usize| {
        char_before(text, s).is_some_and(char::is_alphanumeric)
            || char_after(text, e).is_some_and(char::is_alphanumeric)
    };
    for m in P.phone_intl.find_iter(text) {
        if (7..=15).contains(&digits(m.as_str())) && !glued(m.start(), m.end()) {
            f.push(m.start(), m.end(), Kind::Phone);
        }
    }
    for m in P.phone_national.find_iter(text) {
        let c = m.as_str();
        if (7..=12).contains(&digits(c)) && !thousands(c) && !glued(m.start(), m.end()) {
            f.push(m.start(), m.end(), Kind::Phone);
        }
    }
}

/// `word` as a name in a script without capital letters is compared: Arabic short vowels and
/// other marks (U+064B to U+065F, U+0670) and the stretching tatweel (U+0640) dropped, the alef
/// with a hamza or wasla (`أ`, `إ`, `ٱ`) written as a bare `ا`, and the Persian keheh and yeh
/// (`ک`, `ی`) as Arabic `ك` and `ي`. So `مُحَمَّد` and `محـمد` are `محمد`. It never joins `ي` and `ى`
/// (`علي`, the name Ali, against `على`, "on"), `ة` and `ه` (`رحمة`, Rahma, against `رحمه`, "his
/// mercy"), nor the madda `آ` and `ا` (`آمل`, "I hope", against `أمل`, Amal; `آمين`, "amen",
/// against `أمين`, Amin), which search engines fold but which would make common words match names.
pub(crate) fn fold_name(word: &str) -> String {
    word.chars()
        .filter(|&c| !matches!(c, '\u{64b}'..='\u{65f}' | '\u{670}' | '\u{640}'))
        .map(|c| match c {
            'أ' | 'إ' | 'ٱ' => 'ا',
            'ک' => 'ك',
            'ی' => 'ي',
            c => c,
        })
        .collect()
}

/// Words of an Arabic name that are ordinary words or particles on their own: "son of", "daughter
/// of", "father of", "mother of", "family of", "servant of", "God", "the faith". As folded by
/// [`fold_name`]. A header's `عبد الله` is matched as one name (`عبدالله`), never as `الله`.
pub(crate) const NAME_PARTICLES: &[&str] = &[
    "بن",
    "ابن",
    "بنت",
    "ابو",
    "ام",
    "آل",
    "ال",
    "عبد",
    "الله",
    "الدين",
];

/// Arabic words that make a display name an organisation's: "bank", "company", "institution",
/// "group", "service(s)", "customers", "store", "team", "support", "ministry", "authority",
/// "centre", "platform", "administration", "university", "hospital", "municipality", "post",
/// "app", "portal", "office", "airlines", "aviation", "telecom", "fund", "academy", "pharmacy",
/// "school", "newspaper" (two words), "channel", "magazine", "bookshop" or "library", "club",
/// "association" or "charity", "website", "council", "clinic", "market(s)", "newsletter". As folded
/// by [`fold_name`], without the article. No given or family name is one of them, and mail from
/// `البنك الأهلي` or `خدمة العملاء` uses these words throughout its body.
pub(crate) const ORG_WORDS: &[&str] = &[
    "بنك",
    "مصرف",
    "شركة",
    "مؤسسة",
    "مجموعة",
    "خدمة",
    "خدمات",
    "عملاء",
    "متجر",
    "فريق",
    "دعم",
    "وزارة",
    "هيئة",
    "مركز",
    "منصة",
    "ادارة",
    "جامعة",
    "مستشفى",
    "بلدية",
    "بريد",
    "تطبيق",
    "بوابة",
    "مكتب",
    "خطوط",
    "طيران",
    "اتصالات",
    "صندوق",
    "اكاديمية",
    "صيدلية",
    "مدرسة",
    "صحيفة",
    "جريدة",
    "قناة",
    "مجلة",
    "مكتبة",
    "نادي",
    "جمعية",
    "موقع",
    "مجلس",
    "عيادة",
    "سوق",
    "اسواق",
    "نشرة",
];

/// Whether a display name's folded words name an organisation: one of them, with or without the
/// article, is in [`ORG_WORDS`]. A final `ى` counts as `ي` here, as Egyptian spelling writes `نادى`
/// for `نادي` ("club"); names keep the two apart ([`fold_name`]).
pub(crate) fn is_org_name(words: &[String]) -> bool {
    words.iter().any(|w| {
        let w = w.strip_prefix("ال").unwrap_or(w);
        let yeh = w.strip_suffix('ى').map(|stem| format!("{stem}ي"));
        ORG_WORDS.contains(&w) || yeh.is_some_and(|w| ORG_WORDS.contains(&w.as_str()))
    })
}

/// What may be joined to the front of an Arabic name: the letters "and", "with" and "to". The
/// article is never stripped, as no given name carries it: `الكريم` ("the valued") is not the name
/// `كريم`. A family name keeps its article in the header (`العتيبي`), so `والعتيبي` and `بالعتيبي`
/// match through `و` and `ب`, and `للعتيبي` through `لل`, which is `ل` before the article with its
/// alef dropped. `ف` ("so") and `ك` ("like") are left out: they rarely come before a name, and
/// `فعلي` ("actual") or `كريم` ("generous") would then match the names `علي` and `ريم`.
const PREFIXES: [&str; 3] = ["و", "ب", "ل"];

/// Common words that are a prefix and a name: `لعلي` ("perhaps I") is not `ل` + `علي`, `لدينا`
/// ("we have") not `ل` + `دينا`, `وجود` ("existence") not `و` + `جود`. As folded by [`fold_name`].
const PREFIXED_WORDS: [&str; 3] = ["لعلي", "لدينا", "وجود"];

/// Whether `folded` (a folded body word) is a known name in a script without capitals, written
/// alone, with a [`PREFIXES`] entry joined to it, `و` before one, or `يا`; `Some(true)` when it is
/// the particle `عبد`, which is a name only with the next word (`عبد الله`).
fn caseless_name(known: &Known, folded: &str) -> Option<bool> {
    one_prefix(known, folded)
        // `و` ("and") before another prefix: `ولمحمد`, `وبسارة`, `وللعتيبي`; `ولدينا` ("and we
        // have") stays, as `لدينا` is in [`PREFIXED_WORDS`].
        .or_else(|| {
            folded
                .strip_prefix('و')
                .filter(|_| !PREFIXED_WORDS.contains(&folded))
                .and_then(|rest| one_prefix(known, rest))
        })
        // The vocative `يا` joined to a name, as informal mail writes it: `يامحمد`.
        .or_else(|| {
            folded
                .strip_prefix("يا")
                .filter(|rest| known.caseless.contains(*rest))
                .map(|_| false)
        })
}

/// [`caseless_name`] with at most one prefix.
fn one_prefix(known: &Known, folded: &str) -> Option<bool> {
    let bare = std::iter::once(folded);
    let stripped = PREFIXES
        .iter()
        .filter(|_| !PREFIXED_WORDS.contains(&folded))
        .filter_map(|p| folded.strip_prefix(p))
        .filter(|rest| rest.chars().count() >= 2);
    // `لل` is `ل` before the article, whose alef is dropped: `للعتيبي` is `ل` + `العتيبي`.
    let article = folded
        .strip_prefix("لل")
        .map(|rest| format!("ال{rest}"))
        .filter(|_| !PREFIXED_WORDS.contains(&folded));
    for w in bare.chain(stripped).chain(article.as_deref()) {
        if known.caseless.contains(w) {
            return Some(false);
        }
        if w == "عبد" && known.has_compounds {
            return Some(true);
        }
    }
    None
}

/// Known names as whole words; a possessive `'s` stays outside the mask. A name in a script
/// without capitals is matched after [`fold_name`], with a joined prefix or not, and the whole word
/// is masked (`لمحمد` becomes `<name>`).
fn names(text: &str, known: &Known, f: &mut Found) {
    if known.names.is_empty() && known.caseless.is_empty() {
        return;
    }
    let mut words = P.word.find_iter(text).peekable();
    while let Some(m) = words.next() {
        let w = m.as_str();
        if !w.is_ascii() && !known.caseless.is_empty() {
            let folded = fold_name(w);
            // A name in the accusative keeps the alef of its ending once the vowels are dropped:
            // `خالداً` and `خالدًا` are `خالدا`, so `خالد` is tried as well.
            let accusative = (w.ends_with("\u{627}\u{64b}") || w.ends_with("\u{64b}\u{627}"))
                .then(|| folded.strip_suffix('ا'))
                .flatten();
            let name = caseless_name(known, &folded)
                .or_else(|| accusative.and_then(|a| caseless_name(known, a)));
            match name {
                Some(false) => {
                    f.push(m.start(), m.end(), Kind::Name);
                    continue;
                }
                // `عبد` and the next word, past spaces only, as one name.
                Some(true) => {
                    let next = words.peek().filter(|n| {
                        text[m.end()..n.start()]
                            .chars()
                            .all(|c| c == ' ' || is_mark(c))
                    });
                    if let Some(n) = next {
                        let joined = format!("{folded}{}", fold_name(n.as_str()));
                        if caseless_name(known, &joined) == Some(false) {
                            f.push(m.start(), n.end(), Kind::Name);
                            words.next();
                            continue;
                        }
                    }
                }
                None => {}
            }
        }
        let stem = w
            .strip_suffix('s')
            .and_then(|r| r.strip_suffix('\'').or_else(|| r.strip_suffix(APOSTROPHES)))
            .unwrap_or(w);
        if known.is_name(&fold_apostrophes(stem)) {
            f.push(m.start(), m.start() + stem.len(), Kind::Name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iban_lengths_are_unique_and_plausible() {
        for (i, (cc, len)) in IBAN_LENGTHS.iter().enumerate() {
            assert!((15..=34).contains(len), "{cc}");
            assert!(
                IBAN_LENGTHS[i + 1..].iter().all(|(o, _)| o != cc),
                "{cc} twice"
            );
        }
    }

    #[test]
    fn the_country_code_list_is_sorted_for_its_binary_search() {
        assert!(CC_TLD.windows(2).all(|w| w[0] < w[1]));
        assert!(real_tld("acme.co") && real_tld("acme.example") && !real_tld("x.ok"));
    }

    #[test]
    fn ranges_merge_so_any_overlap_is_found() {
        let mut r = Ranges::default();
        for (s, e) in [(10, 20), (40, 50), (5, 12), (18, 41), (60, 70), (70, 75)] {
            r.insert(s, e);
        }
        assert_eq!(
            r.0.iter().map(|(&s, &e)| (s, e)).collect::<Vec<_>>(),
            [(5, 50), (60, 75)]
        );
        assert!(r.overlaps(49, 52) && r.overlaps(0, 6) && r.overlaps(74, 80));
        assert!(!r.overlaps(0, 5) && !r.overlaps(50, 60) && !r.overlaps(75, 90));
    }

    #[test]
    fn url_trimming_keeps_balanced_brackets() {
        assert_eq!(
            trim_url("https://a.example/x_(y))."),
            "https://a.example/x_(y)"
        );
        let parts = |u: &str| {
            let p = link_parts(u);
            (p.host, p.end)
        };
        assert_eq!(parts("https://u:p@a.example:8443/x"), (Some((12, 21)), 28));
        assert_eq!(parts("http://[::1]:8080/"), (Some((7, 12)), 18));
        assert_eq!(parts("https://my_host.example/x"), (Some((8, 23)), 25));
        // With no path after it, the host ends the link.
        assert_eq!(parts("https://a.example—call"), (Some((8, 17)), 17));
        assert_eq!(parts("https://a.example:8443,x"), (Some((8, 17)), 22));
        assert_eq!(parts("https://a.example.—x"), (Some((8, 17)), 17));
        // A backslash starts a path as `/` does.
        assert_eq!(parts(r"https://a.example\x"), (Some((8, 17)), 19));
        assert_eq!(parts(r"https://u\p@a.example/x"), (Some((8, 9)), 23));
        // No host, or one that isn't plain ASCII before a path: the whole URL, with no host.
        assert_eq!(parts("https://[zz]/"), (None, 13));
        assert_eq!(parts("https://—x"), (None, 12));
        assert_eq!(parts("https://bücher.example/x"), (None, 25));
        // A host that goes on in letters that aren't ASCII, whatever follows.
        assert_eq!(parts("https://bücher.example"), (None, 23));
        assert_eq!(parts("https://bu\u{308}cher.example&t=x"), (None, 28));
        assert_eq!(parts("https://a.example:8443ü"), (None, 24));
        // An invisible character alone after the host ends the link there.
        assert_eq!(parts("https://a.example\u{200b}"), (Some((8, 17)), 17));
    }
}
