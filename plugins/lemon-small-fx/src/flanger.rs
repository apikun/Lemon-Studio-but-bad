// Copyright (C) 2026 apikun
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Flanger: Lemon Studio's flanger as a plugin (Lemon Studio's own design; there is no original to match). The
//! sound is `os_engine::effects::flanger`; the editor is the six knobs in a grid of tiles, three across and two down.

use nih_plug::prelude::*;

use os_engine::effects::flanger::{self as dsp, Flanger, Settings};
use os_ui::Rect;

use crate::params::{leading_number, nearest_rising};
use crate::{Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

const fn setting(id: &'static str, name: &'static str, k: usize) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet: None }
}

/// The settings in the knobs' order.
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    setting("rate", "Rate", dsp::RATE),
    setting("depth", "Depth", dsp::DEPTH),
    setting("delay", "Delay", dsp::DELAY),
    Setting {
        id: "feedback",
        name: "Feedback",
        range: dsp::RANGES[dsp::FEEDBACK],
        start: dsp::DEFAULTS[dsp::FEEDBACK],
        magnet: Some(0),
    },
    setting("stereo", "Stereo", dsp::STEREO),
    Setting {
        id: "mix",
        name: "Mix",
        range: dsp::RANGES[dsp::MIX],
        start: dsp::DEFAULTS[dsp::MIX],
        magnet: Some(512),
    },
];

/// Editor layout: a grid of tiles, three across and two down.
const fn tile(id: &'static str, col: usize, row: usize, caption: &'static str) -> Control {
    let (x0, y0) = (8.0 + 164.0 * col as f32, 8.0 + 52.0 * row as f32);
    Control::Tile { id, x0, y0, x1: x0 + 158.0, y1: y0 + 46.0, caption }
}

const CONTROLS: [Control; 6] = [
    tile("rate", 0, 0, "RATE"),
    tile("depth", 1, 0, "DEPTH"),
    tile("delay", 2, 0, "DELAY"),
    tile("feedback", 0, 1, "FEEDBACK"),
    tile("stereo", 1, 1, "STEREO"),
    tile("mix", 2, 1, "MIX"),
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Flanger",
    word: "FLANGER",
    about: "Lemon Studio's flanger",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (502, 146),
    cards: &[],
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 494.0, y: 127.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 116.0, 486.0, 22.0)),
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

/// The depth as a share of the swing: `x`.
fn depth_share(v: i32) -> f64 {
    f64::from(dsp::position(dsp::DEPTH, v))
}

/// The delay in ms: `5 x`.
fn delay_ms(v: i32) -> f64 {
    f64::from(dsp::position(dsp::DELAY, v)) * 5.0
}

/// The feedback as a percent: `90 x`, signed.
fn feedback_percent(v: i32) -> f64 {
    f64::from(dsp::bipolar(dsp::FEEDBACK, v)) * 90.0
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
        dsp::DEPTH => format!("{:.1}%", depth_share(v) * 100.0),
        dsp::DELAY => format!("{:.2}ms", delay_ms(v)),
        dsp::FEEDBACK => format!("{:.1}%", feedback_percent(v)),
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
        dsp::DEPTH => Some(nearest_rising(range, x, |v| depth_share(v) * 100.0)),
        dsp::DELAY => Some(nearest_rising(range, x, delay_ms)),
        dsp::FEEDBACK => Some(nearest_rising(range, x, feedback_percent)),
        dsp::STEREO => Some(nearest_rising(range, x, stereo_deg)),
        _ => Some(nearest_rising(range, x, mix_percent)),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct FlangerSound(Flanger);

impl Sound for FlangerSound {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The lines empty and the LFOs together, as at the start of a render.
    fn reset(&mut self) {
        self.0.start();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(FlangerSound(Flanger::with_settings(rate, &settings(v))))
}

pub struct FlangerKind;

impl Kind for FlangerKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-flanger";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's flanger";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Flanger, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioFlngr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

pub type LemonFlanger = SmallFx<FlangerKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        // The defaults.
        assert_eq!(text(dsp::RATE, 1500), "0.45Hz");
        assert_eq!(text(dsp::DEPTH, 717), "70.0%");
        assert_eq!(text(dsp::DELAY, 1000), "1.00ms");
        assert_eq!(text(dsp::FEEDBACK, 256), "45.0%");
        assert_eq!(text(dsp::FEEDBACK, 0), "0.0%");
        assert_eq!(text(dsp::FEEDBACK, -512), "-90.0%");
        assert_eq!(text(dsp::STEREO, 512), "90.0 deg");
        assert_eq!(text(dsp::MIX, 512), "50%");
        assert_eq!(parse(dsp::RATE, "0.45"), Some(1500));
        assert_eq!(parse(dsp::DEPTH, "70"), Some(717));
        assert_eq!(parse(dsp::DELAY, "1 ms"), Some(1000));
        assert_eq!(parse(dsp::FEEDBACK, "-90%"), Some(-512));
        assert_eq!(parse(dsp::FEEDBACK, "45.0"), Some(256));
        assert_eq!(parse(dsp::STEREO, "90"), Some(512));
        assert_eq!(parse(dsp::MIX, "50"), Some(512));
        assert_eq!(parse(dsp::RATE, "loud"), None);
    }

    /// The plugin plays the engine's sound: the same samples as the engine effect on the same settings.
    #[test]
    fn the_plugin_plays_the_engines_sound() {
        let v = [1500, 717, 1000, 256, 512, 512];
        let mut s = sound(48000, &v);
        let mut f = Flanger::with_settings(48000, &settings(&v));
        let input = |i: usize| ((i as f32) * 0.031).sin() * 0.5;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) =
            (0..1500).map(|i| { let x = input(i); (x, -input(i + 7)) }).unzip();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        f.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // A reset empties the lines: silence in, silence out.
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }
}
