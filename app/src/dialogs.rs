//! Dialog windows: messages, unsaved-changes question, Misc Settings, Audio Settings, Colour
//! Schemes, Application Mappings, Event Log, Documentation and About.

use crate::app::{App, Cmd};
use crate::bars::baseline;
use crate::colours::{self, draw, overlaid, Scheme};
use crate::engine::{asio, wasapi};
use crate::graph::NodeKind;
use crate::plugins::PluginUi;
use crate::settings::{self, AudioConfig, Settings};
use crate::app_mappings::editor::{MappingEditor, prompt_id};
use crate::subwin::{self, Kind, Spec};
use crate::widgets::{bold, font, scrollbar, text_at, text_fit};
use egui::color_picker::{color_edit_button_srgba, Alpha};
use egui::*;

pub struct AudioDraft {
    pub cfg: AudioConfig,
    pub asio: Vec<String>,
    pub outs: Vec<String>,
    pub ins: Vec<String>,
    pub midi_ports: Vec<String>,
    pub midi_on: Vec<String>,
}

pub struct ColourDraft {
    pub names: Vec<String>,
    pub name: String,
}

#[derive(Default)]
pub struct Dialogs {
    pub messages: Vec<(String, String)>,
    /// Counts the messages answered, so each one opens as a new window in the middle.
    pub message_no: u32,
    pub confirm: Option<Cmd>,
    pub misc: bool,
    /// The Misc Settings text boxes (OSC port, multicast address) while the window is open.
    pub misc_text: Option<(String, String)>,
    pub audio: Option<AudioDraft>,
    pub plugins: Option<crate::plugin_list::PluginListState>,
    pub colours: Option<ColourDraft>,
    pub app_maps: bool,
    pub app_draft: MappingEditor,
    pub organiser: bool,
    /// The organiser's selected rows and in-place rename while it's open.
    pub organiser_state: crate::editors::OrganiserState,
    /// How many times the organiser has been opened, so each opening is a fresh window in the
    /// middle at the original's size, as the original builds a new one each time (0x1400384e4).
    pub organiser_opens: u32,
    /// The organiser's "Select patch..." box, open after Import... read a file.
    pub import: Option<crate::editors::ImportPick>,
    pub presets: Option<crate::editors::PresetManager>,
    pub mappings: Option<u32>,
    pub map_draft: crate::editors::MapDraft,
    pub log: bool,
    pub about: bool,
    pub docs: bool,
    /// Editor windows the app draws (built-ins, plugins without an editor of their own), by box
    /// id, and the one to bring to the front this frame.
    pub editors: Vec<u32>,
    pub raise: Option<u32>,
}

impl Dialogs {
    pub fn open_audio(&mut self, s: &Settings, _e: &crate::engine::Engine) {
        self.audio = Some(AudioDraft {
            cfg: s.audio.clone(),
            asio: asio::list_drivers().into_iter().map(|d| d.0).collect(),
            outs: wasapi::output_names(),
            ins: wasapi::input_names().into_iter().filter(|n| n != wasapi::NO_INPUT).collect(),
            midi_ports: crate::midi::port_names(),
            midi_on: s.midi_inputs.clone(),
        });
    }

    pub fn open_colours(&mut self, s: &Scheme) {
        self.colours = Some(ColourDraft { names: list_schemes(), name: if s.name == "default" { String::new() } else { s.name.clone() } });
    }
}

pub fn load_scheme(name: &str) -> Option<Scheme> {
    if name.is_empty() || name == "default" {
        return None;
    }
    let text = std::fs::read_to_string(settings::dir().join(format!("{name}.colourscheme"))).ok()?;
    Scheme::from_xml(&text, name)
}

pub fn list_schemes() -> Vec<String> {
    let mut v = vec!["default".to_string()];
    if let Ok(rd) = std::fs::read_dir(settings::dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "colourscheme") {
                if let Some(s) = p.file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| s != "default") {
                    v.push(s);
                }
            }
        }
    }
    v
}

/// egui's own widgets (dialogs, lists) in the colour scheme's colours.
pub fn apply_style(ctx: &Context, c: &Scheme) {
    let mut st = (*ctx.style()).clone();
    let mut v = Visuals::light();
    let bg = c.get(colours::WINDOW_BG);
    v.panel_fill = bg;
    v.window_fill = bg;
    v.extreme_bg_color = c.get(colours::TEXT_EDITOR);
    v.faint_bg_color = c.get(colours::FIELD_BG);
    v.override_text_color = Some(c.get(colours::TEXT));
    v.selection.bg_fill = c.get(colours::LIST_SELECTED);
    v.selection.stroke = Stroke::new(1.0f32, c.get(colours::TEXT));
    v.window_stroke = Stroke::new(1.0f32, Color32::from_gray(140));
    let btn = c.get(colours::BUTTON);
    let rim = colours::darker(btn, 0.85);
    v.widgets.inactive.weak_bg_fill = btn;
    v.widgets.inactive.bg_fill = btn;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0f32, rim);
    v.widgets.hovered.weak_bg_fill = colours::brighter(btn, 0.2);
    v.widgets.hovered.bg_fill = colours::brighter(btn, 0.2);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0f32, c.get(colours::TEXT));
    v.widgets.active.weak_bg_fill = c.get(colours::LIST_SELECTED);
    v.widgets.active.bg_fill = c.get(colours::LIST_SELECTED);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0f32, Color32::from_gray(190));
    st.visuals = v;
    st.text_styles.insert(TextStyle::Body, font(14.0));
    st.text_styles.insert(TextStyle::Button, font(14.0));
    st.text_styles.insert(TextStyle::Heading, bold(17.0));
    st.text_styles.insert(TextStyle::Small, font(11.0));
    st.spacing.button_padding = vec2(8.0, 3.0);
    st.spacing.item_spacing = vec2(8.0, 6.0);
    ctx.set_style(st);
}

