//! `--attach`: the files read for a message, each provider's size limit, and how sizes are shown.
//!
//! The limits are on the files' total size as they are on disk, as each provider states it:
//!
//! - Gmail: "For personal Gmail accounts, the limit is 25 MB"
//!   (<https://support.google.com/mail/answer/6584>); a Google Workspace administrator may set
//!   another, which the server then enforces.
//! - Proton Mail, sending through Bridge: "25 MB for outgoing emails"
//!   (<https://proton.me/support/attaching-multiple-documents-to-a-message>).
//! - Outlook, through Microsoft Graph: "The attachment size limit for files is 25 MB" for
//!   Outlook.com (<https://support.microsoft.com/en-us/outlook/sending-limits-in-outlook-com>);
//!   work and school mailboxes send "35 MB" by default
//!   (<https://learn.microsoft.com/en-us/office365/servicedescriptions/exchange-online-service-description/exchange-online-limits>),
//!   so 25 MB holds for both.
//! - Any other server: none here; a server that names its limit with SMTP's SIZE extension gets
//!   nothing larger ([`nm_imap::smtp`]).
//!
//! "MB" is read as 1,000,000 bytes, the smaller reading. Whatever the account, neomail reads at
//! most [`MAX_TOTAL`] of files: 150 MB, the largest message any of these documents allows
//! (Exchange Online's maximum), so a mistaken path to a huge file cannot fill the memory.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use nm_core::compose::AttachedFile;

use crate::config::Outbound;

/// The most file bytes one message may carry, whatever the account.
pub const MAX_TOTAL: u64 = 150_000_000;

/// A provider's limit on a message's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub provider: &'static str,
    pub bytes: u64,
}

const TWENTY_FIVE_MB: u64 = 25_000_000;

/// The limit of the provider `outbound` sends through, when neomail knows it: Graph is Outlook,
/// smtp.gmail.com is Gmail, and an SMTP server on 127.0.0.1 with a pinned certificate is Proton
/// Mail Bridge, the only local server neomail's setup pins.
pub fn limit(outbound: &Outbound) -> Option<Limit> {
    let provider = match outbound {
        Outbound::Graph => "Outlook",
        Outbound::Smtp { config, .. } if config.host.eq_ignore_ascii_case("smtp.gmail.com") => {
            "Gmail"
        }
        Outbound::Smtp { config, .. }
            if config.host == "127.0.0.1" && config.cert_pin.is_some() =>
        {
            "Proton Mail"
        }
        Outbound::Smtp { .. } => return None,
    };
    Some(Limit {
        provider,
        bytes: TWENTY_FIVE_MB,
    })
}

/// Reads each `--attach` file once, in order. A missing file, a folder, anything else that is not
/// a regular file (a device, a pipe or a socket, which have no size to check and may never end),
/// one that cannot be read, or files over [`MAX_TOTAL`] in all stop here, before anything is
/// sent. The read itself stops past [`MAX_TOTAL`], so a file that grows after its size was
/// checked cannot fill the memory either.
pub fn read(paths: &[PathBuf]) -> Result<Vec<AttachedFile>> {
    let mut total = 0u64;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let shown = path.display();
        let meta = std::fs::metadata(path)
            .with_context(|| format!("--attach {shown}: cannot read it; nothing was sent"))?;
        if meta.is_dir() {
            bail!("--attach {shown}: it is a folder, not a file; nothing was sent");
        }
        if !meta.is_file() {
            bail!("--attach {shown}: not a regular file; nothing was sent");
        }
        let too_many = || {
            anyhow::anyhow!(
                "--attach {shown}: the files come to more than {}, the most neomail sends; \
                 nothing was sent",
                size(MAX_TOTAL)
            )
        };
        if total.saturating_add(meta.len()) > MAX_TOTAL {
            return Err(too_many());
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_TOTAL - total + 1).read_to_end(&mut bytes))
            .with_context(|| format!("--attach {shown}: cannot read it; nothing was sent"))?;
        total += bytes.len() as u64;
        if total > MAX_TOTAL {
            return Err(too_many());
        }
        files.push(AttachedFile::new(&file_name(path), bytes));
    }
    Ok(files)
}

