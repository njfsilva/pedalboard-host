//! Shared drawing helpers in the original's look: fonts, gradient buttons and scrollbars.

use crate::colours::{self, Scheme};
use egui::{pos2, Color32, CornerRadius, FontFamily, FontId, Id, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui};

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn text_w(ui: &Ui, s: &str, f: FontId) -> f32 {
    ui.fonts(|fo| fo.layout_no_wrap(s.to_string(), f, Color32::BLACK).size().x)
}

/// Text placed by its baseline, as JUCE draws it.
pub fn text_at(p: &egui::Painter, x: f32, baseline: f32, align: egui::Align, s: &str, f: FontId, col: Color32) {
    let g = p.layout_no_wrap(s.to_string(), f, col);
    let asc = g.rows.first().and_then(|r| r.glyphs.first()).map(|gl| gl.pos.y).unwrap_or(g.size().y * 0.8);
    let x = match align {
        egui::Align::Min => x,
        egui::Align::Center => x - g.size().x / 2.0,
        egui::Align::Max => x - g.size().x,
    };
    p.galley(pos2(x, baseline - asc), g, col);
}

/// One line of text fitted into `w` the way JUCE's `drawFittedText` does it: trimmed, squeezed
/// sideways down to 70% when too wide, and cut with "..." when even that doesn't fit.
#[allow(clippy::too_many_arguments)]
pub fn text_fit(p: &egui::Painter, x: f32, w: f32, baseline: f32, align: egui::Align, s: &str, f: FontId, col: Color32) {
    const MIN_SCALE: f32 = 0.7;
    let s = s.trim();
    if s.is_empty() || w <= 0.0 {
        return;
    }
    let full = p.layout_no_wrap(s.to_string(), f.clone(), col);
    let tw = full.size().x;
    if tw <= w {
        let ax = match align {
            egui::Align::Min => x,
            egui::Align::Center => x + w / 2.0,
            egui::Align::Max => x + w,
        };
        text_at(p, ax, baseline, align, s, f, col);
        return;
    }
    let (g, scale) = if tw * MIN_SCALE < w {
        (full, w / tw)
    } else {
        let chars: Vec<char> = s.chars().collect();
        let mut cut = None;
        for n in (0..chars.len()).rev() {
            let t: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "...";
            let g = p.layout_no_wrap(t, f.clone(), col);
            if g.size().x * MIN_SCALE <= w || n == 0 {
                cut = Some(g);
                break;
            }
        }
        (cut.unwrap_or(full), MIN_SCALE)
    };
    let sw = g.size().x * scale;
    let x0 = match align {
        egui::Align::Min => x,
        egui::Align::Center => x + (w - sw) / 2.0,
        egui::Align::Max => x + w - sw,
    };
    let asc = g.rows.first().and_then(|r| r.glyphs.first()).map(|gl| gl.pos.y).unwrap_or(g.size().y * 0.8);
    let ctx = p.ctx();
    let tex = ctx.fonts(|fo| fo.font_image_size());
    let mut t = egui::epaint::Tessellator::new(ctx.pixels_per_point(), Default::default(), tex, Vec::new());
    let mut m = Mesh::default();
    t.tessellate_text(&egui::epaint::TextShape::new(pos2(x0, baseline - asc), g, col), &mut m);
    for v in &mut m.vertices {
        v.pos.x = x0 + (v.pos.x - x0) * scale;
    }
    p.add(Shape::mesh(m));
}

/// Fills the ellipse in `r` with a colour worked out at each point (premultiplied), with a soft
/// one-pixel rim, the way JUCE's gradient-filled ellipses look.
pub fn shade_ellipse(p: &egui::Painter, r: Rect, col: impl Fn(Pos2) -> Color32) {
    const SEG: u32 = 48;
    let (c, rx, ry) = (r.center(), r.width() / 2.0, r.height() / 2.0);
    if rx <= 0.0 || ry <= 0.0 {
        return;
    }
    let (ex, ey) = ((rx - 0.5).max(0.0), (ry - 0.5).max(0.0));
    let mut rings: Vec<(f32, f32, bool)> = [0.2f32, 0.4, 0.6, 0.7, 0.8, 0.9].iter().map(|k| (rx * k, ry * k, true)).filter(|q| q.0 < ex && q.1 < ey).collect();
    rings.push((ex, ey, true));
    rings.push((rx + 0.5, ry + 0.5, false));
    let mut m = Mesh::default();
    m.colored_vertex(c, col(c));
    for &(ax, ay, solid) in &rings {
        for i in 0..SEG {
            let a = i as f32 * std::f32::consts::TAU / SEG as f32;
            let q = pos2(c.x + ax * a.cos(), c.y + ay * a.sin());
            m.colored_vertex(q, if solid { col(q) } else { Color32::TRANSPARENT });
        }
    }
    let at = |ring: u32, i: u32| 1 + ring * SEG + i % SEG;
    for i in 0..SEG {
        m.add_triangle(0, at(0, i), at(0, i + 1));
        for k in 0..rings.len() as u32 - 1 {
            m.add_triangle(at(k, i), at(k + 1, i), at(k + 1, i + 1));
            m.add_triangle(at(k, i), at(k + 1, i + 1), at(k, i + 1));
        }
    }
    p.add(Shape::mesh(m));
}