/// JUCE's generic plugin editor: 25 px rows, scrolled past 400 px with an 18 px bar.
const GEN_ROW: f32 = 25.0;
const GEN_SB: f32 = 18.0;

/// The size a generic editor asks for: 400 wide, a row per parameter, between 25 and 400 high.
fn generic_editor_size(params: i32) -> Vec2 {
    vec2(400.0, (params.max(0) as f32 * GEN_ROW).clamp(GEN_ROW, 400.0))
}

/// A vertical gradient through `stops` (position 0..1 down the rectangle, colour).
fn vgrad(p: &Painter, r: Rect, stops: &[(f32, Color32)]) {
    let mut m = Mesh::default();
    for (k, &(t, c)) in stops.iter().enumerate() {
        let y = r.top() + r.height() * t;
        m.colored_vertex(pos2(r.left(), y), c);
        m.colored_vertex(pos2(r.right(), y), c);
        if k > 0 {
            let i = 2 * k as u32;
            m.add_triangle(i - 2, i - 1, i + 1);
            m.add_triangle(i - 2, i + 1, i);
        }
    }
    p.add(Shape::mesh(m));
}

/// The editor for a plugin without one of its own, as JUCE's generic editor draws it: on white, a
/// row per parameter with its name in the left third (at most 200 px) and a bar slider holding
/// the plugin's text for the value. Pressing or dragging on a slider sets the value under the
/// pointer.
fn generic_editor(ui: &mut Ui, r: Rect, id: Id, pl: &mut dyn PluginUi) {
    let n = pl.num_params().max(0);
    let total = n as f32 * GEN_ROW;
    let scroll_id = id.with("scroll");
    let mut pos = ui.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    ui.painter().rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
    let w = if total > r.height() {
        if ui.rect_contains_pointer(r) {
            pos -= ui.input(|i| i.smooth_scroll_delta.y);
        }
        pos = pos.clamp(0.0, total - r.height());
        scrollbar(ui, Rect::from_min_max(pos2(r.right() - GEN_SB, r.top()), r.max), true, &mut pos, r.height(), total, id.with("sb"));
        r.width() - GEN_SB
    } else {
        pos = 0.0;
        r.width()
    };
    ui.data_mut(|d| d.insert_temp(scroll_id, pos));
    let view = Rect::from_min_size(r.min, vec2(w, r.height()));
    let p = ui.painter().with_clip_rect(view.intersect(ui.clip_rect()));
    let text_w = ((w as i32) / 3).min(200) as f32;
    let first = (pos / GEN_ROW) as i32;
    let last = (((pos + r.height()) / GEN_ROW).ceil() as i32).min(n);
    for i in first..last {
        let top = r.top() + i as f32 * GEN_ROW - pos;
        p.rect_filled(Rect::from_min_size(pos2(r.left(), top), vec2(w, GEN_ROW - 1.0)), CornerRadius::ZERO, draw([255, 255, 255, 0x66]));
        let name = pl.param_name(i);
        let name = if name.trim().is_empty() { "Unnamed" } else { name.as_str() };
        let lr = Rect::from_min_size(pos2(r.left() + 3.0, top + 1.0), vec2(text_w - 5.0, GEN_ROW - 3.0));
        text_fit(&p, lr.left(), lr.width(), baseline(lr, 15.6), Align::Min, name, font(15.6), Color32::BLACK);
        let sr = Rect::from_min_size(pos2(r.left() + text_w, top + 1.0), vec2(w - text_w - 1.0, GEN_ROW - 3.0));
        let resp = ui.interact(sr.intersect(view), id.with(("param", i)), Sense::click_and_drag());
        let mut v = pl.param_value(i).clamp(0.0, 1.0);
        if resp.is_pointer_button_down_on() {
            if let Some(m) = resp.interact_pointer_pos() {
                let nv = ((m.x - sr.left() - 1.0) / (sr.width() - 2.0)).clamp(0.0, 1.0);
                if nv != v {
                    pl.set_param(i, nv);
                    v = nv;
                }
            }
        }
        // The bar is JUCE's shiny button shape in the slider's thumb colour, darker under the mouse.
        let bar = Rect::from_min_size(sr.min + vec2(1.0, 1.0), vec2(v * (sr.width() - 2.0), sr.height() - 2.0));
        if bar.width() > 0.99 {
            let base = if resp.hovered() || resp.dragged() { [154, 154, 203, 255] } else { [194, 194, 255, 255] };
            let stops = [(0.0, base), (0.5, overlaid(base, [255, 255, 255, 0x33])), (0.51, overlaid(base, [0, 0, 255, 0x11])), (1.0, overlaid(base, [0, 0, 255, 0x07]))];
            vgrad(&p, bar, &stops.map(|(t, c)| (t, draw(c))));
            p.rect_stroke(bar, CornerRadius::ZERO, Stroke::new(0.9f32, Color32::from_black_alpha(0x80)), StrokeKind::Middle);
        }
        text_fit(&p, sr.left() + 5.0, sr.width() - 10.0, baseline(sr, 15.0), Align::Center, &pl.param_text(i), font(15.0), Color32::BLACK);
        p.rect_stroke(sr, CornerRadius::ZERO, Stroke::new(1.0f32, draw([0x80, 0x80, 0x80, 0xb2])), StrokeKind::Inside);
    }
}