/// The name a path's file is sent as, before [`AttachedFile::new`] cleans it.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Refuses `files` over `limit`, naming both sizes.
pub fn check_size(limit: Option<Limit>, files: &[AttachedFile]) -> Result<()> {
    let total: u64 = files.iter().map(AttachedFile::size).sum();
    if let Some(limit) = limit
        && total > limit.bytes
    {
        bail!(
            "the files come to {}, over the {} {} takes for a message's files; nothing was sent",
            size(total),
            size(limit.bytes),
            limit.provider
        );
    }
    Ok(())
}

/// A size for people, in decimal units as the providers state their limits, rounded up to a
/// tenth so a size over a limit never shows as equal to it: `512 bytes`, `12.3 kB`, `25 MB`.
pub fn size(bytes: u64) -> String {
    let unit = |n: u64, name: &str| {
        let tenths = (bytes * 10).div_ceil(n);
        match tenths % 10 {
            0 => format!("{} {name}", tenths / 10),
            tenth => format!("{}.{tenth} {name}", tenths / 10),
        }
    };
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => unit(1_000, "kB"),
        _ => unit(1_000_000, "MB"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const PIN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn outbound(account: &str) -> Outbound {
        let text = format!("[[accounts]]\nname = \"a\"\nusername = \"me@example.com\"\n{account}");
        Config::parse(&text).unwrap().accounts[0]
            .outbound()
            .unwrap()
    }

    #[test]
    fn each_provider_gets_its_documented_limit_and_other_servers_none() {
        let password = "password_command = \"true\"\n";
        let gmail = outbound(&format!(
            "{password}[accounts.imap]\nhost = \"imap.gmail.com\"\n"
        ));
        let bridge = outbound(&format!(
            "{password}[accounts.imap]\nhost = \"127.0.0.1\"\nport = 1143\nsecurity = \"starttls\"\n\
             cert_sha256 = \"{PIN}\"\n"
        ));
        let outlook = outbound("oauth = { provider = \"microsoft\", client_id = \"id\" }\n");
        let other = outbound(&format!(
            "{password}[accounts.imap]\nhost = \"imap.example.com\"\n\
             [accounts.smtp]\nhost = \"smtp.example.com\"\n"
        ));
        let local_unpinned = outbound(&format!(
            "{password}[accounts.imap]\nhost = \"imap.example.com\"\n\
             [accounts.smtp]\nhost = \"127.0.0.1\"\nport = 1025\n"
        ));
        let limit_of = |o: &Outbound| limit(o).map(|l| (l.provider, l.bytes));
        assert_eq!(limit_of(&gmail), Some(("Gmail", 25_000_000)));
        assert_eq!(limit_of(&bridge), Some(("Proton Mail", 25_000_000)));
        assert_eq!(limit_of(&outlook), Some(("Outlook", 25_000_000)));
        assert_eq!(limit_of(&other), None);
        assert_eq!(limit_of(&local_unpinned), None);
    }

    #[test]
    fn files_up_to_the_limit_pass_and_one_byte_more_is_refused() {
        let limit = Some(Limit {
            provider: "Gmail",
            bytes: 10,
        });
        let file = |n: usize| AttachedFile::new("f", vec![0; n]);
        assert!(check_size(limit, &[file(4), file(6)]).is_ok());
        let err = check_size(limit, &[file(4), file(7)]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "the files come to 11 bytes, over the 10 bytes Gmail takes for a message's files; \
             nothing was sent"
        );
        assert!(check_size(None, &[file(1_000_000_000)]).is_ok());
    }

    #[test]
    fn sizes_round_up_so_one_over_a_limit_never_shows_as_equal() {
        for (bytes, shown) in [
            (0, "0 bytes"),
            (999, "999 bytes"),
            (1_000, "1 kB"),
            (12_301, "12.4 kB"),
            (25_000_000, "25 MB"),
            (25_000_001, "25.1 MB"),
        ] {
            assert_eq!(size(bytes), shown, "{bytes}");
        }
    }
}