/// The combo box face measured from the original's patch selector: a text area with an arrow
/// button on its right. Returns the part left for the text.
pub fn combo_face(p: &egui::Painter, cr: Rect, c: &Scheme) -> Rect {
    p.rect_filled(cr, CornerRadius::ZERO, c.get(colours::TEXT_EDITOR));
    let ar = Rect::from_min_max(pos2(cr.right() - 20.0, cr.top()), cr.max);
    // The arrow button has a light upper half with a highlight line and a darker lower half; the
    // box has a double top edge, a soft shadow below and a light glow above and to the sides.
    let fill = |r: Rect, v: u32| p.rect_filled(r, CornerRadius::ZERO, rgb(v));
    let row = |y: f32, x0: f32, x1: f32| Rect::from_min_max(pos2(x0, y), pos2(x1, y + 1.0));
    let col = |x: f32, y0: f32, y1: f32| Rect::from_min_max(pos2(x, y0), pos2(x + 1.0, y1));
    let (bl, br) = (ar.left() + 1.0, ar.right() - 2.0);
    let mid = (cr.top() + cr.bottom()).div_euclid(2.0);
    fill(Rect::from_min_max(pos2(bl, cr.top() + 2.0), pos2(br, mid)), 0xEEECE1);
    fill(row(cr.top() + 2.0, bl, br), 0xF2F0E5);
    fill(Rect::from_min_max(pos2(bl, mid), pos2(br, cr.bottom() - 2.0)), 0xE6E4D9);
    fill(row(cr.bottom() - 2.0, bl, br), 0xEAE8DD);
    p.vline(ar.left() + 0.5, cr.y_range(), Stroke::new(1.0f32, Color32::from_rgb(0xC7, 0xC6, 0xC3)));
    fill(row(cr.top() - 1.0, cr.left(), cr.right()), 0xF2F0E4);
    fill(col(cr.left() - 1.0, cr.top(), cr.bottom()), 0xF2EFE4);
    fill(col(cr.right(), cr.top(), cr.bottom()), 0xF1EFE4);
    fill(row(cr.bottom(), cr.left(), cr.right()), 0xE0DED5);
    fill(row(cr.top(), cr.left(), cr.right()), 0xC7C6C0);
    fill(row(cr.top() + 1.0, cr.left() + 1.0, cr.right() - 1.0), 0xCCCCCD);
    fill(col(cr.left(), cr.top(), cr.bottom()), 0xA9A9A8);
    fill(col(cr.left() + 1.0, cr.top() + 2.0, cr.bottom() - 1.0), 0xF7F7F7);
    fill(col(cr.right() - 2.0, cr.top() + 2.0, cr.bottom() - 1.0), 0xC9C8BE);
    fill(col(cr.right() - 1.0, cr.top() + 1.0, cr.bottom()), 0xD1CFC5);
    fill(row(cr.bottom() - 1.0, cr.left(), cr.right()), 0xB2B2B3);
    let m = ar.center() + egui::vec2(0.0, 0.5);
    let chev = Stroke::new(1.9f32, rgb(0x6A6964));
    p.line_segment([m + egui::vec2(-4.8, -3.2), m + egui::vec2(0.0, 2.6)], chev);
    p.line_segment([m + egui::vec2(0.0, 2.6), m + egui::vec2(4.8, -3.2)], chev);
    Rect::from_min_max(cr.min, pos2(ar.left() - 1.0, cr.bottom()))
}

/// Mixes premultiplied colours, as JUCE's `Colour::interpolatedWith` does.
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

fn rrect_points(r: Rect, rad: f32) -> Vec<Pos2> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0);
    let mut pts = Vec::with_capacity(40);
    let corners = [(r.right() - rad, r.bottom() - rad, 0.0f32), (r.left() + rad, r.bottom() - rad, 90.0), (r.left() + rad, r.top() + rad, 180.0), (r.right() - rad, r.top() + rad, 270.0)];
    for (cx, cy, a0) in corners {
        for i in 0..=8 {
            let a = (a0 + i as f32 * 90.0 / 8.0).to_radians();
            pts.push(pos2(cx + rad * a.cos(), cy + rad * a.sin()));
        }
    }
    pts
}

