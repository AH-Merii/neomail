//! Minimal hand-rolled argument parsing (keeps the dependency list short).

use crate::anim::{MotionMode, StyleId};

pub struct Args {
    pub style: StyleId,
    pub motion: MotionMode,
    /// Animation speed multiplier (2.0 = twice as fast).
    pub speed: f32,
    pub seed: u64,
    /// Auto-exit after this many seconds (for recordings).
    pub duration: Option<f32>,
    /// Restart the simulation once everything is sorted.
    pub looping: bool,
    /// Speed of the simulated jev stream (2.0 = results twice as fast).
    pub sim_speed: f32,
    /// Start in the grouped inbox view.
    pub grouped: bool,
}

const USAGE: &str = "\
sort-animation: neomail live sorting animation prototype

USAGE:
  sort-animation [OPTIONS]

OPTIONS:
  --style machine|stamp|decode   animation concept (default: machine)
  --motion full|reduced|off      motion mode (default: full)
  --speed <x>                    animation speed multiplier (default: 1.0)
  --sim-speed <x>                speed of the simulated jev stream (default: 1.0)
  --grouped                      start in the grouped inbox view
  --seed <n>                     simulation seed (default: 7)
  --duration <secs>              auto-exit after <secs> (for recording)
  --loop                         restart once everything is sorted
  -h, --help                     show this help

KEYS:
  q quit   1/2/3 switch style   m cycle motion   g grouped view\n  j/k select (stays anchored)   r restart";

pub fn parse() -> Result<Args, String> {
    let mut args = Args {
        style: StyleId::Machine,
        motion: MotionMode::Full,
        speed: 1.0,
        seed: 7,
        duration: None,
        looping: false,
        sim_speed: 1.0,
        grouped: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match key.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--style" => {
                args.style = match value("--style")?.as_str() {
                    "machine" | "1" => StyleId::Machine,
                    "stamp" | "2" => StyleId::Stamp,
                    "decode" | "3" => StyleId::Decode,
                    other => return Err(format!("unknown style '{other}'\n\n{USAGE}")),
                }
            }
            "--motion" => {
                args.motion = match value("--motion")?.as_str() {
                    "full" | "on" => MotionMode::Full,
                    "reduced" => MotionMode::Reduced,
                    "off" => MotionMode::Off,
                    other => return Err(format!("unknown motion '{other}'\n\n{USAGE}")),
                }
            }
            "--speed" => {
                let v: f32 = value("--speed")?.parse().map_err(|e| format!("--speed: {e}"))?;
                if !(0.1..=10.0).contains(&v) {
                    return Err("--speed must be between 0.1 and 10".into());
                }
                args.speed = v;
            }
            "--seed" => args.seed = value("--seed")?.parse().map_err(|e| format!("--seed: {e}"))?,
            "--duration" => {
                let v: f32 = value("--duration")?.parse().map_err(|e| format!("--duration: {e}"))?;
                args.duration = Some(v.max(0.1));
            }
            "--sim-speed" => {
                let v: f32 = value("--sim-speed")?.parse().map_err(|e| format!("--sim-speed: {e}"))?;
                if !(0.1..=20.0).contains(&v) {
                    return Err("--sim-speed must be between 0.1 and 20".into());
                }
                args.sim_speed = v;
            }
            "--grouped" => args.grouped = true,
            "--loop" => args.looping = true,
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }
    Ok(args)
}
