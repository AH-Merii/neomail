//! The 11 default jev groups and the demo mail accounts.
//!
//! Glyphs are plain Unicode (no Nerd Font), each one terminal cell wide.

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Main,
    FollowUp,
    FollowUpOld,
    Recruitment,
    ReceiptsInvoices,
    OrdersShipping,
    Newsletters,
    Promotions,
    Notifications,
    Spam,
    Other,
}

impl Group {
    pub const ALL: [Group; 11] = [
        Group::Main,
        Group::FollowUp,
        Group::FollowUpOld,
        Group::Recruitment,
        Group::ReceiptsInvoices,
        Group::OrdersShipping,
        Group::Newsletters,
        Group::Promotions,
        Group::Notifications,
        Group::Spam,
        Group::Other,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Main => "main",
            Group::FollowUp => "follow-up",
            Group::FollowUpOld => "follow-up-old",
            Group::Recruitment => "recruitment",
            Group::ReceiptsInvoices => "receipts-invoices",
            Group::OrdersShipping => "orders-shipping",
            Group::Newsletters => "newsletters",
            Group::Promotions => "promotions",
            Group::Notifications => "notifications",
            Group::Spam => "spam",
            Group::Other => "other",
        }
    }

    /// Short label for narrow columns.
    pub fn short(self) -> &'static str {
        match self {
            Group::Main => "main",
            Group::FollowUp => "follow-up",
            Group::FollowUpOld => "follow-old",
            Group::Recruitment => "recruit",
            Group::ReceiptsInvoices => "receipts",
            Group::OrdersShipping => "orders",
            Group::Newsletters => "newsletter",
            Group::Promotions => "promos",
            Group::Notifications => "notifs",
            Group::Spam => "spam",
            Group::Other => "other",
        }
    }

    pub fn glyph(self) -> char {
        match self {
            Group::Main => '★',
            Group::FollowUp => '↻',
            Group::FollowUpOld => '◷',
            Group::Recruitment => '§',
            Group::ReceiptsInvoices => '¤',
            Group::OrdersShipping => '▣',
            Group::Newsletters => '¶',
            Group::Promotions => '%',
            Group::Notifications => '◉',
            Group::Spam => '⊘',
            Group::Other => '○',
        }
    }

    pub fn color(self) -> Color {
        let (r, g, b) = self.rgb();
        Color::Rgb(r, g, b)
    }

    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Group::Main => (97, 175, 239),
            Group::FollowUp => (229, 192, 123),
            Group::FollowUpOld => (209, 140, 92),
            Group::Recruitment => (198, 120, 221),
            Group::ReceiptsInvoices => (152, 195, 121),
            Group::OrdersShipping => (86, 182, 194),
            Group::Newsletters => (176, 186, 204),
            Group::Promotions => (255, 121, 198),
            Group::Notifications => (138, 155, 255),
            Group::Spam => (224, 92, 104),
            Group::Other => (128, 133, 145),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Account {
    Gmail,
    Outlook,
    Proton,
}

impl Account {
    pub const ALL: [Account; 3] = [Account::Gmail, Account::Outlook, Account::Proton];

    pub fn badge(self) -> char {
        match self {
            Account::Gmail => 'G',
            Account::Outlook => 'O',
            Account::Proton => 'P',
        }
    }

    pub fn address(self) -> &'static str {
        match self {
            Account::Gmail => "amira.k@gmail.example",
            Account::Outlook => "a.khalil@work.example",
            Account::Proton => "amira@proton.example",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Account::Gmail => Color::Rgb(234, 67, 53),
            Account::Outlook => Color::Rgb(0, 120, 212),
            Account::Proton => Color::Rgb(124, 92, 255),
        }
    }
}

/// Small deterministic PRNG (SplitMix64). Shared by the simulator and effects.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo + 1)
    }
}
