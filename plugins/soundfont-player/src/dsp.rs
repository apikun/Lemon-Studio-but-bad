// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Soundfont Player's voices: the regions a preset plays, note on / off, and the per-block render. A voice reads one
//! sample region through the file's volume envelope, a two-pole low-pass driven by the modulation envelope and the LFO
//! (the player's Envelope 2 / LFO 2 / Misc controls), its pan and its loop. The reverb and chorus sends leave as their
//! own mono sums, so the effects hear the voices as they sound.
// spec: plugins/soundfont-player.md

use std::sync::Arc;

use crate::sf2::{self, gens, SampleHeader, SoundFont};

/// A timecent field to seconds (`-32768` and below mean none).
pub fn tc_seconds(tc: i32) -> f64 {
    if tc < -1000 {
        0.0
    } else {
        2f64.powf(f64::from(tc) / 1200.0)
    }
}

/// Cents to a frequency ratio.
pub fn cents(v: f64) -> f64 {
    2f64.powf(v / 1200.0)
}

/// The envelope the file's generators give, in its units (timecents; sustain in centibels for the volume envelope and
/// permilles for the modulation one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Env {
    pub delay: i32,
    pub attack: i32,
    pub hold: i32,
    pub decay: i32,
    pub sustain: i32,
    pub release: i32,
    pub key_to_hold: i32,
    pub key_to_decay: i32,
}

impl Env {
    const fn with(sustain: i32) -> Self {
        Env { delay: -32768, attack: -32768, hold: -32768, decay: 1200, sustain, release: 1200, key_to_hold: 0, key_to_decay: 0 }
    }

    /// The SoundFont defaults: the volume envelope's sustain is no attenuation at all.
    pub fn volume() -> Self {
        Env::with(0)
    }

    /// The modulation envelope's default holds its decay's end (a level of 1000 permilles).
    pub fn modulation() -> Self {
        Env::with(1000)
    }
}

/// An LFO's generators (the vibrato LFO shares the shape with its own numbers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoGen {
    pub delay: i32,
    /// Timecents (2^(tc/1200) Hz).
    pub freq: i32,
    pub to_pitch: i32,
    pub to_filter: i32,
    /// Centibels of tremolo at the wave's peak.
    pub to_volume: i32,
}

/// A voice's resolved zone: the instrument zone's generators with the preset zone's on top, the sample linked.
#[derive(Clone, Debug)]
pub struct Region {
    pub sample: usize,
    pub start: usize,
    pub end: usize,
    pub loop_start: usize,
    pub loop_end: usize,
    /// 0 no loop, 1 loop till release, 3 loop always.
    pub loop_mode: i32,
    pub rate: u32,
    pub root_key: i32,
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    pub coarse_tune: i32,
    pub fine_tune: i32,
    pub scale_tuning: i32,
    /// -1..1 (the file's -500..500).
    pub pan: f64,
    pub attenuation: f64,
    pub filter_fc: f64,
    /// Resonance in dB (the file's quarter-dB units / 10).
    pub filter_q: f64,
    /// 0..1; a zone that sets no send plays the player's level in full.
    pub reverb_send: f64,
    pub chorus_send: f64,
    pub exclusive: i32,
    pub vol_env: Env,
    pub mod_env: Env,
    pub mod_env_to_pitch: i32,
    pub mod_env_to_filter: i32,
    pub mod_lfo: LfoGen,
    pub vib_lfo: LfoGen,
}

