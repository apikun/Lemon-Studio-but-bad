// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// Copyright (C) 2026 ravenmarkdev-alt
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin menus (the full-screen plugin picker is retired): F8, the toolbar's plugin button, the channel rack's "+",
//! Add one ▸ / Insert ▸ and the mixer slots open the app's own popup listing the installed plugins, and a pick adds
//! the plugin where that menu says.
// spec: ui/plugin-menu.md

use std::collections::BTreeMap;

use crate::app::{App, State};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::plugin_db::{self as db, Entry, MenuView, Mode};

/// What a plugin menu entry does (`Cmd::Plugin`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlugCmd {
    /// Generator `index` (into `Plugins::available`): a new channel at the bottom of the rack, or right below row
    /// `after`.
    Generator { index: usize, after: Option<usize> },
    /// Generator `index` loaded straight onto arranger track `track` (a generator dropped on the playlist's grid or a
    /// track header, the header menu's Insert instrument ▸; issue #2): the channel, and a clip of a new pattern named
    /// after the instrument on the track at `tick`.
    GeneratorOnTrack { index: usize, track: u16, tick: u32 },
    /// Generator `index` in place of rack row `row`'s (channel menu / plugin window menu > Replace ▸).
    Replace { index: usize, row: usize },
    /// Effect `index` (into `Plugins::effects`) into the selected mixer track: slot `slot`, else its first empty slot.
    Effect { index: usize, slot: Option<u8> },
    /// Find new plugins (a background scan).
    Find,
    /// Settings > Plugins.
    Manage,
    View(MenuView),
    /// The plugin browser's star on the family of plugin `index` (an effect when `effect`, else a generator).
    Star { effect: bool, index: usize },
}

/// Which plugins a menu lists and where a pick goes (spec §1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// F8, the toolbar button, VIEW / ADD > Plugin picker: generators and effects.
    All,
    /// Generators: a new channel at the bottom (`None`) or right below row `r`.
    Gen(Option<usize>),
    /// Generators in place of rack row `r`'s generator.
    Replace(usize),
    /// Effects into the selected track: slot `k`, else its first empty slot.
    Fx(Option<u8>),
}

fn pick(target: Target, effect: bool, index: usize) -> Cmd {
    Cmd::Plugin(match (target, effect) {
        (Target::Fx(slot), _) => PlugCmd::Effect { index, slot },
        (_, true) => PlugCmd::Effect { index, slot: None },
        (Target::Gen(after), false) => PlugCmd::Generator { index, after },
        (Target::Replace(row), false) => PlugCmd::Replace { index, row },
        (Target::All, false) => PlugCmd::Generator { index, after: None },
    })
}

fn item(e: &Entry, target: Target, effect: bool) -> PItem {
    PItem::new(e.label.clone(), pick(target, effect, e.index))
}

/// A header with a run-time label.
fn head(label: String) -> PItem {
    PItem { head: true, ..PItem::new(label, Cmd::Nothing) }
}

