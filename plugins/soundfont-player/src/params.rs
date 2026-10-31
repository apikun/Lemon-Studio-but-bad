// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Soundfont Player's parameters: every control of the editor is a host-visible parameter, stored as its position
//! 0..1 (FL's knobs are positions too) and mapped where it is used. The preset is an index into the loaded soundfont's
//! list; the patch name the file gives rides on its value text.
// spec: plugins/soundfont-player.md (§Controls: ids, ranges, defaults, value texts)

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_plugkit::live::LiveEdits;

/// The preset list's names, shared with the preset parameter's value text (the soundfont's, kept by the plugin).
pub type Names = Arc<RwLock<Vec<String>>>;

pub struct SfParams {
    /// The loaded soundfont's path (the state remembers it; a new instance loads it in `initialize`). No parameter of
    /// its own: it rides in the persisted fields, as Lemon Sampler's sample path does.
    pub sf2_path: RwLock<String>,
    /// The preset played: its place in the file's list.
    pub preset: IntParam,
    /// REVERB: the send level, its BUILT-IN switch and the send target the state keeps.
    pub rev_level: FloatParam,
    pub rev_builtin: BoolParam,
    pub rev_send: IntParam,
    /// CHORUS: the same three.
    pub cho_level: FloatParam,
    pub cho_builtin: BoolParam,
    pub cho_send: IntParam,
    /// ENVELOPE 2 (the filter envelope): attack, decay, sustain, release.
    pub env_a: FloatParam,
    pub env_d: FloatParam,
    pub env_s: FloatParam,
    pub env_r: FloatParam,
    /// LFO 2: delay, amount (to the pitch), speed.
    pub lfo_del: FloatParam,
    pub lfo_amt: FloatParam,
    pub lfo_spd: FloatParam,
    /// MISC: the filter's cutoff and the envelope's depth.
    pub cut: FloatParam,
    pub mod_amt: FloatParam,
    /// The instance's editor edits the sound hears before the host hands them back.
    pub live: Arc<LiveEdits>,
}

// The `live` edits list and the soundfont's path ride along (they are no parameters of their own), so `Params` is
// implemented by hand.
unsafe impl Params for SfParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![
            ("preset".into(), self.preset.as_ptr(), String::new()),
            ("rev_level".into(), self.rev_level.as_ptr(), String::new()),
            ("rev_builtin".into(), self.rev_builtin.as_ptr(), String::new()),
            ("rev_send".into(), self.rev_send.as_ptr(), String::new()),
            ("cho_level".into(), self.cho_level.as_ptr(), String::new()),
            ("cho_builtin".into(), self.cho_builtin.as_ptr(), String::new()),
            ("cho_send".into(), self.cho_send.as_ptr(), String::new()),
            ("env_a".into(), self.env_a.as_ptr(), String::new()),
            ("env_d".into(), self.env_d.as_ptr(), String::new()),
            ("env_s".into(), self.env_s.as_ptr(), String::new()),
            ("env_r".into(), self.env_r.as_ptr(), String::new()),
            ("lfo_del".into(), self.lfo_del.as_ptr(), String::new()),
            ("lfo_amt".into(), self.lfo_amt.as_ptr(), String::new()),
            ("lfo_spd".into(), self.lfo_spd.as_ptr(), String::new()),
            ("cut".into(), self.cut.as_ptr(), String::new()),
            ("mod".into(), self.mod_amt.as_ptr(), String::new()),
        ]
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let path = self.sf2_path.read().map(|p| p.clone()).unwrap_or_default();
        BTreeMap::from([("sf2_path".to_string(), format!("\"{path}\""))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(p) = serialized.get("sf2_path") {
            if let Ok(mut path) = self.sf2_path.write() {
                *path = p.trim_matches('"').to_string();
            }
        }
    }
}

impl SfParams {
    pub fn new(names: Names) -> Self {
        let patch_text = Arc::new(move |v: i32| {
            if let Ok(l) = names.read() {
                if let Some(t) = l.get(v.max(0) as usize) {
                    return t.clone();
                }
            }
            format!("Preset {}", v.max(0) + 1)
        });
        SfParams {
            sf2_path: RwLock::new(String::new()),
            preset: IntParam::new("Preset", 0, IntRange::Linear { min: 0, max: 4095 }).with_value_to_string(patch_text.clone()),
            rev_level: knob("Reverb level", 0.5),
            rev_builtin: BoolParam::new("Reverb built-in", true),
            rev_send: IntParam::new("Reverb send", 1, IntRange::Linear { min: 0, max: 64 }),
            cho_level: knob("Chorus level", 0.5),
            cho_builtin: BoolParam::new("Chorus built-in", true),
            cho_send: IntParam::new("Chorus send", 2, IntRange::Linear { min: 0, max: 64 }),
            env_a: knob("Envelope 2 attack", 0.0),
            env_d: knob("Envelope 2 decay", 0.35),
            env_s: knob("Envelope 2 sustain", 0.0),
            env_r: knob("Envelope 2 release", 0.35),
            lfo_del: knob("LFO 2 delay", 0.0),
            lfo_amt: knob("LFO 2 amount", 0.0),
            lfo_spd: knob("LFO 2 speed", 0.85),
            cut: knob("Filter cutoff", 0.929),
            mod_amt: knob("Filter modulation", 0.375),
            live: Arc::default(),
        }
    }

