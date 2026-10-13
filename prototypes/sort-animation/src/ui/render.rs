//! Base UI rendering. It always shows the real model state; animations only
//! draw on top of it.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};

use super::layout::{Item, Line, Plan};
use crate::anim::labels::{WIDE_TAG, conf_bar, confidence_text, stamp_text, stamp_width};
use crate::anim::util::{mix, put, put_str, theme};
use crate::anim::{Animator, Geometry, MotionMode, StyleId};
use crate::groups::{Account, Group};
use crate::model::{JevState, Message, Model};

/// First sidebar line of the group bins.
pub const GROUPS_TOP: u16 = 6;

pub struct Hud {
    /// Wall-clock seconds since start (spinners).
    pub t: f32,
    /// Measured frame rate while animating; `None` when idle.
    pub fps: Option<f32>,
    /// Terminal output rate (KB/s).
    pub kbps: f32,
    pub total: usize,
}

fn st(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

fn fill(buf: &mut Buffer, r: Rect, bg: Color) {
    buf.set_style(r, Style::new().bg(bg).fg(theme::FG));
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_char(' ');
            }
        }
    }
}

/// Truncates to `w` cells with an ellipsis.
pub fn fit(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub fn spinner(t: f32, moving: bool) -> char {
    if moving { ['◐', '◓', '◑', '◒'][((t * 8.0) as usize) % 4] } else { '…' }
}

pub fn draw(buf: &mut Buffer, plan: &Plan, geo: &Geometry, model: &Model, anim: &Animator, hud: &Hud) {
    fill(buf, buf.area, theme::BG);
    header(buf, plan.header, anim);
    if let Some(side) = plan.sidebar {
        sidebar(buf, side, model, anim, hud);
    }
    if let Some(tabs) = plan.tabs {
        fill(buf, tabs, theme::PANEL);
    }
    bins(buf, geo, model);
    list(buf, plan, model, anim, hud);
    status(buf, plan.status, model, anim, hud);
}

fn header(buf: &mut Buffer, r: Rect, anim: &Animator) {
    fill(buf, r, theme::PANEL);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let (x, y, w) = (r.x as i32, r.y as i32, r.width as usize);
    put_str(buf, x, y, " neomail ", bold.fg(theme::BG).bg(theme::JEV), 9);
    let title = if anim.grouped { "Unified Inbox · grouped" } else { "Unified Inbox · 3 accounts" };
    put_str(buf, x + 10, y, title, st(theme::FG, theme::PANEL), w.saturating_sub(10));
    let mut right = String::new();
    for (i, s) in [StyleId::Machine, StyleId::Stamp, StyleId::Decode].iter().enumerate() {
        right.push_str(&format!(" {}{} ", i + 1, if *s == anim.style_id { "▸" } else { " " }));
        right.push_str(s.name());
    }
    right.push_str(&format!("  m motion:{}  g group  q quit ", anim.motion.name()));
    let mut len = right.chars().count();
    if w > len + 40 {
        let rx = r.right() as i32 - len as i32;
        put_str(buf, rx, y, &right, st(theme::DIM, theme::PANEL), len);
        let pos = right.find(anim.style_id.name()).unwrap_or(0);
        let px = rx + right[..pos].chars().count() as i32;
        put_str(buf, px, y, anim.style_id.name(), bold.fg(theme::JEV).bg(theme::PANEL), 8);
    } else {
        len = 0;
    }
    if anim.effective_motion() != anim.motion {
        // Longest note that fits between the title and the key hints.
        let room = w.saturating_sub(10 + title.chars().count() + len);
        let note = [" · small window: reduced motion", " · reduced (small window)", " · reduced"]
            .into_iter()
            .find(|n| n.chars().count() <= room);
        if let Some(n) = note {
            let nx = x + 10 + title.chars().count() as i32;
            put_str(buf, nx, y, n, st(theme::DIM, theme::PANEL), room);
        }
    }
}

fn sidebar(buf: &mut Buffer, r: Rect, model: &Model, anim: &Animator, hud: &Hud) {
    fill(buf, r, theme::PANEL);
    for y in r.top()..r.bottom() {
        put(buf, r.right() as i32 - 1, y as i32, '│', theme::BORDER, None, false);
    }
    let (x, w) = (r.x as i32, r.width as usize);
    let label = Style::new().fg(theme::DIM).bg(theme::PANEL).add_modifier(Modifier::BOLD);
    put_str(buf, x + 1, r.y as i32, "ACCOUNTS", label, w);
    for (i, a) in Account::ALL.iter().enumerate() {
        let y = r.y as i32 + 1 + i as i32;
        put(buf, x + 1, y, a.badge(), a.color(), None, true);
        put_str(buf, x + 3, y, &fit(a.address(), w.saturating_sub(9)), st(theme::FG, theme::PANEL), w);
        let n = format!("{:>3}", model.unread(*a));
        put_str(buf, r.right() as i32 - 5, y, &n, st(theme::DIM, theme::PANEL), 3);
    }
    put_str(buf, x + 1, r.y as i32 + GROUPS_TOP as i32 - 1, "JEV GROUPS", label, w);
    let y = r.y as i32 + GROUPS_TOP as i32 + 12;
    if y < r.bottom() as i32 {
        let total = hud.total.max(1);
        let cells = w.saturating_sub(14);
        let done = (model.sorted * cells) / total;
        let bar: String = (0..cells).map(|i| if i < done { '█' } else { '░' }).collect();
        put_str(buf, x + 1, y, "jev", Style::new().fg(theme::JEV).bg(theme::PANEL), 3);
        put_str(buf, x + 5, y, &bar, st(theme::JEV, theme::PANEL), cells);
        let n = format!("{}/{}", model.sorted, hud.total);
        put_str(buf, x + 6 + cells as i32, y, &n, st(theme::DIM, theme::PANEL), 7);
        let reading = model.messages.iter().filter(|m| m.state == JevState::Thinking).count();
        let moving = anim.effective_motion() != MotionMode::Off;
        let msg = if reading > 0 { format!("{} reading {}", spinner(hud.t, moving), reading) } else { "idle".into() };
        put_str(buf, x + 1, y + 1, &msg, st(theme::DIM, theme::PANEL), w - 2);
    }
}

/// Group bins (sidebar rows or compact tabs) with the real counts.
fn bins(buf: &mut Buffer, geo: &Geometry, model: &Model) {
    for g in Group::ALL {
        let (Some(rect), Some((cx, cy))) = (geo.bins[g.index()], geo.counters[g.index()]) else { continue };
        let n = model.counts[g.index()];
        let fg = if n == 0 { theme::FAINT } else { theme::FG };
        put(buf, rect.x as i32 + 1, rect.y as i32, g.glyph(), g.color(), None, true);
        if geo.compact {
            put_str(buf, cx as i32, cy as i32, &n.to_string(), st(fg, theme::PANEL), 3);
        } else {
            put_str(buf, rect.x as i32 + 3, rect.y as i32, g.label(), st(fg, theme::PANEL), 17);
            put_str(buf, cx as i32, cy as i32, &format!("{n:>3}"), st(fg, theme::PANEL), 3);
        }
    }
}

fn list(buf: &mut Buffer, plan: &Plan, model: &Model, anim: &Animator, hud: &Hud) {
    let c = plan.cols;
    let h = plan.list_head;
    let s = st(theme::FAINT, theme::BG);
    put_str(buf, c.from.0 as i32, h.y as i32, "From", s, 4);
    put_str(buf, c.subject.0 as i32, h.y as i32, "Subject", s, 7);
    put_str(buf, c.tag.0 as i32, h.y as i32, "jev", s, 3);
    put_str(buf, c.time_x as i32, h.y as i32, "Time", s, 4);
    let mut order: Vec<&Line> = plan.lines.iter().collect();
    order.sort_by_key(|l| l.moving);
    let moving = anim.effective_motion() != MotionMode::Off;
    for line in order {
        let y = line.y.round() as i32;
        if y < 0 || y >= plan.body.height as i32 {
            continue;
        }
        let y = plan.body.y as i32 + y;
        match line.item {
            Item::Header { group, count } => section(buf, plan, y, group, count),
            Item::Msg(id) => {
                if let Some(m) = model.get(id) {
                    row(buf, plan, y, m, line.selected, anim.style_id, spinner(hud.t, moving));
                }
            }
        }
    }
}

fn section(buf: &mut Buffer, plan: &Plan, y: i32, group: Option<Group>, count: usize) {
    let b = plan.body;
    let rule: String = "─".repeat(b.width as usize);
    put_str(buf, b.x as i32, y, &rule, st(theme::FAINT, theme::BG), b.width as usize);
    let (text, color) = match group {
        None => (format!(" jev is sorting · {count} "), theme::JEV),
        Some(g) => (format!(" {} {} · {count} ", g.glyph(), g.label()), g.color()),
    };
    let style = Style::new().fg(color).bg(theme::BG).add_modifier(Modifier::BOLD);
    put_str(buf, b.x as i32 + 2, y, &text, style, (b.width as usize).saturating_sub(2));
}

fn row(buf: &mut Buffer, plan: &Plan, y: i32, m: &Message, selected: bool, style: StyleId, spin: char) {
    let c = plan.cols;
    let x = plan.body.x as i32;
    let mut bg = if (y % 2) == 0 { theme::BG } else { theme::ROW_ALT };
    if selected {
        bg = mix(bg, theme::JEV, 0.16);
    }
    let w = plan.body.width as usize;
    put_str(buf, x, y, &" ".repeat(w), st(theme::FG, bg), w);
    if selected {
        put(buf, x, y, '▌', theme::JEV, None, false);
    }
    put(buf, x + 1, y, m.account.badge(), m.account.color(), None, true);
    if m.unread {
        put(buf, x + 3, y, '●', theme::JEV, None, false);
    }
    let weight = if m.unread { Modifier::BOLD } else { Modifier::empty() };
    let strong = Style::new().fg(theme::FG).bg(bg).add_modifier(weight);
    put_str(buf, c.from.0 as i32, y, &fit(m.from, c.from.1 as usize), strong, c.from.1 as usize);
    let subj = st(if m.unread { theme::FG } else { mix(theme::FG, theme::DIM, 0.5) }, bg);
    put_str(buf, c.subject.0 as i32, y, &fit(m.subject, c.subject.1 as usize), subj, c.subject.1 as usize);
    let (tx, tw) = (c.tag.0 as i32, c.tag.1 as usize);
    let narrow = tw < 6;
    match m.state {
        JevState::Pending => put_str(buf, tx, y, if narrow { "·" } else { "·  queued" }, st(theme::FAINT, bg), tw),
        JevState::Thinking => {
            put(buf, tx, y, spin, theme::JEV, None, false);
            if !narrow {
                let text = if tw >= 13 { "jev reading" } else { "reading" };
                put_str(buf, tx + 2, y, text, st(mix(theme::JEV, bg, 0.3), bg), tw - 2);
            }
        }
        JevState::Sorted { group, confidence } => {
            if narrow {
                put(buf, tx, y, group.glyph(), group.color(), None, true);
            } else if style == StyleId::Stamp {
                let sw = stamp_width(c.tag.1);
                let s = Style::new().fg(group.color()).bg(mix(bg, group.color(), 0.2)).add_modifier(Modifier::BOLD);
                put_str(buf, tx, y, &stamp_text(group, sw), s, sw);
                if c.tag.1 >= WIDE_TAG {
                    put_str(buf, tx + 15, y, &conf_bar(confidence, 4), st(mix(group.color(), bg, 0.3), bg), 4);
                }
            } else {
                put(buf, tx, y, group.glyph(), group.color(), None, true);
                let label = if tw >= 19 { group.label() } else { group.short() };
                put_str(buf, tx + 2, y, label, st(group.color(), bg), tw - 2);
            }
        }
    }
    put_str(buf, c.time_x as i32, y, &m.time, st(theme::DIM, bg), 5);
}

fn status(buf: &mut Buffer, r: Rect, model: &Model, anim: &Animator, hud: &Hud) {
    fill(buf, r, theme::PANEL);
    let mut left = format!(" jev {}/{} sorted · avg {} ms", model.sorted, hud.total, model.avg_latency_ms());
    if let Some(c) = &model.last {
        let conf = confidence_text(c.group, c.confidence);
        left.push_str(&format!(" · last {} {} {conf}", c.group.glyph(), c.group.label()));
    }
    put_str(buf, r.x as i32, r.y as i32, &left, st(theme::FG, theme::PANEL), r.width as usize);
    let rate = match hud.fps {
        Some(f) => format!("{f:>2.0} fps · {:.0} KB/s", hud.kbps),
        None => "idle".into(),
    };
    let (right, warn) = if anim.skip_note() {
        (" » backlog: skipped to final state ".to_string(), true)
    } else if anim.bulk && anim.effective_motion() == MotionMode::Full {
        (format!(" bulk sync: counters only · {rate} "), true)
    } else {
        (format!(" backlog {} ×{:.1} · {rate} ", anim.backlog(), anim.time_scale()), false)
    };
    let len = right.chars().count();
    if r.width as usize > left.chars().count() + len {
        let fg = if warn { Color::Rgb(229, 192, 123) } else { theme::DIM };
        put_str(buf, r.right() as i32 - len as i32, r.y as i32, &right, st(fg, theme::PANEL), len);
    }
}
