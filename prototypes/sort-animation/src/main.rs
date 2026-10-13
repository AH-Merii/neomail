//! neomail live sorting animation prototype.
//!
//! Layers: `jev` (simulated classifier) -> `model` (applied immediately) ->
//! `ui` (renders the real model) -> `anim` (presentation-only effects on top).

mod anim;
mod cli;
mod fixtures;
mod groups;
mod jev;
mod model;
mod term;
mod ui;

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;

use anim::{Animator, Geometry, StyleId};
use model::{Model, MsgId};
use term::{Pacer, Term};
use ui::layout::{Item, View};
use ui::render::Hud;

const LOOP_PAUSE: Duration = Duration::from_millis(2500);

struct App {
    model: Model,
    sim: jev::JevSim,
    anim: Animator,
    geo: Geometry,
    view: View,
    /// Message order of the last frame (for j/k).
    order: Vec<MsgId>,
    start: Instant,
}

impl App {
    fn new(anim: Animator, seed: u64) -> Self {
        let (model, sim) = jev::JevSim::new(seed);
        App { model, sim, anim, geo: Geometry::default(), view: View::default(), order: Vec::new(), start: Instant::now() }
    }

    fn restart(&mut self, seed: u64) {
        let a = &self.anim;
        let mut anim = Animator::new(a.style_id, a.motion, a.speed, seed, a.grouped);
        anim.tiny = a.tiny;
        *self = App::new(anim, seed);
    }

    fn draw(&mut self, f: &mut Frame, hud: &Hud) {
        self.anim.sync(&self.model);
        let plan = ui::layout::plan(f.area(), &self.model, &mut self.anim, &mut self.view);
        let geo = ui::layout::geometry(&plan);
        ui::render::draw(f.buffer_mut(), &plan, &geo, &self.model, &self.anim, hud);
        self.anim.overlay(f.buffer_mut(), &geo, &self.model);
        self.order = plan.lines.iter().filter_map(|l| if let Item::Msg(id) = l.item { Some(id) } else { None }).collect();
        self.geo = geo;
    }

    /// Moves the selection by `delta` rows in presented order.
    fn select(&mut self, delta: i32) {
        let Some(last) = self.order.len().checked_sub(1) else { return };
        let cur = self.view.selected.and_then(|s| self.order.iter().position(|&id| id == s));
        let next = match cur {
            Some(i) => (i as i32 + delta).clamp(0, last as i32) as usize,
            None => 0,
        };
        self.view.selected = Some(self.order[next]);
        self.view.anchor = None;
    }

    /// Simulation time: `--sim-speed` scales the jev stream.
    fn sim_ms(&self, sim_speed: f32) -> u64 {
        (self.start.elapsed().as_secs_f64() * sim_speed as f64 * 1000.0) as u64
    }
}

fn main() -> io::Result<()> {
    let args = match cli::parse() {
        Ok(a) => a,
        Err(msg) => {
            let help = msg.starts_with("sort-animation:");
            eprintln!("{msg}");
            std::process::exit(if help { 0 } else { 2 });
        }
    };
    let mut term = Term::init()?;
    let result = run(&mut term, &args);
    ratatui::restore();
    result
}

fn run(term: &mut Term, args: &cli::Args) -> io::Result<()> {
    let launch = Instant::now();
    let mut app = App::new(Animator::new(args.style, args.motion, args.speed, args.seed, args.grouped), args.seed);
    let size = term.terminal.size()?;
    app.anim.set_viewport(size.width, size.height);
    let mut pacer = Pacer::default();
    let (mut dirty, mut next_draw, mut last_tick) = (true, None::<Instant>, Instant::now());
    let mut last_draw = Instant::now();
    let (mut done_at, mut loops) = (None::<Instant>, 0u64);

    loop {
        let now = Instant::now();
        if args.duration.is_some_and(|d| now.duration_since(launch).as_secs_f32() >= d) {
            return Ok(());
        }

        // 1. jev events: model first, then the (purely visual) animation.
        //    The screen catches up at the next frame slot (within one frame
        //    budget), so bursts of results do not exceed the frame rate.
        let events = app.sim.poll(app.sim_ms(args.sim_speed));
        if !events.is_empty() {
            let slot = last_draw + Duration::from_secs_f32(pacer.budget());
            next_draw = Some(next_draw.map_or(slot, |t| t.min(slot)));
        }
        for ev in events {
            app.model.apply(&ev);
            app.anim.on_event(&ev, &app.model, &app.geo);
        }

        // 2. Optional loop once everything is sorted and settled.
        if args.looping && app.sim.finished() && next_draw.is_none() && !dirty {
            let since = *done_at.get_or_insert(now);
            if now.duration_since(since) >= LOOP_PAUSE {
                loops += 1;
                app.restart(args.seed.wrapping_add(loops));
                (done_at, dirty) = (None, true);
            }
        }

        // 3. Draw when something changed or a frame is due. What comes next
        //    is decided after drawing, when layout and geometry are current.
        app.anim.update(now.duration_since(last_tick).as_secs_f32().min(0.1));
        last_tick = now;
        if dirty || next_draw.is_some_and(|t| now >= t) {
            let hud = Hud { t: launch.elapsed().as_secs_f32(), fps: pacer.fps(), kbps: pacer.kbps(), total: app.sim.total };
            let bytes = term.draw(|f| app.draw(f, &hud))?;
            pacer.record(now, now.elapsed().as_secs_f32(), bytes);
            (dirty, last_draw) = (false, now);
            next_draw = app.anim.next_frame(&app.geo, &app.model).map(|s| {
                now + Duration::from_secs_f32(s.max(pacer.budget()))
            });
            if next_draw.is_none() && hud.fps.is_some() {
                pacer.idle();
                dirty = true; // one more frame to show "idle"
            }
        }

        // 4. Sleep until the next frame, jev event, input or deadline.
        let mut timeout = Duration::from_secs(3600);
        if dirty {
            timeout = Duration::ZERO;
        }
        if let Some(t) = next_draw {
            timeout = timeout.min(t.saturating_duration_since(Instant::now()));
        }
        if let Some(due) = app.sim.next_due_ms() {
            let real_ms = (due as f64 / args.sim_speed as f64) as u64;
            let elapsed = app.start.elapsed().as_millis() as u64;
            timeout = timeout.min(Duration::from_millis(real_ms.saturating_sub(elapsed)));
        }
        if let Some(d) = args.duration {
            timeout = timeout.min(Duration::from_secs_f32(d).saturating_sub(launch.elapsed()));
        }
        if let Some(since) = done_at {
            timeout = timeout.min(LOOP_PAUSE.saturating_sub(since.elapsed()));
        } else if args.looping && app.sim.finished() {
            timeout = timeout.min(Duration::from_millis(250));
        }

        if event::poll(timeout)? {
            match event::read()? {
                Event::Resize(w, h) => app.anim.set_viewport(w, h),
                Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Char('1') => app.anim.set_style(StyleId::Machine),
                    KeyCode::Char('2') => app.anim.set_style(StyleId::Stamp),
                    KeyCode::Char('3') => app.anim.set_style(StyleId::Decode),
                    KeyCode::Char('m') => app.anim.cycle_motion(),
                    KeyCode::Char('g') => app.anim.toggle_grouped(),
                    KeyCode::Char('j') | KeyCode::Down => app.select(1),
                    KeyCode::Char('k') | KeyCode::Up => app.select(-1),
                    KeyCode::Char('r') => {
                        app.restart(args.seed);
                        done_at = None;
                    }
                    _ => {}
                },
                _ => {}
            }
            dirty = true;
        }
    }
}
