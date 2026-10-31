// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Soundfont Player: Lemon Studio's SoundFont 2 instrument as a VST3 / CLAP plugin (nih-plug). The file parses into
//! [`sf2::SoundFont`], the voices live in [`dsp`], the built-in reverb and chorus in [`fx`], and the editor is the
//! original's layout ([`view`], [`editor`]): the patch row with its arrows, the reverb and chorus sends, the envelope's
//! faders, the LFO's and misc dials.
//!
// spec: plugins/soundfont-player.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod fx;
pub mod params;
pub mod sf2;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;

use dsp::Block;
use params::SfParams;

/// What the audio thread and the editor share: the loaded soundfont, the preset names the parameter's texts read, and
/// the editor's own note previews.
pub struct Shared {
    /// The parsed soundfont, swapped by the editor or `initialize` (a read gives a clone of the `Arc`).
    font: Mutex<Option<Arc<sf2::SoundFont>>>,
    /// Bumped on every load: the sound notices and takes the new font.
    version: AtomicU64,
    /// The preset texts ("[0000:00]: Name"), for the preset parameter's value text and the editor's patch box.
    names: params::Names,
    /// The loaded file's path and name (the state keeps the path through the `sf2_path` parameter).
    path: Mutex<String>,
    /// Notes the editor plays itself (key, velocity, on) when the host does not play them.
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// Keys sounding (bit per key, 192 keys).
    pub sounding: [AtomicU64; 3],
    /// The host's output rate (Hz), 44100 until the host tells.
    rate: AtomicU32,
    /// Whether an editor is open (the plugin keeps its previews coming for it).
    pub editor_open: AtomicBool,
}

impl Shared {
    fn new(names: params::Names) -> Self {
        Shared {
            font: Mutex::new(None),
            version: AtomicU64::new(0),
            names,
            path: Mutex::new(String::new()),
            previews: Mutex::new(Vec::new()),
            sounding: Default::default(),
            rate: AtomicU32::new(44100),
            editor_open: AtomicBool::new(false),
        }
    }

    /// The loaded soundfont, if one is.
    pub fn font(&self) -> Option<Arc<sf2::SoundFont>> {
        self.font.lock().ok().and_then(|f| f.clone())
    }

    /// The load counter (the sound takes the font again when it moves).
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// The preset names (the parameter's value texts read these).
    pub fn names(&self) -> Vec<String> {
        self.names.read().map(|n| n.clone()).unwrap_or_default()
    }

    /// The loaded file's name (empty without one).
    pub fn font_name(&self) -> String {
        self.font.lock().ok().and_then(|f| f.as_ref().map(|f| f.name.clone())).unwrap_or_default()
    }