/// Applies the Misc Settings text boxes. A port that isn't a number from 1 to 65535 goes back to
/// the current one; the multicast address is kept as typed, blank meaning one-to-one.
fn commit_osc_text(s: &mut Settings, port: &mut String, group: &mut String) {
    match port.trim().parse::<u16>() {
        Ok(n) if n > 0 => s.osc_port = n,
        _ => *port = s.osc_port.to_string(),
    }
    s.osc_multicast = group.trim().to_string();
}

impl App {
    /// The app's own windows, each a desktop window of its own, shown every pass while open.
    /// A window's lists and prompts come after it, as they belong to it.
    pub fn windows(&mut self, ctx: &Context) {
        self.editor_windows(ctx);
        self.message_window(ctx);
        self.confirm_window(ctx);
        if self.dlg.misc {
            self.misc_window(ctx);
        }
        if self.dlg.audio.is_some() {
            self.audio_window(ctx);
        }
        if self.dlg.plugins.is_some() {
            self.plugin_list_window(ctx);
        }
        if self.dlg.colours.is_some() {
            self.colours_window(ctx);
        }
        if self.dlg.app_maps {
            self.app_maps_window(ctx);
        }
        if self.dlg.organiser {
            self.organiser_window(ctx);
        }
        // The organiser's "Select patch..." box belongs to it and closes with it.
        if !self.dlg.organiser {
            self.dlg.import = None;
        }
        if self.dlg.import.is_some() {
            self.import_window(ctx);
        }
        if self.dlg.presets.is_some() {
            self.presets_window(ctx);
        }
        if self.dlg.presets.as_ref().is_some_and(|m| m.prompting()) {
            self.preset_prompt(ctx);
        }
        if self.dlg.mappings.is_some() {
            self.mappings_window(ctx);
        }
        if self.dlg.log {
            self.log_window(ctx);
        }
        if self.dlg.about || self.dlg.docs {
            self.info_windows(ctx);
        }
        // While a prompt is up, the main window and the window that asked can't be used, as
        // with the original's modal AlertWindow.
        let mut blocked = Vec::new();
        if self.dlg.import.is_some() && subwin::shown(crate::editors::import_vid()) {
            blocked = vec![ViewportId::ROOT, self.organiser_vid()];
        }
        if let Some(m) = self.dlg.presets.as_ref().filter(|m| m.prompting() && subwin::shown(crate::editors::prompt_vid(m))) {
            blocked = vec![ViewportId::ROOT, crate::editors::presets_vid(m)];
        }
        if self.dlg.app_draft.prompting() && subwin::shown(prompt_id()) {
            blocked.extend([ViewportId::ROOT, subwin::id("app_maps")]);
        }
        subwin::end_frame(ctx, &blocked);
    }

    /// Editor windows for built-ins and for plugins without an editor of their own. They keep
    /// their editor's size unless Misc Settings allow resizing windows.
    fn editor_windows(&mut self, ctx: &Context) {
        if self.dlg.editors.is_empty() {
            return;
        }
        let c = self.scheme.clone();
        let fixed = self.settings.fixed_size_windows;
        let on_top = self.settings.windows_on_top;
        let raise = self.dlg.raise.take();
        let (bar_w, bar_h) = (crate::presets::BAR_W as f32, crate::presets::BAR_H as f32);
        let mut keep = Vec::new();
        let mut events = Vec::new();
        for nid in std::mem::take(&mut self.dlg.editors) {
            let Some(n) = self.graph.node_mut(nid) else { continue };
            let body = match &n.kind {
                NodeKind::Builtin(b) => b.kind.editor_size(),
                NodeKind::Plugin(p) => generic_editor_size(p.num_params()),
                _ => continue,
            };
            // As in the original (0x140076240), the editor sits under a presets bar and the
            // window is at least as wide as the bar.
            let size = vec2(body.x.max(crate::presets::BAR_W as f32), body.y + bar_h);
            let slot = self.preset_bars.entry(nid).or_insert_with(|| {
                let mut s = crate::presets::BarSlot::default();
                s.bar.fill(n, &crate::presets::folder(&n.info));
                s
            });
            slot.bar.follow(n);
            let vid = subwin::id(("editor", nid));
            let wid = Id::new(("editor", nid));
            let title = n.title().to_string();
            let spec = Spec::new(vid, &title, Kind::fill(size, vec2(bar_w, bar_h), !fixed)).margin(Margin::ZERO).auto().on_top(on_top);
            let close = subwin::show(ctx, spec, |ui| {
                let want = if fixed { size } else { ui.available_size().max(vec2(bar_w, bar_h)) };
                let (r, _) = ui.allocate_exact_size(want, Sense::hover());
                let (top, r) = r.split_top_bottom_at_y(r.top() + bar_h);
                events.extend(crate::presets::bar_ui(ui, top, wid.with("bar"), slot, &c).into_iter().map(|e| (nid, e)));
                match &mut n.kind {
                    NodeKind::Builtin(b) => b.editor_ui(ui, r, wid.with("body"), &c),
                    NodeKind::Plugin(p) => generic_editor(ui, r, wid.with("body"), p.as_mut()),
                    _ => {}
                }
            });
            // The bar's list of presets opens under it as a window of its own.
            events.extend(crate::presets::bar_list(ctx, subwin::id(("editor_list", nid)), vid, slot, &c).map(|e| (nid, e)));
            if raise == Some(nid) {
                subwin::focus(ctx, vid);
            }
            if close {
                slot.dd.close();
            } else {
                keep.push(nid);
            }
        }
        self.dlg.editors = keep;
        for (nid, e) in events {
            self.bar_event(nid, e);
        }
    }

