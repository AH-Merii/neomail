//! The rules. Each detector works in byte offsets and adds spans to [`Found`]; [`redact`] merges
//! them and writes the placeholders.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::{Kind, Known, Mask, Redacted};

const CUR_CODE: &str =
    "USD|EUR|GBP|CHF|JPY|CNY|INR|CAD|AUD|NZD|SEK|NOK|DKK|PLN|CZK|HUF|BRL|MXN|ZAR|SGD|HKD|AED|SAR";
const CUR_SYM: &str = r"[$€£¥₹₩₽]|(?:US|CA|AU|NZ|HK|SG|A|C|R)\$";
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
    domain: Regex,
    date_iso: Regex,
    date_numeric: Regex,
    date_month: Regex,
    iban: Regex,
    card: Regex,
    amount: Regex,
    code: Regex,
    secret: Regex,
    phone_intl: Regex,
    phone_national: Regex,
    digits: Regex,
    digit: Regex,
    other_digit: Regex,
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
        domain: re(&format!(r"(?-u:\b){domain_host}(?-u:\b)")),
        date_iso: re(
            r"(?-u:\b)(?:19|20)\d{2}[-/.]\d{1,2}[-/.]\d{1,2}(?:[T ]\d{1,2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:Z|[+-]\d{2}:?\d{2})?)?",
        ),
        date_numeric: re(r"(?-u:\b)\d{1,2}[-/.]\d{1,2}[-/.](?:\d{4}|\d{2})(?-u:\b)"),
        date_month: re(&format!(
            r"(?i)(?-u:\b)(?:\d{{1,2}}(?:st|nd|rd|th)?\s+)?(?:{MONTH})\.?(?:\s+\d{{1,2}}(?:st|nd|rd|th)?)?,?\s+(?:19|20)\d{{2}}(?-u:\b)"
        )),
        iban: re(r"(?-u:\b)[A-Z]{2}[0-9]{2}"),
        card: re(r"(?-u:\b)\d(?:[ -]?\d){12,18}(?-u:\b)"),
        amount: re(&format!(
            r"(?:{CUR_SYM}|(?-u:\b)(?:{CUR_CODE}))\s?(?:{NUM}){MAG}|(?-u:\b)(?:{NUM}){MAG}\s?(?:[€£$¥₹]|(?:{CUR_CODE}|{CUR_WORD})(?-u:\b))"
        )),
        code: re(&format!(
            r"(?i)(?-u:\b)(?:{CODE_WORD})(?:\s+(?:{CODE_WORD}))*(?-u:\b)(?:\s+(?:is|was))?\s*[:#=\-]?\s*(\d{{2,4}}(?:[ .\-]\d{{2,4}}){{1,3}}|\d(?:[ .\-]\d){{3,15}}|[a-z0-9][a-z0-9\-]{{2,14}}[a-z0-9])(?-u:\b)"
        )),
        secret: re(&format!(
            r"(?i)(?-u:\b)(?:{SECRET_WORD})(?-u:\b)(?:\s+(?:is|was))?\s*[:#=\-]?\s*(\S+)"
        )),
        phone_intl: re(r"(?:\+|(?-u:\b)00)\d[\d \-.()/]{5,20}\d(?-u:\b)"),
        phone_national: re(
            r"(?:\(\d{2,5}\)\s?|(?-u:\b)\d{2,5}[ .\-/])\d{2,4}(?:[ .\-/]\d{2,5}){0,3}(?-u:\b)",
        ),
        digits: re(r"\d{4,}"),
        digit: re(r"^\d$"),
        other_digit: re(r"[\d--0-9]"),
        word: re(r"[\p{L}\p{M}\p{N}]+(?:['’][\p{L}\p{M}\p{N}]+)*"),
    }
});

/// The words of `text` as the name rule sees them: letters, marks and digits, with inner
/// apostrophes (`O'Brien`). Hyphens, dots and spaces separate words (`Jean-Luc` is two).
pub(crate) fn words(text: &str) -> impl Iterator<Item = &str> + '_ {
    P.word.find_iter(text).map(|m| m.as_str())
}