/// One section's entries (spec §2): favourites, a separator, then the rest by display mode. `nested`: the list is a
/// submenu itself, so "By vendor" uses headers instead of submenus (popups open one submenu level). `mk` names what
/// a pick of entry `e` does (`item`, or the track-menu's GeneratorOnTrack).
fn section(s: &State, effect: bool, mk: impl Fn(&Entry) -> PItem + Copy, nested: bool) -> Vec<PItem> {
    let list = if effect { &s.plugdb.lists.fx } else { &s.plugdb.lists.gens };
    if list.is_empty() {
        return vec![PItem::new(if effect { "(no effect plugins found)" } else { "(no instruments found)" }, Cmd::Nothing).off()];
    }
    let mut v: Vec<PItem> = list.iter().filter(|e| e.favorite).map(mk).collect();
    let rest: Vec<&Entry> = list.iter().filter(|e| !e.favorite).collect();
    if !v.is_empty() && !rest.is_empty() {
        v.push(PItem::sep());
    }
    match s.plugdb.prefs.menu_view {
        MenuView::Simple => v.extend(rest.iter().map(|e| mk(e))),
        MenuView::Categories => {
            for cat in db::categories(effect) {
                let in_cat: Vec<&&Entry> = rest.iter().filter(|e| e.category == cat).collect();
                if !in_cat.is_empty() {
                    v.push(PItem::head(cat));
                    v.extend(in_cat.iter().map(|e| mk(e)));
                }
            }
        }
        MenuView::Vendor => {
            let mut by: BTreeMap<String, (String, Vec<&Entry>)> = BTreeMap::new();
            for e in &rest {
                by.entry(e.vendor.to_lowercase()).or_insert_with(|| (e.vendor.clone(), Vec::new())).1.push(e);
            }
            for (_, (vendor, es)) in by {
                let items: Vec<PItem> = es.iter().map(|e| mk(e)).collect();
                if nested {
                    v.push(head(vendor));
                    v.extend(items);
                } else {
                    v.push(PItem { children: items, ..PItem::new(vendor, Cmd::Nothing) });
                }
            }
        }
    }
    v
}

/// The display modes as their radio items name them (spec §2-3); ADD's radios are the same.
pub const VIEWS: &[(&str, MenuView)] = &[("Categories", MenuView::Categories), ("Simple", MenuView::Simple), ("By vendor", MenuView::Vendor)];

/// After the lists (spec §2): Find new plugins, Manage plugins..., the display modes.
fn footer(s: &State) -> Vec<PItem> {
    let view = s.plugdb.prefs.menu_view;
    let scanning = s.plugdb.progress.is_some();
    let find = PItem::new(if scanning { "Find new plugins (scanning...)" } else { "Find new plugins" }, Cmd::Plugin(PlugCmd::Find));
    let mut v = vec![PItem::sep(), if scanning { find.off() } else { find }, PItem::new("Manage plugins...", Cmd::Plugin(PlugCmd::Manage)), PItem::sep()];
    v.extend(VIEWS.iter().map(|&(label, m)| PItem::new(label, Cmd::Plugin(PlugCmd::View(m))).mark(Mark::Radio(view == m))));
    v
}

/// The entries of a plugin menu for `target`; `nested` when they fill a submenu.
pub fn items(s: &State, target: Target, nested: bool) -> Vec<PItem> {
    let mut v = Vec::new();
    match target {
        Target::All => {
            v.push(PItem::head("Generators"));
            v.extend(section(s, false, |e| item(e, target, false), nested));
            v.push(PItem::col());
            v.push(PItem::head("Effects"));
            v.extend(section(s, true, |e| item(e, target, true), nested));
        }
        Target::Gen(_) | Target::Replace(_) => v.extend(section(s, false, |e| item(e, target, false), nested)),
        Target::Fx(_) => v.extend(section(s, true, |e| item(e, target, true), nested)),
    }
    v.extend(footer(s));
    v
}

/// The generators as "load onto track" entries (the arranger track header menu's Insert instrument ▸, issue #2): a
/// pick adds the instrument as `add_generator` does and places a clip of a new pattern named after it on the track
/// at `tick` — FL adds an instrument dropped straight on a track. `nested`: the list fills a submenu.
pub fn items_on_track(s: &State, track: u16, tick: u32, nested: bool) -> Vec<PItem> {
    let mut v = section(s, false, |e| {
        PItem::new(e.label.clone(), Cmd::Plugin(PlugCmd::GeneratorOnTrack { index: e.index, track, tick }))
    }, nested);
    v.extend(footer(s));
    v
}

/// A plugin menu for `target` at (x, y).
pub fn menu(s: &State, target: Target, x: f32, y: f32) -> Popup {
    Popup { x, y, items: items(s, target, false), sub: None, scroll: [0; 2] }
}