/// A rounded rectangle filled with a vertical gradient.
pub fn gradient_rrect(p: &egui::Painter, r: Rect, rad: f32, top: Color32, bottom: Color32) {
    let pts = rrect_points(r, rad);
    let mut m = Mesh::default();
    let col = |y: f32| lerp(top, bottom, ((y - r.top()) / r.height().max(1.0)).clamp(0.0, 1.0));
    m.colored_vertex(r.center(), col(r.center().y));
    for q in &pts {
        m.colored_vertex(*q, col(q.y));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        m.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    p.add(Shape::mesh(m));
}

/// The bevel JUCE draws inside text boxes (drawBevel, 0x1401f0d10): `n` one-pixel rings inside `r`,
/// the outermost at full strength and each further in weaker by 1/n, the sides at 0.75 of that.
pub fn bevel(p: &egui::Painter, r: Rect, n: u32, col: Color32) {
    let line = |x: f32, y: f32, w: f32, h: f32, c: Color32| p.rect_filled(Rect::from_min_size(pos2(x, y), egui::vec2(w, h)), CornerRadius::ZERO, c);
    for i in 0..n {
        let f = i as f32;
        let op = (n - i) as f32 / n as f32;
        let (edge, side) = (col.gamma_multiply(op), col.gamma_multiply(op * 0.75));
        line(r.left() + f, r.top() + f, r.width() - 2.0 * f, 1.0, edge);
        line(r.left() + f, r.top() + f + 1.0, 1.0, r.height() - 2.0 * f - 2.0, side);
        line(r.left() + f, r.bottom() - f - 1.0, r.width() - 2.0 * f, 1.0, edge);
        line(r.right() - f - 1.0, r.top() + f + 1.0, 1.0, r.height() - 2.0 * f - 2.0, side);
    }
}

/// The bevelled button of the original's look: soft vertical gradient and a dark rim.
pub fn button_face(p: &egui::Painter, r: Rect, rad: f32, c: &Scheme, down: bool, hover: bool) {
    let base = c.get(colours::BUTTON);
    let (mut top, mut bot) = (lerp(base, Color32::WHITE, 0.35), lerp(base, Color32::BLACK, 0.035));
    if hover {
        top = lerp(top, c.get(colours::BUTTON_HIGHLIGHT), 0.25);
        bot = lerp(bot, c.get(colours::BUTTON_HIGHLIGHT), 0.25);
    }
    if down {
        std::mem::swap(&mut top, &mut bot);
    }
    gradient_rrect(p, r.shrink(0.5), rad, top, bot);
    p.rect_stroke(r, CornerRadius::same(rad as u8), Stroke::new(1.0f32, lerp(base, Color32::BLACK, 0.46)), StrokeKind::Inside);
}

pub fn button(ui: &mut Ui, r: Rect, id: Id, rad: f32, c: &Scheme, tip: &str) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    button_face(ui.painter(), r, rad, c, resp.is_pointer_button_down_on(), resp.hovered());
    resp
}

pub fn triangle(p: &egui::Painter, a: Pos2, b: Pos2, cc: Pos2, fill: Color32, stroke: Stroke) {
    p.add(Shape::convex_polygon(vec![a, b, cc], fill, stroke));
}