impl Region {
    /// One voice's region from an instrument zone under a preset zone: the instrument zone's generators, overridden by
    /// the preset zone's (the ranges are both applied; the links only the zone that owns them sets).
    fn build(font: &SoundFont, preset: &sf2::Zone, instrument: &sf2::Zone) -> Option<Region> {
        let sample = instrument.field(gens::SAMPLE_ID).or_else(|| preset.field(gens::SAMPLE_ID))?;
        let sh: &SampleHeader = font.samples.get(sample as usize)?;
        if sh.start >= sh.end || sh.end > font.data.len() {
            return None;
        }
        let value = |op: u16| instrument.field(op).or_else(|| preset.field(op));

        // Key and velocity ranges: the instrument zone's, narrowed by the preset zone's.
        let (key_lo, key_hi) = match (instrument.key_range(), preset.key_range()) {
            (Some((a, b)), Some((c, d))) => (a.max(c), b.min(d)),
            (r, None) | (None, r) => r.unwrap_or((0, 127)),
        };
        let (vel_lo, vel_hi) = match (instrument.vel_range(), preset.vel_range()) {
            (Some((a, b)), Some((c, d))) => (a.max(c), b.min(d)),
            (r, None) | (None, r) => r.unwrap_or((0, 127)),
        };
        if key_lo > key_hi || vel_lo > vel_hi {
            return None;
        }

        // Sample bounds with the offset generators (fine + coarse), inside the file's pool.
        let start = (sh.start as i64 + i64::from(value(gens::START_ADDRS_COARSE).unwrap_or(0)) * 32768
            + i64::from(value(gens::START_ADDRS_OFFSET).unwrap_or(0)))
        .clamp(0, font.data.len() as i64) as usize;
        let end = (sh.end as i64 + i64::from(value(gens::END_ADDRS_COARSE).unwrap_or(0)) * 32768
            + i64::from(value(gens::END_ADDRS_OFFSET).unwrap_or(0)))
        .clamp(start as i64 + 1, font.data.len() as i64) as usize;
        let loop_start = (sh.loop_start as i64 + i64::from(value(gens::STARTLOOP_ADDRS_COARSE).unwrap_or(0)) * 32768
            + i64::from(value(gens::STARTLOOP_ADDRS_OFFSET).unwrap_or(0)))
        .clamp(start as i64, end as i64 - 1) as usize;
        let loop_end = (sh.loop_end as i64 + i64::from(value(gens::ENDLOOP_ADDRS_COARSE).unwrap_or(0)) * 32768
            + i64::from(value(gens::ENDLOOP_ADDRS_OFFSET).unwrap_or(0)))
        .clamp(loop_start as i64 + 1, end as i64) as usize;

        let root_key = value(gens::OVERRIDING_ROOT_KEY).filter(|k| (0..=127).contains(k)).map(|k| k as i32).unwrap_or(i32::from(sh.pitch));
        // A stereo pair's side hard-pans when the zone carries no pan of its own (left sample 1, right 2).
        let kind_pan = match sh.kind {
            1 => -1.0,
            2 => 1.0,
            _ => 0.0,
        };
        let pan = match instrument.field(gens::PAN).or_else(|| preset.field(gens::PAN)) {
            Some(v) => (f64::from(v) / 500.0).clamp(-1.0, 1.0),
            None => kind_pan,
        };

        let env = |d: u16, a: u16, h: u16, dec: u16, s: u16, rel: u16, kh: u16, kd: u16| Env {
            delay: value(d).unwrap_or(-32768),
            attack: value(a).unwrap_or(-32768),
            hold: value(h).unwrap_or(-32768),
            decay: value(dec).unwrap_or(1200),
            sustain: value(s).unwrap_or(0),
            release: value(rel).unwrap_or(1200),
            key_to_hold: value(kh).unwrap_or(0),
            key_to_decay: value(kd).unwrap_or(0),
        };
        let lfo = |dl: u16, fq: u16, p: u16, fl: u16, v: u16| LfoGen {
            delay: value(dl).unwrap_or(-32768),
            freq: value(fq).unwrap_or(0),
            to_pitch: value(p).unwrap_or(0),
            to_filter: if fl == u16::MAX { 0 } else { value(fl).unwrap_or(0) },
            to_volume: if v == u16::MAX { 0 } else { value(v).unwrap_or(0) },
        };
        Some(Region {
            sample: sample as usize,
            start,
            end,
            loop_start,
            loop_end,
            loop_mode: value(gens::SAMPLE_MODES).unwrap_or(0) & 3,
            rate: sh.rate.max(400),
            root_key,
            key_lo,
            key_hi,
            vel_lo,
            vel_hi,
            coarse_tune: value(gens::COARSE_TUNE).unwrap_or(0),
            fine_tune: value(gens::FINE_TUNE).unwrap_or(0),
            scale_tuning: value(gens::SCALE_TUNING).unwrap_or(100),
            pan,
            attenuation: 10f64.powf(-f64::from(value(gens::INITIAL_ATTENUATION).unwrap_or(0)) / 200.0),
            filter_fc: f64::from(value(gens::INITIAL_FILTER_FC).unwrap_or(13500)).clamp(1500.0, 20000.0),
            filter_q: (f64::from(value(gens::INITIAL_FILTER_Q).unwrap_or(0)) / 10.0).clamp(0.0, 96.0),
            reverb_send: f64::from(value(gens::REVERB_SEND).unwrap_or(0)) / 1000.0,
            chorus_send: f64::from(value(gens::CHORUS_SEND).unwrap_or(0)) / 1000.0,
            exclusive: value(gens::EXCLUSIVE_CLASS).unwrap_or(0),
            vol_env: env(gens::DELAY_VOL_ENV, gens::ATTACK_VOL_ENV, gens::HOLD_VOL_ENV, gens::DECAY_VOL_ENV, gens::SUSTAIN_VOL_ENV, gens::RELEASE_VOL_ENV, gens::KEY_TO_VOL_ENV_HOLD, gens::KEY_TO_VOL_ENV_DECAY),
            mod_env: env(gens::DELAY_MOD_ENV, gens::ATTACK_MOD_ENV, gens::HOLD_MOD_ENV, gens::DECAY_MOD_ENV, gens::SUSTAIN_MOD_ENV, gens::RELEASE_MOD_ENV, gens::KEY_TO_MOD_ENV_HOLD, gens::KEY_TO_MOD_ENV_DECAY),
            mod_env_to_pitch: value(gens::MOD_ENV_TO_PITCH).unwrap_or(0),
            mod_env_to_filter: value(gens::MOD_ENV_TO_FILTER_FC).unwrap_or(0),
            mod_lfo: lfo(gens::DELAY_MOD_LFO, gens::FREQ_MOD_LFO, gens::MOD_LFO_TO_PITCH, gens::MOD_LFO_TO_FILTER_FC, gens::MOD_LFO_TO_VOLUME),
            vib_lfo: lfo(gens::DELAY_VIB_LFO, gens::FREQ_VIB_LFO, gens::VIB_LFO_TO_PITCH, u16::MAX, u16::MAX),
        })
    }
}

