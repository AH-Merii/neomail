//! Screen layout: regions, list columns, scrolling and (possibly gliding)
//! line order.

use ratatui::layout::Rect;

use crate::anim::labels::{NARROW_TAG, WIDE_TAG};
use crate::anim::{Animator, Geometry, RowGeo};
use crate::groups::Group;
use crate::model::{Model, MsgId};

/// Pseudo ids for section headers so they can glide like rows.
const HEADER_ID: MsgId = 1_000_000;
/// Below this width the sidebar becomes a row of group tabs.
pub const SIDEBAR_MIN_WIDTH: u16 = 100;

#[derive(Clone, Copy, Debug)]
pub struct Cols {
    pub from: (u16, u16),
    pub subject: (u16, u16),
    pub tag: (u16, u16),
    pub time_x: u16,
}

#[derive(Clone, Copy, Debug)]
pub enum Item {
    Msg(MsgId),
    /// Section header: `None` is the "jev is sorting" section.
    Header { group: Option<Group>, count: usize },
}

pub struct Line {
    pub item: Item,
    /// Presented row index inside the list body (fractional while gliding).
    pub y: f32,
    pub moving: bool,
    pub selected: bool,
}

/// Selection and scrolling. The selected row stays anchored on its screen
/// row while the list changes around it (new mail, section moves).
#[derive(Default)]
pub struct View {
    pub selected: Option<MsgId>,
    /// Screen row of the selection; `None` re-derives it (after j/k).
    pub anchor: Option<usize>,
    offset: usize,
}

pub struct Plan {
    pub compact: bool,
    pub header: Rect,
    pub status: Rect,
    pub sidebar: Option<Rect>,
    pub tabs: Option<Rect>,
    /// Column header line of the list.
    pub list_head: Rect,
    /// List body (rows).
    pub body: Rect,
    pub cols: Cols,
    pub lines: Vec<Line>,
}

fn cols(body: Rect) -> Cols {
    let (x, w) = (body.x, body.width);
    let from_w = if w >= 90 { 18 } else if w >= 64 { 14 } else { 10 };
    let tag_w = if w >= 88 { WIDE_TAG } else if w >= 64 { NARROW_TAG } else { 3 };
    let from = (x + 5, from_w);
    let subject_x = from.0 + from_w + 1;
    let time_x = (x + w).saturating_sub(6);
    let tag_x = time_x.saturating_sub(tag_w + 1);
    let subject_w = tag_x.saturating_sub(subject_x + 1);
    Cols { from, subject: (subject_x, subject_w), tag: (tag_x, tag_w), time_x }
}

fn items(model: &Model, anim: &Animator) -> Vec<(MsgId, Item)> {
    if !anim.grouped {
        return model.messages.iter().map(|m| (m.id, Item::Msg(m.id))).collect();
    }
    let mut out = Vec::new();
    let pending: Vec<MsgId> = model.messages.iter().filter(|m| anim.section(m.id).is_none()).map(|m| m.id).collect();
    if !pending.is_empty() {
        out.push((HEADER_ID, Item::Header { group: None, count: pending.len() }));
        out.extend(pending.into_iter().map(|id| (id, Item::Msg(id))));
    }
    for g in Group::ALL {
        let members: Vec<MsgId> =
            model.messages.iter().filter(|m| anim.section(m.id) == Some(g)).map(|m| m.id).collect();
        if !members.is_empty() {
            out.push((HEADER_ID + 1 + g.index() as MsgId, Item::Header { group: Some(g), count: members.len() }));
            out.extend(members.into_iter().map(|id| (id, Item::Msg(id))));
        }
    }
    out
}

/// Keeps the selection visible and on its anchor row; returns the offset.
fn scroll(view: &mut View, sel: Option<usize>, len: usize, height: usize) -> usize {
    let max_off = len.saturating_sub(height);
    let mut off = view.offset.min(max_off);
    if let Some(i) = sel {
        if let Some(a) = view.anchor {
            off = i.saturating_sub(a);
        }
        if i < off {
            off = i;
        } else if height > 0 && i >= off + height {
            off = i + 1 - height;
        }
        off = off.min(max_off);
        view.anchor = Some(i.saturating_sub(off));
    }
    view.offset = off;
    off
}

