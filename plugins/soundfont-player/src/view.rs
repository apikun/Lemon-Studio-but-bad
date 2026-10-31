// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Soundfont Player's editor: painting and hit regions (platform independent), after the original's look — a top
//! bar with the soundfont's name, the patch row with its previous / next arrows, outlined panels for the reverb and
//! chorus sends, the envelope's faders, the LFO's and misc dials, the options gear and the wordmark.
// spec: plugins/soundfont-player.md (§Look)

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::inst;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{self, State};
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

/// Painted size in logical pixels.
pub const W: u32 = 560;
pub const H: u32 = 501;

const LABEL: Font = inst::LABEL;
const CAP: Font = inst::CAPTION;
const HEAD: Font = Font::new(10).bold();
const WORD: Font = Font::new(19).bold();
const NAME: Font = Font::new(12);

/// The tag icon of the top bar (the kit's table has none).
const TAG: tb::TablerIcon = tb::TablerIcon { filled: false, d: "M 4 4 L 10 4 L 20 14 L 14 20 L 4 10 Z M 8 8 L 8.01 8" };

/// Read access to parameter values by id.
pub trait Model {
    /// Normalised value 0..=1.
    fn norm(&self, id: &str) -> f32;
    /// Plain value (bools 0 / 1, ints as numbers).
    fn plain(&self, id: &str) -> f32;
    /// Value text.
    fn text(&self, id: &str) -> String;
}

impl Model for os_plugkit::vals::Vals {
    fn norm(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::norm(self, id)
    }
    fn plain(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::plain(self, id)
    }
    fn text(&self, id: &str) -> String {
        os_plugkit::vals::Vals::text(self, id)
    }
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel; menu on right click.
    Knob(String),
    /// As a knob, the drag over the fader's travel (px).
    Fader(String, f32),
    /// A bool switch (the BUILT-IN radios).
    Switch(String),
    /// A push button: the folder, the tag, the gear, the patch arrows.
    Button(&'static str),
    /// The patch name box: the preset list's menu.
    Patch,
    /// The wordmark (the About menu).
    Logo,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The pointer and the pressed control.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
    m: &'a dyn Model,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }

    fn state(&self, r: Rect, t: &Target) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, ..State::NORMAL }
    }

    fn panel(&mut self, r: Rect, title: &str) {
        lemon::plug_panel(self.c, r);
        let lh = self.c.line_height(HEAD);
        self.c.text(HEAD, r.x + 8.0, r.y + 6.0, brand::TEXT, title);
        let _ = lh;
    }

    /// A send panel (REVERB / CHORUS): its SEND TO number, the BUILT-IN radio and the LEVEL dial.
    fn send(&mut self, r: Rect, title: &str, level: &str, builtin: &str, send: &str, number: &str) {
        self.panel(r, title);
        // SEND TO with its number box.
        let f = Font::new(11).bold();
        let lh = self.c.line_height(LABEL);
        let lab_w = self.c.text_width(LABEL, "SEND TO");
        self.c.text(LABEL, r.x + 8.0, (r.y + 35.0 - lh / 2.0).round(), plug::DIM, "SEND TO");
        let box_r = Rect::new(r.x + 16.0 + lab_w, r.y + 26.0, 30.0, 18.0);
        lemon::field(self.c, box_r, State::NORMAL);
        self.c.text_center(f, box_r, brand::MAIN, number);
        // The LEVEL dial at the right, its caption under it.
        let (cx, cy) = (r.right() - 42.0, r.y + 52.0);
        knob_dial(self, level, cx, cy, 20.0, false);
        inst::caption(self.c, self.t, cx, cy + 30.0, "LEVEL", false);
        // BUILT-IN radio.
        let ry = r.y + 84.0;
        let target = Target::Switch(builtin.into());
        let rr = Rect::new(r.x + 6.0, ry - 4.0, 96.0, 18.0);
        let st = self.state(rr, &target);
        lemon::toggle(self.c, r.x + 14.0, ry + 4.0, self.m.plain(builtin) > 0.5, st);
        let ink = if self.m.plain(builtin) > 0.5 { brand::TEXT } else { plug::DIM };
        self.c.text(LABEL, r.x + 26.0, (ry + 4.0 - lh / 2.0).round(), ink, "BUILT-IN");
        self.hit(rr, target);
        // The send number rides in the state but has no control of its own.
        let _ = send;
    }

    /// A dial with its hit area (and the value text under it when its caption is empty).
    fn dial(&mut self, id: &str, cx: f32, cy: f32, r: f32, caption: &str, bipolar: bool) {
        knob_dial(self, id, cx, cy, r, bipolar);
        if caption.is_empty() {
            let lh = self.c.line_height(LABEL);
            let text = self.m.text(id);
            self.c.text_center(LABEL, Rect::new(cx - 50.0, cy + r + 5.0, 100.0, lh), brand::MAIN, &text);
        } else {
            inst::caption(self.c, self.t, cx, cy + r + 7.0, caption, false);
        }
    }

    /// A vertical fader with ticks beside its track, its caption under it.
    fn fader(&mut self, id: &str, x: f32, top: f32, bot: f32, caption: &str) {
        let target = Target::Fader(id.into(), bot - top);
        let hr = Rect::new(x - 11.0, top - 8.0, 22.0, bot - top + 16.0);
        let st = self.state(hr, &target);
        // The track slot and its ticks (the envelope's marks, as the original's).
        kit::eq::slot(self.c, Rect::new(x - 3.0, top - 4.0, 6.0, bot - top + 8.0), st);
        for k in 0..7 {
            let y = (bot - (bot - top) * k as f32 / 6.0).round();
            self.c.fill_rect(Rect::new(x - 10.0, y, 4.0, 1.0), plug::TICK);
            self.c.fill_rect(Rect::new(x + 6.0, y, 4.0, 1.0), plug::TICK);
        }
        let y = bot - (bot - top) * self.m.norm(id).clamp(0.0, 1.0);
        kit::eq::handle(self.c, x, y.round(), brand::MAIN, st);
        let lh = self.c.line_height(LABEL);
        self.c.text_center(LABEL, Rect::new(x - 20.0, bot + 8.0, 40.0, lh), plug::DIM, caption);
        self.hit(hr, target);
    }

    /// A push button with an icon.
    fn button(&mut self, which: &'static str, r: Rect, icon: &'static tb::TablerIcon) {
        let target = Target::Button(which);
        let st = self.state(r, &target);
        kit::icon_button(self.c, self.t, r, st, icon, 5.0);
        self.hit(r, target);
    }
}