    /// Help > Documentation, as in the original (its case at 0x140038f08): the page
    /// "documentation/index.html" beside the program opens in the default browser. Where that page
    /// isn't there, the original says "Documentation Missing"; the rebuild ships no such page, so it
    /// shows its own short guide instead.
    pub fn open_documentation(&mut self) {
        let exe = std::env::current_exe().ok();
        if !docs_page(exe.as_deref()).is_some_and(|page| open_in_browser(&page)) {
            self.dlg.docs = true;
        }
    }

    fn message_window(&mut self, ctx: &Context) {
        let Some((title, text)) = self.dlg.messages.first().cloned() else { return };
        let mut ok = false;
        let spec = Spec::new(subwin::id(("message", self.dlg.message_no)), &title, Kind::fit_between(300.0, 460.0));
        let close = subwin::show(ctx, spec, |ui| {
            ui.label(&text);
            ui.add_space(6.0);
            // The original's plain box has one button, "ok" (showMessageBox 0x140218bd0, text at
            // 0x140425664), and it takes Return and Escape (createAlertWindow, 0x1401e4050).
            ok |= ui.button("   ok   ").clicked() || ui.input(|i| i.modifiers.is_none() && (i.key_pressed(Key::Enter) || i.key_pressed(Key::Escape)));
        });
        if ok || close {
            self.dlg.messages.remove(0);
            self.dlg.message_no += 1;
        }
    }

    /// The original's "Closing document..." box (FileBasedDocument::saveIfNeededAndUserAgrees,
    /// 0x1402324e0, through AlertWindow::showYesNoCancelBox, 0x140219400). Its keys come from
    /// createAlertWindow (0x1401e4050): S saves, D discards, Escape or the close button cancels,
    /// and Return does nothing.
    fn confirm_window(&mut self, ctx: &Context) {
        let Some(cmd) = self.dlg.confirm else { return };
        let name = doc_title(self.file.as_deref());
        let mut answer = None;
        let spec = Spec::new(subwin::id("confirm"), "Closing document...", Kind::fit_between(300.0, 460.0));
        let close = subwin::show(ctx, spec, |ui| {
            ui.label(format!("Do you want to save the changes to \"{name}\"?"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("save").clicked() {
                    answer = Some(0);
                }
                if ui.button("discard changes").clicked() {
                    answer = Some(1);
                }
                if ui.button("cancel").clicked() {
                    answer = Some(2);
                }
            });
            let key = ui.input(|i| {
                let k = |key| i.modifiers.is_none() && i.key_pressed(key);
                [(Key::S, 0), (Key::D, 1), (Key::Escape, 2)].into_iter().find(|(key, _)| k(*key)).map(|(_, a)| a)
            });
            answer = answer.or(key);
        });
        // Closing the box is the same as Cancel.
        if close && answer.is_none() {
            answer = Some(2);
        }
        match answer {
            Some(0) => {
                self.dlg.confirm = None;
                if self.save(false) {
                    self.run_now(cmd);
                }
            }
            Some(1) => {
                self.dlg.confirm = None;
                self.dirty = false;
                self.run_now(cmd);
            }
            Some(_) => self.dlg.confirm = None,
            None => {}
        }
    }