pub fn plan(area: Rect, model: &Model, anim: &mut Animator, view: &mut View) -> Plan {
    let compact = area.width < SIDEBAR_MIN_WIDTH || area.height < 24;
    let header = Rect::new(area.x, area.y, area.width, 1);
    let status = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    let mid_h = area.height.saturating_sub(2);
    let (sidebar, tabs, list) = if compact {
        let tabs = Rect::new(area.x, area.y + 1, area.width, 1.min(mid_h));
        let list = Rect::new(area.x, area.y + 2, area.width, mid_h.saturating_sub(1));
        (None, Some(tabs), list)
    } else {
        let sw = if area.width >= 110 { 30 } else { 26 };
        let side = Rect::new(area.x, area.y + 1, sw, mid_h);
        let list = Rect::new(area.x + sw, area.y + 1, area.width - sw, mid_h);
        (Some(side), None, list)
    };
    let list_head = Rect::new(list.x, list.y, list.width, 1.min(list.height));
    let body = Rect::new(list.x, list.y + 1, list.width, list.height.saturating_sub(1));

    let items = items(model, anim);
    let sel = view.selected.and_then(|s| items.iter().position(|(id, _)| *id == s));
    let off = scroll(view, sel, items.len(), body.height as usize) as f32;
    let mut ys: Vec<(MsgId, f32)> = items.iter().enumerate().map(|(i, (id, _))| (*id, i as f32 - off)).collect();
    anim.place_rows(&mut ys);
    let lines = items
        .into_iter()
        .zip(ys)
        .enumerate()
        .map(|(i, ((id, item), (_, y)))| Line {
            item,
            y,
            moving: (y - (i as f32 - off)).abs() > 0.3,
            selected: view.selected == Some(id),
        })
        .collect();
    Plan { compact, header, status, sidebar, tabs, list_head, body, cols: cols(body), lines }
}

/// Publishes where rows and bins are, for the animation layer.
pub fn geometry(plan: &Plan) -> Geometry {
    let area = Rect::new(plan.header.x, plan.header.y, plan.header.width, plan.status.bottom() - plan.header.y);
    let stage_top = plan.tabs.map_or(plan.body.y, |t| t.y);
    let stage = Rect::new(area.x, stage_top, area.width, plan.body.bottom().saturating_sub(stage_top));
    let mut geo = Geometry { area, list: plan.body, stage, compact: plan.compact, ..Default::default() };
    for line in &plan.lines {
        if let Item::Msg(id) = line.item {
            let y = line.y.round() as i32;
            if y >= 0 && y < plan.body.height as i32 {
                let row = RowGeo {
                    x: plan.body.x,
                    y: plan.body.y + y as u16,
                    width: plan.body.width,
                    subject: plan.cols.subject,
                    tag: plan.cols.tag,
                };
                geo.rows.insert(id, row);
            }
        }
    }
    if let Some(side) = plan.sidebar {
        for g in Group::ALL {
            let y = side.y + super::render::GROUPS_TOP + g.index() as u16;
            if y < side.bottom() {
                geo.bins[g.index()] = Some(Rect::new(side.x, y, side.width - 1, 1));
                geo.counters[g.index()] = Some((side.right() - 6, y));
            }
        }
    } else if let Some(tabs) = plan.tabs {
        let tw = (tabs.width / 11).clamp(3, 6);
        for g in Group::ALL {
            let x = tabs.x + g.index() as u16 * tw;
            if x + tw <= tabs.right() {
                geo.bins[g.index()] = Some(Rect::new(x, tabs.y, tw, 1));
                geo.counters[g.index()] = Some((x + 2, tabs.y));
            }
        }
    }
    geo
}
