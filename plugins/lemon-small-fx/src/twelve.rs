// Copyright (C) 2026 mikobuntu
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mango 12: the colour of a classic 12-bit sampler as a small effect. One sound: the input is gained into a
//! 26.04 kHz, 12-bit converter that samples it without an anti-alias filter (the aliasing is mandatory) and leaves
//! through a soft-saturating output stage, with an optional sixth-order Butterworth low pass in front of the
//! converter (`99` = off) for the filter-into-the-sampler workflow. The tune fader detunes the converter's clock the
//! way the original's tune knob pitches a source and compensates it: pitch and duration never change, only the
//! aliasing and the bandwidth do. The fader's marks are turntable speeds relative to 33 (45, x2 and 78); `0.1 ST`
//! steps it in tenths of a semitone where `1 ST` steps whole ones, and `Ext.` reaches twice as deep, to -30 ST. The
//! route picks what the converter hears (the stereo pair, a mono sum, one channel, the mid or the side — what it
//! does not hear passes as it is), the gains set how hard it is hit and how loud it leaves (`Link` holds their
//! total at the gain out, and while it is on the out knob shows the effective output gain, so it follows the in
//! knob), then the mix. Not a recreation of an FL plugin: its state is its own (modelled on the
//! E-mu SP-1200's converter and output stage, in the spirit of Wave Tracing's SP-950).

use nih_plug::prelude::*;
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::plug;
use os_ui::{Canvas, Font, Rect};

use crate::params::leading_number;
use crate::{Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec};

pub const TUNE: usize = 0;
pub const FINE: usize = 1;
pub const EXT: usize = 2;
pub const IN: usize = 3;
pub const OUT: usize = 4;
pub const LINK: usize = 5;
pub const FILTER: usize = 6;
pub const ROUTE: usize = 7;
pub const MIX: usize = 8;

/// The routes, in the switch's order: what the converter hears.
pub const STEREO: i32 = 0;
pub const MONO_SUM: i32 = 1;
pub const MONO_L: i32 = 2;
pub const MONO_R: i32 = 3;
pub const STEREO_L: i32 = 4;
pub const STEREO_R: i32 = 5;
pub const MID: i32 = 6;
pub const SIDE: i32 = 7;

pub const ROUTE_NAMES: &[&str] = &["Stereo", "Mono Sum", "Mono L", "Mono R", "Stereo L", "Stereo R", "Mid", "Side"];

pub const SETTINGS: [Setting; 9] = [
    // Tenths of a semitone of detune, down from the 33 mark (0) to 78 (150); `Ext.` doubles it.
    Setting { id: "tune", name: "Tune", range: (0, 150), start: 0, magnet: Some(0) },
    Setting { id: "fine", name: "Fine", range: (0, 1), start: 0, magnet: None },
    Setting { id: "ext", name: "Range", range: (0, 1), start: 0, magnet: None },
    // Tenths of a dB.
    Setting { id: "in", name: "Gain In", range: (-120, 120), start: 0, magnet: Some(0) },
    Setting { id: "out", name: "Gain Out", range: (-120, 120), start: 0, magnet: Some(0) },
    Setting { id: "link", name: "Link", range: (0, 1), start: 0, magnet: None },
    // 99 = the filter is bypassed.
    Setting { id: "filter", name: "Filter", range: (0, 99), start: 99, magnet: Some(99) },
    Setting { id: "route", name: "Route", range: (0, 7), start: STEREO, magnet: None },
    Setting { id: "mix", name: "Mix", range: (0, 100), start: 100, magnet: None },
];

const CARDS: [Card; 5] = [
    Card { x0: 8.0, x1: 148.0, y0: 28.0, y1: 190.0, heading: "TUNE" },
    Card { x0: 156.0, x1: 264.0, y0: 28.0, y1: 178.0, heading: "ROUTE" },
    Card { x0: 272.0, x1: 428.0, y0: 28.0, y1: 104.0, heading: "GAIN" },
    Card { x0: 272.0, x1: 428.0, y0: 126.0, y1: 190.0, heading: "FILTER" },
    Card { x0: 436.0, x1: 612.0, y0: 28.0, y1: 187.0, heading: "MIX" },
];

