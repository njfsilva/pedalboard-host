//! The pedalboard field: boxes, pins, cables, scrollbars and the add-processor menu, painted the
//! way the original does (box 0x140072960, size 0x140073d10, pins 0x140075630, cables 0x1400767e0).

use crate::bars::baseline;
use crate::builtins::Kind;
use crate::colours::{self, Scheme};
use crate::graph::{Graph, Node, NodeKind};
use crate::plugins::PluginInfo;
use crate::widgets::{bold, button, button_face, font, scrollbar, text_at, text_fit, text_w};
use egui::*;

pub const TITLE_FONT: f32 = 13.0;
pub const LABEL_FONT: f32 = 12.0;
pub const HINT_FONT: f32 = 14.0;
const PITCH: f32 = 13.0;
/// Pins and their labels step 12 px (0x140074e40), though each pin adds 13 px to the box height.
const PIN_STEP: f32 = 12.0;
/// First pin centre below `r.top()`: the box component starts 1 px in, the first pin sits at 25
/// and its centre is half a 12 px pin lower (0x140074e40).
const PIN0: f32 = 1.0 + 25.0 + 6.0;
const SB: f32 = 18.0;
const BTN: f32 = 16.0;

pub enum Action {
    AddBuiltin(Kind, Pos2),
    AddPlugin(usize, Pos2),
    /// (source box, destination box, param cable)
    Connected(u32, u32, bool),
    Moved,
    OpenEditor(u32),
    OpenMappings(u32),
    Bypass(u32),
    Delete(u32),
    DeleteConns,
    SetProgram(u32, i32),
    UserPresets(u32),
    /// A box got a new name of the person's choosing.
    Renamed,
}

#[derive(Clone, Copy)]
struct Pin {
    node: u32,
    out: bool,
    pin: usize,
    param: bool,
    at: Pos2,
}

#[derive(Default)]
pub struct Field {
    pub scroll: Vec2,
    pub selected: Vec<(u32, u32, u32, u32)>,
    wire: Option<Pin>,
    add_menu: Option<(Pos2, Pos2, bool)>,
    /// The box whose title is being edited, the text so far, and whether editing just began.
    rename: Option<(u32, String, bool)>,
}

/// Box size (0x140073d10): pins × 13 + 34 for the I/O boxes, + 60 for processors (room for the
/// button row). Built-ins are sized from their control alone (see `builtin_box`).
pub fn box_size(ui: &Ui, n: &Node) -> Vec2 {
    let title = text_w(ui, n.title(), bold(TITLE_FONT));
    let pins = n.in_pins().max(n.out_pins()) as f32;
    if let NodeKind::Builtin(b) = &n.kind {
        return builtin_box(title, b.kind.control_size());
    }
    let widest = |names: Vec<String>| names.iter().map(|s| text_w(ui, s, font(LABEL_FONT))).fold(0.0f32, f32::max);
    let iw = widest((0..n.in_pins()).map(|i| n.in_name(i)).collect());
    let ow = widest((0..n.out_pins()).map(|i| n.out_name(i)).collect());
    let mut w = (title + 20.0).max(iw + ow + 30.0);
    let h = if n.is_io() {
        pins * PITCH + 34.0
    } else {
        w = w.max(4.0 * (BTN + 2.0) + 20.0);
        pins * PITCH + 60.0
    };
    vec2(w.round(), h.round())
}

/// A built-in's box (0x140074d56..0x140074d91, which skips the pin loops): with t = title + 20,
/// it is t + 20 wide when t > control + 24, otherwise control + 24, and always control + 52 high.
fn builtin_box(title: f32, c: Vec2) -> Vec2 {
    let t = title + 20.0;
    let w = if t > c.x + 24.0 { t + 20.0 } else { c.x + 24.0 };
    vec2(w.trunc(), c.y + 52.0)
}

/// Where a built-in's control sits in its box (0x140072569..0x1400725bb): centred with both halves
/// truncated, 24 px from the top.
fn control_rect(r: Rect, cs: Vec2) -> Rect {
    Rect::from_min_size(r.min + vec2((r.width() / 2.0).trunc() - (cs.x / 2.0).trunc(), 24.0), cs)
}

