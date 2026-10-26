// Copyright (C) 2026 apikun
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's flanger, Lemon Studio's own design (no original's saved state to read). Each side has a short
//! delay line whose write point also feeds the input; a slow sine LFO moves the read towards the write point and
//! back, and the read is fed back into the line and mixed with the input. The right side's LFO runs a settable share
//! of a cycle ahead, so a stereo signal flanges out of step. Renders repeat: the LFOs always start together.
//!
//! Settings are plain knob values (0 to the range's end, the feedback bipolar), not a project file's saved state:
//! the plugin stores them through its parameters (spec: plugins/lemon-small-effects.md §5).

use std::f64::consts::TAU;

use super::Effect;

/// The settings, in the knobs' order.
pub const PARAMS: usize = 6;
pub const RATE: usize = 0;
pub const DEPTH: usize = 1;
pub const DELAY: usize = 2;
pub const FEEDBACK: usize = 3;
pub const STEREO: usize = 4;
pub const MIX: usize = 5;

/// The knobs' ranges. The feedback is bipolar (its centre is a magnet on the knob).
pub const RANGES: [(i32, i32); PARAMS] = [(0, 5000), (0, 1024), (0, 5000), (-512, 512), (0, 1024), (0, 1024)];

/// What a new instance holds: a 0.45 Hz LFO, 70 % depth, a 1 ms delay, 45 % feedback, the right side a quarter cycle
/// behind, an even mix.
pub const DEFAULTS: [i32; PARAMS] = [1500, 717, 1000, 256, 512, 512];

/// The saved state's version (the plugin's own; a later one is not read, spec: plugins/lemon-small-effects.md §5).
pub const VERSION: i32 = 1;

/// The delay line per side, in samples: 5 ms at 384 kHz, the most the delay knob asks for anywhere. The base delay is
/// held to `LINE - 2` at higher rates.
pub const LINE: usize = 2048;
const MASK: usize = LINE - 1;

/// The shortest distance a read keeps from the write point, in samples: the sweep reaches towards it, never onto it,
/// so a read never lands on the sample being written.
pub const MIN_DISTANCE: f32 = 1.0;

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

/// The base delay in samples: `x` of 5 ms, held to the line's size at rates it does not fit.
pub fn base_samples(x: f32, rate: u32) -> f32 {
    (f64::from(x) * 5.0 * f64::from(rate) / 1000.0).min((LINE - 2) as f64) as f32
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
    pub base: f32,
    pub fb: f32,
    pub stereo: u32,
    pub mix: f32,
}

impl Working {
    pub fn new(v: &[i32; PARAMS], rate: u32) -> Working {
        Working {
            step: step(position(RATE, v[RATE]), rate),
            depth: position(DEPTH, v[DEPTH]),
            base: base_samples(position(DELAY, v[DELAY]), rate),
            fb: feedback(bipolar(FEEDBACK, v[FEEDBACK])),
            stereo: stereo_offset(position(STEREO, v[STEREO])),
            mix: position(MIX, v[MIX]),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The effect

pub struct Flanger {
    applied: [i32; PARAMS],
    rate: u32,
    w: Working,
    /// 2^24 per cycle; the right side runs `w.stereo` ahead.
    phase: u32,
    line: [Box<[f32; LINE]>; 2],
    at: usize,
}

impl Flanger {
    /// A new instance started as at a render: the settings in force, the lines empty, the LFOs together.
    pub fn with_settings(rate: u32, s: &Settings) -> Flanger {
        Flanger {
            applied: s.v,
            rate,
            w: Working::new(&s.v, rate),
            phase: 0,
            line: [Box::new([0.0; LINE]), Box::new([0.0; LINE])],
            at: 0,
        }
    }

    /// The host enables the effect: the lines are emptied and the LFOs start together again.
    pub fn start(&mut self) {
        for l in &mut self.line {
            l.fill(0.0);
        }
        self.phase = 0;
        self.at = 0;
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: they act from the next sample (nothing is smoothed).
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            self.applied = s.v;
            self.w = Working::new(&s.v, self.rate);
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

    /// One read of the line `distance` samples behind the write point: a straight line between two samples.
    #[inline]
    fn read(line: &[f32; LINE], distance: f32, at: usize) -> f32 {
        let p = (at as f32 - distance).rem_euclid(LINE as f32);
        let j = p as usize;
        let f = p - j as f32;
        let a = line[j];
        a + (line[(j + 1) & MASK] - a) * f
    }

    /// One sample of both sides. The read comes before the write, so the feedback is one sample round the loop.
    #[inline]
    fn sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        let w = self.w;
        let x = [l, r];
        let angle = TAU * f64::from(self.phase) / 16_777_216.0;
        let angle_r = TAU * f64::from(self.phase.wrapping_add(w.stereo)) / 16_777_216.0;
        self.phase = self.phase.wrapping_add(w.step);
        let lfo = [(angle.sin() + 1.0) as f32 * 0.5, (angle_r.sin() + 1.0) as f32 * 0.5];
        let mut out = [0.0f32; 2];
        for s in 0..2 {
            let distance = (w.base * (1.0 - w.depth * (1.0 - lfo[s]))).max(MIN_DISTANCE);
            let wet = Flanger::read(&self.line[s], distance, self.at);
            let written = x[s] + w.fb * wet;
            self.line[s][self.at] = if written.abs() < 1e-20 { 0.0 } else { written };
            out[s] = x[s] * (1.0 - w.mix) + wet * w.mix;
        }
        self.at = (self.at + 1) & MASK;
        (out[0], out[1])
    }

    /// One block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.sample(*l, *r);
        }
    }
}

impl Effect for Flanger {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            (f[0], f[1]) = self.sample(f[0], f[1]);
        }
    }

    /// A slot switched on during the song: the lines empty and the LFOs start together again.
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
        // The speed matches the chorus's default LFO 1 step (0.45 Hz).
        assert_eq!(w.step, 171);
        assert!((speed_hz(0.3) - 0.45).abs() < 1e-6, "{}", speed_hz(0.3));
        assert_eq!((w.depth, w.fb, w.stereo, w.mix), (0.700_195_3, 0.45, 4_194_304, 0.5));
        // The knob's position went through a single (0.2), so the base sits a whisker off 44.1.
        assert!((w.base - 44.1).abs() < 1e-4, "{}", w.base);
        assert_eq!(base_samples(1.0, 44100), 220.5);
        // Every knob at its end: 5 ms, the feedback at 0.9, the right side half a cycle ahead.
        let w = Working::new(&[5000, 1024, 5000, 512, 1024, 1024], 44100);
        assert_eq!((w.depth, w.base, w.fb, w.stereo, w.mix), (1.0, 220.5, 0.9, 8_388_608, 1.0));
        // The feedback is signed.
        let w = Working::new(&[0, 0, 0, -512, 0, 0], 44100);
        assert_eq!(w.fb, -0.9);
        // The base delay is held to the line's size at extreme rates.
        assert_eq!(base_samples(1.0, 1_536_000), (LINE - 2) as f32);
        // Stored values past the ranges act as the ends.
        assert_eq!(position(DEPTH, 9000), 1.0);
        assert_eq!(position(DELAY, -5), 0.0);
        assert_eq!(bipolar(FEEDBACK, -1024), -2.0);
    }

    /// Silence in, silence out; a reset empties the lines.
    #[test]
    fn silence_stays_silence_and_a_reset_empties_the_lines() {
        let mut f = Flanger::with_settings(44100, &Settings { v: [5000, 1024, 5000, 512, 1024, 1024] });
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        f.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
        // A loud run leaves a ringing tail; a reset silences it.
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..4096).map(|i| { let x = ((i as f32) * 0.11).sin(); (x, -x) }).unzip();
        f.process(&mut l, &mut r);
        f.start();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        f.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }

    /// With no depth and no feedback, the wet part is the input read `base` samples late: a delta comes back
    /// 0.9 / 0.1 split over the two samples the straight-line read spans.
    #[test]
    fn the_wet_part_is_the_delayed_input() {
        let mut f = Flanger::with_settings(44100, &Settings { v: [0, 0, 1000, 0, 0, 1024] });
        let (mut l, mut r) = (vec![0.0f32; 48], vec![0.0f32; 48]);
        l[0] = 1.0;
        f.process(&mut l, &mut r);
        // The base delay is 1 ms = 44.1 samples; the read sits 0.9 into the span (f32 rounding in the position).
        assert!((l[44] - 0.9).abs() < 0.001 && (l[45] - 0.1).abs() < 0.001, "{} {}", l[44], l[45]);
        assert!(r.iter().all(|&x| x == 0.0));
        // The right side, its LFO offset apart, plays the same delay when the depth is 0.
        let (mut l, mut r) = (vec![0.0f32; 48], vec![0.0f32; 48]);
        r[0] = 1.0;
        f.process(&mut l, &mut r);
        assert!((r[44] - 0.9).abs() < 0.001 && (r[45] - 0.1).abs() < 0.001, "{} {}", r[44], r[45]);
    }

    /// The effect stays stable and finite with every knob at its end, at both feedback polarities.
    #[test]
    fn the_extremes_stay_finite_and_bounded() {
        for fb in [512, -512] {
            let mut f = Flanger::with_settings(44100, &Settings { v: [5000, 1024, 5000, fb, 1024, 1024] });
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
                f.process(&mut l, &mut r);
                worst = worst.max(l.iter().chain(&r).map(|x| x.abs()).fold(0.0, f32::max));
            }
            assert!(worst.is_finite() && worst < 16.0, "fb {fb}: {worst}");
        }
    }

    /// A settings change acts from the next block: the mix crosses over at once.
    #[test]
    fn a_settings_change_acts_at_once() {
        let mut f = Flanger::with_settings(44100, &Settings::default());
        let (mut l, mut r) = (vec![0.5f32; 16], vec![0.25f32; 16]);
        f.process(&mut l, &mut r);
        f.set(&Settings { v: [0, 0, 0, 0, 0, 0] }); // mix 0: the input passes through
        assert_eq!(f.working().mix, 0.0);
        let (mut l, mut r) = (vec![0.5f32; 4], vec![0.25f32; 4]);
        f.process(&mut l, &mut r);
        assert_eq!(l[3], 0.5);
        assert_eq!(f.settings(), Settings { v: [0, 0, 0, 0, 0, 0] });
    }

    /// The engine's slot form plays the same samples as the sides form.
    #[test]
    fn the_slot_form_matches_the_sides_form() {
        let mut f = Flanger::with_settings(44100, &Settings { v: [1500, 717, 1000, 256, 512, 512] });
        let mut e = Flanger::with_settings(44100, &Settings { v: [1500, 717, 1000, 256, 512, 512] });
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..1000).map(|i| { let x = ((i as f32) * 0.031).sin() * 0.5; (x, -x) }).unzip();
        let (l0, r0) = (l.clone(), r.clone());
        f.process(&mut l, &mut r);
        let mut buf: Vec<f32> = l0.iter().zip(&r0).flat_map(|(a, b)| [*a, *b]).collect();
        Effect::process(&mut e, &mut buf);
        let mine: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(buf, mine);
    }
}