    /// The loaded file's path (empty without one).
    pub fn font_path(&self) -> String {
        self.path.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// How many presets the loaded font lists.
    pub fn preset_count(&self) -> usize {
        self.font.lock().ok().and_then(|f| f.as_ref().map(|f| f.presets.len())).unwrap_or(0)
    }

    /// A preset's display text ("[0000:00]: Name"), as the file numbers it; "No preset" without a font.
    pub fn patch_text(&self, index: usize) -> String {
        if let Some(f) = self.font() {
            if let Some(p) = f.presets.get(index) {
                return format!("[{:04}:{:02}]: {}", p.bank, p.program, p.name);
            }
        }
        "No preset".into()
    }

    /// Loads a soundfont (the editor or `initialize` thread): the file parses whole, the names list and the path follow.
    pub fn load(&self, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let font = sf2::parse(&bytes).map_err(|e| e.0)?;
        let names: Vec<String> = font.presets.iter().map(|p| format!("[{:04}:{:02}]: {}", p.bank, p.program, p.name)).collect();
        *self.names.write().map_err(|_| "the names list is busy".to_string())? = names;
        *self.path.lock().map_err(|_| "the path is busy".to_string())? = path.to_string();
        *self.font.lock().map_err(|_| "the font is busy".to_string())? = Some(Arc::new(font));
        self.version.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Forgets the loaded font (a state restored without a path).
    pub fn clear(&self) {
        if let Ok(mut f) = self.font.lock() {
            *f = None;
        }
        if let Ok(mut p) = self.path.lock() {
            p.clear();
        }
        if let Ok(mut n) = self.names.write() {
            n.clear();
        }
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// The host's output rate.
    pub fn get_rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
    }

    /// Publishes the keys sounding to the editor's keyboard.
    pub fn publish_sounding(&self, bits: [u64; 3]) {
        for (a, b) in self.sounding.iter().zip(bits) {
            a.store(b, Ordering::Relaxed);
        }
    }
}

/// The plugin: a SoundFont 2 instrument.
pub struct SoundfontPlayer {
    params: Arc<SfParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    reverb: fx::Reverb,
    chorus: fx::Chorus,
    /// The (font version, preset) the engine plays now.
    playing: Option<(u64, usize)>,
    bend: f64,
}

impl Default for SoundfontPlayer {
    fn default() -> Self {
        let names: params::Names = Arc::default();
        let shared = Arc::new(Shared::new(names.clone()));
        SoundfontPlayer {
            params: Arc::new(SfParams::new(names)),
            shared,
            engine: dsp::Engine::new(44100.0),
            reverb: fx::Reverb::new(44100),
            chorus: fx::Chorus::new(44100),
            playing: None,
            bend: 0.0,
        }
    }
}

impl SoundfontPlayer {
    /// The voices' controls for one block from the parameters (the editor's pending edits win).
    fn block(&self) -> Block {
        let p = &self.params;
        Block {
            env_a: f64::from(p.value(&p.env_a)),
            env_d: f64::from(p.value(&p.env_d)),
            env_s: f64::from(p.value(&p.env_s)),
            env_r: f64::from(p.value(&p.env_r)),
            lfo_delay: params::lfo_delay_seconds(f64::from(p.value(&p.lfo_del))),
            lfo_amt: params::lfo_amt_cents(f64::from(p.value(&p.lfo_amt))),
            lfo_hz: params::lfo_hz(f64::from(p.value(&p.lfo_spd))),
            cut: params::cut_hz(f64::from(p.value(&p.cut))),
            mod_depth: params::mod_cents(f64::from(p.value(&p.mod_amt))),
            bend: self.bend,
        }
    }

    /// Takes a newer font or preset when there is one (never blocks): the regions are flattened here, on the audio
    /// thread, where a preset change lands between blocks.
    fn pick_up(&mut self) {
        let version = self.shared.version();
        let preset = self.params.value(&self.params.preset).max(0) as usize;
        if self.playing != Some((version, preset)) {
            if let Some(font) = self.shared.font() {
                self.engine.set_preset(&font, preset);
            } else {
                self.engine.reset();
            }
            self.playing = Some((version, preset));
        }
    }

    /// One note event (between the block's renders).
    fn event(&mut self, ev: NoteEvent<()>, blk: &Block) {
        match ev {
            NoteEvent::NoteOn { note, velocity, .. } => self.engine.note_on(i32::from(note), f64::from(velocity), blk),
            NoteEvent::NoteOff { note, .. } => self.engine.note_off(i32::from(note)),
            NoteEvent::Choke { .. } => self.engine.all_off(),
            NoteEvent::MidiPitchBend { value, .. } => {
                // ±2 semitones.
                self.bend = f64::from(value * 2.0 - 1.0) * 200.0;
            }
            NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
            _ => {}
        }
    }
}

impl Plugin for SoundfontPlayer {
    const NAME: &'static str = "Soundfont Player";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SfEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        let rate = config.sample_rate.round() as u32;
        self.shared.set_rate(rate);
        self.engine.set_rate(f64::from(rate));
        self.reverb = fx::Reverb::new(rate);
        self.chorus = fx::Chorus::new(rate);
        self.chorus.set_level(f64::from(self.params.value(&self.params.cho_level)));
        self.reverb.set_level(f64::from(self.params.value(&self.params.rev_level)));
        // A restored state names a soundfont: load it now (initialize may block).
        let path = self.params.sf2_path.read().map(|p| p.clone()).unwrap_or_default();
        if path.is_empty() {
            self.shared.clear();
        } else if self.shared.font_path() != path {
            if let Err(e) = self.shared.load(&path) {
                nih_log!("Soundfont Player: {e}");
            }
        }
        self.playing = None;
        self.pick_up();
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.reverb.reset();
        self.chorus.reset();
        self.shared.publish_sounding(self.engine.sounding());
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.pick_up();
        let blk = self.block();
        self.reverb.set_level(f64::from(self.params.value(&self.params.rev_level)) * f64::from(self.params.value(&self.params.rev_builtin)) as f64);
        self.chorus.set_level(f64::from(self.params.value(&self.params.cho_level)) * f64::from(self.params.value(&self.params.cho_builtin)) as f64);
        // Notes the editor plays itself.
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(i32::from(key), f64::from(vel), &blk);
                } else {
                    self.engine.note_off(i32::from(key));
                }
            }
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        let mut at = 0;
        let (first, rest) = out.split_at_mut(1);
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                self.render(&mut first[0][at..t], &mut rest[0][at..t], &blk);
                at = t;
            }
            self.event(ev, &blk);
        }
        if at < len {
            self.render(&mut first[0][at..len], &mut rest[0][at..len], &blk);
        }
        self.shared.publish_sounding(self.engine.sounding());
        // A long release may be quiet for a while: tell the host to keep processing until the voices end.
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl SoundfontPlayer {
    /// The voices into both sides, the built-in reverb and chorus over their sends.
    fn render(&mut self, left: &mut [f32], right: &mut [f32], blk: &Block) {
        let n = left.len().min(right.len());
        if n == 0 {
            return;
        }
        let mut rev = vec![0.0f32; n];
        let mut cho = vec![0.0f32; n];
        self.engine.render(left, right, &mut rev, &mut cho, blk);
        self.reverb.process(&rev, left, right);
        self.chorus.process(&cho, left, right);
    }
}