    /// Misc Settings, laid out as the original's window: its size (560x530, set at 0x140085077),
    /// each control's place (resized, 0x140085670), its labels (the window's constructor,
    /// 0x140081dc0) and the framed panel behind the I/O switches (paint, 0x140085450).
    fn misc_window(&mut self, ctx: &Context) {
        let before = self.settings.clone();
        let (port_text, group_text) = self.dlg.misc_text.get_or_insert_with(|| (before.osc_port.to_string(), before.osc_multicast.clone()));
        let s = &mut self.settings;
        let c = &self.scheme;
        let spec = Spec::new(subwin::id("misc"), "Misc Settings", Kind::fit(560.0).sizable()).margin(Margin::ZERO).escape();
        let close = subwin::show(ctx, spec, |ui| {
            let (area, _) = ui.allocate_exact_size(vec2(560.0, 530.0), Sense::hover());
            let at = |x: f32, y: f32, w: f32| Rect::from_min_size(area.min + vec2(x, y), vec2(w, 24.0));
            let p = ui.painter().clone();
            let panel = Rect::from_min_size(area.min + vec2(12.0, 132.0), vec2(560.0 - 24.0, 82.0));
            p.rect_filled(panel, CornerRadius::ZERO, c.get(colours::DIALOG_INNER));
            p.rect_stroke(panel, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_black_alpha(0x40)), StrokeKind::Inside);
            let text = c.get(colours::TEXT);
            let label = |x: f32, y: f32, w: f32, s: &str, f: FontId| {
                let r = at(x, y, w);
                text_at(&p, r.left() + 5.0, baseline(r, f.size), Align::Min, s, f, text);
            };
            label(0.0, 8.0, 208.0, "Open Sound Control Options", bold(15.0));
            label(8.0, 40.0, 72.0, "OSC Port:", font(15.0));
            label(8.0, 72.0, 160.0, "OSC Multicast Address:", font(15.0));
            label(280.0, 72.0, 272.0, "(leave blank for a one-to-one connection)", font(15.0));
            label(0.0, 104.0, 136.0, "Visible I/O Nodes", bold(15.0));
            label(0.0, 224.0, 104.0, "Midi Options", bold(15.0));
            label(0.0, 304.0, 150.0, "Other Options", bold(15.0));

            // The text boxes apply on Return or when they lose focus, as the original's do.
            let edit = |ui: &mut Ui, r: Rect, t: &mut String| ui.put(r, TextEdit::singleline(t).font(font(15.0)).min_size(r.size())).lost_focus();
            if edit(ui, at(80.0, 40.0, 64.0), port_text) | edit(ui, at(168.0, 72.0, 112.0), group_text) {
                commit_osc_text(s, port_text, group_text);
            }

            let tray = s.use_tray_icon;
            let toggles = [
                (136.0, 96.0, &mut s.audio_input, "Audio Input", true),
                (160.0, 88.0, &mut s.midi_input, "Midi Input", true),
                (184.0, 88.0, &mut s.osc_input, "OSC Input", true),
                (248.0, 288.0, &mut s.midi_program_change, "Program Change messages switch patches", true),
                (272.0, 232.0, &mut s.mmc_transport, "Main transport responds to MMC", true),
                (328.0, 376.0, &mut s.auto_mappings_window, "Open mappings window on successful param connection", true),
                (352.0, 208.0, &mut s.loop_patches, "Loop next/prev patch action", true),
                (376.0, 256.0, &mut s.windows_on_top, "Set plugin windows Always On Top", true),
                (400.0, 176.0, &mut s.ignore_pin_names, "Ignore plugin pin names", true),
                (424.0, 200.0, &mut s.use_tray_icon, "Display tray icon (not OSX)", true),
                (448.0, 168.0, &mut s.start_in_tray, "Start in tray (not OSX)", tray),
                (472.0, 224.0, &mut s.fixed_size_windows, "Force fixed-size plugin windows", true),
                (496.0, 224.0, &mut s.pdl_audio_settings, "Save audio settings in .pdl files", true),
            ];
            for (y, w, on, name, enabled) in toggles {
                let r = at(16.0, y, w);
                ui.scope_builder(UiBuilder::new().max_rect(r.with_min_x(r.left() + 4.0)).layout(Layout::left_to_right(Align::Center)), |ui| {
                    ui.add_enabled(enabled, Checkbox::new(on, RichText::new(name).font(font(15.0))));
                });
            }
        });
        self.dlg.misc = !close;
        if close {
            if let Some((mut port, mut group)) = self.dlg.misc_text.take() {
                commit_osc_text(&mut self.settings, &mut port, &mut group);
            }
        }
        let s = self.settings.clone();
        if s == before {
            return;
        }
        if (s.audio_input, s.midi_input, s.osc_input) != (before.audio_input, before.midi_input, before.osc_input) {
            let ctx2 = self.ctx();
            let removed = self.graph.ensure_io(&ctx2);
            self.rebuild();
            drop(removed);
            self.dirty = true;
        }
        if s.windows_on_top != before.windows_on_top {
            for n in &mut self.graph.nodes {
                if let Some(p) = n.plugin() {
                    p.set_editor_on_top(s.windows_on_top);
                }
            }
        }
        if (s.osc_input, s.osc_port, &s.osc_multicast) != (before.osc_input, before.osc_port, &before.osc_multicast) {
            self.open_osc();
        }
        if s.use_tray_icon != before.use_tray_icon {
            self.update_tray();
        }
        self.settings.save();
    }

    fn audio_window(&mut self, ctx: &Context) {
        let (mut apply, mut panel) = (false, false);
        let info = self.engine.info.clone();
        let Some(d) = self.dlg.audio.as_mut() else { return };
        let close = subwin::show(ctx, Spec::new(subwin::id("audio"), "Audio Settings", Kind::fit(480.0).sizable()).escape(), |ui| {
            let asio = d.cfg.backend == "ASIO";
            let same = info.as_ref().is_some_and(|i| i.backend == d.cfg.backend && (d.cfg.device.is_empty() || i.name == d.cfg.device));
            Grid::new("audio_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Audio system:");
                ComboBox::from_id_salt("backend").selected_text(&d.cfg.backend).width(300.0).show_ui(ui, |ui| {
                    for b in ["Windows Audio", "ASIO"] {
                        if ui.selectable_value(&mut d.cfg.backend, b.to_string(), b).changed() {
                            d.cfg.device.clear();
                            d.cfg.input_device.clear();
                        }
                    }
                });
                ui.end_row();
                ui.label(if asio { "ASIO driver:" } else { "Output:" });
                let shown = if d.cfg.device.is_empty() { "(default device)".to_string() } else { d.cfg.device.clone() };
                ComboBox::from_id_salt("device").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                    if !asio {
                        ui.selectable_value(&mut d.cfg.device, String::new(), "(default device)");
                    }
                    for n in if asio { &d.asio } else { &d.outs } {
                        ui.selectable_value(&mut d.cfg.device, n.clone(), n);
                    }
                    if asio && d.asio.is_empty() {
                        ui.label("No ASIO drivers are installed.");
                    }
                });
                ui.end_row();
                if !asio {
                    ui.label("Input:");
                    let shown = if d.cfg.input_device.is_empty() { "(default device)".to_string() } else { d.cfg.input_device.clone() };
                    ComboBox::from_id_salt("input").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.cfg.input_device, String::new(), "(default device)");
                        ui.selectable_value(&mut d.cfg.input_device, wasapi::NO_INPUT.to_string(), wasapi::NO_INPUT);
                        for n in &d.ins {
                            ui.selectable_value(&mut d.cfg.input_device, n.clone(), n);
                        }
                    });
                    ui.end_row();
                }
                let rates: Vec<f64> = match &info {
                    Some(i) if same && !i.rates.is_empty() => i.rates.clone(),
                    _ => vec![44100.0, 48000.0, 88200.0, 96000.0],
                };
                ui.label("Sample rate:");
                let shown = if d.cfg.sample_rate > 0.0 { format!("{} Hz", d.cfg.sample_rate) } else { "(device default)".into() };
                ComboBox::from_id_salt("rate").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.cfg.sample_rate, 0.0, "(device default)");
                    for r in rates {
                        ui.selectable_value(&mut d.cfg.sample_rate, r, format!("{r} Hz"));
                    }
                });
                ui.end_row();
                let blocks: Vec<u32> = match &info {
                    Some(i) if same && !i.blocks.is_empty() => i.blocks.clone(),
                    _ => vec![32, 64, 128, 256, 512, 1024],
                };
                let sr = info.as_ref().map_or(48000.0, |i| i.sample_rate).max(1.0);
                ui.label("Buffer size:");
                let shown = if d.cfg.buffer_size > 0 { format!("{} samples", d.cfg.buffer_size) } else { "(device default)".into() };
                ComboBox::from_id_salt("block").selected_text(shown).width(300.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.cfg.buffer_size, 0, "(device default)");
                    for b in blocks {
                        ui.selectable_value(&mut d.cfg.buffer_size, b, format!("{b} samples ({:.1} ms)", b as f64 * 1000.0 / sr));
                    }
                });
                ui.end_row();
            });
            if asio && ui.add_enabled(same, Button::new("Open the driver's control panel")).clicked() {
                panel = true;
            }
            if let Some(i) = info.as_ref().filter(|_| same) {
                for (title, names, active) in [("Active outputs:", &i.out_names, &mut d.cfg.output_channels), ("Active inputs:", &i.in_names, &mut d.cfg.input_channels)] {
                    if names.is_empty() {
                        continue;
                    }
                    ui.label(title);
                    ui.horizontal_wrapped(|ui| {
                        for (k, n) in names.iter().enumerate() {
                            let mut on = active.contains(&(k as u32));
                            if ui.checkbox(&mut on, n).changed() {
                                active.retain(|c| *c != k as u32);
                                if on {
                                    active.push(k as u32);
                                    active.sort_unstable();
                                }
                            }
                        }
                    });
                }
                ui.separator();
                let ms = |n: f64| n * 1000.0 / i.sample_rate.max(1.0);
                ui.label(format!("Running: {} ({}), {} Hz, {} samples per buffer", i.name, i.backend, i.sample_rate, i.block));
                ui.label(format!(
                    "Latency reported by the device: input {:.1} ms, output {:.1} ms; round trip about {:.1} ms",
                    ms(i.in_latency as f64),
                    ms(i.out_latency as f64),
                    ms((i.in_latency + i.out_latency) as f64 + i.block as f64)
                ));
            } else {
                ui.label("Press Apply to open this device and see its channels.");
            }
            ui.separator();
            ui.label("MIDI inputs:");
            if d.midi_ports.is_empty() {
                ui.label("(none found)");
            }
            let all = d.midi_on.is_empty();
            let mut changed = false;
            let mut on: Vec<bool> = d.midi_ports.iter().map(|p| all || d.midi_on.contains(p)).collect();
            for (k, p) in d.midi_ports.iter().enumerate() {
                changed |= ui.checkbox(&mut on[k], p).changed();
            }
            if changed {
                let chosen: Vec<String> = d.midi_ports.iter().zip(&on).filter(|(_, o)| **o).map(|(p, _)| p.clone()).collect();
                d.midi_on = if chosen.len() == d.midi_ports.len() {
                    Vec::new()
                } else if chosen.is_empty() {
                    vec![crate::midi::NONE.to_string()]
                } else {
                    chosen
                };
            }
            ui.add_space(6.0);
            apply |= ui.button("  Apply  ").clicked();
        });
        if panel {
            self.engine.control_panel();
        }
        if apply {
            if let Some(d) = &self.dlg.audio {
                self.settings.audio = d.cfg.clone();
                self.settings.midi_inputs = d.midi_on.clone();
            }
            if let Err(e) = self.open_device() {
                self.message("Audio Device Error", &e);
            }
            if let (Some(d), Some(i)) = (self.dlg.audio.as_mut(), &self.engine.info) {
                d.cfg.output_channels = i.active_outs.clone();
                d.cfg.input_channels = i.active_ins.clone();
            }
            self.open_midi();
            self.settings.save();
        }
        if close {
            self.dlg.audio = None;
        }
    }

    fn colours_window(&mut self, ctx: &Context) {
        let (mut load, mut save, mut delete, mut reset) = (None, false, false, false);
        let Some(d) = self.dlg.colours.as_mut() else { return };
        let scheme = &mut self.scheme;
        let close = subwin::show(ctx, Spec::new(subwin::id("colours"), "Colour Schemes", Kind::fit(380.0).sizable()).escape(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Scheme:");
                ComboBox::from_id_salt("schemes").selected_text(&scheme.name).show_ui(ui, |ui| {
                    for n in &d.names {
                        if ui.selectable_label(*n == scheme.name, n).clicked() {
                            load = Some(n.clone());
                        }
                    }
                });
            });
            ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                Grid::new("colour_grid").num_columns(2).striped(true).show(ui, |ui| {
                    for (i, (n, _)) in colours::NAMES.iter().enumerate() {
                        ui.label(*n);
                        if let Some(c) = scheme.colours.get_mut(i) {
                            color_edit_button_srgba(ui, c, Alpha::OnlyBlend);
                        }
                        ui.end_row();
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Save as:");
                ui.text_edit_singleline(&mut d.name);
                save |= ui.add_enabled(!d.name.trim().is_empty() && d.name.trim() != "default", Button::new("Save")).clicked();
            });
            ui.horizontal(|ui| {
                reset |= ui.button("Default colours").clicked();
                delete |= scheme.name != "default" && ui.button("Delete this scheme").clicked();
            });
        });
        let dir = settings::dir();
        if let Some(n) = load {
            self.scheme = crate::dialogs::load_scheme(&n).unwrap_or_default();
            self.settings.colour_scheme = self.scheme.name.clone();
            self.settings.save();
        }
        if reset {
            let name = self.scheme.name.clone();
            self.scheme = Scheme { name, ..Default::default() };
        }
        if save {
            let name: String = d_name(&self.dlg).chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).collect();
            let _ = std::fs::create_dir_all(&dir);
            self.scheme.name = name.clone();
            match std::fs::write(dir.join(format!("{name}.colourscheme")), self.scheme.to_xml()) {
                Ok(()) => {
                    self.settings.colour_scheme = name;
                    self.settings.save();
                }
                Err(e) => self.message("File error", &format!("Could not save the colour scheme: {e}")),
            }
        }
        if delete {
            let _ = std::fs::remove_file(dir.join(format!("{}.colourscheme", self.scheme.name)));
            self.scheme = Scheme::default();
            self.settings.colour_scheme = "default".into();
            self.settings.save();
        }
        if save || delete {
            if let Some(d) = self.dlg.colours.as_mut() {
                d.names = list_schemes();
            }
        }
        if close {
            self.dlg.colours = None;
        }
    }

    fn log_window(&mut self, ctx: &Context) {
        let mut clear = false;
        let log = &self.log;
        // The original keeps its Event Log above every window (the non-modal helper 0x140030b30
        // called with alwaysOnTop set at 0x1400392d1).
        let spec = Spec::new(subwin::id("log"), "Event Log", Kind::fill(vec2(520.0, 320.0), vec2(300.0, 160.0), true)).on_top(true).escape();
        let close = subwin::show(ctx, spec, |ui| {
            ui.horizontal(|ui| {
                clear |= ui.button("Clear").clicked();
                if ui.button("Copy all").clicked() {
                    ui.ctx().copy_text(log.iter().cloned().collect::<Vec<_>>().join("\n"));
                }
            });
            ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                for l in log {
                    ui.label(RichText::new(l).monospace());
                }
            });
        });
        if clear {
            self.log.clear();
        }
        self.dlg.log = !close;
    }

    fn info_windows(&mut self, ctx: &Context) {
        if self.dlg.about {
            let info = self.engine.info.as_ref();
            let close = subwin::show(ctx, Spec::new(subwin::id("about"), "About", Kind::fit_between(300.0, 420.0).sizable()).escape(), |ui| {
                ui.heading(crate::app::TITLE);
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("A live effects host for VST and VST3 plugins, with ASIO and Windows audio.");
                ui.label("Built with Rust and egui.");
                if let Some(i) = info {
                    ui.label(format!("Audio: {} ({}), {} Hz, {} samples", i.name, i.backend, i.sample_rate, i.block));
                }
            });
            self.dlg.about = !close;
        }
        if self.dlg.docs {
            let close = subwin::show(ctx, Spec::new(subwin::id("docs"), "Documentation", Kind::fit(460.0)).escape(), |ui| {
                for line in [
                    "Double-click an empty part of the pedalboard to add a built-in processor or a plugin.",
                    "Drag from an output pin to an input pin to connect two boxes. Click a cable to select it (Ctrl-click adds more) and press Delete to remove it.",
                    "Connect Midi Input or OSC Input to a box's param pin, then use the mappings button to tie controllers to its parameters.",
                    "In Options > Application Mappings, click + beside a command to set a shortcut, learn a MIDI controller, or assign an OSC address. Click an assignment to remove it.",
                    "Switch patches with the patch selector or the - and + buttons; Edit > Organise patches renames and reorders them.",
                    "Options > Plugin List scans your VST and VST3 folders. Options > Audio Settings picks the device; ASIO with a small buffer gives the lowest latency.",
                    "The bypass button passes audio straight through a box; the cross removes it.",
                    "Put the original program's documentation folder next to this program, and Documentation opens its full guide in your browser instead.",
                ] {
                    ui.label(line);
                    ui.add_space(4.0);
                }
            });
            self.dlg.docs = !close;
        }
    }
}