/// Whether the open popup is a plugin menu (F8 closes it).
pub fn is_open(s: &State) -> bool {
    s.playlist.popup.as_ref().is_some_and(|p| p.items.iter().any(|i| i.cmd == Cmd::Plugin(PlugCmd::Manage)))
}

impl App {
    /// Opens the plugin menu for `target` at (x, y); F8 while one is open closes it.
    pub fn open_plugin_menu(&mut self, target: Target, at: (f32, f32)) {
        self.state.menu_open = None;
        self.state.playlist.popup = Some(menu(&self.state, target, at.0, at.1));
        self.invalidate();
    }

    /// F8 (spec §1): the all-plugins menu at the mouse, or closes an open plugin menu.
    pub fn toggle_plugin_menu(&mut self) {
        if is_open(&self.state) {
            self.state.playlist.popup = None;
            self.invalidate();
        } else {
            self.open_plugin_menu(Target::All, self.mouse);
        }
    }
}

/// Runs a plugin menu command. Returns a status message.
pub fn run(app: &mut App, c: PlugCmd) -> Option<String> {
    let m = match c {
        PlugCmd::Generator { index, after } => add_generator(app, index, after),
        PlugCmd::GeneratorOnTrack { index, track, tick } => add_generator_on_track(app, index, track, tick),
        PlugCmd::Replace { index, row } => replace_generator(app, index, row),
        PlugCmd::Effect { index, slot } => add_effect(app, index, slot),
        PlugCmd::Find => {
            app.start_scan(Mode::Changed);
            None
        }
        PlugCmd::Manage => {
            app.open_settings(crate::views::settings::PLUGINS);
            None
        }
        PlugCmd::View(v) => {
            app.state.plugdb.prefs.menu_view = v;
            app.prefs_changed();
            None
        }
        PlugCmd::Star { effect, index } => {
            star(app, effect, index);
            None
        }
    };
    app.invalidate();
    m
}

/// Stars the family of plugin `index` (its variant a click adds), or unstars every starred variant of it.
fn star(app: &mut App, effect: bool, index: usize) {
    let db = &mut app.state.plugdb;
    let list = if effect { &db.lists.fx } else { &db.lists.gens };
    let Some(fam) = db::families(list, effect).into_iter().find(|f| f.variants.iter().any(|e| e.index == index)) else { return };
    let starred: Vec<&str> = fam.variants.iter().filter(|e| e.favorite).map(|e| e.uid.as_str()).collect();
    if starred.is_empty() {
        db.prefs.toggle_favorite(&fam.entry().uid);
    } else {
        for uid in starred {
            db.prefs.toggle_favorite(uid);
        }
    }
    app.prefs_changed();
}

/// Plugin `uid` was added: first in the plugin browser's Recent.
fn note_recent(app: &mut App, uid: &str) {
    let prefs = &mut app.state.plugdb.prefs;
    prefs.note_recent(uid);
    prefs.save();
}

/// A new channel for generator `index` (spec §1): at the bottom of the rack, or right below row `after`; selected and
/// in view. Lemon Sampler makes an empty sampler channel (hosted on the next tick); any other generator a plugin
/// channel hosting it. Its plugin window opens once it is hosted.
pub fn add_generator(app: &mut App, index: usize, after: Option<usize>) -> Option<String> {
    let d = app.plugins.available.get(index)?.desc.clone();
    note_recent(app, &d.uid());
    let s = &mut app.state;
    let sampler = d.name == crate::plugins::LEMON_SAMPLER && crate::project_plugins::is_builtin(&d);
    // One step, at the bottom (`history::add_channel`) or inserted higher up (`history::insert_channel`).
    let plugin = (!sampler).then_some(index);
    match after.filter(|&r| r + 1 < s.project.channels.len()) {
        Some(r) => crate::history::insert_channel(s, r + 1, plugin),
        None => crate::history::add_channel(s, plugin),
    }
    let mut ch = if sampler { crate::app::add_channel(&mut s.project) } else { crate::app::add_plugin_channel(&mut s.project, &d.name) };
    if sampler {
        s.pending_open = Some(ch);
    } else {
        s.pending_plugin = Some((ch, index));
    }
    s.piano.dirty = true;
    if let Some(r) = after.filter(|&r| r + 1 < ch) {
        let order: Vec<usize> = (0..=r).chain([ch]).chain(r + 1..ch).collect();
        crate::rack::reorder(app, &order);
        ch = r + 1;
    }
    let s = &mut app.state;
    s.selected_channel = ch;
    s.rack.select_only(ch);
    crate::rack::autosize(s);
    if after.is_none() {
        // Keep the new row in view.
        let r = crate::views::rack::rect(s);
        let view_h = crate::views::rack::panel(r).h;
        s.rack_scroll = (crate::views::rack::content_h(s) - view_h).max(0.0);
    }
    sampler.then(|| "Added a sampler channel (Mango Sampler)".into())
}