/// A dial's face and hit area (shared by the send panels' LEVEL and the other dials).
fn knob_dial(ui: &mut Ui<'_>, id: &str, cx: f32, cy: f32, r: f32, bipolar: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0);
    let st = ui.state(hr, &target);
    lemon::dial(ui.c, cx, cy, r, if r > 19.0 { 3.2 } else { 2.8 }, ui.m.norm(id), bipolar, brand::MAIN, st);
    ui.hit(hr, target);
}

/// The patch name box and its previous / next arrows.
fn patch_row(ui: &mut Ui<'_>, text: &str) {
    let box_r = Rect::new(4.0, 42.0, 466.0, 24.0);
    let st = ui.state(box_r, &Target::Patch);
    lemon::field(ui.c, box_r, st);
    let lh = ui.c.line_height(NAME);
    ui.c.text_clip(NAME, box_r.x + 8.0, (box_r.cy() - lh / 2.0).round(), box_r.w - 16.0, brand::TEXT, text);
    ui.hit(box_r, Target::Patch);
    for (i, (which, dir)) in [("prev", kit::Dir::Left), ("next", kit::Dir::Right)].into_iter().enumerate() {
        let x = 474.0 + i as f32 * 26.0;
        let r = Rect::new(x, 42.0, 22.0, 24.0);
        let target = Target::Button(which);
        let st = ui.state(r, &target);
        lemon::plug_button(ui.c, r, 5.0, st);
        let ink = if st.hover || st.pressed { brand::TEXT } else { plug::DIM };
        kit::chevron(ui.c, r.cx(), r.cy(), 4.0, dir, ink);
        ui.hit(r, target);
    }
}