fn d_name(d: &Dialogs) -> String {
    d.colours.as_ref().map(|c| c.name.trim().to_string()).unwrap_or_default()
}

/// The original's documentation page: "index.html" in a "documentation" folder beside the
/// program (getSpecialLocation 7, getParentDirectory, getChildFile, 0x140038f08). It counts only
/// when it's a file, as in the original's check (GetFileAttributesW without the folder flag).
fn docs_page(exe: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    let page = exe?.parent()?.join("documentation").join("index.html");
    page.is_file().then_some(page)
}

/// Opens a page the way the original's URL::launchInDefaultBrowser (0x14011d660) does: the
/// shell's default action for it, shown as the program was asked (SW_SHOWDEFAULT, 10). True when
/// the shell took it, a result above 32.
fn open_in_browser(page: &std::path::Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWDEFAULT;
    let file: Vec<u16> = page.as_os_str().encode_wide().chain(Some(0)).collect();
    let null = std::ptr::null();
    let done = unsafe { ShellExecuteW(std::ptr::null_mut(), null, file.as_ptr(), null, null, SW_SHOWDEFAULT) };
    done as isize > 32
}

/// What the save question calls the patch file (getDocumentTitle, 0x14002cd10): its name without
/// the extension, or "Unnamed" (0x14040ffc0) while there's no file on the disk.
fn doc_title(file: Option<&std::path::Path>) -> String {
    match file.filter(|f| f.exists()).and_then(|f| f.file_stem()) {
        Some(n) => n.to_string_lossy().into_owned(),
        None => "Unnamed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_save_question_names_the_file_or_unnamed() {
        let f = std::env::temp_dir().join(format!("pedalhost-title-{}.pdl", std::process::id()));
        std::fs::write(&f, "x").unwrap();
        assert_eq!(doc_title(Some(&f)), format!("pedalhost-title-{}", std::process::id()));
        std::fs::remove_file(&f).unwrap();
        // A file that has gone, or none yet, is "Unnamed".
        assert_eq!(doc_title(Some(&f)), "Unnamed");
        assert_eq!(doc_title(None), "Unnamed");
    }

    #[test]
    fn documentation_is_the_page_beside_the_program_only_when_it_is_a_file() {
        let dir = std::env::temp_dir().join(format!("pedalhost-docs-{}", std::process::id()));
        let exe = dir.join("pedalhost.exe");
        let page = dir.join("documentation").join("index.html");
        assert_eq!(docs_page(Some(&exe)), None);
        // A folder with the page's name isn't the page.
        std::fs::create_dir_all(&page).unwrap();
        assert_eq!(docs_page(Some(&exe)), None);
        std::fs::remove_dir(&page).unwrap();
        std::fs::write(&page, "<html></html>").unwrap();
        assert_eq!(docs_page(Some(&exe)), Some(page.clone()));
        assert_eq!(docs_page(None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn osc_boxes_keep_a_usable_port_and_trim_the_group() {
        let mut s = Settings::default();
        let (mut port, mut group) = ("9000 ".to_string(), " 239.0.0.7 ".to_string());
        commit_osc_text(&mut s, &mut port, &mut group);
        assert_eq!((s.osc_port, s.osc_multicast.as_str()), (9000, "239.0.0.7"));
        for bad in ["0", "70000", "abc", ""] {
            let mut port = bad.to_string();
            commit_osc_text(&mut s, &mut port, &mut String::new());
            assert_eq!((s.osc_port, port.as_str(), s.osc_multicast.as_str()), (9000, "9000", ""));
        }
    }
}