/// Generator `index` loaded straight onto arranger track `track` (a generator dropped on the playlist's grid or a
/// track header, the header menu's Insert instrument ▸; issue #2): the channel as `add_generator` makes it (at the
/// bottom of the rack, hosted, selected), then a new pattern named after the instrument whose clip is placed on the
/// track at `tick` — on a header that is the song position, FL adds an instrument dropped on the playlist as a clip
/// on it. One undo step ("Add instrument to track"): undone, the clip, the pattern's name and the channel (with its
/// instance) go. Returns the status message.
pub fn add_generator_on_track(app: &mut App, index: usize, track: u16, tick: u32) -> Option<String> {
    let d = app.plugins.available.get(index)?.desc.clone();
    let s = &mut app.state;
    let sampler = d.name == crate::plugins::LEMON_SAMPLER && crate::project_plugins::is_builtin(&d);
    // One step holding what the whole load changes: the playlist's side (snapshot_with's), the rack's (the channel
    // goes and comes back with it, a redo hosts the generator again) and the patterns' (the name).
    let side = crate::history::RackSide::now(s, s.project.channels.len());
    let snap = crate::history::Snap::Several(vec![
        crate::history::Snap::Generator(Box::new(side), (!sampler).then_some(index)),
        crate::history::Snap::Patterns { patterns: s.project.patterns.clone(), current: s.pattern },
    ]);
    crate::playlist::snapshot_with(s, crate::history::ADD_TO_TRACK, snap);
    // The channel, as add_generator makes it at the bottom of the rack.
    let ch = if sampler { crate::app::add_channel(&mut s.project) } else { crate::app::add_plugin_channel(&mut s.project, &d.name) };
    if sampler {
        s.pending_open = Some(ch);
    } else {
        s.pending_plugin = Some((ch, index));
    }
    s.piano.dirty = true;
    let s = &mut app.state;
    s.selected_channel = ch;
    s.rack.select_only(ch);
    crate::rack::autosize(s);
    let r = crate::views::rack::rect(s);
    let view_h = crate::views::rack::panel(r).h;
    s.rack_scroll = (crate::views::rack::content_h(s) - view_h).max(0.0);
    // A pattern of its own, named after the instrument ("Lemon Trio", then "Lemon Trio #2" once taken), current and
    // the playlist's clip source, as the rack's "+" leaves a picked pattern.
    let n = crate::patterns::first_empty(s);
    let taken = |cand: &str| s.project.patterns.iter().any(|p| p.name.as_deref() == Some(cand));
    let name = (1..).map(|k| if k == 1 { d.name.clone() } else { format!("{} #{k}", d.name) }).find(|c| !taken(&c)).expect("unbounded");
    crate::patterns::pat_mut(s, n).name = Some(name.clone());
    crate::patterns::select(s, n);
    s.playlist.picker = crate::playlist::PickKind::Pattern;
    let i = crate::playlist::place(s, track, tick);
    s.playlist.sel = vec![i];
    Some(format!("Added {name} on track {}", track + 1))
}

