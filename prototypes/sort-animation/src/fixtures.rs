//! About 40 realistic demo messages across three accounts.
//!
//! `hint` is the group the simulated jev leans towards, and `signal` is how
//! clear-cut the message is (low signal tends to fall back to `other`; blatant
//! spam has a high signal).

use crate::groups::{Account, Group};
use Account::*;
use Group::*;

pub struct Fixture {
    pub account: Account,
    pub from: &'static str,
    pub subject: &'static str,
    pub hint: Group,
    pub signal: f32,
}

const fn m(
    account: Account,
    from: &'static str,
    subject: &'static str,
    hint: Group,
    signal: f32,
) -> Fixture {
    Fixture { account, from, subject, hint, signal }
}

/// Messages already in the inbox when jev starts (newest first).
pub const INITIAL: &[Fixture] = &[
    m(Gmail, "Priya Raman", "Re: Q4 roadmap review - can we move to Thursday?", Main, 3.2),
    m(Outlook, "Daniel Okafor", "Action needed: sign-off on the vendor contract", FollowUp, 3.0),
    m(Proton, "Proton", "Your monthly security report", Notifications, 2.8),
    m(Gmail, "Amazon.co.uk", "Your order #204-7781 has been dispatched", OrdersShipping, 3.6),
    m(Outlook, "Sarah Whitfield", "Senior Rust Engineer role at Ferrous Systems", Recruitment, 3.3),
    m(Gmail, "GitHub", "[neomail] PR #42: IMAP IDLE reconnect backoff", Notifications, 3.1),
    m(Proton, "Stripe", "Receipt from Fly.io #2291-4410", ReceiptsInvoices, 3.5),
    m(Gmail, "This Week in Rust", "This Week in Rust 618", Newsletters, 3.4),
    m(Outlook, "Mark Ellison", "Re: Re: budget numbers (still waiting on yours)", FollowUpOld, 2.7),
    m(Gmail, "WINNER-NOTICE", "URGENT!!! Claim your $4,500,000 inheritance now", Spam, 6.0),
    m(Proton, "Lena Vogel", "Dinner on Saturday?", Main, 2.9),
    m(Gmail, "Uniqlo", "48h only: 30% off HEATTECH", Promotions, 3.4),
    m(Outlook, "IT Service Desk", "Password expires in 3 days", Notifications, 2.2),
    m(Gmail, "Unknown sender", "quick question", Other, 0.4),
    m(Proton, "DHL Express", "Delivery attempt failed - reschedule", OrdersShipping, 3.0),
    m(Outlook, "Jordan Lee", "Following up on my note from last month", FollowUpOld, 2.9),
    m(Gmail, "Hacker Newsletter", "Hacker Newsletter #712", Newsletters, 3.2),
    m(Gmail, "Apple", "Your receipt from Apple", ReceiptsInvoices, 3.6),
];

/// Messages that arrive while the demo runs: (delay in ms, fixture).
pub const ARRIVALS: &[(u64, Fixture)] = &[
    (1800, m(Outlook, "Hannah Brooks", "Interview loop: next steps", Recruitment, 2.6)),
    (2600, m(Gmail, "Google Calendar", "Reminder: 1:1 with Priya @ 14:00", Notifications, 3.0)),
    (2650, m(Gmail, "Booking.com", "Price drop on your saved stay in Lisbon", Promotions, 2.8)),
    (3700, m(Proton, "Mum", "Photos from the weekend", Main, 2.7)),
    (4500, m(Outlook, "Contoso Billing", "Invoice INV-0937 is due on 15 Oct", ReceiptsInvoices, 3.1)),
    (5200, m(Gmail, "crypto-gains.biz", "Make 10x returns with this ONE trick", Spam, 5.5)),
    // A Proton sync burst: lots of mail at once exercises coalescing.
    (6400, m(Proton, "Kickstarter", "Project update #14: shipping begins", OrdersShipping, 2.5)),
    (6410, m(Proton, "Substack", "The Pragmatic Engineer: on-call costs", Newsletters, 3.2)),
    (6420, m(Proton, "Revolut", "You spent £42.10 at Pret A Manger", ReceiptsInvoices, 2.9)),
    (6430, m(Proton, "Tom Haverford", "Re: lunch?", Main, 1.2)),
    (6440, m(Proton, "noreply@secure-paypa1.example", "Your account has been limited", Spam, 6.0)),
    (6450, m(Proton, "Patagonia", "New arrivals for autumn", Promotions, 3.0)),
    (6460, m(Proton, "Linear", "3 issues assigned to you", Notifications, 2.9)),
    (6470, m(Proton, "Unknown", "Fwd: fyi", Other, 0.3)),
    (6480, m(Proton, "Talent at Oxide", "Hello from Oxide Computer", Recruitment, 2.8)),
    (8100, m(Outlook, "Rachel Kim", "Can you review the Q3 deck before Friday?", FollowUp, 2.8)),
    (8900, m(Gmail, "Royal Mail", "Your parcel is out for delivery", OrdersShipping, 3.4)),
    (9600, m(Outlook, "Microsoft 365", "Weekly digest: your team's activity", Newsletters, 1.8)),
    (10300, m(Gmail, "Ahmed Saleh", "Reminder: you owe me those slides :)", FollowUpOld, 2.4)),
    (10900, m(Gmail, "LUXURY-WATCHES", "R0lex replicas 90% OFF", Spam, 6.0)),
    (11500, m(Outlook, "Facilities", "Building access survey", Other, 0.6)),
    (12100, m(Proton, "Lena Vogel", "Re: Dinner on Saturday?", Main, 3.0)),
];