/// A JUCE-style scrollbar with arrow buttons. Returns true when `pos` changed.
pub fn scrollbar(ui: &mut Ui, bar: Rect, vertical: bool, pos: &mut f32, view: f32, total: f32, id: Id) -> bool {
    let p = ui.painter().clone();
    let along = |r: Rect| if vertical { (r.top(), r.bottom()) } else { (r.left(), r.right()) };
    let (a0, a1) = along(bar);
    // The arrow buttons are two pixels longer than the bar is thick.
    let btn = (if vertical { bar.width() } else { bar.height() }) + 2.0;
    let mk = |s: f32, e: f32| if vertical { Rect::from_min_max(pos2(bar.left(), s), pos2(bar.right(), e)) } else { Rect::from_min_max(pos2(s, bar.top()), pos2(e, bar.bottom())) };
    let (b0, b1) = (mk(a0, a0 + btn), mk(a1 - btn, a1));
    let track = mk(a0 + btn, a1 - btn);
    let len = track_len(track, vertical);
    let max = (total - view).max(0.0);
    let thumb_len = if total > 0.0 { (len * view / total).clamp(16.0_f32.min(len), len) } else { len };
    let t0 = (if vertical { track.top() } else { track.left() }) + if max > 0.0 { (len - thumb_len) * *pos / max } else { 0.0 };
    let thumb = mk(t0, t0 + thumb_len);
    sb_track(&p, track, vertical);
    sb_face(&p, thumb, vertical, rgb(0x8F8E88));
    for (r, back) in [(b0, true), (b1, false)] {
        sb_face(&p, r, vertical, rgb(0xA9A8A1));
        sb_arrow(&p, r.center(), vertical, back);
    }
    let old = *pos;
    let step = 16.0;
    if ui.interact(b0, id.with("b0"), Sense::click()).clicked() {
        *pos -= step;
    }
    if ui.interact(b1, id.with("b1"), Sense::click()).clicked() {
        *pos += step;
    }
    let tr = ui.interact(track, id.with("tr"), Sense::click_and_drag());
    if let Some(m) = tr.interact_pointer_pos() {
        let mv = if vertical { m.y } else { m.x };
        let (ts, te) = along(thumb);
        if tr.drag_started() || tr.clicked() {
            if mv < ts {
                *pos -= view;
            } else if mv > te {
                *pos += view;
            }
        } else if tr.dragged() && (mv >= ts - 30.0 && mv <= te + 30.0) && len > thumb_len {
            let d = if vertical { tr.drag_delta().y } else { tr.drag_delta().x };
            *pos += d * max / (len - thumb_len);
        }
    }
    *pos = pos.clamp(0.0, max);
    *pos != old
}

fn track_len(r: Rect, vertical: bool) -> f32 {
    if vertical { r.height() } else { r.width() }
}

pub fn rgb(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// A 1-pixel line across the bar's thickness at index `i`, running along its length.
fn sb_line(p: &egui::Painter, r: Rect, vertical: bool, i: usize, inset: f32, c: Color32) {
    let f = i as f32;
    let line = if vertical {
        Rect::from_min_max(pos2(r.left() + f, r.top() + inset), pos2(r.left() + f + 1.0, r.bottom() - inset))
    } else {
        Rect::from_min_max(pos2(r.left() + inset, r.top() + f), pos2(r.right() - inset, r.top() + f + 1.0))
    };
    p.rect_filled(line, CornerRadius::ZERO, c);
}

/// Thumb and arrow-button faces as measured on the original: a light leading half, a darker
/// trailing half, a highlight inside the rim and a rounded outline.
fn sb_face(p: &egui::Painter, r: Rect, vertical: bool, rim: Color32) {
    let n = (if vertical { r.width() } else { r.height() }).round().max(6.0) as usize;
    let (mid, hi, light, dark, edge) = (rgb(0xC3C1B8), rgb(0xF4F2E6), rgb(0xEEECE1), rgb(0xE6E4D9), rgb(0xEAE8DD));
    for i in 1..n - 1 {
        let c = match (vertical, i) {
            (true, 1) => mid,
            (true, 2) | (false, 1) => hi,
            (false, _) if i == n - 2 => mid,
            (true, _) if i == n - 2 => edge,
            (false, _) if i == n - 3 => edge,
            _ if i < n / 2 => light,
            _ => dark,
        };
        sb_line(p, r, vertical, i, if i == 1 || i == n - 2 { 1.0 } else { 0.0 }, c);
    }
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0f32, rim), StrokeKind::Inside);
}

/// The sunken track: darker on the leading side, fading lighter across the bar.
fn sb_track(p: &egui::Painter, r: Rect, vertical: bool) {
    let n = (if vertical { r.width() } else { r.height() }).round().max(4.0) as usize;
    for i in 0..n {
        let c = match i {
            0 => rgb(0xB3B2AC),
            1 => rgb(0xB1AFA8),
            _ if i == n - 1 => rgb(0xCBCAC1),
            _ => lerp(rgb(0xC4C2BA), rgb(0xE9E7DC), (i - 2) as f32 / (n - 3).max(1) as f32),
        };
        sb_line(p, r, vertical, i, 0.0, c);
    }
}

/// A two-pixel chevron pointing to the start (`back`) or the end of the bar.
fn sb_arrow(p: &egui::Painter, c: Pos2, vertical: bool, back: bool) {
    let (across, along) = (4.5, 2.5);
    let s = if back { -1.0 } else { 1.0 };
    let pts = if vertical {
        vec![pos2(c.x - across, c.y - s * along), pos2(c.x, c.y + s * along), pos2(c.x + across, c.y - s * along)]
    } else {
        vec![pos2(c.x - s * along, c.y - across), pos2(c.x + s * along, c.y), pos2(c.x - s * along, c.y + across)]
    };
    p.add(Shape::line(pts, Stroke::new(2.0f32, rgb(0x73726C))));
}
