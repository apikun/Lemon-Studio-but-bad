// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Soundfont Player's editor: a child of the host's window (Win32, X11 on Linux, an NSView on macOS) that paints
//! `view` with the os-ui canvas, and the editor logic (platform independent): knob and fader drags with the cursor held
//! in place, wheel and reset gestures, the preset list, the soundfont's open dialog and the options menu. Edits show and
//! sound at once (`os_plugkit::vals`).
// spec: plugins/soundfont-player.md (§Controls, §Interaction), plugins/host-link.md

use std::any::Any;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_plugkit::vals::Vals;
use os_plugkit::MenuDef;
use os_ui::kit::menu::{MenuItem, MenuStyle};

use crate::params::{self, SfParams};
use crate::view::{self, Target, ViewState};
use crate::Shared;

pub struct SfEditor {
    pub params: Arc<SfParams>,
    pub shared: Arc<Shared>,
    /// DPI scale as f32 bits.
    pub scale: AtomicU32,
    /// The open editor window (0 = closed): parameter changes post a repaint to it.
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: Arc<AtomicIsize>,
    /// Linux and macOS: the open editor's thread waker, for the same.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    slot: os_plugkit::gui::Slot,
}

impl SfEditor {
    pub fn new(params: Arc<SfParams>, shared: Arc<Shared>) -> Self {
        SfEditor {
            params,
            shared,
            scale: AtomicU32::new(1f32.to_bits()),
            hwnd: Arc::new(AtomicIsize::new(0)),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            slot: Default::default(),
        }
    }

    fn notify(&self) {
        #[cfg(windows)]
        win32::post_changed(self.hwnd.load(Ordering::Acquire));
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        os_plugkit::gui::notify(&self.slot);
    }
}