/// The regions a preset plays, flattened over its instruments.
pub fn regions(font: &SoundFont, preset: usize) -> Vec<Region> {
    let mut out = Vec::new();
    let Some(p) = font.presets.get(preset) else { return out };
    for pz in &p.zones {
        let Some(inst) = pz.field(gens::INSTRUMENT) else { continue };
        let Some(i) = font.instruments.get(inst as usize) else { continue };
        for iz in &i.zones {
            if let Some(r) = Region::build(font, pz, iz) {
                out.push(r);
            }
        }
    }
    out
}

/// The player's controls for one block (from the parameters).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    /// Envelope 2 (the filter envelope's A / D / S / R), 0..1 each.
    pub env_a: f64,
    pub env_d: f64,
    pub env_s: f64,
    pub env_r: f64,
    /// LFO 2: delay seconds, depth in cents (to the pitch), speed Hz.
    pub lfo_delay: f64,
    pub lfo_amt: f64,
    pub lfo_hz: f64,
    /// Misc: the filter's cutoff (Hz) and the envelope's depth in cents.
    pub cut: f64,
    pub mod_depth: f64,
    /// Pitch bend in cents.
    pub bend: f64,
}

impl Default for Block {
    fn default() -> Self {
        Block { env_a: 0.0, env_d: 0.5, env_s: 0.0, env_r: 0.3, lfo_delay: 0.0, lfo_amt: 0.0, lfo_hz: 5.0, cut: 13500.0, mod_depth: 3600.0, bend: 0.0 }
    }
}