const CONTROLS: [Control; 9] = [
    Control::Fader {
        id: "tune",
        x: 104.0,
        top: 52.0,
        bot: 138.0,
        label: "TUNE",
        col: brand::MAIN,
        bipolar: false,
        // 33 at the bottom, 45, x2 and 78 up the travel (turntable speeds relative to 33).
        marks: &[0.0, 0.347, 0.8, 0.98],
    },
    Control::Choice { id: "fine", x: 20.0, top: 150.0, names: &["1 ST", "0.1 ST"] },
    Control::Choice { id: "ext", x: 88.0, top: 150.0, names: &["Int.", "Ext."] },
    Control::Choice { id: "route", x: 172.0, top: 44.0, names: ROUTE_NAMES },
    Control::Knob { id: "in", cx: 306.0, cy: 50.0, r: 16.0, caption: "IN", bipolar: true },
    Control::Knob { id: "out", cx: 352.0, cy: 50.0, r: 16.0, caption: "OUT", bipolar: true },
    Control::Choice { id: "link", x: 384.0, top: 44.0, names: &["Off", "Link"] },
    Control::Knob { id: "filter", cx: 350.0, cy: 150.0, r: 16.0, caption: "FILTER", bipolar: false },
    Control::Knob { id: "mix", cx: 524.0, cy: 152.0, r: 15.0, caption: "MIX", bipolar: false },
];

pub const SPEC: &Spec = &Spec {
    name: "Mango 12",
    word: "12",
    about: "Mango Studio's 12-bit sampler colour",
    settings: &SETTINGS,
    version: None,
    any_version: false,
    fields: &[],
    tail: &[],
    size: (620, 214),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: Rect::new(444.0, 36.0, 160.0, 92.0), draw: screen }),
    mark: Mark { x: 612.0, y: 201.0, anchor: 1.0, stacked: false },
    hint: None,
    // The fader's turntable speeds, beside their ticks.
    labels: &[(83.0, 47.0, "78"), (83.0, 63.0, "X2"), (83.0, 102.0, "45"), (83.0, 132.0, "33")],
    text,
    parse,
    shown: Some(shown),
    sound,
    rings: false,
};

