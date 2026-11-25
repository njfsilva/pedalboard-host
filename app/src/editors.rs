//! The mappings editor (MIDI controllers and OSC addresses per box), the Patch Organiser and
//! user presets.

use crate::app::{App, Patch};
use crate::engine::{MidiMap, OscMap};
use crate::settings;
use egui::*;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct MapDraft {
    /// The MIDI mapping (index into the graph's list) waiting for a controller to learn.
    pub learn: Option<usize>,
    params_for: Option<u32>,
    params: Vec<(i32, String)>,
}

#[derive(Default)]
pub struct PresetState {
    pub node: Option<u32>,
    pub folder: Option<String>,
    pub name: String,
    pub selected: Option<String>,
    pub rename: String,
}

impl PresetState {
    pub fn for_node(id: u32) -> PresetState {
        PresetState { node: Some(id), ..Default::default() }
    }
}

fn presets_dir() -> PathBuf {
    settings::dir().join("presets")
}

/// A name made safe for a file or folder.
fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "_".into() } else { t }
}

fn list_dir(dir: &Path, folders: bool) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir() == folders && (folders || p.extension().is_some_and(|x| x == "preset")))
                .filter_map(|p| if folders { p.file_name() } else { p.file_stem() }.map(|s| s.to_string_lossy().to_string()))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

fn param_combo(ui: &mut Ui, id: impl std::hash::Hash, v: &mut i32, params: &[(i32, String)]) -> bool {
    let before = *v;
    let shown = params.iter().find(|p| p.0 == *v).map(|p| p.1.clone()).unwrap_or_else(|| format!("Parameter {}", *v + 1));
    ComboBox::from_id_salt(id).selected_text(shown).width(190.0).show_ui(ui, |ui| {
        for (i, n) in params {
            ui.selectable_value(v, *i, n);
        }
    });
    *v != before
}

fn channel_combo(ui: &mut Ui, id: impl std::hash::Hash, ch: &mut u8) -> bool {
    let before = *ch;
    let name = |c: u8| if c == 0 { "Any".to_string() } else { c.to_string() };
    ComboBox::from_id_salt(id).selected_text(name(*ch)).width(56.0).show_ui(ui, |ui| {
        for c in 0..=16u8 {
            ui.selectable_value(ch, c, name(c));
        }
    });
    *ch != before
}

enum Op {
    Add,
    Copy,
    Remove,
    Up,
    Down,
}

impl App {
    /// MIDI learn: the next controller that arrives is given to the waiting mapping.
    pub fn learn_cc(&mut self, cc: u8) {
        let Some(k) = self.dlg.map_draft.learn.take() else { return };
        if let Some(m) = self.graph.midi_maps.get_mut(k) {
            m.cc = cc;
            self.rebuild();
            self.dirty = true;
        }
    }