/// A pin's centre (0x140074e40): pins are 10×12 components at x = -5 (inputs) or width - 5
/// (outputs) and y = 25 + 12k, so the centre sits on the box edge. `r.left() + 1` is the left edge
/// of the original component, whose top is at `r.top() + 1`.
fn pin_pos(r: Rect, out: bool, i: usize) -> Pos2 {
    let y = r.top() + PIN0 + PIN_STEP * i as f32;
    if out { pos2(r.right(), y) } else { pos2(r.left() + 1.0, y) }
}

/// Pins (0x140075630): an 8×10 ellipse with a 1 px black outline, filled with the connection
/// colour. The box clips it, so only the inner half shows.
fn paint_pin(p: &Painter, clip: Rect, at: Pos2, param: bool, c: &Scheme) {
    let fill = c.get(if param { colours::PARAM_CONNECTION } else { colours::AUDIO_CONNECTION });
    let p = p.with_clip_rect(clip);
    p.add(Shape::ellipse_filled(at, vec2(4.5, 5.5), Color32::BLACK));
    p.add(Shape::ellipse_filled(at, vec2(4.0, 5.0), fill));
}

/// Cable band width (0x140076c90 strokes the centre line 9 px wide with round ends).
const CABLE_W: f32 = 9.0;

/// The cable's centre line (0x140076c90): a cubic from pin to pin whose two control points sit at
/// the horizontal midpoint, level with each end.
fn cable(a: Pos2, b: Pos2) -> Vec<Pos2> {
    let mx = (a.x + b.x) * 0.5;
    let (c1, c2) = (pos2(mx, a.y), pos2(mx, b.y));
    (0..=32)
        .map(|i| {
            let t = i as f32 / 32.0;
            let u = 1.0 - t;
            (a.to_vec2() * (u * u * u) + c1.to_vec2() * (3.0 * u * u * t) + c2.to_vec2() * (3.0 * u * t * t) + b.to_vec2() * (t * t * t)).to_pos2()
        })
        .collect()
}

/// Whether `m` hits the cable (0x140076a10): inside the band with 1 px to spare, but not within
/// 5 px of its left end.
fn hits_cable(pts: &[Pos2], m: Pos2) -> bool {
    let left = pts.first().zip(pts.last()).map_or(f32::MAX, |(a, b)| a.x.min(b.x));
    m.x > left + 5.0 && near_line(pts, m, CABLE_W / 2.0 + 1.0)
}

fn near_line(pts: &[Pos2], m: Pos2, tol: f32) -> bool {
    pts.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = ((m - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
        (a + ab * t).distance(m) <= tol
    })
}

/// Cable painting (0x1400767e0): a 1 px black stroke around the band, or a 4 px red one when
/// selected, then the band filled with the connection colour. The stroke is centred on the band's
/// edge, so the fill covers its inner half.
fn paint_cable(p: &Painter, pts: Vec<Pos2>, col: Color32, selected: bool) {
    let (edge, edge_col) = if selected { (4.0, Color32::RED) } else { (1.0, Color32::BLACK) };
    let ends = [pts[0], pts[pts.len() - 1]];
    for (w, c) in [(CABLE_W + edge, edge_col), (CABLE_W, col)] {
        p.add(Shape::line(pts.clone(), Stroke::new(w, c)));
        // Round ends as geometry: egui's texture-drawn small circles leave a light ring over the band.
        for e in ends {
            p.add(Shape::ellipse_filled(e, Vec2::splat(w / 2.0), c));
        }
    }
}

