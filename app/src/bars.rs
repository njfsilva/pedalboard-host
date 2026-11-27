//! The menu bar and the bottom bar (patch selector, tempo, transport, CPU meter), laid out as
//! MainPanel::resized does (0x1400358a0) and measured from the original's picture.

use crate::app::{App, Cmd};
use crate::colours::{self, Scheme};
use crate::widgets::{bold, button, combo_face, font, gradient_rrect, text_at, text_w, triangle};
use egui::*;
use std::sync::atomic::Ordering;

pub const BAR_FONT: f32 = 15.0;
/// Verdana's ascent as a share of its full height; JUCE centres text by the full height.
const ASCENT: f32 = 2059.0 / 2489.0;
/// The program change notice: its content size (0x1400315fd) and how long it stays (0x14003a1aa).
const NOTICE_SIZE: Vec2 = vec2(250.0, 150.0);
const NOTICE_SECS: f64 = 5.0;

#[derive(Default)]
pub struct MenuState {
    pub open: Option<usize>,
}

const MENUS: [&str; 4] = ["File", "Edit", "Options", "Help"];
type Entry = (&'static str, Option<Cmd>);

const FILE: &[Entry] = &[
    ("New", Some(Cmd::New)),
    ("Open...", Some(Cmd::Open)),
    ("Save", Some(Cmd::Save)),
    ("Save As...", Some(Cmd::SaveAs)),
    ("", None),
    ("Save As Default", Some(Cmd::SaveAsDefault)),
    ("Reset Default", Some(Cmd::ResetDefault)),
    ("", None),
    ("Exit", Some(Cmd::Exit)),
];
const EDIT: &[Entry] = &[
    ("Delete selected connection(s)", Some(Cmd::DeleteConnections)),
    ("", None),
    ("Organise patches", Some(Cmd::OrganisePatches)),
    ("User Preset Management", Some(Cmd::UserPresets)),
];
const OPTIONS: &[Entry] = &[
    ("Misc Settings", Some(Cmd::MiscSettings)),
    ("Audio Settings", Some(Cmd::AudioSettings)),
    ("Plugin List", Some(Cmd::PluginList)),
    ("Colour Schemes", Some(Cmd::ColourSchemes)),
    ("Application Mappings", Some(Cmd::AppMappings)),
];
const HELP: &[Entry] = &[("Documentation", Some(Cmd::Documentation)), ("Event Log", Some(Cmd::EventLog)), ("", None), ("About", Some(Cmd::About))];

/// Baseline of text centred in `r` the way a JUCE label centres it.
pub fn baseline(r: Rect, size: f32) -> f32 {
    r.top() + (r.height() - size) / 2.0 + size * ASCENT
}

/// One popup row: label (empty = separator), shortcut text, enabled, ticked.
pub type Row = (String, String, bool, bool);

/// A popup list in the original's style. `above` is where to put it if it doesn't fit below.
/// Returns the chosen row and the popup's rectangle.
pub fn popup_list(ui: &Ui, id: Id, at: Pos2, above: Option<f32>, rows: &[Row], c: &Scheme) -> (Option<usize>, Rect) {
    popup_list_min(ui, id, at, above, rows, c, 140.0)
}

/// The size of a popup list at least `min_w` wide.
pub fn popup_size(ctx: &Context, rows: &[Row], min_w: f32) -> Vec2 {
    let tw = |s: &str, f: FontId| ctx.fonts(|fo| fo.layout_no_wrap(s.to_string(), f, Color32::BLACK).size().x);
    let lw = rows.iter().map(|r| tw(&r.0, font(BAR_FONT))).fold(0.0f32, f32::max);
    let sw = rows.iter().map(|r| tw(&r.1, font(13.0))).fold(0.0f32, f32::max);
    let h: f32 = rows.iter().map(|r| if r.0.is_empty() { 8.0 } else { 22.0 }).sum::<f32>() + 4.0;
    vec2((lw + sw + 56.0).max(min_w), h)
}

/// A popup list at least `min_w` wide.
pub fn popup_list_min(ui: &Ui, id: Id, at: Pos2, above: Option<f32>, rows: &[Row], c: &Scheme, min_w: f32) -> (Option<usize>, Rect) {
    let Vec2 { x: w, y: h } = popup_size(ui.ctx(), rows, min_w);
    let screen = ui.ctx().screen_rect();
    let mut pos = at;
    if pos.y + h > screen.bottom() {
        pos.y = match above {
            Some(top) => (top - h).max(screen.top()),
            None => (screen.bottom() - h).max(screen.top()),
        };
    }
    pos.x = pos.x.min(screen.right() - w).max(screen.left());
    let mut chosen = None;
    let area = Area::new(id).order(Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.painter().rect_filled(r.translate(vec2(2.0, 3.0)), CornerRadius::same(2), Color32::from_black_alpha(40));
        chosen = popup_rows(ui, r, id, rows, c, None).0;
    });
    (chosen, area.response.rect)
}

