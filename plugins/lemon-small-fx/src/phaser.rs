// Copyright (C) 2026 apikun
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Phaser: Lemon Studio's phaser as a plugin (Lemon Studio's own design; there is no original to match). The
//! sound is `os_engine::effects::phaser`; the editor is the seven knobs in a grid of tiles, three across and three
//! down with the mix on its own row.

use nih_plug::prelude::*;

use os_engine::effects::phaser::{self as dsp, Phaser, Settings};
use os_ui::Rect;

use crate::chorus::hz_text;
use crate::params::{leading_number, nearest_rising};
use crate::{Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

const fn setting(id: &'static str, name: &'static str, k: usize) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet: None }
}

/// The settings in the knobs' order.
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    setting("rate", "Rate", dsp::RATE),
    setting("depth", "Depth", dsp::DEPTH),
    setting("center", "Center", dsp::CENTER),
    Setting {
        id: "feedback",
        name: "Feedback",
        range: dsp::RANGES[dsp::FEEDBACK],
        start: dsp::DEFAULTS[dsp::FEEDBACK],
        magnet: Some(0),
    },
    setting("stages", "Stages", dsp::STAGES),
    setting("stereo", "Stereo", dsp::STEREO),
    Setting {
        id: "mix",
        name: "Mix",
        range: dsp::RANGES[dsp::MIX],
        start: dsp::DEFAULTS[dsp::MIX],
        magnet: Some(512),
    },
];

/// Editor layout: a grid of tiles, three across and three down, the mix alone on the last row.
const fn tile(id: &'static str, col: usize, row: usize, caption: &'static str) -> Control {
    let (x0, y0) = (8.0 + 164.0 * col as f32, 8.0 + 52.0 * row as f32);
    Control::Tile { id, x0, y0, x1: x0 + 158.0, y1: y0 + 46.0, caption }
}

const CONTROLS: [Control; 7] = [
    tile("rate", 0, 0, "RATE"),
    tile("depth", 1, 0, "DEPTH"),
    tile("center", 2, 0, "CENTER"),
    tile("feedback", 0, 1, "FEEDBACK"),
    tile("stages", 1, 1, "STAGES"),
    tile("stereo", 2, 1, "STEREO"),
    tile("mix", 0, 2, "MIX"),
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Phaser",
    word: "PHASER",
    about: "Lemon Studio's phaser",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (502, 198),
    cards: &[],
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 494.0, y: 179.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 168.0, 486.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: true,
};

/// The rate in Hz: `5 x^2` (the engine's curve).
fn rate_hz(v: i32) -> f64 {
    dsp::speed_hz(dsp::position(dsp::RATE, v))
}

/// The depth as a percent: `100 x`.
fn depth_percent(v: i32) -> f64 {
    f64::from(dsp::position(dsp::DEPTH, v)) * 100.0
}

/// The centre in Hz (the chorus's cross cutoff's curve).
fn center(v: i32) -> f64 {
    f64::from(dsp::center_hz(dsp::position(dsp::CENTER, v)))
}

/// The feedback as a percent: `90 x`, signed.
fn feedback_percent(v: i32) -> f64 {
    f64::from(dsp::bipolar(dsp::FEEDBACK, v)) * 90.0
}

/// The stages the knob stands for.
fn stage_count(v: i32) -> f64 {
    f64::from(dsp::stages(v) as u32)
}

/// How far the right side's LFO runs ahead, in degrees: `180 x`.
fn stereo_deg(v: i32) -> f64 {
    f64::from(dsp::position(dsp::STEREO, v)) * 180.0
}

/// The mix as a percent: `100 x`.
fn mix_percent(v: i32) -> f64 {
    f64::from(dsp::position(dsp::MIX, v)) * 100.0
}

/// Value texts (Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::RATE => format!("{:.2}Hz", rate_hz(v)),
        dsp::DEPTH => format!("{:.1}%", depth_percent(v)),
        dsp::CENTER => hz_text(center(v)),
        dsp::FEEDBACK => format!("{:.1}%", feedback_percent(v)),
        dsp::STAGES => format!("{} stages", stage_count(v) as u32),
        dsp::STEREO => format!("{:.1} deg", stereo_deg(v)),
        _ => format!("{:.0}%", mix_percent(v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let x = leading_number(&low)?;
    match k {
        dsp::RATE => Some(nearest_rising(range, x, rate_hz)),
        dsp::DEPTH => Some(nearest_rising(range, x, depth_percent)),
        dsp::CENTER => Some(nearest_rising(range, if low.contains('k') { x * 1000.0 } else { x }, center)),
        dsp::FEEDBACK => Some(nearest_rising(range, x, feedback_percent)),
        dsp::STAGES => Some(nearest_rising(range, x, stage_count)),
        dsp::STEREO => Some(nearest_rising(range, x, stereo_deg)),
        _ => Some(nearest_rising(range, x, mix_percent)),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct PhaserSound(Phaser);

impl Sound for PhaserSound {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The stages empty and the LFOs together, as at the start of a render.
    fn reset(&mut self) {
        self.0.start();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(PhaserSound(Phaser::with_settings(rate, &settings(v))))
}

pub struct PhaserKind;

impl Kind for PhaserKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-phaser";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's phaser";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Phaser, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPhasr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

pub type LemonPhaser = SmallFx<PhaserKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        // The defaults.
        assert_eq!(text(dsp::RATE, 1500), "0.45Hz");
        assert_eq!(text(dsp::DEPTH, 512), "50.0%");
        assert_eq!(text(dsp::CENTER, 2500), "320.2Hz");
        assert_eq!(text(dsp::FEEDBACK, 341), "59.9%");
        assert_eq!(text(dsp::FEEDBACK, -512), "-90.0%");
        assert_eq!(text(dsp::STAGES, 0), "2 stages");
        assert_eq!(text(dsp::STAGES, 1), "4 stages");
        assert_eq!(text(dsp::STAGES, 3), "8 stages");
        assert_eq!(text(dsp::STEREO, 512), "90.0 deg");
        assert_eq!(text(dsp::MIX, 512), "50%");
        assert_eq!(parse(dsp::RATE, "0.45"), Some(1500));
        assert_eq!(parse(dsp::DEPTH, "50"), Some(512));
        assert_eq!(parse(dsp::CENTER, "320.2"), Some(2500));
        assert_eq!(parse(dsp::CENTER, "1.001kHz"), Some(3277));
        assert_eq!(parse(dsp::FEEDBACK, "60"), Some(341));
        assert_eq!(parse(dsp::STAGES, "6 stages"), Some(2));
        assert_eq!(parse(dsp::STAGES, "8"), Some(3));
        assert_eq!(parse(dsp::STEREO, "90"), Some(512));
        assert_eq!(parse(dsp::MIX, "50"), Some(512));
        assert_eq!(parse(dsp::CENTER, "quiet"), None);
    }

    /// The plugin plays the engine's sound: the same samples as the engine effect on the same settings.
    #[test]
    fn the_plugin_plays_the_engines_sound() {
        let v = [1500, 512, 2500, 341, 1, 512, 512];
        let mut s = sound(48000, &v);
        let mut p = Phaser::with_settings(48000, &settings(&v));
        let input = |i: usize| ((i as f32) * 0.031).sin() * 0.5;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..1500).map(|i| { let x = input(i); (x, -input(i + 7)) }).unzip();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        p.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // A reset empties the stages: silence in, silence out.
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }
}
