// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The player's built-in reverb and chorus (the BUILT-IN effects the LEVEL knobs send to): a small plate (four combs
//! and two all-passes a side) and a two-line chorus, both in their own sums so the sends stay separate from the dry
//! sound.
// spec: plugins/soundfont-player.md

/// A small stereo plate reverb: Freeverb's combs and all-passes, the sides detuned.
pub struct Reverb {
    combs: [[Comb; 4]; 2],
    passes: [[AllPass; 2]; 2],
    wet: f64,
}

#[derive(Clone)]
struct Comb {
    buf: Vec<f64>,
    at: usize,
    damp: f64,
    store: f64,
}

impl Comb {
    fn new(size: usize) -> Self {
        Comb { buf: vec![0.0; size.max(4)], at: 0, damp: 0.2, store: 0.0 }
    }

    fn next(&mut self, x: f64) -> f64 {
        let y = self.buf[self.at];
        self.store = y * (1.0 - self.damp) + self.store * self.damp;
        self.buf[self.at] = x + self.store * 0.84;
        self.at += 1;
        if self.at >= self.buf.len() {
            self.at = 0;
        }
        y
    }
}

#[derive(Clone)]
struct AllPass {
    buf: Vec<f64>,
    at: usize,
}

impl AllPass {
    fn new(size: usize) -> Self {
        AllPass { buf: vec![0.0; size.max(4)], at: 0 }
    }

    fn next(&mut self, x: f64) -> f64 {
        let b = self.buf[self.at];
        let y = x + b * 0.5;
        self.buf[self.at] = y;
        self.at += 1;
        if self.at >= self.buf.len() {
            self.at = 0;
        }
        b - y * 0.5
    }
}

impl Reverb {
    pub fn new(rate: u32) -> Self {
        // Freeverb's tunings scaled to the rate.
        let k = f64::from(rate) / 44100.0;
        let scale = |n: usize| ((n as f64 * k).round() as usize).max(4);
        let combs_l = [1116, 1188, 1277, 1356];
        let passes_l = [556, 441];
        let (combs_r, passes_r) = ([1141, 1213, 1302, 1381], [579, 464]);
        let mk = |ns: [usize; 4]| ns.map(|n| Comb::new(scale(n)));
        let mkp = |ns: [usize; 2]| ns.map(|n| AllPass::new(scale(n)));
        Reverb { combs: [mk(combs_l), mk(combs_r)], passes: [mkp(passes_l), mkp(passes_r)], wet: 0.0 }
    }

    /// The send level 0..1.
    pub fn set_level(&mut self, v: f64) {
        self.wet = v.clamp(0.0, 1.0);
    }

    /// Adds the wet sound of the mono send to both sides.
    pub fn process(&mut self, send: &[f32], left: &mut [f32], right: &mut [f32]) {
        if self.wet <= 1e-4 {
            return;
        }
        let n = send.len().min(left.len()).min(right.len());
        for i in 0..n {
            let x = f64::from(send[i]);
            let mut l = 0.0;
            let mut r = 0.0;
            for k in 0..4 {
                l += self.combs[0][k].next(x);
                r += self.combs[1][k].next(x);
            }
            for k in 0..2 {
                l = self.passes[0][k].next(l);
                r = self.passes[1][k].next(r);
            }
            left[i] += (l * 0.018 * self.wet) as f32;
            right[i] += (r * 0.018 * self.wet) as f32;
        }
    }

    /// Empties the lines, as at the start of a song.
    pub fn reset(&mut self) {
        for side in &mut self.combs {
            for c in side {
                c.buf.fill(0.0);
                c.store = 0.0;
                c.at = 0;
            }
        }
        for side in &mut self.passes {
            for p in side {
                p.buf.fill(0.0);
                p.at = 0;
            }
        }
    }
}

/// A two-line stereo chorus: the sides' lines delayed a base of 12 ms and swept ±2.5 ms by LFOs a quarter turn apart.
pub struct Chorus {
    buf: [Vec<f32>; 2],
    at: usize,
    phase: f64,
    rate: f64,
    wet: f64,
}

