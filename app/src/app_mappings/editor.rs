//! The command tree and its three assignment dialogs.

use super::{keyboard, Mappings, Trigger, GROUPS};
use crate::app::{App, Cmd};
use crate::bars::{baseline, Row};
use crate::colours::{Scheme, TEXT, VECTOR};
use crate::settings;
use crate::subwin::{self, DropDown, Kind, Spec};
use crate::widgets::{bold, button, font, text_at, text_fit};
use egui::*;
use std::collections::BTreeSet;

const ROW: f32 = 20.0;
const CHIP: f32 = 90.0;
const ADD_MENU: [&str; 3] = ["Add Key Mapping", "Add MIDI CC Mapping", "Add Open Sound Control Mapping"];

pub fn prompt_id() -> ViewportId {
    subwin::id("app_mapping_prompt")
}

enum Input {
    Key(Option<String>),
    Midi { cc: Option<u8>, channel: u8 },
    Osc { address: String, parameter: usize },
}

impl Input {
    fn trigger(&self) -> Option<Trigger> {
        match self {
            Self::Key(key) => key.clone().map(Trigger::Key),
            Self::Midi { cc, channel } => cc.map(|cc| Trigger::Midi { cc, channel: *channel }),
            Self::Osc { address, parameter } if address.trim().starts_with('/') =>
                Some(Trigger::Osc { address: address.trim().to_owned(), parameter: *parameter }),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct MappingEditor {
    collapsed: BTreeSet<&'static str>,
    menu: DropDown,
    target: Option<(Cmd, Option<Trigger>)>,
    prompt: Option<(Cmd, Input)>,
    choices: DropDown,
}

impl MappingEditor {
    pub fn prompting(&self) -> bool { self.prompt.is_some() }

    pub fn capture_key(&mut self, key: String) {
        if let Some((_, Input::Key(selected))) = &mut self.prompt {
            *selected = Some(key);
        }
    }

    pub fn learn_cc(&mut self, controller: u8) {
        if let Some((_, Input::Midi { cc: slot @ None, .. })) = &mut self.prompt {
            *slot = Some(controller);
        }
    }

    fn open_menu(&mut self, ui: &Ui, rect: Rect, cmd: Cmd, trigger: Option<Trigger>) {
        self.menu.toggle(ui, rect);
        self.target = Some((cmd, trigger));
    }

    fn command_row(&mut self, ui: &mut Ui, cmd: Cmd, name: &str, maps: &Mappings, last: bool, scheme: &Scheme) {
        let triggers: Vec<_> = maps.0.iter().filter(|(c, _)| *c == cmd).map(|(_, t)| t).collect();
        let width = ui.available_width();
        let per_line = (((width - 175.0) / (CHIP + 2.0)) as usize).max(1);
        let lines = triggers.len().max(1).div_ceil(per_line);
        let (rect, _) = ui.allocate_exact_size(vec2(width, ROW * lines as f32), Sense::hover());
        let mut top = rect;
        top.set_height(ROW);
        let mid = top.center().y;
        let branch = rect.left() + 34.0;
        let stroke = Stroke::new(1.0f32, scheme.get(VECTOR).gamma_multiply(0.5));
        ui.painter().vline(branch, rect.top()..=if last { mid } else { rect.bottom() }, stroke);
        ui.painter().hline(branch..=branch + 11.0, mid, stroke);
        let plus = Rect::from_min_size(pos2(rect.right() - ROW, rect.top()), Vec2::splat(ROW));
        let response = ui.interact(plus, ui.id().with("add"), Sense::click()).on_hover_text(format!("Add a mapping for {name}"));
        let mark = Stroke::new(3.5f32, scheme.get(if response.hovered() { TEXT } else { VECTOR }));
        let center = plus.center();
        ui.painter().hline(center.x - 6.0..=center.x + 6.0, center.y, mark);
        ui.painter().vline(center.x, center.y - 6.0..=center.y + 6.0, mark);
        if response.clicked() { self.open_menu(ui, plus, cmd, None); }

        let start = branch + 13.0;
        let label_end = plus.left() - (CHIP + 2.0) * triggers.len().min(per_line) as f32;
        text_fit(ui.painter(), start, label_end - start - 2.0, baseline(top, 14.0), Align::Min, name, font(14.0), scheme.get(TEXT));
        for (i, trigger) in triggers.into_iter().enumerate() {
            let x = plus.left() - (CHIP + 2.0) * (1 + i % per_line) as f32;
            let y = rect.top() + ROW * (i / per_line) as f32;
            let chip = Rect::from_min_size(pos2(x, y), vec2(CHIP, ROW - 1.0));
            let label = trigger.label();
            let response = button(ui, chip, ui.id().with(i), 4.0, scheme, &label);
            text_fit(ui.painter(), x + 3.0, CHIP - 6.0, baseline(chip, 12.0), Align::Center, &label, font(12.0), scheme.get(TEXT));
            if response.clicked() { self.open_menu(ui, chip, cmd, Some(trigger.clone())); }
        }
    }

    fn tree(&mut self, ui: &mut Ui, maps: &Mappings, scheme: &Scheme) {
        ui.spacing_mut().item_spacing.y = 0.0;
        for &(group, entries) in GROUPS {
            let (r, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
            let closed = self.collapsed.contains(group);
            let icon = Rect::from_center_size(pos2(r.left() + 11.0, r.center().y), Vec2::splat(10.0));
            let p = ui.painter();
            let stroke = Stroke::new(1.0f32, scheme.get(VECTOR));
            p.rect_stroke(icon, CornerRadius::ZERO, stroke, StrokeKind::Inside);
            p.hline(icon.left()+2.0..=icon.right()-2.0, icon.center().y, stroke);
            if closed { p.vline(icon.center().x, icon.top()+2.0..=icon.bottom()-2.0, stroke); }
            text_at(p, r.left() + 24.0, baseline(r, 16.0), Align::Min, group, bold(16.0), scheme.get(TEXT));
            if response.clicked() && !self.collapsed.remove(group) {
                self.collapsed.insert(group);
            }
            if !self.collapsed.contains(group) {
                for (i, &(cmd, id, name)) in entries.iter().enumerate() {
                    ui.push_id(id, |ui| self.command_row(ui, cmd, name, maps, i + 1 == entries.len(), scheme));
                }
            }
        }
    }
}

fn row(label: impl Into<String>) -> Row { (label.into(), String::new(), true, false) }

fn cc_label(cc: u8) -> String {
    const NAMES: [&str; 128] = [
        "Bank Select", "Mod Wheel", "Breath", "", "Foot Pedal", "Portamento", "Data Entry", "Volume",
        "Balance", "", "Pan", "Expression", "Effect Control 1", "EffectControl 2", "", "",
        "General Purpose 1", "General Purpose 2", "General Purpose 3", "General Purpose 4", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "Bank Select (fine)", "Mod Wheel (fine)", "Breath (fine)", "", "Foot Pedal (fine)", "Portamento (fine)", "Data Entry (fine)", "Volume (fine)",
        "Balance (fine)", "", "Pan (fine)", "Expression (fine)", "Effect Control 1 (fine)", "Effect Control 2 (fine)", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "Hold Pedal", "Portamento (on/off)", "Sustenuto Pedal", "Soft Pedal", "Legato Pedal", "Hold 2 Pedal", "Sound Variation", "Sound Timbre",
        "Sound Release Time", "Sound Attack Time", "Sound Brightness", "Sound Control 6", "Sound Control 7", "Sound Control 8", "Sound Control 9", "Sound Control 10",
        "General Purpose Button 1", "General Purpose Button 2", "General Purpose Button 3", "General Purpose Button 4", "", "", "", "",
        "", "", "", "Effects Level", "Tremolo Level", "Chorus Level", "Celeste Level", "Phaser Level",
        "Data Button Inc", "Data Button Dec", "NRPN (fine)", "NRPN (coarse)", "RPN (fine)", "RPN (coarse)", "", "",
        "", "", "", "", "", "", "", "",
        "", "", "", "", "", "", "", "",
        "All Sound Off", "All Controllers Off", "Local Keyboard", "All Notes Off", "Omni Mode Off", "Omni Mode On", "Mono Operation", "Poly Operation",
    ];
    match NAMES[usize::from(cc)] {
        "" => cc.to_string(),
        name => format!("{cc}: {name}"),
    }
}

impl App {
    pub fn app_maps_window(&mut self, ctx: &Context) {
        let vid = subwin::id("app_maps");
        let spec = Spec::new(vid, "Application Mappings", Kind::fill(vec2(400.0, 524.0), vec2(285.0, 180.0), true)).margin(Margin::ZERO);
        let mut editor = std::mem::take(&mut self.dlg.app_draft);
        let mut reset = false;
        let close = subwin::show(ctx, spec, |ui| {
            let area = ui.available_rect_before_wrap();
            let list = Rect::from_min_max(area.min, area.right_bottom() - vec2(0.0, 28.0));
            let mut tree = ui.new_child(UiBuilder::new().max_rect(list));
            tree.spacing_mut().scroll = egui::style::ScrollStyle {
                bar_width: 16.0,
                bar_inner_margin: 1.0,
                ..egui::style::ScrollStyle::solid()
            };
            ScrollArea::vertical().auto_shrink([false, false]).show(&mut tree, |ui| editor.tree(ui, &self.mappings, &self.scheme));
            let r = Rect::from_min_size(area.right_bottom() - vec2(112.0, 26.0), vec2(110.0, 24.0));
            reset |= button(ui, r, ui.id().with("reset"), 4.0, &self.scheme, "Reset to defaults").clicked();
            text_fit(ui.painter(), r.left() + 3.0, r.width() - 6.0, baseline(r, 14.0), Align::Center, "Reset to defaults", font(14.0), self.scheme.get(TEXT));
        });
        if close {
            self.dlg.app_maps = false;
            return;
        }
        if reset {
            self.apply_mappings(Mappings::default());
            editor = MappingEditor::default();
        }
        if let Some((cmd, existing)) = &editor.target {
            let rows = if existing.is_some() { vec![row("Remove Mapping")] } else { ADD_MENU.map(row).to_vec() };
            if let Some(i) = subwin::drop_list(ctx, subwin::id("app_mapping_menu"), vid, &rows, &mut editor.menu, &self.scheme, 150.0) {
                if let Some(trigger) = existing {
                    let mut maps = self.mappings.clone();
                    maps.0.retain(|(c, t)| c != cmd || t != trigger);
                    self.apply_mappings(maps);
                } else {
                    editor.prompt = Some((*cmd, match i {
                        0 => Input::Key(None),
                        1 => Input::Midi { cc: Some(0), channel: 0 },
                        _ => Input::Osc { address: String::new(), parameter: 0 },
                    }));
                    editor.choices = DropDown::default();
                }
            }
        }
        if editor.menu.open.is_none() { editor.target = None; }
        if let Some((cmd, ref mut input)) = editor.prompt {
            let (accept, cancel) = mapping_prompt(ctx, cmd, input, &mut editor.choices, &self.osc_addresses, &self.mappings, &self.scheme);
            if cancel {
                editor.prompt = None;
            } else if let Some(trigger) = input.trigger().filter(|_| accept) {
                let mut maps = self.mappings.clone();
                maps.assign(cmd, trigger);
                if self.apply_mappings(maps) { editor.prompt = None; }
            }
        }
        self.dlg.app_draft = editor;
    }

    fn apply_mappings(&mut self, maps: Mappings) -> bool {
        if self.shot.is_none() {
            if let Err(e) = maps.save(&settings::dir()) {
                self.message("Application Mappings", &format!("Could not save the mappings. {e}"));
                return false;
            }
        }
        keyboard::update_keys(&maps);
        self.mappings = maps;
        true
    }
}

fn mapping_prompt(ctx: &Context, cmd: Cmd, input: &mut Input, choices: &mut DropDown, addresses: &BTreeSet<String>, maps: &Mappings, scheme: &Scheme) -> (bool, bool) {
    let vid = prompt_id();
    let title = match input { Input::Key(_) => "Keypress mapping", Input::Midi { .. } => "MIDI CC mapping", Input::Osc { .. } => "Open Sound Control mapping" };
    let spec = Spec::new(vid, title, Kind::fit(330.0)).owner(subwin::id("app_maps"));
    let mut accept = false;
    let mut cancel = false;
    let close = subwin::show(ctx, spec, |ui| {
        match input {
            Input::Key(key) => {
                ui.label("Enter the key combination to map this command to:");
                ui.strong(key.as_deref().unwrap_or("Press a key combination"));
                if let Some((other, _)) = maps.keys().find(|(c, k)| *c != cmd && Some(*k) == key.as_deref()) {
                    if let Some((_, _, name)) = super::commands().find(|(c, _, _)| *c == other) {
                        ui.label(format!("This shortcut will move from {name}."));
                    }
                }
            }
            Input::Midi { cc, channel } => {
                ui.label("Select a MIDI CC to map this command to:");
                let selected = cc.map(cc_label).unwrap_or_else(|| "Move a MIDI controller…".into());
                let (r, _) = ui.allocate_exact_size(vec2(255.0, 24.0), Sense::hover());
                subwin::combo(ui, r, ui.id().with("cc"), &selected, choices, scheme);
                ui.horizontal(|ui| {
                    ui.label("Channel");
                    ui.add(DragValue::new(channel).range(0..=16).custom_formatter(|v, _| if v == 0.0 { "Any".into() } else { v.to_string() }));
                });
            }
            Input::Osc { address, parameter } => {
                ui.label("Enter OSC address to map this command to:");
                ui.add(TextEdit::singleline(address).hint_text("/pedalboard/next").desired_width(285.0));
                if !addresses.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(220.0, 24.0), Sense::hover());
                    subwin::combo(ui, r, ui.id().with("received"), "Received addresses", choices, scheme);
                }
                ui.horizontal(|ui| { ui.label("Parameter"); ui.add(DragValue::new(parameter).range(0..=999)); });
            }
        }
        let valid = input.trigger().is_some();
        ui.horizontal(|ui| {
            accept |= ui.add_enabled(valid, Button::new("OK")).clicked();
            cancel |= ui.button("Cancel").clicked();
        });
        if !matches!(input, Input::Key(_)) && choices.open.is_none() {
            accept |= valid && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
            cancel |= ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        }
    });
    if matches!(input, Input::Key(_)) { keyboard::listen(subwin::hwnd_of(vid)); }
    if choices.open.is_some() {
        let rows = match input {
            Input::Midi { .. } => std::iter::once(row("<< MIDI Learn >>")).chain((0..=127).map(|n| row(cc_label(n)))).collect(),
            _ => addresses.iter().map(|s| row(s.as_str())).collect::<Vec<_>>(),
        };
        if let Some(i) = subwin::drop_list(ctx, subwin::id("app_mapping_choices"), vid, &rows, choices, scheme, 220.0) {
            match input {
                Input::Midi { cc, .. } => *cc = i.checked_sub(1).map(|v| v as u8),
                Input::Osc { address, .. } => address.clone_from(&rows[i].0),
                _ => {}
            }
        }
    }
    (accept, close || cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_takes_one_cc_and_leaves_the_assignment_pending_until_ok() {
        let mut editor = MappingEditor::default();
        editor.prompt = Some((Cmd::NextPatch, Input::Midi { cc: None, channel: 0 }));
        assert!(editor.prompt.as_ref().unwrap().1.trigger().is_none());
        editor.learn_cc(74);
        editor.learn_cc(12);
        let (cmd, input) = editor.prompt.take().unwrap();
        assert_eq!(cmd, Cmd::NextPatch);
        assert_eq!(input.trigger(), Some(Trigger::Midi { cc: 74, channel: 0 }));
        editor.learn_cc(127);
        assert!(!editor.prompting());
    }
}