impl Field {
    /// Drops an unfinished rename, e.g. when another patch replaces the boxes.
    pub fn stop_rename(&mut self) {
        self.rename = None;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn show(&mut self, ui: &mut Ui, field: Rect, g: &mut Graph, c: &Scheme, plugins: &[PluginInfo]) -> Vec<Action> {
        let mut act = Vec::new();
        let vp = Rect::from_min_max(field.min, field.max - vec2(SB, SB));
        let id = Id::new("pedal_field");
        let sizes: Vec<Vec2> = g.nodes.iter().map(|n| box_size(ui, n)).collect();
        // The field is a little taller than its view, so both scrollbars always show.
        let mut content = vec2(field.width(), field.height() + 23.0);
        for (n, s) in g.nodes.iter().zip(&sizes) {
            content = content.max(n.pos.to_vec2() + *s + vec2(10.0, 10.0));
        }
        self.scroll = self.scroll.clamp(Vec2::ZERO, (content - vp.size()).max(Vec2::ZERO));
        let origin = vp.min - self.scroll;
        let rects: Vec<Rect> = g.nodes.iter().zip(&sizes).map(|(n, s)| Rect::from_min_size(origin + n.pos.to_vec2(), *s)).collect();

        let mut fu = ui.new_child(UiBuilder::new().max_rect(vp).id_salt("field_ui"));
        fu.set_clip_rect(vp);
        let bg = fu.interact(vp, id.with("bg"), Sense::click());
        let p = fu.painter().clone();
        let fbg = c.get(colours::FIELD_BG);
        p.rect_filled(vp, CornerRadius::ZERO, fbg);
        // The view's thin bevel: a dark then a light line along the top and a light left edge.
        let shade = |d: i16| {
            let f = |v: u8| (v as i16 + d).clamp(0, 255) as u8;
            Color32::from_rgb(f(fbg.r()), f(fbg.g()), f(fbg.b()))
        };
        p.rect_filled(Rect::from_min_size(vp.min, vec2(vp.width(), 1.0)), CornerRadius::ZERO, shade(-16));
        p.rect_filled(Rect::from_min_size(vp.min + vec2(0.0, 1.0), vec2(vp.width(), 1.0)), CornerRadius::ZERO, shade(2));
        p.rect_filled(Rect::from_min_max(vp.min + vec2(0.0, 2.0), pos2(vp.left() + 1.0, vp.bottom())), CornerRadius::ZERO, shade(10));
        p.rect_filled(Rect::from_min_max(pos2(vp.left(), vp.bottom() - 1.0), vp.max), CornerRadius::ZERO, shade(-15));
        p.rect_filled(Rect::from_min_max(pos2(vp.right() - 1.0, vp.top() + 2.0), pos2(vp.right(), vp.bottom() - 1.0)), CornerRadius::ZERO, shade(6));
        if !g.has_processors() {
            let at = origin + vec2(485.0 * vp.width() / 992.0, 248.0 * vp.height() / 492.0);
            text_at(&p, at.x, at.y, Align::Center, "<double-click to add processor>", font(HINT_FONT), Color32::from_black_alpha(128));
        }

        // Cables are added after the boxes they join (0x14007dd40, 0x140080500), so they draw on top
        // of the boxes and take the pointer where they cross one.
        let pointer = fu.input(|i| i.pointer.hover_pos());
        let busy = self.wire.is_some() || fu.ctx().dragged_id().is_some();
        let rect_of = |id: u32| g.nodes.iter().position(|n| n.id == id).map(|i| rects[i]);
        let mut hovered_cable = None;
        let mut cables = Vec::with_capacity(g.conns.len());
        for conn in &g.conns {
            let (Some(a), Some(b)) = (g.node(conn.0), g.node(conn.2)) else { continue };
            let (Some(sp), Some(dp)) = (a.out_pin(conn.1), b.in_pin(conn.3)) else { continue };
            let (Some(ra), Some(rb)) = (rect_of(conn.0), rect_of(conn.2)) else { continue };
            let pts = cable(pin_pos(ra, true, sp), pin_pos(rb, false, dp));
            if !busy && pointer.is_some_and(|m| vp.contains(m) && hits_cable(&pts, m)) {
                hovered_cable = Some(*conn);
            }
            cables.push((*conn, pts));
        }

        let mut pins: Vec<Pin> = Vec::new();
        for (i, n) in g.nodes.iter_mut().enumerate() {
            let r = rects[i];
            if !r.intersects(vp) && self.wire.is_none() {
                continue;
            }
            let nid = n.id;
            let under_cable = hovered_cable.is_some() && pointer.is_some_and(|m| r.contains(m));
            let resp = fu.interact(r, id.with(("box", nid)), if under_cable { Sense::hover() } else { Sense::click_and_drag() });
            if resp.dragged() {
                n.pos = (n.pos + resp.drag_delta()).max(Pos2::ZERO);
            }
            if resp.drag_stopped() {
                act.push(Action::Moved);
            }
            // Box (0x140072960): grey rim, 4 px border colour band with the title strip, white body.
            let o = r.min;
            let ring = Rect::from_min_max(o + vec2(1.0, 1.0), r.max);
            p.rect_filled(ring, CornerRadius::same(5), Color32::from_black_alpha(128));
            p.rect_filled(ring.shrink(1.0), CornerRadius::same(4), c.get(colours::PLUGIN_BORDER));
            let body = Rect::from_min_max(o + vec2(5.7, 22.0), r.max - vec2(4.7, 4.7));
            p.rect_filled(body, CornerRadius::ZERO, c.get(colours::PLUGIN_BG));
            let tcol = if n.bypassed() { c.get(colours::TEXT).gamma_multiply(0.4) } else { c.get(colours::TEXT) };
            // Double-clicking the title strip renames the box; the name is saved in the patch's UserNames.
            let title_r = Rect::from_min_max(o + vec2(5.0, 4.0), pos2(r.right() - 5.0, o.y + 22.0));
            if resp.double_clicked() && resp.interact_pointer_pos().is_some_and(|m| m.y < title_r.bottom()) {
                self.rename = Some((nid, n.title().to_string(), true));
            }
            let mut done = false;
            match self.rename.as_mut().filter(|e| e.0 == nid) {
                Some((_, buf, fresh)) => {
                    let mut tu = fu.new_child(UiBuilder::new().max_rect(title_r).id_salt(("rename", nid)));
                    let edit_id = id.with(("rename_edit", nid));
                    let out = TextEdit::singleline(buf)
                        .id(edit_id)
                        .font(bold(TITLE_FONT))
                        .horizontal_align(Align::Center)
                        .margin(vec2(2.0, 1.0))
                        .desired_width(title_r.width())
                        .show(&mut tu);
                    if *fresh {
                        *fresh = false;
                        out.response.request_focus();
                        let mut state = out.state;
                        state.cursor.set_char_range(Some(text::CCursorRange::two(text::CCursor::new(0), text::CCursor::new(buf.chars().count()))));
                        state.store(fu.ctx(), edit_id);
                    } else if !out.response.has_focus() {
                        // Enter or a click elsewhere keeps the new name, Escape drops it.
                        if !fu.input(|i| i.key_pressed(Key::Escape)) {
                            let v = buf.trim();
                            let v = if v == n.info.name { "" } else { v };
                            if v != n.user_name {
                                n.user_name = v.to_string();
                                act.push(Action::Renamed);
                            }
                        }
                        done = true;
                    }
                }
                None => {
                    // The title label leaves room for the delete cross on processor boxes.
                    let tw = if n.is_io() { r.width() - 11.0 } else { r.width() - 18.0 };
                    text_fit(&p, o.x + 6.0, tw, o.y + 16.0, Align::Center, n.title(), bold(TITLE_FONT), tcol);
                }
            }
            if done {
                self.rename = None;
            }
            let label_col = Color32::from_gray(64);
            match &mut n.kind {
                NodeKind::Builtin(b) => {
                    let cr = control_rect(r, b.kind.control_size());
                    b.ui(&mut fu, cr, id.with(("ctl", nid)), c);
                }
                NodeKind::Missing(_) => {
                    text_at(&p, r.center().x, o.y + r.height() - 30.0, Align::Center, "(not found)", font(LABEL_FONT), Color32::from_rgb(170, 30, 30));
                }
                _ => {}
            }
            if !matches!(n.kind, NodeKind::Builtin(_)) {
                for k in 0..n.in_pins() {
                    text_at(&p, o.x + 10.0, o.y + 36.0 + PIN_STEP * k as f32, Align::Min, &n.in_name(k), font(LABEL_FONT), label_col);
                }
                for k in 0..n.out_pins() {
                    text_at(&p, r.right() - 10.0, o.y + 36.0 + PIN_STEP * k as f32, Align::Max, &n.out_name(k), font(LABEL_FONT), label_col);
                }
            }
            for (out, count) in [(false, n.in_pins()), (true, n.out_pins())] {
                for k in 0..count {
                    let param = if out { k >= n.outs } else { k >= n.ins };
                    let at = pin_pos(r, out, k);
                    paint_pin(&p, ring, at, param, c);
                    let pin = Pin { node: nid, out, pin: k, param, at };
                    pins.push(pin);
                    let hit = Rect::from_center_size(at + vec2(if out { -3.0 } else { 3.0 }, 0.0), vec2(12.0, PIN_STEP));
                    let name = if out { n.out_name(k) } else { n.in_name(k) };
                    let pr = fu.interact(hit, id.with(("pin", nid, out, k)), Sense::drag()).on_hover_text(name);
                    if pr.drag_started() {
                        self.wire = Some(pin);
                    }
                }
            }
            if !n.is_io() {
                // The box's buttons (0x1400716c0): "e" and "m" at the bottom left, "b" at the bottom
                // right, and the delete cross in the title strip's right corner.
                let text = c.get(colours::TEXT);
                let er = Rect::from_min_size(pos2(r.left() + 11.0, r.bottom() - 30.0), vec2(20.0, 20.0));
                if button(&mut fu, er, id.with(("btn_e", nid)), 3.0, c, "Open plugin editor").clicked() {
                    act.push(Action::OpenEditor(nid));
                }
                text_at(&p, er.center().x, baseline(er, 12.0), Align::Center, "e", font(12.0), text);
                let mr = Rect::from_min_size(pos2(r.left() + 33.0, r.bottom() - 30.0), vec2(24.0, 20.0));
                if button(&mut fu, mr, id.with(("btn_m", nid)), 3.0, c, "Open mappings editor").clicked() {
                    act.push(Action::OpenMappings(nid));
                }
                text_at(&p, mr.center().x, baseline(mr, 12.0), Align::Center, "m", font(12.0), text);
                let br = Rect::from_min_size(pos2(r.right() - 30.0, r.bottom() - 30.0), vec2(20.0, 20.0));
                let b = fu.interact(br, id.with(("btn_b", nid)), Sense::click());
                button_face(&p, br, 3.0, c, b.is_pointer_button_down_on(), b.hovered());
                let bf = if n.bypassed() { bold(10.3) } else { font(10.3) };
                text_at(&p, br.center().x, br.top() + 13.8, Align::Center, "b", bf, Color32::BLACK);
                if b.clicked() {
                    act.push(Action::Bypass(nid));
                }
                // The cross is half-white over the border band, half-black when hovered, and moves a
                // pixel down and right while pressed. Opaque, so the round caps don't show darker.
                let xr = Rect::from_min_size(pos2(r.right() - 17.0, r.top() + 6.0), vec2(12.0, 12.0));
                let x = fu.interact(xr, id.with(("btn_x", nid)), Sense::click());
                let under = colours::over(c.get(colours::PLUGIN_BORDER), colours::over(Color32::from_black_alpha(128), c.get(colours::FIELD_BG)));
                let xc = colours::over(if x.hovered() { Color32::from_black_alpha(128) } else { Color32::from_white_alpha(128) }, under);
                let ctr = pos2(r.right() - 11.0, r.top() + 12.0) + if x.is_pointer_button_down_on() { vec2(1.0, 1.0) } else { Vec2::ZERO };
                for s in [vec2(2.524, 2.524), vec2(2.524, -2.524)] {
                    p.line_segment([ctr - s, ctr + s], Stroke::new(3.2f32, xc));
                    for e in [ctr - s, ctr + s] {
                        p.add(Shape::ellipse_filled(e, Vec2::splat(1.6), xc));
                    }
                }
                if x.clicked() {
                    act.push(Action::Delete(nid));
                }
                let n: &Node = n;
                resp.context_menu(|ui| {
                    if ui.button("Open plugin editor").clicked() {
                        act.push(Action::OpenEditor(nid));
                        ui.close_menu();
                    }
                    if ui.button("Open mappings editor").clicked() {
                        act.push(Action::OpenMappings(nid));
                        ui.close_menu();
                    }
                    if let NodeKind::Plugin(pl) = &n.kind {
                        let np = pl.num_programs();
                        if np > 1 {
                            ui.menu_button("Programs", |ui| {
                                ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                                    for k in 0..np {
                                        if ui.selectable_label(k == pl.program(), format!("{} - {}", k + 1, pl.program_name(k))).clicked() {
                                            act.push(Action::SetProgram(nid, k));
                                            ui.close_menu();
                                        }
                                    }
                                });
                            });
                        }
                        if ui.button("User presets...").clicked() {
                            act.push(Action::UserPresets(nid));
                            ui.close_menu();
                        }
                    }
                    let mut by = n.bypassed();
                    if ui.checkbox(&mut by, "Bypass").clicked() {
                        act.push(Action::Bypass(nid));
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Delete").clicked() {
                        act.push(Action::Delete(nid));
                        ui.close_menu();
                    }
                });
            }
        }