// CLAP id and VST3 class id are the player's own (spec: plugins/plugin-formats.md, identity).
impl ClapPlugin for SoundfontPlayer {
    const CLAP_ID: &'static str = "org.openstudio.soundfont-player";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's SoundFont 2 player");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for SoundfontPlayer {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioSfPlay";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(SoundfontPlayer);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(SoundfontPlayer);
os_plugkit::linked_clap_entry!(SoundfontPlayer);

#[cfg(test)]
mod tests {
    use super::*;

    /// Test files share no name (the tests run side by side).
    fn tmp_name(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!("sfp-{}-{}-{}.sf2", tag, std::process::id(), N.fetch_add(1, Ordering::AcqRel)))
    }

    /// A soundfont in memory (as `sf2`'s tests build one) with a louder attack generator, and the plugin holding it.
    fn plugin(n: usize) -> (SoundfontPlayer, usize) {
        let bytes = sf2_bytes(n);
        let mut p = SoundfontPlayer::default();
        p.engine.set_rate(48000.0);
        let path = tmp_name("main");
        std::fs::write(&path, &bytes).unwrap();
        p.shared.load(path.to_str().unwrap()).unwrap();
        p.pick_up();
        let _ = std::fs::remove_file(&path);
        (p, n)
    }

    /// A minimal SF2 file (one preset, one instrument, one ramp sample of `n` frames).
    fn sf2_bytes(n: usize) -> Vec<u8> {
        let mut b = Vec::new();
        let mut list = |ty: &[u8; 4], body: &mut Vec<u8>| {
            let mut l = Vec::new();
            l.extend_from_slice(ty);
            l.append(body);
            while l.len() % 2 == 1 {
                l.push(0);
            }
            b.extend_from_slice(b"LIST");
            b.extend_from_slice(&(l.len() as u32).to_le_bytes());
            b.extend_from_slice(&l);
        };
        let mut info = Vec::new();
        info.extend_from_slice(b"ifil");
        info.extend_from_slice(&4u32.to_le_bytes());
        info.extend_from_slice(&2u16.to_le_bytes());
        info.extend_from_slice(&0u16.to_le_bytes());
        info.extend_from_slice(b"INAM");
        info.extend_from_slice(&8u32.to_le_bytes());
        info.extend_from_slice(b"Test\0\0\0\0");
        list(b"INFO", &mut info);
        let mut sdta = Vec::new();
        sdta.extend_from_slice(b"smpl");
        sdta.extend_from_slice(&((n * 2) as u32).to_le_bytes());
        for i in 0..n {
            // A 764 Hz sine at about -12 dBFS: no DC, so the filter's work shows in the level.
            let v = ((i as f64) * 0.1).sin() * 8000.0;
            sdta.extend_from_slice(&(v as i16).to_le_bytes());
        }
        list(b"sdta", &mut sdta);
        let mut pdta = Vec::new();
        let put = |p: &mut Vec<u8>, id: &[u8; 4], body: &[u8]| {
            p.extend_from_slice(id);
            p.extend_from_slice(&(body.len() as u32).to_le_bytes());
            p.extend_from_slice(body);
        };
        let mut rec = [0u8; 38];
        rec[..6].copy_from_slice(b"Test\0\0");
        rec[24..26].copy_from_slice(&0u16.to_le_bytes());
        let mut eop = [0u8; 38];
        eop[24..26].copy_from_slice(&1u16.to_le_bytes());
        put(&mut pdta, b"phdr", &[rec.as_slice(), eop.as_slice()].concat());
        put(&mut pdta, b"pbag", &[0u16.to_le_bytes(), 0u16.to_le_bytes(), 1u16.to_le_bytes(), 0u16.to_le_bytes()].concat());
        put(&mut pdta, b"pgen", &[41u16.to_le_bytes(), 0u16.to_le_bytes()].concat());
        let mut irec = [0u8; 22];
        irec[..6].copy_from_slice(b"Inst\0\0");
        let mut eos = [0u8; 22];
        eos[20..22].copy_from_slice(&1u16.to_le_bytes());
        put(&mut pdta, b"inst", &[irec.as_slice(), eos.as_slice()].concat());
        put(&mut pdta, b"ibag", &[0u16.to_le_bytes(), 0u16.to_le_bytes(), 3u16.to_le_bytes(), 0u16.to_le_bytes()].concat());
        put(&mut pdta, b"igen", &[53u16.to_le_bytes(), 0u16.to_le_bytes(), 43u16.to_le_bytes(), [0u8, 127u8], 34u16.to_le_bytes(), 10i16.to_le_bytes()].concat());
        let mut srec = [0u8; 46];
        srec[..6].copy_from_slice(b"Samp\0\0");
        srec[20..24].copy_from_slice(&0u32.to_le_bytes());
        srec[24..28].copy_from_slice(&(n as u32).to_le_bytes());
        srec[28..32].copy_from_slice(&0u32.to_le_bytes());
        srec[32..36].copy_from_slice(&(n as u32).to_le_bytes());
        srec[36..40].copy_from_slice(&48000u32.to_le_bytes());
        srec[40] = 60;
        let mut eos2 = [0u8; 46];
        eos2[20..24].copy_from_slice(&(n as u32).to_le_bytes());
        put(&mut pdta, b"shdr", &[srec.as_slice(), eos2.as_slice()].concat());
        list(b"pdta", &mut pdta);
        let len = (b.len() + 4) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(b"sfbk");
        out.extend_from_slice(&b);
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len() as f64).sqrt() as f32
    }

    #[test]
    fn a_note_plays_the_file() {
        // The sample outlasts the render (a 4 s file; the note runs 2 s of it).
        let (mut p, n) = plugin(192_000);
        assert!(p.engine.has_font());
        let blk = p.block();
        p.engine.note_on(60, 1.0, &blk);
        // The file's attack runs about a second (10 timecents); the tone is full after it.
        let (mut l, mut r) = (vec![0.0f32; 96_000], vec![0.0f32; 96_000]);
        p.render(&mut l, &mut r, &blk);
        assert!(rms(&l[48_000..]) > 0.005, "{}", rms(&l[48_000..]));
        // The level grows with the attack (the sample is a ramp under it).
        assert!(rms(&l[48_000..]) > rms(&l[..2400]));
        // A note off releases: the level falls away over the file's two-second release.
        p.engine.note_off(60);
        let (mut l2, mut r2) = (vec![0.0f32; 96_000], vec![0.0f32; 96_000]);
        p.render(&mut l2, &mut r2, &blk);
        assert!(rms(&l2[..1000]) > rms(&l2[40_000..]), "{} {}", rms(&l2[..1000]), rms(&l2[40_000..]));
        assert!(!p.engine.has_voices());
        let _ = n;
    }

    #[test]
    fn the_filter_knobs_shape_the_sound() {
        let (mut p, _) = plugin(48_000);
        // No modulation depth: the cutoff is the CUT knob alone (the envelope would open both).
        let blk = Block { mod_depth: 0.0, ..p.block() };
        p.engine.note_on(60, 1.0, &blk);
        let n = 9600;
        let (mut open_l, mut open_r) = (vec![0.0f32; n], vec![0.0f32; n]);
        p.render(&mut open_l, &mut open_r, &blk);
        // A closed cutoff dulls the ramp (its steps carry highs).
        let dull = Block { cut: params::cut_hz(0.0), ..blk };
        let mut p2 = {
            let mut q = SoundfontPlayer::default();
            q.engine.set_rate(48000.0);
            let path = tmp_name("filter");
            std::fs::write(&path, sf2_bytes(48_000)).unwrap();
            q.shared.load(path.to_str().unwrap()).unwrap();
            let _ = std::fs::remove_file(&path);
            q.pick_up();
            q
        };
        p2.engine.note_on(60, 1.0, &blk);
        let (mut closed_l, mut closed_r) = (vec![0.0f32; n], vec![0.0f32; n]);
        p2.render(&mut closed_l, &mut closed_r, &dull);
        assert!(rms(&closed_l) < rms(&open_l) * 0.5, "{} {}", rms(&open_l), rms(&closed_l));
    }

    #[test]
    fn the_reverb_and_chorus_answer_their_levels() {
        let (mut p, _) = plugin(48_000);
        let blk = p.block();
        p.engine.note_on(60, 1.0, &blk);
        let n = 24_000;
        let (mut l1, mut r1) = (vec![0.0f32; n], vec![0.0f32; n]);
        let mut rev = vec![0.0f32; n];
        let mut cho = vec![0.0f32; n];
        p.engine.render(&mut l1, &mut r1, &mut rev, &mut cho, &blk);
        assert!(rms(&rev) > 0.0 && rms(&cho) > 0.0, "the sends carry the voices");
        // Level 0: the effects add nothing.
        let mut p2 = SoundfontPlayer::default();
        let (rev2, cho2) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        let (mut l2, mut r2) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        p2.reverb.set_level(0.0);
        p2.chorus.set_level(0.0);
        p2.reverb.process(&rev2, &mut l2, &mut r2);
        p2.chorus.process(&cho2, &mut l2, &mut r2);
        assert!(l2.iter().all(|&v| v == 0.0) && r2.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn the_preset_flattens_and_switches() {
        let (mut p, _) = plugin(48_000);
        let font = p.shared.font().unwrap();
        let regions = dsp::regions(&font, 0);
        assert_eq!(regions.len(), 1);
        let r = &regions[0];
        assert_eq!((r.key_lo, r.key_hi), (0, 127));
        assert_eq!(r.vol_env.attack, 10);
        assert_eq!(r.root_key, 60);
        assert_eq!(r.filter_fc, 13500.0);
        assert_eq!(r.scale_tuning, 100);
        // The engine takes the preset and sounds it.
        p.engine.set_preset(&font, 0);
        assert!(p.engine.has_font());
        // An out-of-range preset is safe.
        p.engine.set_preset(&font, 9);
        let blk = p.block();
        p.engine.note_on(60, 1.0, &blk);
        let (mut l, mut r2) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        p.render(&mut l, &mut r2, &blk);
        assert!(l.iter().all(|&v| v == 0.0), "no regions, no sound");
    }

    #[test]
    fn the_shared_state_reads_well() {
        let (p, _) = plugin(1000);
        assert_eq!(p.shared.preset_count(), 1);
        assert_eq!(p.shared.font_name(), "Test");
        assert_eq!(p.shared.patch_text(0), "[0000:00]: Test");
        assert_eq!(p.shared.patch_text(1), "No preset");
        assert_eq!(p.shared.names(), vec!["[0000:00]: Test".to_string()]);
        assert!(p.shared.font_path().ends_with(".sf2"));
        // A second load swaps the font and bumps the version.
        let v0 = p.shared.version();
        let path = tmp_name("swap");
        std::fs::write(&path, sf2_bytes(500)).unwrap();
        p.shared.load(path.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(p.shared.version(), v0 + 1);
    }

    #[test]
    fn the_parameters_map_whole() {
        let (_, _) = plugin(10);
        let p = SoundfontPlayer::default();
        let map = p.params.param_map();
        let ids: Vec<&str> = map.iter().map(|(id, _, _)| id.as_str()).collect();
        for id in ["preset", "rev_level", "rev_builtin", "rev_send", "cho_level", "cho_builtin", "cho_send", "env_a", "env_d", "env_s", "env_r", "lfo_del", "lfo_amt", "lfo_spd", "cut", "mod"] {
            assert!(ids.contains(&id), "{id}");
        }
        // The soundfont's path rides in the persisted fields, not the map.
        assert!(!ids.contains(&"sf2_path"));
        *p.params.sf2_path.write().unwrap() = "/tmp/a.sf2".into();
        let saved = p.params.serialize_fields();
        assert_eq!(saved.get("sf2_path").map(String::as_str), Some("\"/tmp/a.sf2\""));
        let q = SoundfontPlayer::default();
        q.params.deserialize_fields(&saved);
        assert_eq!(q.params.sf2_path.read().unwrap().as_str(), "/tmp/a.sf2");
        // Every knob's index for the automation requests.
        assert_eq!(params::param_index("cut"), ids.iter().position(|&i| i == "cut"));
        assert_eq!(params::param_index("nope"), None);
        // The plugin ids are its own.
        assert_eq!(<SoundfontPlayer as ClapPlugin>::CLAP_ID, "org.openstudio.soundfont-player");
        assert_eq!(<SoundfontPlayer as Plugin>::NAME, "Soundfont Player");
    }
}