impl Chorus {
    pub fn new(rate: u32) -> Self {
        // The line holds the base delay plus the sweep, with room for a rate change.
        let size = ((0.03 * f64::from(rate)).ceil() as usize).max(64);
        Chorus { buf: [vec![0.0; size], vec![0.0; size]], at: 0, phase: 0.0, rate: f64::from(rate), wet: 0.0 }
    }

    pub fn set_level(&mut self, v: f64) {
        self.wet = v.clamp(0.0, 1.0);
    }

    pub fn set_rate(&mut self, rate: u32) {
        let size = ((0.03 * f64::from(rate)).ceil() as usize).max(64);
        self.buf = [vec![0.0; size], vec![0.0; size]];
        self.at = 0;
        self.rate = f64::from(rate);
    }

    /// Adds the wet sound of the mono send to both sides.
    pub fn process(&mut self, send: &[f32], left: &mut [f32], right: &mut [f32]) {
        if self.wet <= 1e-4 {
            return;
        }
        let n = send.len().min(left.len()).min(right.len());
        let size = self.buf[0].len();
        for i in 0..n {
            let x = send[i];
            self.buf[0][self.at] = x;
            self.buf[1][self.at] = x;
            self.at = (self.at + 1) % size;
            // The read position: 12 ms plus 2.5 ms of sweep, the right side a quarter turn behind.
            let base = 0.012 * self.rate;
            let depth = 0.0025 * self.rate;
            let read = |side: usize, ph: f64| -> f64 {
                let d = base + depth * (2.0 * std::f64::consts::PI * ph).sin();
                let p = self.at as f64 - d;
                let p = if p < 0.0 { p + size as f64 } else { p };
                let j = p.floor();
                let f = p - j;
                let a = self.buf[side][j as usize % size] as f64;
                let b = self.buf[side][(j as usize + 1) % size] as f64;
                a + (b - a) * f
            };
            left[i] += (read(0, self.phase) * self.wet) as f32;
            right[i] += (read(1, self.phase + 0.25) * self.wet) as f32;
            self.phase += 0.8 / self.rate;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
        }
    }

    pub fn reset(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reverb_rings_and_fades() {
        let mut rv = Reverb::new(44100);
        rv.set_level(1.0);
        let rms = |x: &[f32], a: usize, b: usize| (x[a..b].iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / (b - a) as f64).sqrt();
        // A burst: the first samples are quiet (the lines fill), the burst's end rings.
        let send = vec![0.5f32; 4410];
        let (mut l, mut r) = (vec![0.0f32; 4410], vec![0.0f32; 4410]);
        rv.process(&send, &mut l, &mut r);
        assert!(rms(&l, 0, 1102) < rms(&l, 2205, 4410), "{} {}", rms(&l, 0, 1102), rms(&l, 2205, 4410));
        // The ring carries on after the input stops, and fades away over it.
        let (mut l2, mut r2) = (vec![0.0f32; 16_000], vec![0.0f32; 16_000]);
        let silent = vec![0.0f32; 16_000];
        rv.process(&silent, &mut l2, &mut r2);
        let (early, late) = (rms(&l2, 0, 2000), rms(&l2, 12_000, 16_000));
        assert!(early > 1e-5, "rings on: {early}");
        assert!(late < early, "fades: {late} vs {early}");
        let _ = r2;
        // Level 0 sends nothing at all.
        rv.set_level(0.0);
        let (mut l3, mut r3) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        rv.process(&send[..512], &mut l3, &mut r3);
        assert!(l3.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn the_chorus_sweeps_and_stays_stereo() {
        let mut ch = Chorus::new(44100);
        ch.set_level(0.5);
        let send: Vec<f32> = (0..8000).map(|i| ((i as f32) * 0.05).sin()).collect();
        let (mut l, mut r) = (vec![0.0f32; 8000], vec![0.0f32; 8000]);
        ch.process(&send, &mut l, &mut r);
        let same = l.iter().zip(&r).filter(|(a, b)| a == b).count();
        // The sides' LFOs differ, so most samples differ (the shared dry path is not counted here).
        assert!(same < 7500, "{same}");
    }
}