/// Envelope stage times in seconds, with the key scaling applied.
#[derive(Clone, Copy, Debug)]
struct Stages {
    delay: f64,
    attack: f64,
    hold: f64,
    decay: f64,
    /// 0..1 sustain level (the volume envelope's: 1 - the file's attenuation share).
    sustain: f64,
    release: f64,
}

impl Stages {
    /// From the file's envelope: `volume` picks the sustain's meaning (centibels of attenuation vs permilles).
    fn of(e: &Env, key: i32, volume: bool) -> Stages {
        let over = f64::from(key - 60);
        let hold_shift = (f64::from(e.key_to_hold) * over / 1200.0).round() as i32;
        let decay_shift = (f64::from(e.key_to_decay) * over / 1200.0).round() as i32;
        Stages {
            delay: tc_seconds(e.delay),
            attack: tc_seconds(e.attack),
            hold: tc_seconds(e.hold + hold_shift),
            decay: tc_seconds(e.decay + decay_shift),
            sustain: if volume { 1.0 - f64::from(e.sustain.clamp(0, 1440)) / 1440.0 } else { f64::from(e.sustain.clamp(0, 1000)) / 1000.0 },
            release: tc_seconds(e.release),
        }
    }

    /// The envelope's value `t` seconds in, 0..1 (a soft rise, an even fall to the sustain).
    fn at(&self, t: f64) -> f64 {
        let mut t = t;
        if t < self.delay {
            return 0.0;
        }
        t -= self.delay;
        if self.attack > 0.0 && t < self.attack {
            return (t / self.attack).powf(1.7);
        }
        let mut t = (t - self.attack).max(0.0);
        if self.hold > 0.0 && t < self.hold {
            return 1.0;
        }
        t -= self.hold;
        let sus = self.sustain.clamp(0.0, 1.0);
        if self.decay > 0.0 && t < self.decay {
            return (1.0 + (sus - 1.0) * (t / self.decay)).clamp(0.0, 1.0);
        }
        sus
    }
}

/// A two-pole low-pass (Chamberlin state variable), per side.
#[derive(Clone, Copy, Default)]
struct Filt {
    low: f64,
    band: f64,
    f: f64,
    damp: f64,
}

impl Filt {
    fn set(&mut self, hz: f64, q_db: f64, rate: f64) {
        let hz = hz.clamp(10.0, rate * 0.45);
        self.f = (2.0 * std::f64::consts::PI * hz / rate).sin().min(0.95);
        let gain = 10f64.powf(q_db.clamp(0.0, 96.0) / 20.0);
        self.damp = (1.0 / gain).clamp(0.05, 1.0);
    }

    fn next(&mut self, x: f64) -> f64 {
        let high = x - self.low - self.damp * self.band;
        self.band += self.f * high;
        self.low += self.f * self.band;
        self.low
    }
}

/// A sounding voice.
struct Voice {
    region: Region,
    note: i32,
    velocity: f64,
    /// Frames into the sample pool.
    pos: f64,
    releasing: bool,
    /// Seconds since it started and since the release began.
    age: f64,
    rel_age: f64,
    /// The envelope levels the release falls from.
    rel_vol: f64,
    rel_mod: f64,
    vol: Stages,
    vol_level: f64,
    mod_level: f64,
    filter: [Filt; 2],
    /// Gone: the release ran out or the sample ended.
    dead: bool,
}

/// The pitch of `note` on a region, as the sample's own playback rate (before the rate ratio).
fn pitch_of(r: &Region, note: i32, bend: f64) -> f64 {
    let semis = f64::from(note - r.root_key) * f64::from(r.scale_tuning) / 100.0
        + f64::from(r.coarse_tune)
        + f64::from(r.fine_tune) / 100.0
        + bend / 100.0;
    f64::from(r.rate) * 2f64.powf(semis / 12.0)
}