/// A popup list's frame and rows in `r`, the `hot` row (picked with the keys) lit as the one
/// under the mouse is. Returns the row clicked and the row under the mouse.
pub fn popup_rows(ui: &Ui, r: Rect, id: Id, rows: &[Row], c: &Scheme, hot: Option<usize>) -> (Option<usize>, Option<usize>) {
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::ZERO, c.get(colours::DIALOG_INNER));
    p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_gray(150)), StrokeKind::Inside);
    let text = c.get(colours::TEXT);
    let (mut clicked, mut hovered) = (None, None);
    let mut y = r.top() + 2.0;
    for (k, row) in rows.iter().enumerate() {
        if row.0.is_empty() {
            p.hline((r.left() + 6.0)..=(r.right() - 6.0), y + 4.0, Stroke::new(1.0f32, Color32::from_black_alpha(50)));
            y += 8.0;
            continue;
        }
        let ir = Rect::from_min_size(pos2(r.left() + 2.0, y), vec2(r.width() - 4.0, 22.0));
        y += 22.0;
        let resp = ui.interact(ir, id.with(k), Sense::click());
        let col = if row.2 { text } else { text.gamma_multiply(0.4) };
        if row.2 && resp.hovered() {
            hovered = Some(k);
        }
        if row.2 && (resp.hovered() || hot == Some(k)) {
            p.rect_filled(ir, CornerRadius::ZERO, c.get(colours::MENU_SELECTION));
        }
        if row.3 {
            let m = pos2(ir.left() + 9.0, ir.center().y);
            p.line_segment([m + vec2(-4.0, 0.0), m + vec2(-1.0, 4.0)], Stroke::new(2.0f32, col));
            p.line_segment([m + vec2(-1.0, 4.0), m + vec2(5.0, -5.0)], Stroke::new(2.0f32, col));
        }
        text_at(p, ir.left() + 20.0, baseline(ir, BAR_FONT), Align::Min, &row.0, font(BAR_FONT), col);
        if !row.1.is_empty() {
            text_at(p, ir.right() - 8.0, baseline(ir, 13.0), Align::Max, &row.1, font(13.0), col.gamma_multiply(0.7));
        }
        if row.2 && resp.clicked() {
            clicked = Some(k);
        }
    }
    (clicked, hovered)
}

pub(crate) fn pressed_outside(ui: &Ui, keep: &[Rect]) -> bool {
    ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|m| !keep.iter().any(|r| r.contains(m))))
}