impl Editor for SfEditor {
    fn spawn(&self, parent: ParentWindowHandle, context: Arc<dyn GuiContext>) -> Box<dyn Any + Send> {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        match parent {
            #[cfg(windows)]
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                Box::new(win32::open(hwnd, scale, self.params.clone(), self.shared.clone(), context, self.hwnd.clone()))
            }
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(window) => {
                Box::new(gui::open(window, scale, self.params.clone(), self.shared.clone(), context, &self.slot))
            }
            // macOS hosts set no scale (sizes are points): the editor paints at its window's backing scale.
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(view) => {
                let scale = os_plugkit::gui::backing_scale(view as usize);
                Box::new(gui::open(view as usize, scale, self.params.clone(), self.shared.clone(), context, &self.slot))
            }
            // Elsewhere no editor opens yet.
            _ => {
                let _ = (context, scale);
                Box::new(())
            }
        }
    }

    fn size(&self) -> (u32, u32) {
        (view::W, view::H)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        self.scale.store(factor.to_bits(), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        self.notify();
    }
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {
        self.notify();
    }
    fn param_values_changed(&self) {
        self.notify();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Laws (as the small effects')

/// Pixels of drag for a knob's whole range.
pub const KNOB_PX: f32 = 200.0;
const FINE: f32 = 0.25;

// ---------------------------------------------------------------------------------------------------------------
// Menus

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    None,
    Reset(String),
    CopyValue(String),
    PasteValue(String),
    TypeIn(String),
    Automate(String),
    LoadFont,
    Preset(usize),
    ResetControls,
    Info,
    About,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuReq {
    Param(String),
    Patch,
    Gear,
    Info,
    About,
}

// ---------------------------------------------------------------------------------------------------------------
// Editor logic

/// Something the window has to do after an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Repaint,
    /// Hide the cursor and hold it in place (knob drags).
    Lock,
    Menu(MenuReq, f32, f32),
    TypeIn(String, f32, f32),
    /// Ask the host for an automation clip of parameter `index` (host link).
    Automate(usize),
    /// Open the soundfont's file dialog.
    OpenDialog,
    /// Load the soundfont at a path (the dialog's answer, or a drop).
    Load(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Btn {
    Left,
    Right,
    Middle,
}

/// A drag: `v` = the unrounded normalised value it has reached, over `px` for the whole range.
#[derive(Clone, Debug)]
struct Drag {
    id: String,
    v: f32,
    px: f32,
}

pub struct Logic {
    pub params: Arc<SfParams>,
    pub shared: Arc<Shared>,
    pub vals: Vals,
    pub view: ViewState,
    pub hits: Vec<view::Hit>,
    drag: Option<Drag>,
    /// Hint text.
    pub hint: String,
    copied: Option<f32>,
    pub hosted: bool,
}

impl Logic {
    pub fn new(params: Arc<SfParams>, shared: Arc<Shared>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        let vals = Vals::new(&*params, params.live.clone(), ctx);
        Logic { params, shared, vals, view: ViewState::default(), hits: Vec::new(), drag: None, hint: String::new(), copied: None, hosted: false }
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn hit(&self, x: f32, y: f32) -> Option<(Target, os_ui::Rect)> {
        self.hits.iter().rev().find(|h| h.r.contains(x, y)).map(|h| (h.t.clone(), h.r))
    }

    fn hint_for(&self, t: Option<&Target>) -> String {
        match t {
            Some(t) => view::hint(t, &self.vals, &self.shared.font_name()),
            None => String::new(),
        }
    }

    fn edit(&mut self, id: &str, norm: f32) {
        self.vals.edit(id, norm);
    }

    fn reset(&mut self, id: &str) {
        let d = self.vals.default_norm(id);
        self.edit(id, d);
    }

    fn begin_drag(&mut self, id: &str, px: f32, t: Target) -> Vec<Act> {
        self.vals.begin(id);
        self.drag = Some(Drag { id: id.into(), v: self.vals.norm(id), px });
        self.view.pressed = Some(t);
        vec![Act::Repaint, Act::Lock]
    }

    /// The preset played now, as the editor shows it (its pending edit over the host's value).
    fn preset(&self) -> usize {
        self.vals.plain("preset").max(0.0) as usize
    }

    /// The preset count of the loaded soundfont (0 without one).
    fn preset_count(&self) -> usize {
        self.shared.preset_count()
    }

    /// Switches the preset (the arrows, the patch list): the sound follows the edit at once, and the controls take the
    /// preset's values, as the original's do at a load.
    fn switch_preset(&mut self, idx: usize) -> Vec<Act> {
        let count = self.preset_count();
        if count == 0 {
            return vec![Act::Repaint];
        }
        let idx = idx.min(count - 1);
        self.vals.edit_plain("preset", idx as f32);
        self.sync_from_preset(idx);
        self.hint = self.hint_for(Some(&Target::Patch));
        vec![Act::Repaint]
    }

    /// Moves the envelope 2 / LFO 2 / Misc controls to a preset's values (its first zone's).
    pub fn sync_from_preset(&mut self, idx: usize) {
        let Some(font) = self.shared.font() else { return };
        let Some(r) = crate::dsp::regions(&font, idx).first().cloned() else { return };
        let put = |l: &mut Self, id: &str, v: f32| {
            if (l.vals.norm(id) - v).abs() > 1e-4 {
                l.edit(id, v);
            }
        };
        put(self, "env_a", params::env_norm(crate::dsp::tc_seconds(r.mod_env.attack)));
        put(self, "env_d", params::env_norm(crate::dsp::tc_seconds(r.mod_env.decay)));
        put(self, "env_s", f64::from(r.mod_env.sustain.clamp(0, 1000)) as f32 / 1000.0);
        put(self, "env_r", params::env_norm(crate::dsp::tc_seconds(r.mod_env.release)));
        put(self, "lfo_del", params::lfo_delay_norm(crate::dsp::tc_seconds(r.mod_lfo.delay)));
        put(self, "lfo_amt", params::lfo_amt_norm(f64::from(r.mod_lfo.to_pitch)));
        put(self, "lfo_spd", params::lfo_spd_norm(crate::dsp::tc_seconds(r.mod_lfo.freq)));
        put(self, "cut", params::cut_norm(r.filter_fc));
        if r.mod_env_to_filter != 0 {
            put(self, "mod", params::mod_norm(f64::from(r.mod_env_to_filter)));
        }
    }

    /// Loads a soundfont (the editor thread): the file parses, the state keeps its path, the first preset plays.
    pub fn load(&mut self, path: &str) -> Vec<Act> {
        if path.is_empty() {
            return vec![Act::Repaint];
        }
        match self.shared.load(path) {
            Ok(()) => {
                if let Ok(mut p) = self.params.sf2_path.write() {
                    *p = path.to_string();
                }
                self.vals.edit_plain("preset", 0.0);
                self.sync_from_preset(0);
                self.hint = format!("Loaded {}: {} presets", self.shared.font_name(), self.preset_count());
                vec![Act::Repaint]
            }
            Err(e) => {
                self.hint = format!("Not loaded: {e}");
                vec![Act::Repaint]
            }
        }
    }

    pub fn down(&mut self, x: f32, y: f32, btn: Btn, m: Mods, _dbl: bool) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        let Some((t, _)) = self.hit(x, y) else { return vec![] };
        let id = t.id().to_string();
        let mut acts = vec![Act::Repaint];
        match (&t, btn) {
            (Target::Logo, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::About, x, y)),
            (Target::Button("load"), Btn::Left) => acts.push(Act::OpenDialog),
            (Target::Button("info"), Btn::Left) => acts.push(Act::Menu(MenuReq::Info, x, y)),
            (Target::Button("gear"), Btn::Left) => acts.push(Act::Menu(MenuReq::Gear, x, y)),
            (Target::Button("prev"), Btn::Left) => {
                let p = self.preset();
                return self.switch_preset(p.saturating_sub(1));
            }
            (Target::Button("next"), Btn::Left) => {
                let p = self.preset();
                return self.switch_preset((p + 1).min(self.preset_count().max(1) - 1));
            }
            (Target::Patch, Btn::Left | Btn::Right) => acts.push(Act::Menu(MenuReq::Patch, x, y)),
            (_, Btn::Left) if m.alt => self.reset(&id),
            (_, Btn::Middle) => self.reset(&id),
            (_, Btn::Right) if !id.is_empty() => acts.push(Act::Menu(MenuReq::Param(id), x, y)),
            (Target::Knob(_), Btn::Left) => acts = self.begin_drag(&id, KNOB_PX, t.clone()),
            (Target::Fader(_, px), Btn::Left) => acts = self.begin_drag(&id, *px, t.clone()),
            (Target::Switch(_), Btn::Left) => {
                let v = self.vals.plain(&id);
                self.edit(&id, if v > 0.5 { 0.0 } else { 1.0 });
            }
            _ => {}
        }
        self.hint = self.hint_for(Some(&t));
        acts
    }

    /// Mouse movement during a locked drag (logical px since the last event).
    pub fn delta(&mut self, _dx: f32, dy: f32, m: Mods) -> Vec<Act> {
        let Some(Drag { id, v, px }) = self.drag.clone() else { return vec![] };
        let f = if m.ctrl { FINE } else { 1.0 };
        let v = (v - dy * f / px).clamp(0.0, 1.0);
        self.vals.set(&id, v);
        self.drag = Some(Drag { id, v, px });
        let t = self.view.pressed.clone();
        self.hint = self.hint_for(t.as_ref());
        vec![Act::Repaint]
    }

    pub fn up(&mut self, _btn: Btn) -> Vec<Act> {
        let Some(d) = self.drag.take() else { return vec![] };
        self.vals.end(&d.id);
        self.view.pressed = None;
        vec![Act::Repaint]
    }

    pub fn moved(&mut self, x: f32, y: f32, _m: Mods) -> Vec<Act> {
        self.view.mouse = Some((x, y));
        if self.drag.is_none() {
            let t = self.hit(x, y).map(|h| h.0);
            self.hint = self.hint_for(t.as_ref());
        }
        vec![Act::Repaint]
    }

    pub fn leave(&mut self) -> Vec<Act> {
        self.view.mouse = None;
        self.hint.clear();
        vec![Act::Repaint]
    }

    /// Wheel: 1/128 of the range per notch on the knobs and faders.
    pub fn wheel(&mut self, x: f32, y: f32, notches: f32, _m: Mods) -> Vec<Act> {
        let Some((t, _)) = self.hit(x, y).filter(|(t, _)| matches!(t, Target::Knob(_) | Target::Fader(..))) else { return vec![] };
        let id = t.id().to_string();
        let v = (self.vals.norm(&id) + notches.signum() / 128.0).clamp(0.0, 1.0);
        self.edit(&id, v);
        self.hint = self.hint_for(Some(&t));
        vec![Act::Repaint]
    }

    pub fn menu(&self, req: &MenuReq) -> MenuDef<Cmd> {
        match req {
            MenuReq::Param(id) => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head(&view::hint(&Target::Knob(id.clone()), &self.vals, &self.shared.font_name()), Cmd::None);
                m.add(MenuItem::item("Reset"), Cmd::Reset(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Copy value"), Cmd::CopyValue(id.clone()));
                m.add(MenuItem::item("Paste value").enabled(self.copied.is_some()), Cmd::PasteValue(id.clone()));
                m.add(MenuItem::item("Enter value..."), Cmd::TypeIn(id.clone()));
                m.sep(Cmd::None);
                m.add(MenuItem::item("Create automation clip").enabled(self.hosted), Cmd::Automate(id.clone()));
                m
            }
            MenuReq::Patch => {
                let mut m = MenuDef::new(MenuStyle::Context);
                let names = self.shared.names();
                let count = names.len();
                m.head(if count > 0 { "Presets" } else { "No soundfont loaded" }, Cmd::None);
                let cur = self.preset();
                for (i, name) in names.iter().take(200).enumerate() {
                    m.add(MenuItem::item(name).enabled(true), Cmd::Preset(i));
                }
                if count > 200 {
                    m.add(MenuItem::item(&format!("... and {} more", count - 200)).enabled(false), Cmd::None);
                }
                let _ = cur;
                m
            }
            MenuReq::Gear => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Options", Cmd::None);
                m.add(MenuItem::item("Load soundfont..."), Cmd::LoadFont);
                m.add(MenuItem::item("Reset controls"), Cmd::ResetControls);
                m.sep(Cmd::None);
                m.add(MenuItem::item("About the Soundfont Player..."), Cmd::About);
                m
            }
            MenuReq::Info => {
                let mut m = MenuDef::new(MenuStyle::Context);
                let name = self.shared.font_name();
                m.head(if name.is_empty() { "No soundfont" } else { &name }, Cmd::None);
                let path = self.shared.font_path();
                if !path.is_empty() {
                    m.add(MenuItem::item(&path).enabled(false), Cmd::None);
                }
                let count = self.preset_count();
                if count > 0 {
                    m.add(MenuItem::item(&format!("{count} presets")).enabled(false), Cmd::None);
                }
                m
            }
            MenuReq::About => {
                let mut m = MenuDef::new(MenuStyle::Context);
                m.head("Soundfont Player", Cmd::None);
                m.add(MenuItem::item(&format!("Version {}", env!("CARGO_PKG_VERSION"))).enabled(false), Cmd::None);
                m.add(MenuItem::item("Plays SoundFont 2 files: Lemon Studio's player.").enabled(false), Cmd::None);
                m
            }
        }
    }

    pub fn pick(&mut self, _req: &MenuReq, cmd: Cmd, x: f32, y: f32) -> Vec<Act> {
        match cmd {
            Cmd::None => {}
            Cmd::Reset(id) => self.reset(&id),
            Cmd::CopyValue(id) => self.copied = Some(self.vals.norm(&id)),
            Cmd::PasteValue(id) => {
                if let Some(v) = self.copied {
                    self.edit(&id, v);
                }
            }
            Cmd::TypeIn(id) => return vec![Act::TypeIn(id, x, y)],
            Cmd::Automate(id) => {
                if let Some(i) = params::param_index(&id) {
                    return vec![Act::Automate(i)];
                }
            }
            Cmd::LoadFont => return vec![Act::OpenDialog],
            Cmd::Preset(i) => return self.switch_preset(i),
            Cmd::ResetControls => {
                for id in ["rev_level", "cho_level", "env_a", "env_d", "env_s", "env_r", "lfo_del", "lfo_amt", "lfo_spd", "cut", "mod"] {
                    self.reset(id);
                }
            }
            Cmd::Info => return vec![Act::Menu(MenuReq::Info, x, y)],
            Cmd::About => return vec![Act::Menu(MenuReq::About, x, y)],
        }
        vec![Act::Repaint]
    }

    /// The value text a type-in box starts with.
    pub fn value_text(&self, id: &str) -> String {
        self.vals.text(id)
    }

    /// Applies a typed value.
    pub fn typed(&mut self, id: &str, text: &str) {
        if let Some(n) = self.vals.parse(id, text) {
            self.edit(id, n);
            if id == "preset" {
                let idx = n * 4095.0;
                self.sync_from_preset(idx.round() as usize);
            }
        }
    }

    /// Paints the editor; `hint` is drawn when the host does not show it.
    pub fn paint(&mut self, c: &mut os_ui::Canvas, palette: &os_theme::Palette) {
        let hint = if self.hosted { String::new() } else { self.hint.clone() };
        let info = view::Info {
            font_name: self.shared.font_name(),
            patch: self.shared.patch_text(self.preset()),
            rev_send: self.vals.plain("rev_send").round().to_string(),
            cho_send: self.vals.plain("cho_send").round().to_string(),
        };
        self.hits = view::paint(c, palette, &self.vals, &self.view, &info, &hint);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The window

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_plugkit::win::{self as pw, CursorLock};
    use os_theme::Palette;
    use os_ui::Canvas;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::InvalidateRect;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OPENFILENAMEW};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::SfParams;
    use crate::Shared;

    const TIMER: usize = 1;
    const WM_CHANGED: u32 = WM_APP + 1;

    struct Win {
        hwnd: HWND,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        /// Painted size in device px.
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        last_hint: String,
        lock: CursorLock,
        tracking: bool,
        slot: Arc<AtomicIsize>,
    }

    /// Owns the window; dropping it closes the editor.
    pub struct Handle(isize, Arc<AtomicIsize>);

    // SAFETY: the handle is only an HWND value; the wrapper drops it on the GUI thread.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            self.1.store(0, Ordering::Release);
            unsafe { DestroyWindow(self.0 as HWND) };
        }
    }

    pub fn post_changed(hwnd: isize) {
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, WM_CHANGED, 0, 0) };
        }
    }

    fn mods() -> Mods {
        let (ctrl, shift, alt) = pw::mods();
        Mods { ctrl, shift, alt }
    }

    pub fn open(parent: *mut c_void, scale: f32, params: Arc<SfParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: Arc<AtomicIsize>) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let px = |v: u32| (v as f32 * scale).round() as u32;
        let size = (px(crate::view::W), px(crate::view::H));
        unsafe {
            // One window procedure per module instance: the class carries its address.
            let class = pw::register("SoundfontPlayerView", Some(wndproc));
            let win = Box::new(Win {
                hwnd: std::ptr::null_mut(),
                canvas: Canvas::new(size.0, size.1, scale).expect("canvas"),
                buf: vec![0; (size.0 * size.1) as usize],
                scale,
                size,
                logic: Logic::new(params, shared, Some(ctx)),
                palette: os_plugkit::palette(),
                last_hint: String::new(),
                lock: CursorLock::default(),
                tracking: false,
                slot: slot.clone(),
            });
            let title = pw::wide("Soundfont Player");
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                0,
                0,
                size.0 as i32,
                size.1 as i32,
                parent as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                Box::into_raw(win) as *const c_void,
            );
            slot.store(hwnd as isize, Ordering::Release);
            // About 33 checks a second (edits the host confirms, the hint).
            SetTimer(hwnd, TIMER, 30, None);
            Handle(hwnd as isize, slot)
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static mut Win> {
        let p = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Win;
        if p.is_null() { None } else { Some(unsafe { &mut *p }) }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe {
            if msg == WM_NCCREATE {
                let cs = lp as *const CREATESTRUCTW;
                let w = (*cs).lpCreateParams as *mut Win;
                (*w).hwnd = hwnd;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, w as isize);
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if msg == WM_NCDESTROY {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
                if !p.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    (*p).slot.store(0, Ordering::Release);
                    drop(Box::from_raw(p));
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let Some(w) = state(hwnd) else { return DefWindowProcW(hwnd, msg, wp, lp) };
            if msg == os_hostlink::message() {
                return w.host_link(wp, lp);
            }
            match msg {
                WM_PAINT => {
                    w.paint();
                    0
                }
                WM_ERASEBKGND => 1,
                WM_TIMER | WM_CHANGED => {
                    w.poll();
                    0
                }
                WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONDOWN | WM_RBUTTONDBLCLK | WM_MBUTTONDOWN => {
                    SetFocus(hwnd);
                    let (x, y) = w.mouse(lp);
                    let btn = match msg {
                        WM_RBUTTONDOWN | WM_RBUTTONDBLCLK => Btn::Right,
                        WM_MBUTTONDOWN => Btn::Middle,
                        _ => Btn::Left,
                    };
                    let acts = w.logic.down(x, y, btn, mods(), msg == WM_LBUTTONDBLCLK);
                    if w.logic.dragging() && !acts.contains(&Act::Lock) {
                        SetCapture(hwnd);
                    }
                    w.run(acts);
                    0
                }
                WM_MOUSEMOVE => {
                    if !w.tracking {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                        TrackMouseEvent(&mut tme);
                        w.tracking = true;
                    }
                    if let Some((dx, dy)) = w.lock.delta() {
                        if dx != 0 || dy != 0 {
                            let acts = w.logic.delta(dx as f32 / w.scale, dy as f32 / w.scale, mods());
                            w.run(acts);
                        }
                        return 0;
                    }
                    let (x, y) = w.mouse(lp);
                    let acts = w.logic.moved(x, y, mods());
                    w.run(acts);
                    0
                }
                WM_MOUSELEAVE => {
                    w.tracking = false;
                    if !w.lock.locked() && !w.logic.dragging() {
                        let acts = w.logic.leave();
                        w.run(acts);
                    }
                    0
                }
                WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                    w.lock.unlock();
                    ReleaseCapture();
                    let acts = w.logic.up(Btn::Left);
                    w.run(acts);
                    0
                }
                WM_CAPTURECHANGED => {
                    // Another window took the mouse (not this one capturing again): end the drag.
                    if w.logic.dragging() && lp as HWND != hwnd {
                        w.lock.unlock();
                        let acts = w.logic.up(Btn::Left);
                        w.run(acts);
                    }
                    0
                }
                WM_MOUSEWHEEL => {
                    // Wheel positions are screen coordinates.
                    let mut r: RECT = std::mem::zeroed();
                    GetWindowRect(hwnd, &mut r);
                    let (sx, sy) = pw::xy(lp);
                    let (x, y) = ((sx - r.left) as f32 / w.scale, (sy - r.top) as f32 / w.scale);
                    let notches = ((wp >> 16) & 0xffff) as u16 as i16 as f32 / 120.0;
                    let acts = w.logic.wheel(x, y, notches, mods());
                    w.run(acts);
                    0
                }
                WM_DESTROY => {
                    KillTimer(hwnd, TIMER);
                    w.lock.unlock();
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }
    }

    impl Win {
        fn mouse(&self, l: LPARAM) -> (f32, f32) {
            let (x, y) = pw::xy(l);
            (x as f32 / self.scale, y as f32 / self.scale)
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            pw::to_screen(self.hwnd, (x * self.scale) as i32, (y * self.scale) as i32)
        }

        fn invalidate(&self) {
            unsafe { InvalidateRect(self.hwnd, std::ptr::null(), 0) };
        }

        /// Timer / parameter change: expire confirmed overlay values and repaint.
        fn poll(&mut self) {
            self.logic.vals.expire();
            self.invalidate();
            if self.logic.hint != self.last_hint {
                self.last_hint = self.logic.hint.clone();
                if self.logic.hosted {
                    os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Changed, 0);
                }
            }
        }

        /// The open soundfont dialog: Windows' own.
        fn open_dialog(&self) -> Option<String> {
            if os_plugkit::no_modal() {
                return None;
            }
            unsafe {
                let filter: Vec<u16> = "SoundFont 2 files\0*.sf2\0All files\0*.*\0\0".encode_utf16().collect();
                let mut file = [0u16; 512];
                let mut ofn: OPENFILENAMEW = std::mem::zeroed();
                ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
                ofn.hwndOwner = self.hwnd;
                ofn.lpstrFilter = filter.as_ptr();
                ofn.lpstrFile = file.as_mut_ptr();
                ofn.nMaxFile = 512;
                ofn.Flags = OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR;
                if GetOpenFileNameW(&mut ofn) == 0 {
                    return None;
                }
                let end = file.iter().position(|&c| c == 0).unwrap_or(0);
                Some(String::from_utf16_lossy(&file[..end]))
            }
        }

        fn run(&mut self, acts: Vec<Act>) {
            let mut repaint = false;
            for a in acts {
                match a {
                    Act::Repaint => repaint = true,
                    Act::Lock => self.lock.lock(self.hwnd),
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = pw::popup(self.hwnd, sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::TypeIn(id, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&id);
                        if let Some(t) = pw::type_in(self.hwnd, sx, sy, &cur, self.scale) {
                            self.logic.typed(&id, &t);
                        }
                        repaint = true;
                    }
                    Act::Automate(i) => {
                        os_hostlink::to_host(self.hwnd as isize, os_hostlink::Cmd::Automate, i as isize);
                    }
                    Act::OpenDialog => {
                        if let Some(path) = self.open_dialog() {
                            let acts = self.logic.load(&path);
                            self.run(acts);
                        }
                        repaint = true;
                    }
                    Act::Load(path) => {
                        let acts = self.logic.load(&path);
                        self.run(acts);
                    }
                }
            }
            if repaint {
                self.invalidate();
            }
        }

        /// Answers the host (spec `plugins/host-link.md`): one page, the hint line. Unknown commands return 0.
        unsafe fn host_link(&mut self, wp: WPARAM, lp: LPARAM) -> LRESULT {
            use os_hostlink::{Cmd, PageIcon};
            let Some(cmd) = Cmd::from_usize(wp) else { return 0 };
            match cmd {
                Cmd::Hello => {
                    self.logic.hosted = true;
                    self.invalidate();
                    os_hostlink::VERSION as LRESULT
                }
                Cmd::PageCount => 1,
                Cmd::PageIcon => PageIcon::Plugin as LRESULT,
                Cmd::GetPage | Cmd::SetPage => 0,
                Cmd::Metrics => os_hostlink::pack_size(crate::view::W, crate::view::H),
                Cmd::Hint => {
                    if lp != 0 {
                        // SAFETY: the host passes a `HintBuf` of its own process (spec: host-link.md).
                        unsafe { &mut *(lp as *mut os_hostlink::HintBuf) }.set(&self.logic.hint);
                    }
                    1
                }
                Cmd::Changed | Cmd::Note | Cmd::Automate | Cmd::Audio | Cmd::Resizable | Cmd::SetSize | Cmd::Notes => 0,
            }
        }

        fn paint(&mut self) {
            let (hdc, ps) = pw::begin_paint(self.hwnd);
            self.logic.paint(&mut self.canvas, &self.palette);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            pw::blit(hdc, &self.buf, self.size.0, self.size.1);
            pw::end_paint(self.hwnd, &ps);
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod gui {
    use std::sync::Arc;

    use nih_plug::prelude::GuiContext;
    use os_hostlink::{Cmd, PageIcon, Peer};
    use os_plugkit::gui::{self as px, Button, Input, Window};
    use os_theme::Palette;
    use os_ui::Canvas;

    use super::{Act, Btn, Logic, Mods};
    use crate::params::SfParams;
    use crate::Shared;

    /// The editor on its GUI thread.
    struct Ed {
        win: Window,
        canvas: Canvas,
        buf: Vec<u32>,
        scale: f32,
        size: (u32, u32),
        logic: Logic,
        palette: Palette,
        last_hint: String,
        dirty: bool,
        /// The host link: the host finds the editor by the window it gave it (spec: plugins/host-link.md).
        link: Arc<Peer>,
    }

    impl Drop for Ed {
        /// The editor closes: the host no longer finds it.
        fn drop(&mut self) {
            self.link.unregister();
        }
    }

    /// Owns the editor thread; dropping it closes the editor.
    pub struct Handle(Option<px::EditorThread>);

    impl Drop for Handle {
        fn drop(&mut self) {
            self.0.take();
        }
    }

    pub fn open(parent: px::Parent, scale: f32, params: Arc<SfParams>, shared: Arc<Shared>, ctx: Arc<dyn GuiContext>, slot: &px::Slot) -> Handle {
        let scale = if scale > 0.0 { scale } else { 1.0 };
        let size = ((crate::view::W as f32 * scale).round() as u32, (crate::view::H as f32 * scale).round() as u32);
        // One page; the editor keeps its size.
        let wake = slot.clone();
        let link = Peer::register(parent as u64, vec![PageIcon::Plugin], (crate::view::W, crate::view::H), move || px::notify(&wake));
        let peer = link.clone();
        let thread = px::spawn(parent, size, slot, move |win| {
            let Ok(canvas) = Canvas::new(size.0, size.1, scale) else { return };
            let logic = Logic::new(params, shared, Some(ctx));
            let ed = Ed { win, canvas, buf: Vec::new(), scale, size, logic, palette: os_plugkit::palette(), last_hint: String::new(), dirty: true, link: peer };
            ed.run();
        });
        if thread.is_none() {
            link.unregister();
        }
        Handle(thread)
    }

    fn btn(b: Button) -> Btn {
        match b {
            Button::Left => Btn::Left,
            Button::Right => Btn::Right,
            Button::Middle => Btn::Middle,
        }
    }

    impl Ed {
        fn run(mut self) {
            while let Some(input) = self.win.next() {
                match input {
                    Input::Expose => self.dirty = true,
                    Input::Tick => self.poll(),
                    Input::Down { x, y, btn: b, dbl } => {
                        let (x, y) = (x / self.scale, y / self.scale);
                        let acts = self.logic.down(x, y, btn(b), self.mods(), dbl && b == Button::Left);
                        self.act(acts);
                    }
                    Input::Move { x, y } => {
                        let acts = self.logic.moved(x / self.scale, y / self.scale, self.mods());
                        self.act(acts);
                    }
                    Input::Delta { dx, dy } => {
                        let acts = self.logic.delta(dx / self.scale, dy / self.scale, self.mods());
                        self.act(acts);
                    }
                    Input::Up { .. } => {
                        self.win.unlock();
                        let acts = self.logic.up(Btn::Left);
                        self.act(acts);
                    }
                    Input::Leave => {
                        if !self.win.locked() && !self.logic.dragging() {
                            let acts = self.logic.leave();
                            self.act(acts);
                        }
                    }
                    Input::Wheel { x, y, notches } => {
                        let acts = self.logic.wheel(x / self.scale, y / self.scale, notches, self.mods());
                        self.act(acts);
                    }
                    Input::Key { .. } => {}
                    // A soundfont dropped on the editor (macOS so far).
                    Input::Drop { path } => self.act(vec![Act::Load(path)]),
                }
                if self.dirty && self.win.idle() {
                    self.paint();
                }
            }
        }

        fn mods(&self) -> Mods {
            let (ctrl, shift, alt) = self.win.mods();
            Mods { ctrl, shift, alt }
        }

        fn screen(&self, x: f32, y: f32) -> (i32, i32) {
            self.win.to_root((x * self.scale) as i32, (y * self.scale) as i32)
        }

        /// Timer / parameter change: expire confirmed overlay values and repaint.
        fn poll(&mut self) {
            self.logic.vals.expire();
            self.dirty = true;
            if self.logic.hint != self.last_hint {
                self.last_hint.clone_from(&self.logic.hint);
                // The host shows the hint in its title bar (hosted mode).
                self.link.set_hint(&self.logic.hint);
                self.link.to_host(Cmd::Changed, 0);
            }
            // Joined the strip: "Create automation clip" works and the host shows the hint.
            self.logic.hosted = self.link.hosted();
        }

        fn act(&mut self, acts: Vec<Act>) {
            for a in acts {
                match a {
                    Act::Repaint => self.dirty = true,
                    Act::Lock => self.win.lock(),
                    Act::Menu(req, x, y) => {
                        let def = self.logic.menu(&req);
                        let (sx, sy) = self.screen(x, y);
                        if let Some(cmd) = self.win.popup(sx, sy, &def, &self.palette, self.scale) {
                            let acts = self.logic.pick(&req, cmd, x, y);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::TypeIn(id, x, y) => {
                        let (sx, sy) = self.screen(x, y);
                        let cur = self.logic.value_text(&id);
                        if let Some(t) = self.win.type_in(sx, sy, &cur, self.scale) {
                            self.logic.typed(&id, &t);
                        }
                        self.dirty = true;
                    }
                    // The host makes the clip; the index is the parameter's list position.
                    Act::Automate(i) => {
                        self.link.to_host(Cmd::Automate, i as isize);
                    }
                    Act::OpenDialog => {
                        if let Some(path) = self.file_dialog() {
                            let acts = self.logic.load(&path);
                            self.act(acts);
                        }
                        self.dirty = true;
                    }
                    Act::Load(path) => {
                        let acts = self.logic.load(&path);
                        self.act(acts);
                    }
                }
            }
        }

        /// The open soundfont dialog: the desktop's own (through its portal on Linux; on macOS AppKit's, run on the
        /// main thread).
        fn file_dialog(&mut self) -> Option<String> {
            if os_plugkit::no_modal() {
                return None;
            }
            let path = self.win.dialog(move || {
                rfd::FileDialog::new().set_title("Open soundfont").add_filter("SoundFont 2 files", &["sf2"]).pick_file()
            });
            path.flatten().map(|p| p.to_string_lossy().into_owned())
        }

        fn paint(&mut self) {
            self.dirty = false;
            self.logic.paint(&mut self.canvas, &self.palette);
            self.buf.resize((self.size.0 * self.size.1) as usize, 0);
            self.canvas.copy_to_xrgb(&mut self.buf);
            self.win.blit(&self.buf, self.size.0, self.size.1);
        }
    }
}