        for (conn, pts) in cables {
            let param = conn.1 == crate::engine::MIDI_PIN;
            let mut col = c.get(if param { colours::PARAM_CONNECTION } else { colours::AUDIO_CONNECTION });
            if hovered_cable == Some(conn) {
                col = colours::brighter(col, 0.75);
            }
            paint_cable(&p, pts, col, self.selected.contains(&conn));
        }

        // A cable being dragged out of a pin.
        if self.rename.as_ref().is_some_and(|e| g.node(e.0).is_none()) {
            self.rename = None;
        }
        if let Some(w) = self.wire {
            let m = fu.input(|i| i.pointer.interact_pos()).unwrap_or(w.at);
            let target = pins.iter().filter(|q| q.out != w.out && q.param == w.param && q.node != w.node).min_by(|a, b| a.at.distance(m).total_cmp(&b.at.distance(m))).filter(|q| q.at.distance(m) < 14.0).copied();
            let end = target.map(|t| t.at).unwrap_or(m);
            let pts = if w.out { cable(w.at, end) } else { cable(end, w.at) };
            let col = c.get(if w.param { colours::PARAM_CONNECTION } else { colours::AUDIO_CONNECTION });
            paint_cable(&p, pts, col, false);
            if !fu.input(|i| i.pointer.any_down()) {
                self.wire = None;
                if let Some(t) = target {
                    let (s, d) = if w.out { (w, t) } else { (t, w) };
                    let (Some(sn), Some(dn)) = (g.node(s.node), g.node(d.node)) else { return act };
                    let (sc, dc) = (sn.out_channel(s.pin), dn.in_channel(d.pin));
                    if g.connect(s.node, sc, d.node, dc) {
                        act.push(Action::Connected(s.node, d.node, w.param));
                    }
                }
            }
        }