impl App {
    pub fn menu_bar(&mut self, ui: &mut Ui, r: Rect) {
        let p = ui.painter().clone();
        gradient_rrect(&p, r, 0.0, Color32::from_rgb(0xF3, 0xF3, 0xEC), Color32::from_rgb(0xE4, 0xE2, 0xD8));
        p.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0f32, Color32::from_rgb(0xBE, 0xBD, 0xB4)));
        let text = self.scheme.get(colours::TEXT);
        let mut x = r.left();
        let mut items = Vec::new();
        for (i, name) in MENUS.iter().enumerate() {
            let w = text_w(ui, name, font(BAR_FONT)) + 16.0;
            let ir = Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height() - 1.0));
            x += w;
            let resp = ui.interact(ir, Id::new(("menubar", i)), Sense::click());
            if resp.clicked() {
                self.menu.open = if self.menu.open == Some(i) { None } else { Some(i) };
            } else if self.menu.open.is_some() && resp.hovered() {
                self.menu.open = Some(i);
            }
            if self.menu.open == Some(i) || resp.hovered() {
                p.rect_filled(ir, CornerRadius::ZERO, self.scheme.get(colours::MENU_SELECTION));
            }
            text_at(&p, ir.left() + 8.0, baseline(Rect::from_min_size(ir.min, vec2(w, r.height())), BAR_FONT), Align::Min, name, font(BAR_FONT), text);
            items.push(ir);
        }
        let Some(open) = self.menu.open else { return };
        let list = match open {
            0 => FILE,
            1 => EDIT,
            2 => OPTIONS,
            _ => HELP,
        };
        let rows: Vec<Row> = list
            .iter()
            .map(|(l, c)| {
                let enabled = match c {
                    Some(Cmd::DeleteConnections) => !self.field.selected.is_empty(),
                    _ => true,
                };
                (l.to_string(), c.map_or_else(String::new, |cmd| self.mappings.menu_keys(cmd)), enabled, false)
            })
            .collect();
        let (chosen, popup) = popup_list(ui, Id::new("menu_popup"), pos2(items[open].left(), r.bottom()), None, &rows, &self.scheme);
        if chosen.is_some() || pressed_outside(ui, &[popup, r]) || ui.input(|i| i.key_pressed(Key::Escape)) {
            self.menu.open = None;
        }
        if let Some(c) = chosen.and_then(|k| list[k].1) {
            self.pending.push(c);
        }
    }

    pub fn bottom_bar(&mut self, ui: &mut Ui, panel: Rect) {
        let c = self.scheme.clone();
        let p = ui.painter().clone();
        let (w, h) = (panel.width(), panel.height());
        let at = |x: f32, y: f32, ww: f32, hh: f32| Rect::from_min_size(panel.min + vec2(x, y), vec2(ww, hh));
        let cx = (w / 2.0).round();
        let text = c.get(colours::TEXT);
        let label = |ui: &Ui, r: Rect, s: &str| {
            // Labels squash long text to fit, as JUCE labels do.
            let inner = r.width() - 10.0;
            let tw = text_w(ui, s, font(BAR_FONT));
            let size = if tw > inner { BAR_FONT * inner / tw } else { BAR_FONT };
            text_at(&p, r.left() + 5.0, baseline(r, BAR_FONT), Align::Min, s, font(size), text);
        };
        let rim = Color32::from_rgb(0xAB, 0xAA, 0xA5);

        // Patch selector.
        label(ui, at(8.0, h - 33.0, 48.0, 24.0), "Patch:");
        let cr = at(55.0, h - 33.0, 198.0, 24.0);
        self.patch_combo = cr;
        let combo = ui.interact(cr, Id::new("patch_combo"), Sense::click());
        let text_area = combo_face(&p, cr, &c);
        let name = self.patches.get(self.current).map(|x| x.name.clone()).unwrap_or_default();
        let shown = format!("{} - {}", self.current + 1, name);
        let clip = p.with_clip_rect(text_area);
        text_at(&clip, cr.left() + 6.0, baseline(cr, BAR_FONT), Align::Min, &shown, font(BAR_FONT), text);
        if combo.clicked() {
            self.combo_open = !self.combo_open;
        }
        if self.combo_open {
            let mut rows: Vec<Row> = self.patches.iter().enumerate().map(|(i, x)| (format!("{} - {}", i + 1, x.name), String::new(), true, i == self.current)).collect();
            rows.push(("<new patch>".into(), String::new(), true, false));
            let (chosen, popup) = popup_list(ui, Id::new("patch_popup"), cr.left_bottom(), Some(cr.top()), &rows, &c);
            if let Some(k) = chosen {
                self.pending.push(if k < self.patches.len() { Cmd::SelectPatch(k) } else { Cmd::NewPatch });
            }
            if chosen.is_some() || pressed_outside(ui, &[popup, cr]) || ui.input(|i| i.key_pressed(Key::Escape)) {
                self.combo_open = false;
            }
        }
        for (r, s, tip, cmd) in [(at(260.0, h - 33.0, 24.0, 24.0), "-", "Previous patch", Cmd::PrevPatch), (at(284.0, h - 33.0, 24.0, 24.0), "+", "Next patch", Cmd::NextPatch)] {
            if button(ui, r, Id::new(tip), 3.0, &c, tip).clicked() {
                self.pending.push(cmd);
            }
            text_at(&p, r.center().x, baseline(r, 14.4), Align::Center, s, font(14.4), text);
        }

        // Tempo and tap tempo.
        label(ui, at(cx - 169.0, h - 33.0, 64.0, 24.0), "Tempo:");
        let er = at(cx - 104.0, h - 33.0, 54.0, 24.0);
        p.rect_filled(er, CornerRadius::ZERO, c.get(colours::TEXT_EDITOR));
        p.rect_stroke(er, CornerRadius::ZERO, Stroke::new(1.0f32, rim), StrokeKind::Inside);
        let te_rect = Rect::from_min_max(pos2(er.left() + 5.0, er.top() + (24.0 - BAR_FONT) / 2.0), pos2(er.right() - 2.0, er.bottom() - 1.0));
        let te = ui
            .scope_builder(UiBuilder::new().max_rect(te_rect), |ui| {
                ui.add(TextEdit::singleline(&mut self.tempo_text).frame(false).margin(Margin::ZERO).font(font(BAR_FONT)).text_color(text).desired_width(te_rect.width()))
            })
            .inner;
        if te.changed() {
            self.tempo_text.retain(|ch| ch.is_ascii_digit() || ch == '.');
        }
        if te.lost_focus() {
            match self.tempo_text.trim().parse::<f64>() {
                Ok(v) if v > 0.0 => self.set_tempo(v),
                _ => self.apply_tempo(self.engine.shared.tempo()),
            }
        }
        let tr = at(cx - 49.0, h - 27.0, 10.0, 16.0);
        let tap = ui.interact(tr.expand(2.0), Id::new("tap"), Sense::click()).on_hover_text("Tap tempo");
        let tri = |e: f32| [pos2(tr.left() - e, tr.top() - e), pos2(tr.left() + 7.0 + e * 1.5, tr.top() + 6.5), pos2(tr.left() - e, tr.top() + 13.0 + e)];
        // ArrowButton look: the 0x40000000 arrow over a soft drop shadow.
        for (e, a) in [(2.0, 18u8), (1.0, 30)] {
            let [a1, b1, c1] = tri(e);
            triangle(&p, a1, b1, c1, Color32::from_black_alpha(a), Stroke::NONE);
        }
        let [a1, b1, c1] = tri(0.0);
        let down = tap.is_pointer_button_down_on();
        triangle(&p, a1, b1, c1, Color32::from_black_alpha(if down { 140 } else if tap.hovered() { 100 } else { 64 }), Stroke::NONE);
        if tap.hovered() && ui.input(|i| i.pointer.primary_pressed()) {
            // Taps count on the press, so the timing follows the finger.
            self.pending.push(Cmd::TapTempo);
        }

        // Transport.
        let vector = c.get(colours::VECTOR);
        let pr = at(cx - 18.0, h - 38.0, 36.0, 36.0);
        let playing = self.engine.shared.playing.load(Ordering::Relaxed);
        if button(ui, pr, Id::new("play"), 6.0, &c, "Play (main transport)").clicked() {
            self.pending.push(Cmd::PlayPause);
        }
        if playing {
            for x in [11.5, 19.5] {
                p.rect_filled(Rect::from_min_size(pr.min + vec2(x, 10.0), vec2(5.0, 16.0)), CornerRadius::ZERO, vector);
            }
        } else {
            triangle(&p, pr.min + vec2(12.0, 9.5), pr.min + vec2(24.5, 18.0), pr.min + vec2(12.0, 26.5), vector, Stroke::NONE);
        }
        let rr = at(cx + 20.0, h - 32.0, 24.0, 24.0);
        if button(ui, rr, Id::new("rtz"), 4.0, &c, "Return to zero (main transport)").clicked() {
            self.pending.push(Cmd::Rtz);
        }
        let m = rr.center();
        triangle(&p, m + vec2(-6.0, 0.0), m + vec2(3.0, -6.0), m + vec2(3.0, 6.0), vector, Stroke::NONE);
        p.rect_filled(Rect::from_min_size(m + vec2(3.5, -6.0), vec2(2.5, 12.0)), CornerRadius::ZERO, vector);

        // Keep the readout beside CPU Usage and shorten the meter on smaller windows.
        let latency_width = (text_w(ui, &self.latency_text, font(BAR_FONT)) + 10.0).min(165.0);
        let cpu_left = (w - 236.0).max(cx + 48.0 + latency_width);
        let latency_rect = at(cpu_left - latency_width, h - 33.0, latency_width, 24.0);
        label(ui, latency_rect, &self.latency_text);
        ui.interact(latency_rect, Id::new("chain_latency"), Sense::hover())
            .on_hover_text("Plug-in delay on the longest audio path to the output. Excludes audio device latency.");
        label(ui, at(cpu_left, h - 33.0, 78.0, 24.0), "CPU Usage:");
        let mr = at(cpu_left + 82.0, h - 33.0, w - cpu_left - 88.0, 24.0);
        let load = self.engine.cpu().clamp(0.0, 1.0);
        if load > 0.0 {
            let inner = mr.shrink(2.0);
            p.rect_filled(Rect::from_min_size(inner.min, vec2(inner.width() * load, inner.height())), CornerRadius::ZERO, c.get(colours::CPU_METER));
        }
        p.rect_stroke(mr, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_rgb(0xA8, 0xA7, 0xA4)), StrokeKind::Inside);
        p.hline((mr.left() + 1.0)..=(mr.right() - 1.0), mr.top() + 1.5, Stroke::new(1.0f32, Color32::from_rgb(0xB4, 0xB3, 0xAC)));
        p.vline(mr.left() + 1.5, (mr.top() + 2.0)..=(mr.bottom() - 1.0), Stroke::new(1.0f32, Color32::from_rgb(0xE1, 0xDF, 0xD6)));
        ui.interact(mr, Id::new("cpu"), Sense::hover()).on_hover_text(format!("Audio processing load: {:.0}%", load * 100.0));
    }

    /// The bubble the original points at the patch list for five seconds when a MIDI program
    /// change names a patch past the last one (0x14003a185; its content is painted by 0x140031640).
    pub fn program_notice(&mut self, ctx: &Context) {
        let Some((n, since)) = self.pc_notice else { return };
        let left = NOTICE_SECS - (ctx.input(|i| i.time) - since);
        if left <= 0.0 || !self.patch_combo.is_positive() {
            self.pc_notice = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        let c = &self.scheme;
        let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("program_notice")));
        // A JUCE call-out box (estimated): the content inside a 20 px border, its arrow tip on
        // the patch list's top edge, the whole box kept inside the window.
        let tip = pos2(self.patch_combo.center().x, self.patch_combo.top());
        let outer = NOTICE_SIZE + Vec2::splat(40.0);
        let screen = ctx.screen_rect();
        let x = (tip.x - outer.x / 2.0).clamp(screen.left(), (screen.right() - outer.x).max(screen.left()));
        let content = Rect::from_min_size(pos2(x, tip.y + 4.0 - outer.y) + Vec2::splat(20.0), NOTICE_SIZE);
        let body = content.expand(4.5);
        let (fill, edge) = (c.get(colours::WINDOW_BG), Stroke::new(1.5f32, c.get(colours::TEXT).gamma_multiply(0.5)));
        p.add(Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(80) }.as_shape(body, CornerRadius::same(9)));
        p.rect_filled(body, CornerRadius::same(9), fill);
        p.rect_stroke(body, CornerRadius::same(9), edge, StrokeKind::Middle);
        let ax = tip.x.clamp(body.left() + 15.0, body.right() - 15.0);
        let (l, r) = (pos2(ax - 5.6, body.bottom()), pos2(ax + 5.6, body.bottom()));
        // The arrow's fill covers the body's edge where they join.
        triangle(&p, l - vec2(0.0, 1.5), r - vec2(0.0, 1.5), tip, fill, Stroke::NONE);
        p.line_segment([l, tip], edge);
        p.line_segment([r, tip], edge);
        let text = c.get(colours::TEXT);
        let line = |y: f32, s: &str, f: FontId| {
            let r = Rect::from_min_size(content.min + vec2(0.0, y), vec2(NOTICE_SIZE.x, 50.0));
            text_at(&p, r.center().x, baseline(r, f.size), Align::Center, s, f, text);
        };
        line(0.0, "Out of bounds", font(24.0));
        line(24.0, "MIDI Program Change", font(24.0));
        line(48.0, "received:", font(24.0));
        line(88.0, &n.to_string(), bold(48.0));
    }
}