    pub fn mappings_window(&mut self, ctx: &Context) {
        let Some(id) = self.dlg.mappings else { return };
        let Some(node) = self.graph.node(id) else {
            self.dlg.mappings = None;
            return;
        };
        let title = format!("{} - Mappings", node.title());
        let d = &mut self.dlg.map_draft;
        if d.params_for != Some(id) {
            // Index -1 is the bypass switch (0x140042290).
            d.params = std::iter::once((-1, "Bypass".to_string()))
                .chain((0..node.num_params()).map(|i| {
                    let n = node.param_name(i);
                    (i, if n.trim().is_empty() { format!("Parameter {}", i + 1) } else { format!("{} - {}", i + 1, n.trim()) })
                }))
                .collect();
            d.params_for = Some(id);
            d.learn = None;
        }
        let first = if node.num_params() > 0 { 0 } else { -1 };
        let slug = safe(node.name()).replace(' ', "_");
        let mut midi: Vec<(usize, MidiMap)> = self.graph.midi_maps.iter().copied().enumerate().filter(|(_, m)| m.node == id).collect();
        let mut osc: Vec<(usize, OscMap)> = self.graph.osc_maps.iter().cloned().enumerate().filter(|(_, m)| m.node == id).collect();
        let (mut changed, mut open) = (false, true);
        let (mut del_midi, mut del_osc, mut add_midi, mut add_osc) = (None, None, false, false);
        let (params, learn) = (&d.params, &mut d.learn);
        Window::new(title).id(Id::new("mappings")).open(&mut open).default_width(640.0).collapsible(false).show(ctx, |ui| {
            ui.strong("MIDI controllers");
            if midi.is_empty() {
                ui.label("No MIDI mappings yet.");
            } else {
                Grid::new("midi_maps").num_columns(7).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
                    for h in ["Parameter", "CC", "Channel", "Latch", "Lower", "Upper", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (k, m) in midi.iter_mut() {
                        changed |= param_combo(ui, ("mp", *k), &mut m.param, params);
                        ui.horizontal(|ui| {
                            changed |= ui.add(DragValue::new(&mut m.cc).range(0..=127)).changed();
                            let on = *learn == Some(*k);
                            if ui.selectable_label(on, if on { "Waiting..." } else { "Learn" }).on_hover_text("Move a controller to pick its CC number").clicked() {
                                *learn = if on { None } else { Some(*k) };
                            }
                        });
                        changed |= channel_combo(ui, ("mc", *k), &mut m.channel);
                        changed |= ui.checkbox(&mut m.latch, "").on_hover_text("Each press toggles between the lower and upper value").changed();
                        changed |= ui.add(DragValue::new(&mut m.lower).range(0.0..=1.0).speed(0.005).max_decimals(3)).changed();
                        changed |= ui.add(DragValue::new(&mut m.upper).range(0.0..=1.0).speed(0.005).max_decimals(3)).changed();
                        if ui.button("Remove").clicked() {
                            del_midi = Some(*k);
                        }
                        ui.end_row();
                    }
                });
            }
            add_midi = ui.button("Add MIDI mapping").clicked();
            ui.separator();
            ui.strong("OSC addresses");
            if osc.is_empty() {
                ui.label("No OSC mappings yet.");
            } else {
                Grid::new("osc_maps").num_columns(3).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
                    for h in ["Parameter", "Address", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (k, m) in osc.iter_mut() {
                        changed |= param_combo(ui, ("op", *k), &mut m.param, params);
                        changed |= ui.add(TextEdit::singleline(&mut m.address).desired_width(240.0)).changed();
                        if ui.button("Remove").clicked() {
                            del_osc = Some(*k);
                        }
                        ui.end_row();
                    }
                });
            }
            add_osc = ui.button("Add OSC mapping").clicked();
        });
        if changed {
            for (k, m) in &midi {
                self.graph.midi_maps[*k] = *m;
            }
            for (k, m) in osc {
                self.graph.osc_maps[k] = m;
            }
        }
        if let Some(k) = del_midi {
            self.graph.midi_maps.remove(k);
            self.dlg.map_draft.learn = None;
            changed = true;
        }
        if let Some(k) = del_osc {
            self.graph.osc_maps.remove(k);
            changed = true;
        }
        if add_midi {
            self.graph.midi_maps.push(MidiMap { node: id, param: first, cc: 1, channel: 0, latch: false, lower: 0.0, upper: 1.0 });
            changed = true;
        }
        if add_osc {
            self.graph.osc_maps.push(OscMap { node: id, param: first, address: format!("/{slug}/{}", first + 1) });
            changed = true;
        }
        if changed {
            self.rebuild();
            self.dirty = true;
        }
        if !open {
            self.dlg.mappings = None;
            self.dlg.map_draft.learn = None;
        }
    }

    pub fn organiser_window(&mut self, ctx: &Context) {
        let (mut open, mut op, mut select) = (true, None, None);
        let cur = self.current;
        let n = self.patches.len();
        let mut name = self.patches.get(cur).map(|p| p.name.clone()).unwrap_or_default();
        let mut renamed = false;
        let patches = &self.patches;
        Window::new("Patch Organiser").open(&mut open).default_width(380.0).collapsible(false).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                Frame::NONE.fill(ui.visuals().extreme_bg_color).stroke(Stroke::new(1.0f32, Color32::from_gray(170))).inner_margin(Margin::same(3)).show(ui, |ui| {
                    ScrollArea::vertical().min_scrolled_height(260.0).max_height(260.0).show(ui, |ui| {
                        ui.set_width(230.0);
                        for (i, p) in patches.iter().enumerate() {
                            if ui.selectable_label(i == cur, format!("{} - {}", i + 1, p.name)).clicked() {
                                select = Some(i);
                            }
                        }
                    });
                });
                ui.vertical(|ui| {
                    for (label, o, on) in [("Add", Op::Add, true), ("Copy", Op::Copy, true), ("Remove", Op::Remove, n > 1), ("Move Up", Op::Up, cur > 0), ("Move Down", Op::Down, cur + 1 < n)] {
                        if ui.add_enabled(on, Button::new(label).min_size(vec2(100.0, 0.0))).clicked() {
                            op = Some(o);
                        }
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Name:");
                renamed = ui.add(TextEdit::singleline(&mut name).desired_width(240.0)).changed();
            });
        });
        if renamed {
            if let Some(p) = self.patches.get_mut(cur) {
                p.name = name;
                self.dirty = true;
            }
        }
        if let Some(i) = select {
            self.select_patch(i);
        }
        match op {
            Some(Op::Add | Op::Copy) => {
                self.store_current();
                let p = if matches!(op, Some(Op::Copy)) {
                    let c = &self.patches[cur];
                    // "(copy)" is the original's suffix for copied patches.
                    Patch { name: format!("{} (copy)", c.name), tempo: c.tempo, el: c.el.clone() }
                } else {
                    Patch::untitled(self.engine.shared.tempo())
                };
                self.patches.insert(cur + 1, p);
                self.current = cur + 1;
                self.load_current();
                self.dirty = true;
            }
            Some(Op::Remove) if n > 1 => {
                self.patches.remove(cur);
                self.current = cur.min(self.patches.len() - 1);
                self.load_current();
                self.dirty = true;
            }
            Some(Op::Up) if cur > 0 => {
                self.patches.swap(cur, cur - 1);
                self.current = cur - 1;
                self.dirty = true;
            }
            Some(Op::Down) if cur + 1 < n => {
                self.patches.swap(cur, cur + 1);
                self.current = cur + 1;
                self.dirty = true;
            }
            _ => {}
        }
        self.dlg.organiser = open;
    }

    pub fn presets_window(&mut self, ctx: &Context) {
        let Some(st) = self.dlg.presets.as_mut() else { return };
        let node_name = st.node.and_then(|id| self.graph.node(id)).map(|n| n.name().to_string());
        if node_name.is_none() {
            st.node = None;
        }
        let own = node_name.as_deref().map(safe);
        if st.folder.is_none() {
            st.folder = own.clone();
        }
        let root = presets_dir();
        let mut folders = list_dir(&root, true);
        if let Some(o) = own.as_ref().filter(|o| !folders.contains(o)) {
            folders.insert(0, o.clone());
        }
        let presets = st.folder.as_ref().map(|f| list_dir(&root.join(f), false)).unwrap_or_default();
        let can_load = own.is_some() && st.folder == own;
        let (mut open, mut save, mut load, mut delete, mut rename) = (true, false, None, None, None);
        Window::new("User Presets").open(&mut open).default_width(440.0).collapsible(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Plugin:");
                ComboBox::from_id_salt("preset_folder").selected_text(st.folder.clone().unwrap_or_else(|| "(choose a plugin)".into())).width(280.0).show_ui(ui, |ui| {
                    for f in &folders {
                        if ui.selectable_label(st.folder.as_deref() == Some(f.as_str()), f).clicked() {
                            st.folder = Some(f.clone());
                            st.selected = None;
                        }
                    }
                });
            });
            if let (Some(n), true) = (&node_name, can_load) {
                ui.horizontal(|ui| {
                    ui.label(format!("Save {n}'s settings as:"));
                    ui.add(TextEdit::singleline(&mut st.name).desired_width(150.0));
                    save = ui.add_enabled(!st.name.trim().is_empty(), Button::new("Save")).clicked();
                });
            }
            ui.separator();
            if st.folder.is_some() && presets.is_empty() {
                ui.label("No presets saved for this plugin yet.");
            }
            ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                Grid::new("presets").num_columns(2).striped(true).show(ui, |ui| {
                    for p in &presets {
                        if ui.selectable_label(st.selected.as_deref() == Some(p.as_str()), p).clicked() {
                            st.selected = Some(p.clone());
                            st.rename = p.clone();
                        }
                        ui.horizontal(|ui| {
                            if can_load && ui.small_button("Load").clicked() {
                                load = Some(p.clone());
                            }
                            if ui.small_button("Delete").clicked() {
                                delete = Some(p.clone());
                            }
                        });
                        ui.end_row();
                    }
                });
            });
            if let Some(sel) = st.selected.clone().filter(|s| presets.contains(s)) {
                ui.horizontal(|ui| {
                    ui.label("Rename to:");
                    ui.add(TextEdit::singleline(&mut st.rename).desired_width(180.0));
                    let to = st.rename.trim().to_string();
                    if ui.add_enabled(!to.is_empty() && to != sel, Button::new("Rename")).clicked() {
                        rename = Some((sel, to));
                    }
                });
            }
        });
        let folder = st.folder.clone().map(|f| root.join(f));
        let (node, name) = (st.node, safe(st.name.trim()));
        if !open {
            self.dlg.presets = None;
        }
        let Some(folder) = folder else { return };
        let file = |n: &str| folder.join(format!("{}.preset", safe(n)));
        let mut err = None;
        if save {
            match node.and_then(|id| self.graph.node_mut(id)).and_then(|n| n.state_b64()) {
                Some(text) => {
                    if let Err(e) = std::fs::create_dir_all(&folder).and_then(|_| std::fs::write(file(&name), text)) {
                        err = Some(format!("Could not save the preset: {e}"));
                    } else if let Some(st) = self.dlg.presets.as_mut() {
                        st.name.clear();
                    }
                }
                None => err = Some("This processor has no settings to save.".into()),
            }
        }
        if let Some(p) = load {
            let ok = std::fs::read_to_string(file(&p)).ok().zip(node.and_then(|id| self.graph.node_mut(id))).is_some_and(|(t, n)| n.set_state_b64(t.trim()));
            if ok {
                self.dirty = true;
            } else {
                err = Some(format!("Could not load the preset \"{p}\"."));
            }
        }
        if let Some(p) = delete {
            if let Err(e) = std::fs::remove_file(file(&p)) {
                err = Some(format!("Could not delete the preset: {e}"));
            }
        }
        if let Some((from, to)) = rename {
            match std::fs::rename(file(&from), file(&to)) {
                Ok(()) => {
                    if let Some(st) = self.dlg.presets.as_mut() {
                        st.selected = Some(safe(&to));
                    }
                }
                Err(e) => err = Some(format!("Could not rename the preset: {e}")),
            }
        }
        if let Some(e) = err {
            self.message("User Presets", &e);
        }
    }
}
