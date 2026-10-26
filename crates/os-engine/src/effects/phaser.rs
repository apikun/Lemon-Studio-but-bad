// Copyright (C) 2026 apikun
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's phaser, Lemon Studio's own design (no original's saved state to read). A chain of first-order
//! all-pass stages, two to eight of them, is swept by a slow sine LFO over a centre frequency; the chain's output is
//! fed back into its input and mixed with it. The notches the feedback deepens sweep through the sound. The right
//! side's LFO runs a settable share of a cycle ahead, so a stereo signal phases out of step. Renders repeat: the LFOs
//! always start together.
//!
//! Settings are plain knob values (0 to the range's end, the feedback bipolar), not a project file's saved state:
//! the plugin stores them through its parameters (spec: plugins/lemon-small-effects.md §5).

use std::f64::consts::{PI, TAU};

use super::Effect;

/// The settings, in the knobs' order.
pub const PARAMS: usize = 7;
pub const RATE: usize = 0;
pub const DEPTH: usize = 1;
pub const CENTER: usize = 2;
pub const FEEDBACK: usize = 3;
pub const STAGES: usize = 4;
pub const STEREO: usize = 5;
pub const MIX: usize = 6;

/// The knobs' ranges. The feedback is bipolar (its centre is a magnet on the knob); the stages knob stands for 2, 4,
/// 6 and 8 stages.
pub const RANGES: [(i32, i32); PARAMS] = [(0, 5000), (0, 1024), (0, 5000), (-512, 512), (0, 3), (0, 1024), (0, 1024)];

/// What a new instance holds: a 0.45 Hz LFO, 50 % depth, a 320 Hz centre, 59.9 % feedback, four stages, the right
/// side a quarter cycle behind, an even mix.
pub const DEFAULTS: [i32; PARAMS] = [1500, 512, 2500, 341, 1, 512, 512];

/// The saved state's version (the plugin's own; a later one is not read, spec: plugins/lemon-small-effects.md §5).
pub const VERSION: i32 = 1;

/// The most stages a chain has; a state asks for at most [`RANGES`]`[STAGES].1 + 1` pairs.
pub const MAX_STAGES: usize = 8;

/// Everything the saved state says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub v: [i32; PARAMS],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { v: DEFAULTS }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// From a knob to its working value

/// Knob `k`'s position: `single(value / max)`, kept within 0..1. The feedback's signed position is `bipolar`.
pub fn position(k: usize, v: i32) -> f32 {
    ((f64::from(v) / f64::from(RANGES[k].1)) as f32).clamp(0.0, 1.0)
}

/// A bipolar knob's position: `value / max`, not clamped (the feedback's).
pub fn bipolar(k: usize, v: i32) -> f32 {
    (f64::from(v) / f64::from(RANGES[k].1)) as f32
}

/// An LFO's speed in Hz: `5 x^2` (the chorus's curve), so the knob's low half holds the slow sweeps.
pub fn speed_hz(x: f32) -> f64 {
    5.0 * f64::from(x) * f64::from(x)
}

/// An LFO's step per sample, where 2^24 is one cycle.
pub fn step(x: f32, rate: u32) -> u32 {
    ((speed_hz(x) * 16_777_216.0) / f64::from(rate)) as u32
}

/// The stages knob's count: 0, 1, 2 and 3 stand for 2, 4, 6 and 8; a stored value outside the range acts as the end
/// it passes.
pub fn stages(v: i32) -> usize {
    2 * (v.clamp(0, 3) as usize + 1)
}

/// The centre knob's frequency: a MIDI note number `127 x` as a frequency (the chorus's cross cutoff's curve), 8.2 Hz
/// to 12.5 kHz.
pub fn center_hz(x: f32) -> f32 {
    ((((127.0 * f64::from(x) - 69.0) / 12.0) * std::f64::consts::LN_2).exp() * 440.0) as f32
}

/// The feedback gain: 0.9 at the knob's ends.
pub fn feedback(x: f32) -> f32 {
    x * 0.9
}

/// How far the right side's LFO runs ahead: `x` of half a cycle (0 to 180 degrees).
pub fn stereo_offset(x: f32) -> u32 {
    (f64::from(x) * 8_388_608.0) as u32
}

/// The working values of a set of settings at a sample rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Working {
    pub step: u32,
    pub depth: f32,
    pub center: f32,
    pub fb: f32,
    pub stages: usize,
    pub stereo: u32,
    pub mix: f32,
}