/// The voices of one preset of one loaded soundfont.
pub struct Engine {
    rate: f64,
    font: Option<Arc<SoundFont>>,
    preset: usize,
    regions: Vec<Region>,
    voices: Vec<Voice>,
    sounding: [u64; 3],
}

/// The most voices at once (a stereo pair is two).
const MAX_VOICES: usize = 64;
/// The most zones one note starts (a stereo pair is two; layered drums stay under it).
const MAX_ZONES_PER_NOTE: usize = 8;

impl Engine {
    pub fn new(rate: f64) -> Self {
        Engine { rate, font: None, preset: 0, regions: Vec::new(), voices: Vec::new(), sounding: [0; 3] }
    }

    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate.max(8000.0);
    }

    /// Plays a loaded soundfont's preset: the zones are flattened now, so a note starts without file work.
    pub fn set_preset(&mut self, font: &Arc<SoundFont>, preset: usize) {
        if self.font.as_ref().is_some_and(|f| Arc::ptr_eq(f, font)) && self.preset == preset {
            return;
        }
        self.font = Some(font.clone());
        self.preset = preset;
        self.regions = regions(font, preset);
        self.voices.clear();
        self.sounding = [0; 3];
    }

    pub fn preset(&self) -> usize {
        self.preset
    }

    pub fn has_font(&self) -> bool {
        self.font.is_some()
    }

    /// The keys sounding (bit per key, 192 keys).
    pub fn sounding(&self) -> [u64; 3] {
        self.sounding
    }

    fn publish(&mut self) {
        let mut bits = [0u64; 3];
        for v in &self.voices {
            let k = usize::try_from(v.note).unwrap_or(0).clamp(0, 191);
            bits[k / 64] |= 1 << (k % 64);
        }
        self.sounding = bits;
    }

    pub fn reset(&mut self) {
        self.voices.clear();
        self.publish();
    }

    pub fn all_off(&mut self) {
        let mut any = false;
        for v in &mut self.voices {
            if !v.releasing {
                v.releasing = true;
                v.rel_age = 0.0;
                v.rel_vol = v.vol_level;
                v.rel_mod = v.mod_level;
                any = true;
            }
        }
        if any {
            self.publish();
        }
    }

    /// Starts every matching region of the preset (up to [`MAX_ZONES_PER_NOTE`]).
    pub fn note_on(&mut self, note: i32, velocity: f64, blk: &Block) {
        let note = note.clamp(0, 127);
        let vel = velocity.clamp(0.005, 1.0);
        let vel_byte = (vel * 127.0).round() as u8;
        let mut started = 0;
        for r in &self.regions {
            if started >= MAX_ZONES_PER_NOTE {
                break;
            }
            if note < i32::from(r.key_lo) || note > i32::from(r.key_hi) || vel_byte < r.vel_lo || vel_byte > r.vel_hi {
                continue;
            }
            started += 1;
            // A same exclusive-class voice chokes with a fast release.
            if r.exclusive > 0 {
                for v in &mut self.voices {
                    if v.region.exclusive == r.exclusive && !v.releasing {
                        v.releasing = true;
                        v.rel_age = 0.0;
                        v.vol.release = 0.01;
                        v.rel_vol = v.vol_level;
                        v.rel_mod = v.mod_level;
                    }
                }
            }
            if self.voices.len() >= MAX_VOICES {
                // Steal the quietest releasing voice, else the quietest.
                let key = |v: &Voice| if v.releasing { v.vol_level } else { v.vol_level + 2.0 };
                let victim = self.voices.iter().enumerate().min_by(|(_, a), (_, b)| key(a).total_cmp(&key(b))).map(|(i, _)| i).unwrap_or(0);
                self.voices.remove(victim);
            }
            let mut filter = [Filt::default(), Filt::default()];
            let hz = cutoff(&r, blk, 0.0, 0.0);
            for f in &mut filter {
                f.set(hz, r.filter_q, self.rate);
            }
            self.voices.push(Voice {
                region: r.clone(),
                note,
                velocity: vel,
                pos: r.start as f64,
                releasing: false,
                age: 0.0,
                rel_age: 0.0,
                rel_vol: 0.0,
                rel_mod: 0.0,
                vol: Stages::of(&r.vol_env, note, true),
                vol_level: 0.0,
                mod_level: 0.0,
                filter,
                dead: false,
            });
        }
        if started > 0 {
            self.publish();
        }
    }

    pub fn note_off(&mut self, note: i32) {
        let mut any = false;
        for v in &mut self.voices {
            if v.note == note && !v.releasing {
                v.releasing = true;
                v.rel_age = 0.0;
                v.rel_vol = v.vol_level;
                v.rel_mod = v.mod_level;
                any = true;
            }
        }
        if any {
            self.publish();
        }
    }

    /// Renders both sides, plus the mono reverb and chorus sends (the voices as they sound). `blk` holds the player's
    /// controls for the whole slice.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32], rev: &mut [f32], cho: &mut [f32], blk: &Block) {
        let n = left.len().min(right.len()).min(rev.len()).min(cho.len());
        left[..n].fill(0.0);
        right[..n].fill(0.0);
        rev[..n].fill(0.0);
        cho[..n].fill(0.0);
        let Some(font) = self.font.clone() else { return };
        let data = font.data.clone();
        for v in &mut self.voices {
            v.render(&data, self.rate, blk, left, right, rev, cho, n);
        }
        let before = self.voices.len();
        self.voices.retain(|v| !v.dead);
        if self.voices.len() != before {
            self.publish();
        }
    }

    /// Ends the voices whose envelope ran out; keeps the rest (KeepAlive).
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    /// How many voices sound now (tests).
    pub fn voices_debug(&self) -> usize {
        self.voices.len()
    }
}

