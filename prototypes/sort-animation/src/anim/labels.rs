//! Group labels shared by the base UI and the effects (owned by the
//! animation layer so it does not depend on the UI).

use crate::groups::Group;

/// Tag column width that fits the full stamp plus confidence bar.
pub const WIDE_TAG: u16 = 19;
/// Narrow tag column: glyph plus a short label.
pub const NARROW_TAG: u16 = 12;

/// The stamp label, `width` cells (14 in the wide layout).
pub fn stamp_text(g: Group, width: usize) -> String {
    let short = g.short().to_uppercase();
    let s = if width >= 14 { format!(" {} {short} ", g.glyph()) } else { format!("{} {short}", g.glyph()) };
    let mut out: String = s.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Width of the stamp inside a tag column of `tag_w` cells.
pub fn stamp_width(tag_w: u16) -> usize {
    if tag_w >= WIDE_TAG { 14 } else { tag_w as usize }
}

/// Confidence bar, `cells` wide, filled to `conf`.
pub fn conf_bar(conf: f32, cells: usize) -> String {
    let n = (conf * cells as f32).round() as usize;
    (0..cells).map(|i| if i < n { '▮' } else { '▯' }).collect()
}

/// Confidence as shown to people. "other" means jev was not confident
/// enough for any group, so its top score would only confuse.
pub fn confidence_text(g: Group, conf: f32) -> String {
    if g == Group::Other { "low confidence".into() } else { format!("{:.0}%", conf * 100.0) }
}

/// Short form for tight spots (4 cells).
pub fn confidence_short(g: Group, conf: f32) -> String {
    if g == Group::Other { " low".into() } else { format!("{:>3.0}%", conf * 100.0) }
}