fn text(k: usize, v: i32) -> String {
    match k {
        TUNE if v == 0 => "0.0ST".into(),
        TUNE => format!("-{:.1}ST", f64::from(v) / 10.0),
        FINE => ["1 ST", "0.1 ST"][v.clamp(0, 1) as usize].into(),
        EXT => ["Int.", "Ext."][v.clamp(0, 1) as usize].into(),
        IN | OUT => format!("{:.1}dB", f64::from(v) / 10.0),
        LINK => ["Off", "Link"][v.clamp(0, 1) as usize].into(),
        FILTER if v >= 99 => "Off".into(),
        FILTER => format!("{v}"),
        ROUTE => ROUTE_NAMES[v.clamp(0, 7) as usize].into(),
        _ => format!("{v}%"),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let s = s.trim();
    match k {
        TUNE => {
            let v = (-leading_number(s)? * 10.0).round() as i32;
            Some(v.clamp(range.0, range.1))
        }
        FINE => match s.to_ascii_lowercase().as_str() {
            "1 st" | "1" => Some(0),
            "0.1 st" | "0.1" => Some(1),
            _ => None,
        },
        EXT => match s.to_ascii_lowercase().trim_end_matches('.') {
            "int" => Some(0),
            "ext" => Some(1),
            _ => None,
        },
        IN | OUT => {
            let v = (leading_number(s)? * 10.0).round() as i32;
            Some(v.clamp(range.0, range.1))
        }
        LINK => match s.to_ascii_lowercase().as_str() {
            "off" => Some(0),
            "link" => Some(1),
            _ => None,
        },
        FILTER if s.eq_ignore_ascii_case("off") => Some(99),
        FILTER => {
            let v = leading_number(s)?.round() as i32;
            Some(v.clamp(range.0, range.1))
        }
        ROUTE => ROUTE_NAMES.iter().position(|m| s.eq_ignore_ascii_case(m)).map(|i| i as i32),
        _ => {
            let v = leading_number(s)?.round() as i32;
            Some(v.clamp(range.0, range.1))
        }
    }
}

/// While the gains are linked the out knob shows the effective output gain: the stored out less the in, what the
/// sound's output stage runs at while the total stays at the gain out. The offset is the in, so the knob follows the
/// in knob, and editing it sets the effective gain (the stored total moves by the same amount). Every other setting
/// shows its stored value.
fn shown(v: &[i32], k: usize) -> Option<i32> {
    match k {
        OUT if v.get(LINK) == Some(&1) => Some(v.get(IN).copied().unwrap_or(SETTINGS[IN].start)),
        _ => None,
    }
}

/// The converter's clock.
const CLOCK: f64 = 26_040.0;

/// The detune the sound runs at, in tenths of a semitone: the tune, doubled by `Ext.`, snapped to whole semitones
/// unless the `0.1 ST` steps are on. The pitch and the duration never follow it; only the clock does.
fn detune_tenths(v: &[i32]) -> f64 {
    let get = |k: usize| v.get(k).copied().unwrap_or(SETTINGS[k].start).clamp(SETTINGS[k].range.0, SETTINGS[k].range.1);
    let mut d = f64::from(get(TUNE)) * f64::from(1 + get(EXT));
    if get(FINE) == 0 {
        d = (d / 10.0).round() * 10.0;
    }
    d
}

/// The converter's 12 bits: round to one of 4096 levels across full scale (and clip there, as the ADC does).
fn quantize(x: f64) -> f32 {
    ((x * 2048.0).round().clamp(-2048.0, 2047.0) / 2048.0) as f32
}

/// The output stage: a soft knee that stays out of the way at the levels the original likes (peaks near -10 dBFS)
/// and grips when the gain pushes.
fn saturate(x: f32) -> f32 {
    (0.75 * x).tanh() / 0.75
}

/// The filter's cutoff for a knob value (`rate` keeps it under half the rate).
fn cutoff(k: i32, rate: f64) -> f64 {
    (100.0 * 130.2f64.powf(f64::from(k) / 98.0)).min(rate * 0.45)
}

/// A sixth-order Butterworth low pass as three biquad sections (b0, b1, b2, a1, a2) at its Qs.
fn low_pass(f: f64, rate: f64) -> [[f64; 5]; 3] {
    const Q: [f64; 3] = [0.517_638_090_205_041_5, std::f64::consts::FRAC_1_SQRT_2, 1.931_851_652_578_136_6];
    let w = std::f64::consts::TAU * f / rate;
    let (s, c) = w.sin_cos();
    Q.map(|q| {
        let a = s / (2.0 * q);
        let b = 1.0 - c;
        let a0 = 1.0 + a;
        [b / 2.0 / a0, b / a0, b / 2.0 / a0, -2.0 * c / a0, (1.0 - a) / a0]
    })
}

/// One biquad section (transposed direct form II is not needed: the state is four numbers).
fn section(x: f64, c: &[f64; 5], z: &mut [f64; 4]) -> f64 {
    let y = c[0] * x + c[1] * z[0] + c[2] * z[1] - c[3] * z[2] - c[4] * z[3];
    z[1] = z[0];
    z[0] = x;
    z[3] = z[2];
    z[2] = y;
    y
}

/// One converter lane: the filter's memory, the clock's phase and the sample it holds.
struct Lane {
    taps: [[f64; 4]; 3],
    phase: f64,
    held: f32,
}

impl Default for Lane {
    fn default() -> Self {
        Lane { taps: [[0.0; 4]; 3], phase: 0.0, held: 0.0 }
    }
}

/// The sound: the SP's converter and output stage, set from the settings.
pub struct Twelve {
    rate: f64,
    route: i32,
    mix: f32,
    in_gain: f32,
    out_gain: f32,
    /// Host samples per clock tick.
    inc: f64,
    detune: f64,
    filter_k: i32,
    coeffs: [[f64; 5]; 3],
    filtering: bool,
    lanes: [Lane; 2],
}

impl Twelve {
    pub fn new(rate: u32, v: &[i32]) -> Self {
        let mut t = Twelve {
            rate: f64::from(rate),
            route: STEREO,
            mix: 1.0,
            in_gain: 1.0,
            out_gain: 1.0,
            inc: CLOCK / f64::from(rate),
            detune: 0.0,
            filter_k: 99,
            coeffs: low_pass(13_020.0, f64::from(rate)),
            filtering: false,
            lanes: [Lane::default(), Lane::default()],
        };
        t.set(v);
        t
    }

    pub fn set(&mut self, v: &[i32]) {
        let get = |k: usize| v.get(k).copied().unwrap_or(SETTINGS[k].start).clamp(SETTINGS[k].range.0, SETTINGS[k].range.1);
        self.route = get(ROUTE);
        self.mix = get(MIX) as f32 / 100.0;
        self.in_gain = 10f32.powf(get(IN) as f32 / 200.0);
        // Linked: the output gain follows the input gain down, so the total stays at the gain out (each knob keeps
        // to its own range).
        let out = if get(LINK) == 1 { (get(OUT) - get(IN)).clamp(-120, 120) } else { get(OUT) };
        self.out_gain = 10f32.powf(out as f32 / 200.0);
        self.detune = detune_tenths(v);
        self.inc = CLOCK * 2f64.powf(-self.detune / 120.0) / self.rate;
        self.filter_k = get(FILTER);
        self.filtering = self.filter_k != 99;
        if self.filtering {
            self.coeffs = low_pass(cutoff(self.filter_k, self.rate), self.rate);
        }
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.rate = f64::from(rate);
        self.inc = CLOCK * 2f64.powf(-self.detune / 120.0) / self.rate;
        if self.filtering {
            self.coeffs = low_pass(cutoff(self.filter_k, self.rate), self.rate);
        }
    }

    /// One sample through lane `i`'s filter, clock and output stage (before the output gain).
    fn lane(&mut self, i: usize, x: f32) -> f32 {
        let mut s = f64::from(x * self.in_gain);
        if self.filtering {
            for (c, z) in self.coeffs.iter().zip(self.lanes[i].taps.iter_mut()) {
                s = section(s, c, z);
            }
        }
        let l = &mut self.lanes[i];
        l.phase += self.inc;
        if l.phase >= 1.0 {
            l.held = quantize(s);
            l.phase -= 1.0;
        }
        saturate(l.held)
    }

    /// One block through the route's shape, the output gain and the mix.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (l0, r0) = (*l, *r);
            let (ol, or) = match self.route {
                MONO_SUM => {
                    let m = (l0 + r0) * 0.5;
                    let v = m * (1.0 - self.mix) + self.lane(0, m) * self.out_gain * self.mix;
                    (v, v)
                }
                MONO_L => {
                    let v = l0 * (1.0 - self.mix) + self.lane(0, l0) * self.out_gain * self.mix;
                    (v, v)
                }
                MONO_R => {
                    let v = r0 * (1.0 - self.mix) + self.lane(0, r0) * self.out_gain * self.mix;
                    (v, v)
                }
                STEREO_L => {
                    let v = l0 * (1.0 - self.mix) + self.lane(0, l0) * self.out_gain * self.mix;
                    (v, r0)
                }
                STEREO_R => {
                    let v = r0 * (1.0 - self.mix) + self.lane(0, r0) * self.out_gain * self.mix;
                    (l0, v)
                }
                MID => {
                    let (m, s) = ((l0 + r0) * 0.5, (l0 - r0) * 0.5);
                    let v = m * (1.0 - self.mix) + self.lane(0, m) * self.out_gain * self.mix;
                    (v + s, v - s)
                }
                SIDE => {
                    let (m, s) = ((l0 + r0) * 0.5, (l0 - r0) * 0.5);
                    let v = s * (1.0 - self.mix) + self.lane(0, s) * self.out_gain * self.mix;
                    (m + v, m - v)
                }
                _ => {
                    let wl = self.lane(0, l0) * self.out_gain * self.mix;
                    let wr = self.lane(1, r0) * self.out_gain * self.mix;
                    (l0 * (1.0 - self.mix) + wl, r0 * (1.0 - self.mix) + wr)
                }
            };
            *l = ol;
            *r = or;
        }
    }

    pub fn reset(&mut self) {
        self.lanes = [Lane::default(), Lane::default()];
    }
}

