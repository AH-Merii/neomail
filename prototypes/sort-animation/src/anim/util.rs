//! Shared drawing and easing helpers for animation styles.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use tachyonfx::Interpolation;

/// Theme colours shared by the UI and the effects.
pub mod theme {
    use ratatui::style::Color;
    pub const BG: Color = Color::Rgb(20, 22, 28);
    pub const PANEL: Color = Color::Rgb(27, 30, 38);
    pub const ROW_ALT: Color = Color::Rgb(24, 26, 33);
    pub const BORDER: Color = Color::Rgb(58, 63, 78);
    pub const FG: Color = Color::Rgb(205, 210, 220);
    pub const DIM: Color = Color::Rgb(110, 116, 132);
    pub const FAINT: Color = Color::Rgb(70, 75, 90);
    pub const JEV: Color = Color::Rgb(120, 220, 230);
}

/// Eases `t` (clamped to 0..=1) with a tachyonfx interpolation curve.
pub fn ease(curve: Interpolation, t: f32) -> f32 {
    curve.alpha(t.clamp(0.0, 1.0))
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn to_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::White => (255, 255, 255),
        Color::Black => (0, 0, 0),
        _ => (128, 128, 128),
    }
}

/// Linear blend between two colours.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = to_rgb(a);
    let (br, bg, bb) = to_rgb(b);
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| lerp(x as f32, y as f32, t).round() as u8;
    Color::Rgb(f(ar, br), f(ag, bg), f(ab, bb))
}

/// Quadratic Bezier point.
pub fn bezier(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
        u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
    )
}

fn inside(buf: &Buffer, x: i32, y: i32) -> bool {
    let a = buf.area;
    x >= a.x as i32 && y >= a.y as i32 && x < a.right() as i32 && y < a.bottom() as i32
}

/// Draws one character, ignoring out-of-bounds positions.
pub fn put(buf: &mut Buffer, x: i32, y: i32, ch: char, fg: Color, bg: Option<Color>, bold: bool) {
    if !inside(buf, x, y) {
        return;
    }
    if let Some(cell) = buf.cell_mut((x as u16, y as u16)) {
        cell.set_char(ch).set_fg(fg);
        if let Some(bg) = bg {
            cell.set_bg(bg);
        }
        if bold {
            cell.modifier.insert(Modifier::BOLD);
        }
    }
}

/// Draws a string, clipped to the buffer and `max` cells.
pub fn put_str(buf: &mut Buffer, x: i32, y: i32, s: &str, style: Style, max: usize) {
    for (i, ch) in s.chars().take(max).enumerate() {
        let xi = x + i as i32;
        if inside(buf, xi, y) {
            if let Some(cell) = buf.cell_mut((xi as u16, y as u16)) {
                cell.set_char(ch).set_style(style);
            }
        }
    }
}

/// Reads a cell's background (theme background when unset).
pub fn bg_at(buf: &Buffer, x: u16, y: u16) -> Color {
    match buf.cell((x, y)).map(|c| c.bg) {
        Some(Color::Reset) | None => theme::BG,
        Some(c) => c,
    }
}

/// Tints the background of a rect towards `color` by `amount`.
pub fn tint_bg(buf: &mut Buffer, rect: Rect, color: Color, amount: f32) {
    let r = rect.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            let base = bg_at(buf, x, y);
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(mix(base, color, amount));
            }
        }
    }
}

/// Tints the foreground of a rect towards `color` by `amount`.
pub fn tint_fg(buf: &mut Buffer, rect: Rect, color: Color, amount: f32) {
    let r = rect.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let base = if cell.fg == Color::Reset { theme::FG } else { cell.fg };
                cell.set_fg(mix(base, color, amount));
            }
        }
    }
}

/// The text of a rect row, one char per cell.
pub fn read_row(buf: &Buffer, x: u16, y: u16, w: u16) -> Vec<char> {
    (x..x + w)
        .map(|xi| {
            buf.cell((xi, y))
                .and_then(|c| c.symbol().chars().next())
                .unwrap_or(' ')
        })
        .collect()
}