impl Working {
    pub fn new(v: &[i32; PARAMS], rate: u32) -> Working {
        Working {
            step: step(position(RATE, v[RATE]), rate),
            depth: position(DEPTH, v[DEPTH]),
            center: center_hz(position(CENTER, v[CENTER])),
            fb: feedback(bipolar(FEEDBACK, v[FEEDBACK])),
            stages: stages(v[STAGES]),
            stereo: stereo_offset(position(STEREO, v[STEREO])),
            mix: position(MIX, v[MIX]),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The effect

pub struct Phaser {
    applied: [i32; PARAMS],
    rate: u32,
    w: Working,
    /// 2^24 per cycle; the right side runs `w.stereo` ahead.
    phase: u32,
    /// Per side, per stage: the stage's previous input and output.
    before: [[f32; MAX_STAGES]; 2],
    after: [[f32; MAX_STAGES]; 2],
    /// Per side: the chain's output last sample (the feedback's).
    last: [f32; 2],
}

impl Phaser {
    /// A new instance started as at a render: the settings in force, the stages empty, the LFOs together.
    pub fn with_settings(rate: u32, s: &Settings) -> Phaser {
        Phaser {
            applied: s.v,
            rate,
            w: Working::new(&s.v, rate),
            phase: 0,
            before: [[0.0; MAX_STAGES]; 2],
            after: [[0.0; MAX_STAGES]; 2],
            last: [0.0; 2],
        }
    }

    /// The host enables the effect: the stages are emptied and the LFOs start together again.
    pub fn start(&mut self) {
        self.before = [[0.0; MAX_STAGES]; 2];
        self.after = [[0.0; MAX_STAGES]; 2];
        self.last = [0.0; 2];
        self.phase = 0;
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: they act from the next sample (nothing is smoothed); a change of the stages' count empties the
    /// stages, whose history belongs to the old chain.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            let changed_count = stages(s.v[STAGES]) != stages(self.applied[STAGES]);
            self.applied = s.v;
            self.w = Working::new(&s.v, self.rate);
            if changed_count {
                self.before = [[0.0; MAX_STAGES]; 2];
                self.after = [[0.0; MAX_STAGES]; 2];
            }
        }
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.w = Working::new(&self.applied, rate);
    }

    /// The working values (tests and the editor).
    pub fn working(&self) -> &Working {
        &self.w
    }

    /// One sample of both sides. The feedback is one sample round the loop; the stage frequency is held within 8 Hz
    /// and 0.45 of the rate, where the all-pass coefficients stay well inside the unit circle.
    #[inline]
    fn sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        let w = self.w;
        let x = [l, r];
        let angle = TAU * f64::from(self.phase) / 16_777_216.0;
        let angle_r = TAU * f64::from(self.phase.wrapping_add(w.stereo)) / 16_777_216.0;
        self.phase = self.phase.wrapping_add(w.step);
        let lfo = [angle.sin(), angle_r.sin()];
        let top = self.rate as f32 * 0.45;
        let mut out = [0.0f32; 2];
        for s in 0..2 {
            let f = (w.center * (1.0 + w.depth * lfo[s] as f32)).clamp(8.0, top) as f64;
            let t = (PI * f / f64::from(self.rate)).tan();
            let g = ((t - 1.0) / (t + 1.0)) as f32;
            let mut y = x[s] + w.fb * self.last[s];
            for k in 0..w.stages {
                let next = g * y + self.before[s][k] - g * self.after[s][k];
                self.before[s][k] = y;
                self.after[s][k] = next;
                y = next;
            }
            self.last[s] = if y.abs() < 1e-20 { 0.0 } else { y };
            out[s] = x[s] * (1.0 - w.mix) + y * w.mix;
        }
        (out[0], out[1])
    }

    /// The end of a block: a state under 1e-11 in magnitude is set to 0.
    fn end_block(&mut self) {
        for s in 0..2 {
            for k in 0..MAX_STAGES {
                if self.before[s][k].abs() < 1e-11 {
                    self.before[s][k] = 0.0;
                }
                if self.after[s][k].abs() < 1e-11 {
                    self.after[s][k] = 0.0;
                }
            }
            if self.last[s].abs() < 1e-11 {
                self.last[s] = 0.0;
            }
        }
    }

    /// One block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.sample(*l, *r);
        }
        self.end_block();
    }
}