/// Paints the editor; `hint` (empty when the host shows it) rides in the patch row's spare width.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, info: &Info, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);

    // The top bar: the folder button, the soundfont's name, the tag button.
    ui.button("load", Rect::new(4.0, 4.0, 24.0, 22.0), &tb::FOLDER);
    let name_r = Rect::new(32.0, 4.0, 494.0, 22.0);
    lemon::field(ui.c, name_r, State::NORMAL);
    let lh = ui.c.line_height(NAME);
    let shown = if info.font_name.is_empty() { "Load a soundfont...".to_string() } else { info.font_name.to_string() };
    let ink = if info.font_name.is_empty() { plug::FAINT } else { brand::TEXT };
    ui.c.text_clip(NAME, name_r.x + 8.0, (name_r.cy() - lh / 2.0).round(), name_r.w - 16.0, ink, &shown);
    ui.button("info", Rect::new(530.0, 4.0, 24.0, 22.0), &TAG);

    // The patch row: PATCH NAME over the box, the arrows at its right.
    let cap_lh = ui.c.line_height(CAP);
    inst::caption_at(ui.c, 6.0, 32.0, plug::DIM, "PATCH NAME", inst::CAPTION_TRACK);
    let _ = cap_lh;
    patch_row(&mut ui, &info.patch);

    // The send panels.
    ui.send(Rect::new(4.0, 70.0, 272.0, 122.0), "REVERB", "rev_level", "rev_builtin", "rev_send", &info.rev_send);
    ui.send(Rect::new(280.0, 70.0, 276.0, 122.0), "CHORUS", "cho_level", "cho_builtin", "cho_send", &info.cho_send);

    // Envelope 2: the faders A D S R.
    let env_r = Rect::new(4.0, 196.0, 272.0, 234.0);
    ui.panel(env_r, "ENVELOPE 2");
    for (i, (id, cap)) in [("env_a", "A"), ("env_d", "D"), ("env_s", "S"), ("env_r", "R")].into_iter().enumerate() {
        ui.fader(id, 62.0 + 56.0 * i as f32, 240.0, 396.0, cap);
    }

    // LFO 2: the dials DEL AMT SPD.
    let lfo_r = Rect::new(280.0, 196.0, 276.0, 112.0);
    ui.panel(lfo_r, "LFO 2");
    for (i, (id, cap)) in [("lfo_del", "DEL"), ("lfo_amt", "AMT"), ("lfo_spd", "SPD")].into_iter().enumerate() {
        ui.dial(id, 336.0 + 62.0 * i as f32, 258.0, 17.0, cap, false);
    }

    // MISC: the dials CUT MOD.
    let misc_r = Rect::new(280.0, 312.0, 276.0, 118.0);
    ui.panel(misc_r, "MISC");
    for (i, (id, cap)) in [("cut", "CUT"), ("mod", "MOD")].into_iter().enumerate() {
        ui.dial(id, 360.0 + 76.0 * i as f32, 380.0, 17.0, cap, false);
    }

    // The gear and the wordmark.
    ui.button("gear", Rect::new(4.0, 444.0, 24.0, 22.0), &tb::SETTINGS);
    let wlh = ui.c.line_height(WORD);
    let w1 = ui.c.text_width(WORD, "SOUNDFONT");
    let w2 = ui.c.text_width(WORD, "PLAYER");
    let x1 = 552.0 - w1 - w2 - 6.0;
    ui.c.text(WORD, x1.round(), (462.0 - wlh / 2.0).round(), brand::TEXT, "SOUNDFONT");
    ui.c.text(WORD, (x1 + w1 + 6.0).round(), (462.0 - wlh / 2.0).round(), plug::WORDMARK, "PLAYER");
    let logo_r = Rect::span(x1 - 4.0, 556.0, 462.0 - wlh, 462.0 + wlh);
    ui.hit(logo_r, Target::Logo);

    // The hint (when the host does not show it) over the envelope panel's spare right edge.
    if !hint.is_empty() {
        let r = Rect::new(322.0, 332.0, 226.0, ui.c.line_height(Font::new(11)) + 8.0);
        ui.c.fill_rrect(r, 4.0, plug::WELL.with_alpha(230));
        ui.c.text_clip(Font::new(11), r.x + 6.0, r.y + 4.0, r.w - 12.0, brand::TEXT, hint);
    }
    ui.hits
}

/// What the view reads beside the parameters: the soundfont's and the patch's names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub font_name: String,
    pub patch: String,
    pub rev_send: String,
    pub cho_send: String,
}

/// The hint line for a target.
pub fn hint(t: &Target, m: &dyn Model, font_name: &str) -> String {
    fn name(id: &str) -> &str {
        match id {
            "rev_level" => "Reverb level",
            "rev_builtin" => "Reverb built-in",
            "cho_level" => "Chorus level",
            "cho_builtin" => "Chorus built-in",
            "env_a" => "Envelope 2 attack",
            "env_d" => "Envelope 2 decay",
            "env_s" => "Envelope 2 sustain",
            "env_r" => "Envelope 2 release",
            "lfo_del" => "LFO 2 delay",
            "lfo_amt" => "LFO 2 amount",
            "lfo_spd" => "LFO 2 speed",
            "cut" => "Filter cutoff",
            "mod" => "Filter modulation",
            other => other,
        }
    }
    match t {
        Target::Logo => format!("Soundfont Player{}", if font_name.is_empty() { String::new() } else { format!(": {font_name}") }),
        Target::Button("load") => "Load a soundfont".into(),
        Target::Button("info") => "The soundfont's details".into(),
        Target::Button("gear") => "Options".into(),
        Target::Button("prev") => "Previous preset".into(),
        Target::Button("next") => "Next preset".into(),
        Target::Patch => "The preset (click for the list)".into(),
        Target::Switch(id) => format!("{}: {}", name(id), if m.plain(id) > 0.5 { "On" } else { "Off" }),
        t => format!("{}: {}", name(t.id()), m.text(t.id())),
    }
}

impl Target {
    /// The setting's id ("" for the rest).
    pub fn id(&self) -> &str {
        match self {
            Target::Knob(id) | Target::Fader(id, _) | Target::Switch(id) => id,
            _ => "",
        }
    }
}

/// The dim ink of a disabled control (shared with the editor's tests).
pub fn off_ink() -> Color {
    plug::OFF
}