/// The filter's cutoff now: the player's CUT knob with the envelope's depth (the MOD knob, or the file's own depth when
/// its zone sets one) and the LFO's, in cents over the knob's Hz.
fn cutoff(r: &Region, blk: &Block, menv: f64, lfo: f64) -> f64 {
    let depth = if r.mod_env_to_filter != 0 { f64::from(r.mod_env_to_filter) } else { blk.mod_depth };
    (blk.cut * cents(menv * depth + lfo * f64::from(r.mod_lfo.to_filter))).clamp(10.0, 20000.0)
}

impl Voice {
    /// Renders `n` frames into the sides and the sends. The LFOs run from the voice's age (they start at the note on);
    /// the envelopes fall from where they were when the release began.
    #[allow(clippy::too_many_arguments)]
    fn render(&mut self, data: &[i16], rate: f64, blk: &Block, left: &mut [f32], right: &mut [f32], rev: &mut [f32], cho: &mut [f32], n: usize) {
        let r = self.region.clone();
        let base_step = pitch_of(&r, self.note, blk.bend) / rate;
        // Envelope 2 is the player's: the knobs' A / D / S / R over the file's delay (the editor syncs the knobs at
        // each preset load, so they are the truth here).
        let menv = Stages {
            delay: tc_seconds(r.mod_env.delay),
            attack: 0.005 + blk.env_a * blk.env_a * 20.0,
            hold: 0.0,
            decay: 0.005 + blk.env_d * blk.env_d * 20.0,
            sustain: blk.env_s.clamp(0.0, 1.0),
            release: 0.005 + blk.env_r * blk.env_r * 20.0,
        };
        let vib_hz = if r.vib_lfo.freq != 0 { 2f64.powf(f64::from(r.vib_lfo.freq) / 1200.0) } else { 0.0 };
        let vib_delay = tc_seconds(r.vib_lfo.delay);
        let lfo_delay = if r.mod_lfo.delay > -1000 { tc_seconds(r.mod_lfo.delay) } else { blk.lfo_delay };
        let lfo_hz = if r.mod_lfo.freq != 0 { 2f64.powf(f64::from(r.mod_lfo.freq) / 1200.0) } else { blk.lfo_hz };
        // The pan's equal-power gains; the sends' weights (a zone that sets none sends in full).
        let a = (r.pan + 1.0) * std::f64::consts::FRAC_PI_4;
        let (gl, gr) = (a.cos(), a.sin());
        let rs = if r.reverb_send > 0.0 { r.reverb_send } else { 1.0 };
        let cs = if r.chorus_send > 0.0 { r.chorus_send } else { 1.0 };
        let loop_all = r.loop_mode == 3;
        let loop_sustain = r.loop_mode == 1;

        for i in 0..n {
            if self.dead {
                break;
            }
            self.age += 1.0 / rate;
            // Envelopes: the attack side runs the stages; the release falls from where it began.
            let (v, m) = if self.releasing {
                let f = 1.0 - self.rel_age / self.vol.release.max(0.005);
                let fm = 1.0 - self.rel_age / menv.release.max(0.005);
                (self.rel_vol * f.max(0.0), self.rel_mod * fm.max(0.0))
            } else {
                (self.vol.at(self.age), menv.at(self.age))
            };
            self.vol_level = v;
            self.mod_level = m;
            // The mod LFO (the player's LFO 2 delay and speed when the file sets none) and the file's vibrato LFO.
            let lt = self.age - lfo_delay;
            let lfo = if lt > 0.0 { (2.0 * std::f64::consts::PI * lfo_hz * lt).sin() } else { 0.0 };
            let vib_t = self.age - vib_delay;
            let vib = if vib_t > 0.0 && vib_hz > 0.0 { (2.0 * std::f64::consts::PI * vib_hz * vib_t).sin() * f64::from(r.vib_lfo.to_pitch) } else { 0.0 };
            // Pitch, then the filter's cutoff.
            let lfo_pitch = lfo * f64::from(r.mod_lfo.to_pitch) + f64::from(r.mod_env_to_pitch) * m + vib;
            let step = base_step * cents(lfo_pitch);
            let hz = cutoff(&r, blk, m, lfo);
            for k in 0..2 {
                self.filter[k].set(hz, r.filter_q, rate);
            }
            // The sample, linearly interpolated.
            let (s0, s1, frac) = self.frames(data);
            let x = s0 + (s1 - s0) * frac;
            self.pos += step;
            // Looping: mode 3 loops always, mode 1 until the release; a released voice plays on to the end.
            let looping = loop_all || (loop_sustain && !self.releasing);
            if looping && self.pos >= r.loop_end as f64 {
                self.pos -= (r.loop_end - r.loop_start) as f64;
            }
            let ended = self.pos >= (r.end - 1) as f64;
            // Level: the envelope, the velocity, the file's attenuation and the LFO's tremolo.
            let trem = 1.0 - lfo.max(0.0) * f64::from(r.mod_lfo.to_volume) / 200.0;
            let g = v * self.velocity * r.attenuation * trem.max(0.0);
            let (l, rt) = (x * g * gl, x * g * gr);
            let fl = self.filter[0].next(l);
            let fr = self.filter[1].next(rt);
            left[i] += fl as f32;
            right[i] += fr as f32;
            rev[i] += (fl * rs) as f32;
            cho[i] += (fr * cs) as f32;
            if self.releasing {
                self.rel_age += 1.0 / rate;
                if self.rel_age >= self.vol.release.max(0.005) {
                    self.dead = true;
                }
            }
            if ended && (!looping || self.releasing) {
                self.dead = true;
            }
        }
    }

    /// The frames at `pos`: (s0, s1, fraction) from the 16-bit pool.
    fn frames(&self, data: &[i16]) -> (f64, f64, f64) {
        let p = self.pos.floor();
        let i = (p as usize).min(data.len().saturating_sub(1));
        let j = (i + 1).min(data.len().saturating_sub(1));
        // The pool's 16-bit words to floats (-1..1).
        (f64::from(data[i]) / 32768.0, f64::from(data[j]) / 32768.0, self.pos - p)
    }
}