        if bg.clicked() {
            let ctrl = fu.input(|i| i.modifiers.command || i.modifiers.shift);
            match hovered_cable {
                Some(cn) if self.selected.contains(&cn) && ctrl => self.selected.retain(|x| *x != cn),
                Some(cn) if ctrl => self.selected.push(cn),
                Some(cn) => self.selected = vec![cn],
                None => self.selected.clear(),
            }
        }
        if bg.double_clicked() && hovered_cable.is_none() {
            if let Some(m) = bg.interact_pointer_pos() {
                self.add_menu = Some((m, (m - origin).to_pos2(), true));
            }
        }
        self.selected.retain(|s| g.conns.contains(s));
        if !self.selected.is_empty() && !fu.ctx().wants_keyboard_input() && fu.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            act.push(Action::DeleteConns);
        }
        if pointer.is_some_and(|m| vp.contains(m)) {
            let d = fu.input(|i| i.smooth_scroll_delta);
            self.scroll -= d;
        }

        let max = (content - vp.size()).max(Vec2::ZERO);
        let vbar = Rect::from_min_max(pos2(vp.right(), field.top()), pos2(field.right(), vp.bottom()));
        let hbar = Rect::from_min_max(pos2(field.left(), vp.bottom()), pos2(vp.right(), field.bottom()));
        scrollbar(ui, vbar, true, &mut self.scroll.y, vp.height(), vp.height() + max.y, id.with("vs"));
        scrollbar(ui, hbar, false, &mut self.scroll.x, vp.width(), vp.width() + max.x, id.with("hs"));
        ui.painter().rect_filled(Rect::from_min_max(vp.max, field.max), CornerRadius::ZERO, c.get(colours::WINDOW_BG));

        self.add_menu_ui(ui, plugins, &mut act);
        act
    }

    fn add_menu_ui(&mut self, ui: &mut Ui, plugins: &[PluginInfo], act: &mut Vec<Action>) {
        let Some((screen, at, fresh)) = self.add_menu else { return };
        let mut close = false;
        let area = Area::new(Id::new("add_menu")).order(Order::Foreground).fixed_pos(screen).show(ui.ctx(), |ui| {
            Frame::menu(ui.style()).show(ui, |ui| {
                ui.style_mut().visuals.button_frame = false;
                ui.set_min_width(190.0);
                ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                    ui.label(RichText::new("Built-in").strong());
                    for k in Kind::ALL {
                        if ui.button(k.name()).on_hover_text(k.description()).clicked() {
                            act.push(Action::AddBuiltin(k, at));
                            close = true;
                        }
                    }
                    let mut cats: Vec<String> = plugins.iter().map(|p| p.menu_category()).collect();
                    cats.sort_by_key(|c| c.to_lowercase());
                    cats.dedup();
                    for cat in cats {
                        ui.separator();
                        ui.label(RichText::new(&cat).strong());
                        let mut list: Vec<(usize, &PluginInfo)> = plugins.iter().enumerate().filter(|(_, p)| p.menu_category() == cat).collect();
                        list.sort_by_key(|(_, p)| p.name.to_lowercase());
                        for (i, p) in list {
                            let label = if p.format == "VST3" { format!("{} (VST3)", p.name) } else { p.name.clone() };
                            if ui.button(label).on_hover_text(&p.manufacturer).clicked() {
                                act.push(Action::AddPlugin(i, at));
                                close = true;
                            }
                        }
                    }
                    if plugins.is_empty() {
                        ui.separator();
                        ui.weak("No plugins yet: Options > Plugin List");
                    }
                });
            });
        });
        let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|m| !area.response.rect.contains(m)));
        if close || (outside && !fresh) || ui.input(|i| i.key_pressed(Key::Escape)) {
            self.add_menu = None;
        } else {
            self.add_menu = Some((screen, at, false));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Ctx as GraphCtx, AUDIO_IN, AUDIO_OUT, MIDI_IN};

    /// Runs the field headless, one egui frame per call, 50 ms apart.
    struct Rig {
        ctx: Context,
        field: Field,
        g: Graph,
        t: f64,
        field_r: Rect,
    }

    impl Rig {
        fn new() -> Rig {
            let ctx = Context::default();
            crate::setup_fonts(&ctx);
            let gc = GraphCtx { sample_rate: 44100.0, block: 256, dev_ins: 2, dev_outs: 2, window: vec2(1024.0, 580.0), audio_in: true, midi_in: true, osc_in: true };
            let mut rig = Rig { ctx, field: Field::default(), g: Graph::new_default(&gc), t: 0.0, field_r: Rect::NOTHING };
            rig.frame(vec![]);
            rig
        }
        fn frame(&mut self, events: Vec<Event>) -> Vec<Action> {
            self.t += 0.05;
            let input = RawInput { time: Some(self.t), screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1024.0, 600.0))), events, ..Default::default() };
            let (field, g, field_r) = (&mut self.field, &mut self.g, &mut self.field_r);
            let mut acts = Vec::new();
            let _ = self.ctx.run(input, |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    *field_r = ui.max_rect();
                    acts.extend(field.show(ui, *field_r, g, &Scheme::default(), &[]));
                });
            });
            acts
        }
        fn input_box(&self) -> &Node {
            self.g.nodes.iter().find(|n| n.name() == AUDIO_IN).unwrap()
        }
        fn double_click_title(&mut self) {
            // A second apart from earlier clicks, or egui would count a triple click.
            self.t += 1.0;
            let at = self.field_r.min + self.input_box().pos.to_vec2() + vec2(25.0, 12.0);
            self.frame(vec![Event::PointerMoved(at)]);
            for _ in 0..2 {
                for pressed in [true, false] {
                    self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
                }
            }
            assert!(self.field.rename.is_some(), "double-click should start renaming");
        }
        fn key(&mut self, key: Key) -> Vec<Action> {
            self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }])
        }
    }

    fn renamed(acts: &[Action]) -> bool {
        acts.iter().any(|a| matches!(a, Action::Renamed))
    }

    #[test]
    fn double_clicking_a_title_renames_the_box() {
        let mut rig = Rig::new();
        rig.double_click_title();
        rig.frame(vec![Event::Text("Guitar".into())]);
        assert!(renamed(&rig.key(Key::Enter)));
        assert_eq!(rig.input_box().title(), "Guitar");

        // Escape drops what was typed.
        rig.double_click_title();
        rig.frame(vec![Event::Text("Bass".into())]);
        assert!(!renamed(&rig.key(Key::Escape)));
        assert!(rig.field.rename.is_none());
        assert_eq!(rig.input_box().title(), "Guitar");

        // An empty name gives the box its own name back.
        rig.double_click_title();
        rig.key(Key::Backspace);
        assert!(renamed(&rig.key(Key::Enter)));
        assert_eq!((rig.input_box().title(), rig.input_box().user_name.as_str()), (AUDIO_IN, ""));
    }

    #[test]
    fn a_cable_crossing_a_box_takes_the_click() {
        let mut rig = Rig::new();
        let id_of = |g: &Graph, name: &str| g.nodes.iter().find(|n| n.name() == name).unwrap().id;
        let (src, dst, midi) = (id_of(&rig.g, AUDIO_IN), id_of(&rig.g, AUDIO_OUT), id_of(&rig.g, MIDI_IN));
        assert!(rig.g.connect(src, 0, dst, 0));
        rig.g.nodes.iter_mut().find(|n| n.id == midi).unwrap().pos = pos2(400.0, 0.0);
        rig.frame(vec![]);
        // Level with the first pins, where the cable runs straight across the moved box.
        let at = rig.field_r.min + vec2(430.0, 10.0 + PIN0);
        rig.t += 1.0;
        rig.frame(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            rig.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        assert_eq!(rig.field.selected, vec![rig.g.conns[0]]);
        assert_eq!(rig.g.node(midi).map(|n| n.pos), Some(pos2(400.0, 0.0)), "the box under the cable stays put");
    }

    #[test]
    fn cables_hit_inside_the_band_but_not_at_their_left_end() {
        let pts = cable(pos2(100.0, 50.0), pos2(300.0, 50.0));
        assert!(hits_cable(&pts, pos2(200.0, 55.4)), "half the 9 px band plus 1 px");
        assert!(!hits_cable(&pts, pos2(200.0, 56.0)));
        assert!(!hits_cable(&pts, pos2(104.0, 50.0)), "the pin under the left end stays reachable");
        assert!(hits_cable(&pts, pos2(106.0, 50.0)));
    }

    #[test]
    fn builtin_boxes_follow_their_control_unless_the_title_is_wider() {
        assert_eq!(builtin_box(40.0, vec2(64.0, 64.0)), vec2(88.0, 116.0), "Level: control + 24 by control + 52");
        assert_eq!(builtin_box(52.0, vec2(48.0, 48.0)), vec2(72.0, 100.0), "title + 20 equal to control + 24 keeps the control width");
        assert_eq!(builtin_box(85.6, vec2(48.0, 48.0)), vec2(125.0, 100.0), "a wider title makes the box title + 40, truncated");
    }

    #[test]
    fn builtin_controls_sit_24_px_down_centred_with_truncated_halves() {
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(125.0, 100.0));
        assert_eq!(control_rect(r, vec2(48.0, 48.0)), Rect::from_min_size(pos2(48.0, 34.0), vec2(48.0, 48.0)));
    }
}