    /// The plain value the sound uses for `p` (the editor's pending edit, else the host's).
    pub fn value<P: Param>(&self, p: &P) -> P::Plain {
        self.live.value(p)
    }

    /// The value of a float parameter by its id (the editor's overlay on top).
    pub fn live_of(&self, p: ParamPtr) -> f32 {
        self.live.norm(p)
    }
}

/// The parameters in the plugin's map order (an automation request names one by its place here).
pub const PARAM_IDS: [&str; 16] = [
    "preset", "rev_level", "rev_builtin", "rev_send", "cho_level", "cho_builtin", "cho_send", "env_a", "env_d", "env_s", "env_r", "lfo_del", "lfo_amt", "lfo_spd", "cut", "mod",
];

/// A parameter's place in [`PARAM_IDS`] (the automation clip's target).
pub fn param_index(id: &str) -> Option<usize> {
    PARAM_IDS.iter().position(|&i| i == id)
}

/// A 0..1 knob shown in percent.
fn knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.0)))
}

// ---------------------------------------------------------------------------------------------------------------
// The knobs' laws (the value texts follow them)

/// Envelope 2's A / D / R: the fader's seconds (a squared law over 20 s, with a short floor).
pub fn env_seconds(v: f64) -> f64 {
    0.005 + v.clamp(0.0, 1.0) * v.clamp(0.0, 1.0) * 20.0
}

/// The fader position of an envelope time.
pub fn env_norm(seconds: f64) -> f32 {
    ((seconds - 0.005).max(0.0) / 20.0).sqrt().clamp(0.0, 1.0) as f32
}

/// LFO 2's delay: 0..5 s.
pub fn lfo_delay_seconds(v: f64) -> f64 {
    v.clamp(0.0, 1.0) * 5.0
}

pub fn lfo_delay_norm(seconds: f64) -> f32 {
    (seconds / 5.0).clamp(0.0, 1.0) as f32
}

/// LFO 2's amount: 0..1200 cents to the pitch.
pub fn lfo_amt_cents(v: f64) -> f64 {
    v.clamp(0.0, 1.0) * 1200.0
}

pub fn lfo_amt_norm(cents: f64) -> f32 {
    (cents / 1200.0).clamp(0.0, 1.0) as f32
}

/// LFO 2's speed: 0.1..10 Hz, log.
pub fn lfo_hz(v: f64) -> f64 {
    0.1 * 2f64.powf(v.clamp(0.0, 1.0) * 6.6439)
}

pub fn lfo_spd_norm(hz: f64) -> f32 {
    (f64::log2(hz.max(0.05) / 0.1) / 6.6439).clamp(0.0, 1.0) as f32
}

/// MISC's cutoff: 80..20000 Hz, log.
pub fn cut_hz(v: f64) -> f64 {
    80.0 * 250f64.powf(v.clamp(0.0, 1.0))
}

pub fn cut_norm(hz: f64) -> f32 {
    (f64::ln(hz.max(40.0) / 80.0) / f64::ln(250.0)).clamp(0.0, 1.0) as f32
}

/// MISC's modulation: 0..9600 cents.
pub fn mod_cents(v: f64) -> f64 {
    v.clamp(0.0, 1.0) * 9600.0
}

pub fn mod_norm(cents: f64) -> f32 {
    (cents / 9600.0).clamp(0.0, 1.0) as f32
}

/// A frequency's text ("1.2kHz").
pub fn hz_text(hz: f64) -> String {
    if hz >= 10000.0 {
        format!("{:.1}kHz", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.2}kHz", hz / 1000.0)
    } else {
        format!("{:.0}Hz", hz)
    }
}

/// A time's text ("1.20s", "350ms").
pub fn time_text(seconds: f64) -> String {
    if seconds >= 10.0 {
        format!("{:.1}s", seconds)
    } else if seconds >= 1.0 {
        format!("{:.2}s", seconds)
    } else {
        format!("{:.0}ms", seconds * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_laws_round_trip() {
        for &v in &[0.0f32, 0.15, 0.5, 0.85, 1.0] {
            assert!((env_norm(env_seconds(f64::from(v))) - v).abs() < 1e-4);
            assert!((lfo_delay_norm(lfo_delay_seconds(f64::from(v))) - v).abs() < 1e-4);
            assert!((lfo_amt_norm(lfo_amt_cents(f64::from(v))) - v).abs() < 1e-4);
            assert!((lfo_spd_norm(lfo_hz(f64::from(v))) - v).abs() < 1e-4);
            assert!((cut_norm(cut_hz(f64::from(v))) - v).abs() < 1e-4);
            assert!((mod_norm(mod_cents(f64::from(v))) - v).abs() < 1e-4);
        }
        // The defaults land on their mapped values.
        assert!((cut_hz(0.929) - 13500.0).abs() < 60.0);
        assert!((lfo_hz(0.85) - 5.0).abs() < 0.2);
        assert!((mod_cents(0.375) - 3600.0).abs() < 1.0);
    }

    #[test]
    fn the_texts_read_well() {
        assert_eq!(hz_text(13500.0), "13.5kHz");
        assert_eq!(hz_text(1200.0), "1.20kHz");
        assert_eq!(hz_text(440.0), "440Hz");
        assert_eq!(time_text(0.35), "350ms");
        assert_eq!(time_text(3.2), "3.20s");
        assert_eq!(time_text(15.0), "15.0s");
    }
}