/// `word` with each typographic apostrophe (`’` U+2019, `ʼ` U+02BC) written as `'`, so a header's
/// `O'Brien` and a body's `O’Brien` (as HTML mail is usually rendered) are one name.
pub(crate) fn fold_apostrophes(word: &str) -> Cow<'_, str> {
    if word.contains(['’', 'ʼ']) {
        Cow::Owned(word.replace(['’', 'ʼ'], "'"))
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

/// Masks every match in `text`. Digits from other scripts (`０９０ １２３ ４５６`) are folded to
/// ASCII first, so every rule's digit counts, checksums and word boundaries see them as digits.
pub(crate) fn redact(text: &str, known: &Known) -> Redacted {
    let Some((folded, map)) = fold_digits(text) else {
        return write(text, find(text, known));
    };
    let spans = find(&folded, known)
        .into_iter()
        .map(|s| Span {
            start: map[s.start],
            end: map[s.end],
            kind: s.kind,
            host: s.host.map(|(hs, he)| (map[hs], map[he])),
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

/// `text` with every non-ASCII decimal digit replaced by its ASCII digit, and for each byte of the
/// result (and its end) the matching byte offset in `text`; `None` when there is nothing to fold.
fn fold_digits(text: &str) -> Option<(String, Vec<usize>)> {
    if !P.other_digit.is_match(text) {
        return None;
    }
    let mut folded = String::with_capacity(text.len());
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        match digit(c).filter(|_| !c.is_ascii()) {
            Some(d) => {
                folded.push(char::from(b'0' + d));
                map.push(i);
            }
            None => {
                folded.push(c);
                map.extend(i..i + c.len_utf8());
            }
        }
    }
    map.push(text.len());
    Some((folded, map))
}

/// Every span to mask in `text`, merged.
fn find(text: &str, known: &Known) -> Vec<Span> {
    let mut f = Found::default();
    links(text, &mut f);
    emails(text, known, &mut f);
    links_schemeless(text, &mut f);
    domains(text, &mut f);
    // A code follows its keyword, so it wins over a date's keep (`sort code 12-03-45`).
    codes(text, &mut f);
    dates(text, &mut f);
    ibans(text, &mut f);
    cards(text, &mut f);
    for m in P.amount.find_iter(text) {
        f.push(m.start(), m.end(), Kind::Amount);
    }
    phones(text, &mut f);
    for m in P.digits.find_iter(text) {
        f.push(m.start(), m.end(), Kind::Number);
    }
    names(text, known, &mut f);
    merge(f.spans)
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
    /// The host: a run of letters, digits, dots and hyphens, or one `[…]` IPv6 literal.
    host: (usize, usize),
    /// Where the link ends: the end of the URL when [`URL_GOES_ON`] (or nothing) follows the host
    /// and port, else straight after them, so the rest of the text (`https://a.example—call …`,
    /// `https://a.example。電話…`) goes through the other rules instead of into the host.
    end: usize,
}

/// What, straight after a host (and port), keeps the rest in the link: a path, query or fragment,
/// path parameters (`;jsessionid=…`) and percent-encoding. Anything else, such as a comma, a dash
/// or CJK text, ends the link at the host.
const URL_GOES_ON: [char; 9] = ['/', '?', '#', ';', '%', '=', '&', '+', '~'];

/// Characters RFC 3986 (section 3.2.1) allows in a URL's userinfo, besides letters and digits.
const USERINFO: &str = "-._~%!$&'()*+,;=:";

/// The host and end of the URL `u`, which starts with a scheme or `www.`; `None` without a host.
fn link_parts(u: &str) -> Option<LinkParts> {
    let scheme_end = u.find("://").map_or(0, |i| i + 3);
    let rest = &u[scheme_end..];
    let auth = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    let start = auth
        .rfind('@')
        .filter(|&at| {
            auth[..at]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || USERINFO.contains(c))
        })
        .map_or(0, |at| at + 1);
    let host = &auth[start..];
    let mut len = if host.starts_with('[') {
        let close = host.find(']')?;
        let ipv6 = |c: char| c.is_ascii_hexdigit() || matches!(c, ':' | '.');
        if close == 1 || !host[1..close].chars().all(ipv6) {
            return None;
        }
        close + 1
    } else {
        host.find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-')))
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
    let after = host_start + len;
    let (host_len, end) = if after == u.len() || u[after..].starts_with(URL_GOES_ON) {
        (host_len, u.len())
    } else if len == host_len {
        // Only the host is a link here: a full stop straight after it ends the sentence.
        let trimmed = host[..host_len].trim_end_matches('.').len();
        (trimmed, host_start + trimmed)
    } else {
        (host_len, after)
    };
    let host_end = host_start + host_len;
    (host_start < host_end).then_some(LinkParts {
        host: (host_start, host_end),
        end,
    })
}

/// A host longer than DNS allows (RFC 1035 section 2.3.4: 255 bytes on the wire, so 253 written) is
/// no host: the link is masked as a bare `<link>`.
const HOST_MAX: usize = 253;

fn push_link(f: &mut Found, start: usize, u: &str) {
    let Some(parts) = link_parts(u) else {
        return;
    };
    let end = start + parts.end;
    let (hs, he) = parts.host;
    if !f.is_kept(start, end) {
        f.spans.push(Span {
            start,
            end,
            kind: Kind::Link,
            host: (he - hs <= HOST_MAX).then_some((start + hs, start + he)),
        });
        f.keep(start, end);
    }
}

/// URLs with a scheme, or starting with `www.`: the whole URL becomes `<link host=…>`.
fn links(text: &str, f: &mut Found) {
    for m in P.url.find_iter(text) {
        if in_address(text, m.start(), m.as_str()) {
            continue;
        }
        push_link(f, m.start(), trim_url(m.as_str()));
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
fn links_schemeless(text: &str, f: &mut Found) {
    for m in P.url_schemeless.find_iter(text) {
        if char_before(text, m.start()) == Some('@') {
            continue;
        }
        let u = trim_url(m.as_str());
        if link_parts(u).is_some_and(|p| real_tld(&u[p.host.0..p.host.1])) {
            push_link(f, m.start(), u);
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
/// month-day-year with one separator, and dates with an English month name.
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
}

/// IBAN length per country, from the SWIFT IBAN registry (ISO 13616), release 99, December 2024.
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
/// 16 digits. After a password, passcode or PIN keyword, any token of 4 to 64 characters, symbols
/// and all (`Xk#92!pq7`).
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
    for c in P.code.captures_iter(text) {
        if let Some(g) = c.get(1).filter(|g| ok(g.as_str())) {
            found.push((g.start(), g.end()));
        }
    }
    for c in P.secret.captures_iter(text) {
        if let Some(g) = c.get(1) {
            let token = secret_token(g.as_str());
            if (4..=64).contains(&token.chars().count()) {
                found.push((g.start(), g.start() + token.len()));
            }
        }
    }
    for (s, e) in found {
        if f.push(s, e, Kind::Code) {
            f.codes.insert(s, e);
        }
    }
}

/// A secret without the punctuation that ends its sentence, as [`trim_url`] trims it, except a `!`
/// or `?`, which a password more often ends in than a sentence does after one.
fn secret_token(token: &str) -> &str {
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

/// Known names as whole words; a possessive `'s` stays outside the mask.
fn names(text: &str, known: &Known, f: &mut Found) {
    if known.names.is_empty() {
        return;
    }
    for m in P.word.find_iter(text) {
        let w = m.as_str();
        let stem = w
            .strip_suffix("'s")
            .or_else(|| w.strip_suffix("’s"))
            .or_else(|| w.strip_suffix("ʼs"))
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
        let parts = |u: &str| link_parts(u).map(|p| (p.host, p.end));
        assert_eq!(parts("https://u:p@a.example:8443/x"), Some(((12, 21), 28)));
        assert_eq!(parts("http://[::1]:8080/"), Some(((7, 12), 18)));
        // With no path after it, the host ends the link.
        assert_eq!(parts("https://a.example—call"), Some(((8, 17), 17)));
        assert_eq!(parts("https://a.example:8443,x"), Some(((8, 17), 22)));
        assert_eq!(parts("https://a.example.—x"), Some(((8, 17), 17)));
        assert_eq!(parts("https://[zz]/"), None);
    }
}