struct Sp(Twelve);

impl Sound for Sp {
    fn set(&mut self, v: &[i32]) {
        self.0.set(v);
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Sp(Twelve::new(rate, v)))
}

// ---------------------------------------------------------------------------------------------------------------
// The display: the LCD's line, the converter's staircase and its clock.

const HEAD: Font = Font::new(10).bold();
const FOOT: Font = Font::condensed(10, 0).bold();

fn screen(c: &mut Canvas, r: Rect, v: &[i32]) {
    let d = detune_tenths(v);
    let tune = if d == 0.0 { "0.0ST".to_string() } else { format!("-{:.1}ST", d / 10.0) };
    let k = v.get(FILTER).copied().unwrap_or(99);
    let head = format!("TUNE {tune} · FILTER {}", if k >= 99 { "OFF".to_string() } else { format!("{k}") });
    c.text_clip(HEAD, r.x + 7.0, r.y + 5.0, r.w - 14.0, brand::TEXT, &head);
    // Two cycles of a sine through the clock and the 12 bits: the staircase coarsens as the tune detunes.
    let (x0, x1) = (r.x + 8.0, r.right() - 8.0);
    let (top, bot) = (r.y + 22.0, r.bottom() - 22.0);
    let cy = ((top + bot) / 2.0).round();
    let half = (bot - top) / 2.0 - 3.0;
    c.fill_rect(Rect::span(x0, x1, cy, cy + 1.0), plug::GRID);
    let n = ((x1 - x0) as usize).max(2);
    let ppc = (13.0 * 2f64.powf(-d / 120.0)).max(3.0);
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let u = i as f64 / n as f64;
            let holds = (u * 2.0 * ppc).floor();
            let y = (0.8 * (std::f64::consts::TAU * holds / ppc).sin() * 2048.0).round() / 2048.0;
            (x0 + (x1 - x0) * u as f32, cy - (y * f64::from(half) / 0.9) as f32)
        })
        .collect();
    c.polyline(&pts, 1.5, brand::MAIN);
    let clock = CLOCK * 2f64.powf(-d / 120.0);
    c.text_center(FOOT, Rect::new(r.x, r.bottom() - 16.0, r.w, 12.0), plug::DIM, &format!("12-BIT · {:.1}KHZ", clock / 1000.0));
}