/// Generator `index` in place of rack row `row`'s (channel menu and plugin window menu > Replace ▸; FL: "the plugin list
/// of Add one", spec ui/channel-rack.md §18). The channel keeps its row, notes and steps, mixer track, levels and
/// Misc settings; its old instance goes, the new one starts from its defaults, and the channel takes the new
/// plugin's name. Lemon Studio choice where FL was not measured: the name follows the plugin.
pub fn replace_generator(app: &mut App, index: usize, row: usize) -> Option<String> {
    let d = app.plugins.available.get(index)?.desc.clone();
    app.state.project.channels.get(row)?;
    let sampler = d.name == crate::plugins::LEMON_SAMPLER && crate::project_plugins::is_builtin(&d);
    // One step, with the old instrument's state (`history::replace_channel`).
    let old = app.plugin_copy(row);
    crate::history::replace_channel(&mut app.state, row, old, (!sampler).then_some(index));
    app.plugins.drop_key(row);
    app.state.hosted_rows.retain(|&r| r != row);
    app.transport.set_instruments(app.plugins.instruments());
    let s = &mut app.state;
    let ch = &mut s.project.channels[row];
    ch.plugin_state = None;
    if sampler {
        ch.kind = os_model::ChannelKind::Sampler;
        ch.plugin = None;
        ch.clear_sample();
        ch.name = Some(d.name.clone());
        s.pending_open = Some(row);
    } else {
        ch.kind = os_model::ChannelKind::Plugin;
        ch.plugin = Some(crate::osc3::file_name(&d.name).to_string());
        ch.name = Some(d.name.clone());
        // A Lemon Trio channel's sample is its custom shape's (spec: plugins/lemon-3x-osc.md §6): the new instance has
        // none, so neither does the channel.
        if crate::osc3::is_osc3(ch) {
            ch.sample_path = None;
        }
        s.pending_plugin = Some((row, index));
    }
    s.piano.dirty = true;
    s.selected_channel = row;
    s.rack.select_only(row);
    Some(format!("Replaced with {}", d.name))
}