impl Effect for Phaser {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            (f[0], f[1]) = self.sample(f[0], f[1]);
        }
        self.end_block();
    }

    /// A slot switched on during the song: the stages empty and the LFOs start together again.
    fn reset(&mut self) {
        self.start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_become_working_values() {
        let w = Working::new(&DEFAULTS, 44100);
        // The speed matches the chorus's default LFO 1 step (0.45 Hz); the centre matches the chorus's cross cutoff
        // at the same knob (320.2 Hz).
        assert_eq!(w.step, 171);
        assert_eq!(w.center, 320.243_7);
        assert_eq!((w.depth, w.fb, w.stages, w.stereo, w.mix), (0.5, 0.599_414_05, 4, 4_194_304, 0.5));
        assert_eq!(stages(0), 2);
        assert_eq!(stages(3), 8);
        assert_eq!(stages(-5), 2);
        assert_eq!(stages(9), 8);
        // The feedback is signed.
        let w = Working::new(&[0, 0, 0, -512, 0, 0, 0], 44100);
        assert_eq!(w.fb, -0.9);
        // Stored values past the ranges act as the ends.
        assert_eq!(position(DEPTH, 9000), 1.0);
        assert_eq!(bipolar(FEEDBACK, -1024), -2.0);
        assert_eq!(center_hz(0.0), 8.175_799);
        assert_eq!(center_hz(1.0), 12_543.854);
    }

    /// Silence in, silence out; a reset empties the stages.
    #[test]
    fn silence_stays_silence_and_a_reset_empties_the_stages() {
        let mut p = Phaser::with_settings(44100, &Settings { v: [5000, 1024, 5000, 512, 3, 1024, 1024] });
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        p.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
        // A loud run leaves a ringing tail; a reset silences it.
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..4096).map(|i| { let x = ((i as f32) * 0.11).sin(); (x, -x) }).unzip();
        p.process(&mut l, &mut r);
        p.start();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        p.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }

    /// With no feedback, the wet part is two all-pass stages of the input: a DC input passes at full level (an
    /// all-pass's gain is one at DC), and a delta's first sample comes back at the squared coefficient.
    #[test]
    fn the_wet_part_is_the_all_passed_input() {
        let mut p = Phaser::with_settings(44100, &Settings { v: [0, 0, 2500, 0, 0, 0, 1024] });
        // Rate 0 holds the LFO at phase 0, so the stages sit at the centre, 320.2437 Hz.
        let f = center_hz(position(CENTER, 2500)) as f64;
        let t = (PI * f / 44100.0).tan();
        let g = ((t - 1.0) / (t + 1.0)) as f32;
        // A DC input of 0.25 comes back near 0.25 (an all-pass's gain is one at DC; the two stages share their
        // coefficient, whose double pole settles slowly, so the run is long).
        let (mut l, mut r) = (vec![0.25f32; 2048], vec![0.0f32; 2048]);
        p.process(&mut l, &mut r);
        assert!((l[2047] - 0.25).abs() < 0.001, "{}", l[2047]);
        // A delta comes back at g² first.
        let mut p = Phaser::with_settings(44100, &Settings { v: [0, 0, 2500, 0, 0, 0, 1024] });
        let (mut l, mut r) = (vec![0.0f32; 8], vec![0.0f32; 8]);
        l[0] = 1.0;
        p.process(&mut l, &mut r);
        assert_eq!(l[0], g * g);
    }

    /// The effect stays stable and finite with every knob at its end, at both feedback polarities.
    #[test]
    fn the_extremes_stay_finite_and_bounded() {
        for fb in [512, -512] {
            let mut p = Phaser::with_settings(44100, &Settings { v: [5000, 1024, 5000, fb, 3, 1024, 1024] });
            let mut seed = 0x1234_5678u32;
            let mut next = move || {
                seed = seed.wrapping_mul(1_035_152_45).wrapping_add(12_345);
                (seed >> 8) as f32 / 16_777_216.0 * 2.0 - 1.0
            };
            let mut worst = 0.0f32;
            for _ in 0..100 {
                let (mut l, mut r) = (Vec::with_capacity(512), Vec::with_capacity(512));
                for _ in 0..512 {
                    l.push(next());
                    r.push(next());
                }
                p.process(&mut l, &mut r);
                worst = worst.max(l.iter().chain(&r).map(|x| x.abs()).fold(0.0, f32::max));
            }
            assert!(worst.is_finite() && worst < 64.0, "fb {fb}: {worst}");
        }
    }

    /// A change of the stages' count empties the stages; other changes keep them.
    #[test]
    fn a_change_of_the_stages_empties_them() {
        let mut p = Phaser::with_settings(44100, &Settings::default());
        let (mut l, mut r) = (vec![0.5f32; 16], vec![0.5f32; 16]);
        p.process(&mut l, &mut r);
        assert!(p.before[0][0] != 0.0);
        // The same count: the stages keep their history.
        p.set(&Settings { v: [0, 0, 0, 0, 1, 0, 0] });
        assert!(p.before[0][0] != 0.0);
        // A different count: they are emptied.
        p.set(&Settings { v: [0, 0, 0, 0, 3, 0, 0] });
        assert!(p.before.iter().all(|b| b.iter().all(|&x| x == 0.0)));
        assert!(p.after.iter().all(|a| a.iter().all(|&x| x == 0.0)));
    }

    /// The engine's slot form plays the same samples as the sides form.
    #[test]
    fn the_slot_form_matches_the_sides_form() {
        let mut p = Phaser::with_settings(44100, &Settings::default());
        let mut e = Phaser::with_settings(44100, &Settings::default());
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..1000).map(|i| { let x = ((i as f32) * 0.031).sin() * 0.5; (x, -x) }).unzip();
        let (l0, r0) = (l.clone(), r.clone());
        p.process(&mut l, &mut r);
        let mut buf: Vec<f32> = l0.iter().zip(&r0).flat_map(|(a, b)| [*a, *b]).collect();
        Effect::process(&mut e, &mut buf);
        let mine: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(buf, mine);
    }

    #[test]
    fn small_states_are_cleared_at_block_ends() {
        let mut p = Phaser::with_settings(44100, &Settings::default());
        p.before[0][0] = 5e-12;
        p.after[1][3] = -2e-11;
        p.last[1] = 5e-12;
        p.end_block();
        assert_eq!(p.before[0][0], 0.0);
        assert_eq!(p.after[1][3], -2e-11);
        assert_eq!(p.last[1], 0.0);
        assert_eq!(p.last[0], 0.0);
    }
}