pub struct TwelveKind;

impl Kind for TwelveKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.mango-12";
    const CLAP_DESCRIPTION: &'static str = "Mango Studio's 12-bit sampler colour";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Distortion, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"MangoStudio12   ";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

pub type MangoTwelve = SmallFx<TwelveKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!([text(TUNE, 0), text(TUNE, 52), text(TUNE, 147)], ["0.0ST", "-5.2ST", "-14.7ST"]);
        assert_eq!([parse(TUNE, "0.0ST"), parse(TUNE, "-5.2ST"), parse(TUNE, "-0.1ST")], [Some(0), Some(52), Some(1)]);
        assert_eq!([text(FINE, 0), text(FINE, 1)], ["1 ST", "0.1 ST"]);
        assert_eq!([parse(FINE, "1 ST"), parse(FINE, "0.1 ST")], [Some(0), Some(1)]);
        assert_eq!([text(EXT, 0), text(EXT, 1)], ["Int.", "Ext."]);
        assert_eq!([parse(EXT, "int."), parse(EXT, "ext")], [Some(0), Some(1)]);
        assert_eq!([text(IN, 0), text(IN, -55), text(OUT, 120)], ["0.0dB", "-5.5dB", "12.0dB"]);
        assert_eq!([parse(IN, "0dB"), parse(OUT, "-5.5dB")], [Some(0), Some(-55)]);
        assert_eq!([text(LINK, 0), text(LINK, 1)], ["Off", "Link"]);
        assert_eq!(parse(LINK, "link"), Some(1));
        assert_eq!([text(FILTER, 99), text(FILTER, 84)], ["Off", "84"]);
        assert_eq!([parse(FILTER, "off"), parse(FILTER, "84")], [Some(99), Some(84)]);
        assert_eq!([text(ROUTE, 0), text(ROUTE, 1), text(ROUTE, 7)], ["Stereo", "Mono Sum", "Side"]);
        assert_eq!(parse(ROUTE, "mono sum"), Some(1));
        assert_eq!([text(MIX, 100), text(MIX, 0)], ["100%", "0%"]);
        assert_eq!(parse(MIX, "40%"), Some(40));
    }

    /// A sine through the converter changes only when the clock ticks: about 26040 holds a second at the SP's rate,
    /// the same on both channels of the stereo route.
    #[test]
    fn the_converter_lands_on_the_sp1200_clock() {
        let (n, rate) = (44100usize, 44100u32);
        let mut t = Twelve::new(rate, &[0, 1, 0, 0, 0, 0, 99, STEREO, 100]);
        let mut l: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / rate as f32).sin()).collect();
        let mut r = l.clone();
        t.process(&mut l, &mut r);
        let steps = l.windows(2).filter(|w| w[0] != w[1]).count();
        assert!((steps as f64 - 26040.0).abs() < 260.0, "{steps}");
        assert_eq!(l, r);
    }

    /// The tune detunes the clock, not the pitch: 78's detune runs it near 11 kHz and the 440 Hz sine still crosses
    /// zero 440 times a second. `Ext.` reaches twice as deep; without `0.1 ST` steps the tune snaps to whole
    /// semitones.
    #[test]
    fn tune_detunes_the_clock_and_not_the_pitch() {
        let (n, rate) = (44100usize, 44100u32);
        let sine = |hz: f32| (0..n).map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin()).collect::<Vec<f32>>();
        let mut steps_of = |t: &mut Twelve, x: &mut Vec<f32>| {
            let mut r = x.clone();
            t.process(x, &mut r);
            x.windows(2).filter(|w| w[0] != w[1]).count()
        };
        let crossings = |x: &[f32]| x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        let mut t = Twelve::new(rate, &[150, 1, 0, 0, 0, 0, 99, STEREO, 100]);
        let mut x = sine(440.0);
        let steps = steps_of(&mut t, &mut x);
        assert!((steps as f64 - 10948.0).abs() < 110.0, "{steps}");
        assert!((crossings(&x) as i64 - 440).abs() <= 2, "{}", crossings(&x));
        let mut t = Twelve::new(rate, &[150, 1, 1, 0, 0, 0, 99, STEREO, 100]);
        let mut x = sine(440.0);
        let steps = steps_of(&mut t, &mut x);
        assert!((steps as f64 - 4603.0).abs() < 46.0, "{steps}");
        let mut t = Twelve::new(rate, &[52, 0, 0, 0, 0, 0, 99, STEREO, 100]);
        let mut x = sine(440.0);
        let steps = steps_of(&mut t, &mut x);
        assert!((steps as f64 - 19509.0).abs() < 195.0, "{steps}");
    }

    /// The routes pick what the converter hears; what it does not hear passes as it is.
    #[test]
    fn the_routes_pick_what_the_converter_hears() {
        let mut t = Twelve::new(44100, &[0, 1, 0, 0, 0, 0, 99, STEREO_L, 100]);
        let mut l = vec![0.5f32];
        let mut r = vec![0.25f32];
        t.process(&mut l, &mut r);
        assert_eq!(r[0], 0.25);
        assert_eq!(l[0], 0.0); // the clock has not ticked yet: the converter holds silence
        let mut t = Twelve::new(44100, &[0, 1, 0, 0, 0, 0, 99, SIDE, 100]);
        let mut l = vec![0.3f32];
        let mut r = vec![0.3f32];
        t.process(&mut l, &mut r);
        assert_eq!([l[0], r[0]], [0.3, 0.3]); // a mono signal has no side to convert
        let mut t = Twelve::new(44100, &[0, 1, 0, 0, 0, 0, 99, MONO_SUM, 100]);
        let mut l = vec![0.3f32];
        let mut r = vec![-0.1f32];
        t.process(&mut l, &mut r);
        assert_eq!(l[0], r[0]); // the converted sum feeds both channels
    }

    /// Mix 0 is the dry signal, whatever the rest of the settings do.
    #[test]
    fn mix_zero_is_the_dry_signal() {
        let mut t = Twelve::new(44100, &[120, 0, 1, 100, -90, 1, 12, STEREO, 0]);
        let mut l: Vec<f32> = (0..64).map(|i| (i as f32 * 0.37).sin().clamp(-0.9, 0.9)).collect();
        let mut r: Vec<f32> = (0..64).map(|i| (i as f32 * 0.11).sin()).collect();
        let (l0, r0) = (l.clone(), r.clone());
        t.process(&mut l, &mut r);
        assert_eq!(l, l0);
        assert_eq!(r, r0);
    }

    /// Linked gains: raising the input lowers the output by the same amount, so the total stays at the gain out.
    #[test]
    fn link_holds_the_total_gain() {
        let linked = Twelve::new(44100, &[0, 1, 0, 60, -60, 1, 99, STEREO, 100]);
        let free = Twelve::new(44100, &[0, 1, 0, 60, -60, 0, 99, STEREO, 100]);
        assert!((linked.out_gain - 10f32.powf(-0.6)).abs() < 1e-5); // -12 dB: the total is the -6 dB out
        assert!((free.out_gain - 10f32.powf(-0.3)).abs() < 1e-5); // -6 dB
        assert!(free.out_gain > linked.out_gain * 1.9);
    }

    /// The filter: 100 Hz leaves a 5 kHz line in the quantisation noise; bypassed, it rides the converter.
    #[test]
    fn the_filter_tames_the_highs() {
        let (n, rate) = (44100usize, 44100u32);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let mut t = Twelve::new(rate, &[0, 1, 0, 0, 0, 0, 0, STEREO, 100]);
        let mut x: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 5000.0 * i as f32 / rate as f32).sin()).collect();
        let mut r = x.clone();
        t.process(&mut x, &mut r);
        assert!(rms(&x) < 0.01, "{}", rms(&x));
        let mut u = Twelve::new(rate, &[0, 1, 0, 0, 0, 0, 99, STEREO, 100]);
        let mut y: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 5000.0 * i as f32 / rate as f32).sin()).collect();
        let mut r = y.clone();
        u.process(&mut y, &mut r);
        assert!(rms(&y) > 0.3, "{}", rms(&y));
    }

    /// The output stage grips when the gain pushes the converter past full scale, and stays under it.
    #[test]
    fn the_output_stage_holds_the_peaks() {
        let mut t = Twelve::new(44100, &[0, 1, 0, 120, 0, 0, 99, STEREO, 100]);
        let mut l: Vec<f32> = (0..4410).map(|i| 0.9 * (std::f32::consts::TAU * 110.0 * i as f32 / 44100.0).sin()).collect();
        let mut r = l.clone();
        t.process(&mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        assert!(peak > 0.7 && peak < 1.0, "{peak}");
    }

    /// While the gains are linked the out knob shows the effective output gain (the stored out less the in, what
    /// the output stage runs at); free, it and every other setting show their stored values.
    #[test]
    fn the_linked_out_knob_shows_the_effective_output_gain() {
        let v = |link: i32, i: i32, o: i32| {
            let mut v = SPEC.start();
            v[LINK] = link;
            v[IN] = i;
            v[OUT] = o;
            v
        };
        assert_eq!(shown(&v(1, 60, 0), OUT), Some(60)); // the offset is the in: the knob shows 0 - 60 = -6 dB
        assert_eq!(shown(&v(0, 60, 0), OUT), None); // free: the stored value
        assert_eq!(shown(&v(1, 60, 0), IN), None); // the in knob is the drive, never shown otherwise
        assert_eq!(shown(&v(1, 120, -30), OUT), Some(120)); // the shown value clamps to the knob's -12 dB...
        let t = Twelve::new(44100, &v(1, 120, -30)); // ...as the sound's output stage clamps the same way
        assert!((t.out_gain - 10f32.powf(-0.6)).abs() < 1e-5);
    }

    /// The 12 bits: round to the nearest of 4096 levels, and full scale is the ceiling.
    #[test]
    fn the_twelve_bits_round_and_clip() {
        assert_eq!(quantize(0.0003), 1.0 / 2048.0);
        assert_eq!(quantize(-0.0003), -1.0 / 2048.0);
        assert_eq!(quantize(0.0001), 0.0);
        assert_eq!(quantize(3.0), 2047.0 / 2048.0);
        assert_eq!(quantize(-3.0), -1.0);
    }
}