/// Effect `index` into the selected mixer track (spec §1): slot `slot`, else the first empty one.
pub fn add_effect(app: &mut App, index: usize, slot: Option<u8>) -> Option<String> {
    let s = &app.state;
    let t = s.selected_mixer;
    let slot = slot.or_else(|| s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8));
    match slot {
        Some(k) => {
            if let Some(a) = app.plugins.effects.get(index) {
                let uid = a.desc.uid();
                note_recent(app, &uid);
            }
            crate::mixer::run(app, crate::mixer::MCmd::AddEffect(k, index))
        }
        None => Some("Every slot of this track is in use".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{PlugCmd, items_on_track};
    use crate::app::App;
    use crate::osc3::LEMON_TRIO;
    use crate::playlist::Cmd;

    /// Insert instrument ▸ on a track header, a generator dropped on the track (issue #2): the pick adds the
    /// instrument and places a clip of a new pattern named after it on the track, at the tick given; one undo step
    /// takes the clip, the name and the channel back, a redo brings all three (and the hosting) up again.
    #[test]
    fn an_instrument_loaded_onto_a_track_places_its_clip() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let index = a.plugins.index_of(LEMON_TRIO).unwrap();
        let n = a.state.project.channels.len();
        let (track, tick) = (2, 384);
        let cmd = Cmd::Plugin(PlugCmd::GeneratorOnTrack { index, track, tick });
        let m = crate::playlist::run(&mut a, cmd).expect("a status message");
        assert_eq!(m, format!("Added {LEMON_TRIO} on track 3"));
        assert_eq!(a.state.project.channels.len(), n + 1);
        assert_eq!(a.state.project.channels[n].name.as_deref(), Some(LEMON_TRIO));
        assert_eq!(a.state.pending_plugin, Some((n, index)), "the instrument is asked to host");
        assert_eq!(a.state.selected_channel, n);
        assert_eq!(a.state.pattern, 1, "the project's first empty pattern takes the name");
        assert_eq!(crate::patterns::name_of(&a.state, 1), LEMON_TRIO);
        assert_eq!(a.state.playlist.source, Some(os_model::ClipSource::Pattern(1)));
        let last = *crate::playlist::clips(&a.state).last().expect("the clip");
        assert_eq!((last.track, last.position, last.source), (track, tick, os_model::ClipSource::Pattern(1)));
        assert_eq!(a.state.playlist.sel, vec![crate::playlist::clips(&a.state).len() - 1]);
        // One step back: the clip, the name and the channel (with its hosting) go.
        assert_eq!(crate::history::undo(&mut a.state), Some(crate::history::ADD_TO_TRACK));
        assert_eq!(a.state.project.channels.len(), n);
        assert!(a.state.pending_plugin.is_none());
        assert!(crate::playlist::clips(&a.state).iter().all(|c| c.source != os_model::ClipSource::Pattern(1)));
        assert_eq!(crate::patterns::name_of(&a.state, 1), "Pattern 1");
        // And forward again: clip, name, channel and its hosting come back.
        assert_eq!(crate::history::redo(&mut a.state), Some(crate::history::ADD_TO_TRACK));
        assert_eq!(a.state.project.channels.len(), n + 1);
        assert_eq!(a.state.pending_plugin, Some((n, index)));
        let last = *crate::playlist::clips(&a.state).last().expect("the clip");
        assert_eq!((last.track, last.position, last.source), (track, tick, os_model::ClipSource::Pattern(1)));
        assert_eq!(crate::patterns::name_of(&a.state, 1), LEMON_TRIO);
    }

    /// The same twice on one track: the second pattern takes the instrument's name with a " #2", as Clone's does.
    #[test]
    fn a_second_instrument_on_a_track_names_its_pattern_uniquely() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let index = a.plugins.index_of(LEMON_TRIO).unwrap();
        crate::playlist::run(&mut a, Cmd::Plugin(PlugCmd::GeneratorOnTrack { index, track: 0, tick: 0 })).unwrap();
        a.state.pending_plugin = None; // the windows are noise here
        crate::playlist::run(&mut a, Cmd::Plugin(PlugCmd::GeneratorOnTrack { index, track: 3, tick: 96 })).unwrap();
        assert_eq!(crate::patterns::name_of(&a.state, 2), format!("{LEMON_TRIO} #2"));
        let last = *crate::playlist::clips(&a.state).last().unwrap();
        assert_eq!((last.track, last.position, last.source), (3, 96, os_model::ClipSource::Pattern(2)));
    }

    /// The track header menu's Insert instrument ▸ lists the generators as on-track loads for the track it was
    /// opened on, at the tick it was opened with (issue #2).
    #[test]
    fn the_track_menu_lists_the_instruments_for_its_track() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let index = a.plugins.index_of(LEMON_TRIO).unwrap();
        let p = crate::playlist::track_menu(&a.state, 3, 100.0, 200.0, 480);
        let ins = p.items.iter().find(|i| i.label == "Insert instrument").expect("the submenu");
        let pick = ins.children.iter().find(|i| i.label == LEMON_TRIO).expect("the generator");
        assert_eq!(pick.cmd, Cmd::Plugin(PlugCmd::GeneratorOnTrack { index, track: 3, tick: 480 }));
    }

    /// The submenu's items come from the same database view the rack's Add one ▸ shows: favourites first, then the
    /// rest by the display mode.
    #[test]
    fn on_track_items_match_the_racks_menu() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.plugdb.rebuild(&a.plugins);
        let rack = crate::rack::add_one_items(&a.state).into_iter().filter(|i| !i.head && !i.sep && i.children.is_empty()).map(|i| i.label).collect::<Vec<_>>();
        let on = items_on_track(&a.state, 0, 0, true).into_iter().filter(|i| !i.head && !i.sep && i.children.is_empty()).map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(on, rack);
    }
}
