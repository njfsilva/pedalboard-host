//! The built-ins' controls in their boxes and in their editor windows, drawn from the decoded
//! paint and layout routines listed in MECHANICS.md ("Built-in controls"). Positions are given
//! relative to the control's top left corner, as the original's setBounds calls are.

use super::*;
use crate::bars::baseline;
use crate::colours::{self, draw, mult_alpha, mult_brightness, overlaid, with_alpha, Scheme};
use crate::widgets::{bold, button_face, combo_face, font, lerp, shade_ellipse, text_at, text_fit, text_w};
use egui::text::{CCursor, CCursorRange};
use egui::{pos2, vec2, Align, Color32, CornerRadius, FontId, Id, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, Vec2};
use std::f32::consts::PI;
use std::path::PathBuf;
use std::sync::atomic::Ordering::Acquire;

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// The record button's dot. Estimated: the original's record and stop pictures weren't decoded.
const RECORD_RED: Color32 = Color32::from_rgb(0xD0, 0x20, 0x20);
/// FilenameComponent's browse button tip (string 0x140424a70).
const BROWSE_TIP: &str = "click to browse for a different file";
const SYNC_TEXT: &str = "Sync to main transport";
const PLAYER_SYNC_TIP: &str = "Sync file playback to the main transport";
const PLAY_TIP: &str = "Play/pause audio file";
const RTZ_TIP: &str = "Return to the start of the audio file";

impl Kind {
    /// Each built-in editor's size as its constructor sets it.
    pub fn editor_size(self) -> Vec2 {
        match self {
            Kind::Level | Kind::OutputToggle => vec2(192.0, 192.0),
            Kind::VuMeter => vec2(128.0, 256.0),
            Kind::FilePlayer | Kind::Recorder => vec2(600.0, 200.0),
            Kind::Metronome => vec2(400.0, 200.0),
            Kind::Looper => vec2(500.0, 300.0),
        }
    }
}

impl Builtin {
    /// Draws the control inside its box and handles its input.
    pub fn ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme) {
        match self.kind {
            Kind::Level => {
                if let Some(v) = dial(ui, r, id.with("dial"), c, getf(&self.sh.level)) {
                    self.set_param(0, v);
                }
            }
            Kind::OutputToggle => {
                let on = self.sh.on.load(Relaxed);
                if toggle_picture(ui, r, id.with("toggle"), on, c.get(colours::PLUGIN_BG)) {
                    self.set_param(0, b2f(!on));
                }
            }
            Kind::VuMeter => vu(ui.painter(), r, c, self.vu_peaks()),
            Kind::FilePlayer => self.player_ui(ui, r, id, c),
            Kind::Recorder => self.recorder_ui(ui, r, id, c),
            Kind::Metronome => self.metronome_ui(ui, r, id, c, false),
            Kind::Looper => self.looper_ui(ui, r, id, c, false),
        }
    }

    /// The editor window's contents; `r` is the whole window area. Every editor fills the window
    /// colour; the File Player, Recorder and Metronome editors hold their control 4 px in.
    pub fn editor_ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme) {
        let bg = c.get(colours::WINDOW_BG);
        ui.painter().rect_filled(r, CornerRadius::ZERO, bg);
        let inner = r.shrink(4.0);
        match self.kind {
            Kind::Level => {
                // A rotary slider with its value box (80 × 20) below.
                let knob = Rect::from_min_max(r.min, pos2(r.right(), r.bottom() - 20.0));
                if let Some(v) = dial(ui, knob, id.with("dial"), c, getf(&self.sh.level)) {
                    self.set_param(0, v);
                }
                let vb = Rect::from_min_size(pos2(r.left() + (r.width() / 2.0).trunc() - 40.0, r.bottom() - 20.0), vec2(80.0, 20.0));
                ui.painter().rect_filled(vb, CornerRadius::ZERO, Color32::WHITE);
                let text = format!("{:.7}", getf(&self.sh.level) * 2.0);
                if let Some(t) = number_label(ui, vb, id.with("value"), &text, font(15.0), Color32::BLACK, Color32::WHITE) {
                    self.set_param(0, parse_num(&t) / 2.0);
                }
                ui.painter().rect_stroke(vb, CornerRadius::ZERO, Stroke::new(1.0f32, draw([0x80, 0x80, 0x80, 0xb2])), StrokeKind::Inside);
            }
            Kind::OutputToggle => {
                let on = self.sh.on.load(Relaxed);
                if toggle_picture(ui, r, id.with("toggle"), on, bg) {
                    self.set_param(0, b2f(!on));
                }
            }
            Kind::VuMeter => vu(ui.painter(), r, c, self.vu_peaks()),
            Kind::FilePlayer => self.player_ui(ui, inner, id, c),
            Kind::Recorder => self.recorder_ui(ui, inner, id, c),
            Kind::Metronome => self.metronome_ui(ui, inner, id, c, true),
            Kind::Looper => self.looper_ui(ui, r, id, c, true),
        }
    }

    fn vu_peaks(&self) -> [f32; 2] {
        [getf(&self.sh.peak[0]), getf(&self.sh.peak[1])]
    }

    /// File Player (0x14002a420): file row with play and return buttons, the waveform with the
    /// read position, and the sync and loop tick boxes.
    fn player_ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme) {
        let (w, h) = (r.width(), r.height());
        let s = self.sh.clone();
        if file_box(ui, at(r, 0.0, 0.0, w - 58.0, 24.0), id.with("file"), c, &self.file, true) {
            if let Some(f) = pick_wav(&self.file) {
                if let Err(m) = self.load_file(&f) {
                    self.error = Some(("File Player Error".into(), format!("{m}\n{FILE_BUSY}")));
                }
            }
        }
        let playing = s.on.load(Relaxed);
        if icon_button(ui, at(r, w - 54.0, 0.0, 24.0, 24.0), id.with("play"), c, if playing { Icon::Pause } else { Icon::Play }, true, PLAY_TIP) {
            self.set_param(0, 1.0);
        }
        if icon_button(ui, at(r, w - 26.0, 0.0, 24.0, 24.0), id.with("rtz"), c, Icon::Rtz, true, RTZ_TIP) {
            self.set_param(1, 1.0);
        }
        let dr = at(r, 0.0, 28.0, w - 2.0, h - 48.0);
        let clip = self.clips[0].clone();
        let (frames, secs, stereo) = clip.as_ref().map(|k| (k.frames as u64, k.frames as f64 / k.rate.max(1.0), k.data.len() > 1)).unwrap_or((0, 0.0, false));
        let bins: &[[f32; 4]] = clip.as_ref().map(|k| k.overview.as_slice()).unwrap_or(&[]);
        let bin = |i: usize| bins[i];
        let pos = if frames > 0 { s.pos.load(Relaxed) as f32 / frames as f32 } else { 0.0 };
        waveform(ui.painter(), dr, c, &Wave { n: bins.len(), bin: &bin, block: overview_block(frames as usize), frames, secs, stereo }, Some(pos));
        if let Some(v) = wave_seek(ui, dr, id.with("wave")) {
            self.set_param(3, v);
        }
        let sync = s.sync.load(Relaxed);
        if tick(ui, at(r, 0.0, h - 23.0, 168.0, 24.0), id.with("sync"), c, sync, SYNC_TEXT, PLAYER_SYNC_TIP) {
            self.set_param(4, b2f(!sync));
        }
        let looping = s.looping.load(Relaxed);
        if tick(ui, at(r, 176.0, h - 23.0, 56.0, 24.0), id.with("loop"), c, looping, "Loop", "Loop this file") {
            self.set_param(2, b2f(!looping));
        }
    }

    /// Audio Recorder (0x140020040): file row with the record button, the waveform of what has
    /// been written so far and the sync tick box.
    fn recorder_ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme) {
        let (w, h) = (r.width(), r.height());
        if file_box(ui, at(r, 0.0, 0.0, w - 28.0, 24.0), id.with("file"), c, &self.file, true) {
            if let Some(f) = save_wav(&self.file) {
                self.file = f.display().to_string();
            }
        }
        let rec = self.sh.record.load(Relaxed);
        if icon_button(ui, at(r, w - 26.0, 0.0, 24.0, 24.0), id.with("rec"), c, if rec { Icon::Stop } else { Icon::Record }, true, "Record audio input") {
            self.set_param(0, 1.0);
        }
        let dr = at(r, 0.0, 28.0, w - 2.0, h - 48.0);
        {
            let pk = self.peaks.lock().unwrap_or_else(|e| e.into_inner());
            let bin = |i: usize| pk.bins[i];
            let secs = pk.frames as f64 / pk.rate.max(1.0);
            waveform(ui.painter(), dr, c, &Wave { n: pk.bins.len(), bin: &bin, block: pk.block, frames: pk.frames, secs, stereo: true }, None);
        }
        let sync = self.sh.sync.load(Relaxed);
        if tick(ui, at(r, 0.0, h - 23.0, 168.0, 24.0), id.with("sync"), c, sync, SYNC_TEXT, "") {
            self.set_param(1, b2f(!sync));
        }
    }

    /// Metronome (0x14000c5c0, resized by 0x14000e360): the time signature in large editable
    /// numbers; the editor adds the click and accent sound files.
    fn metronome_ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme, editor: bool) {
        let (w, h) = (r.width(), r.height());
        let s = self.sh.clone();
        let play_r = if editor { at(r, w - 26.0, 0.0, 24.0, 24.0) } else { at(r, 0.0, 0.0, 24.0, 24.0) };
        let playing = s.on.load(Relaxed);
        if icon_button(ui, play_r, id.with("play"), c, if playing { Icon::Pause } else { Icon::Play }, true, "Play/pause metronome") {
            self.set_param(0, 1.0);
        }
        let (ny, nh) = if editor {
            label(ui.painter(), at(r, 0.0, 0.0, 48.0, 24.0), "Click:", c);
            if file_box(ui, at(r, 56.0, 0.0, w - 84.0, 24.0), id.with("click"), c, &self.sounds[1], true) {
                self.choose_sound(1);
            }
            label(ui.painter(), at(r, 0.0, 24.0, 64.0, 24.0), "Accent:", c);
            if file_box(ui, at(r, 56.0, 24.0, w - 58.0, 24.0), id.with("accent"), c, &self.sounds[0], true) {
                self.choose_sound(0);
            }
            (48.0, h - 71.0)
        } else {
            (24.0, h - 48.0)
        };
        let f = font((h * 0.625).max(14.0));
        let (text, edit) = (c.get(colours::TEXT), c.get(colours::TEXT_EDITOR));
        let nw = (w * 0.4896).round();
        if let Some(t) = number_label(ui, at(r, 0.0, ny, nw, nh), id.with("num"), &s.num.load(Relaxed).to_string(), f.clone(), text, edit) {
            self.set_param(1, parse_num(&t));
        }
        let sr = at(r, (w * 0.4301).round(), ny, (w * 0.1399).round(), nh);
        text_at(&ui.painter().with_clip_rect(sr.intersect(ui.clip_rect())), sr.center().x, baseline(sr, f.size), Align::Center, "/", f.clone(), text);
        if let Some(t) = number_label(ui, at(r, (w * 0.5104).round(), ny, nw, nh), id.with("den"), &s.den.load(Relaxed).to_string(), f, text, edit) {
            self.set_param(2, parse_num(&t));
        }
        let sync = s.sync.load(Relaxed);
        if tick(ui, at(r, 0.0, h - 23.0, 168.0, 24.0), id.with("sync"), c, sync, SYNC_TEXT, "Sync metronome playback to the main transport") {
            self.set_param(3, b2f(!sync));
        }
    }

    fn choose_sound(&mut self, slot: usize) {
        if let Some(f) = pick_wav(&self.sounds[slot]) {
            if let Err(m) = self.set_sound(slot, Some(&f)) {
                self.error = Some(("Metronome Error".into(), m));
            }
        }
    }

    /// Looper (box 0x1400060a0, editor 0x140008bf0): a read-only file row, transport buttons, the
    /// take's waveform and tick boxes; the editor adds the bar length and the two level dials.
    fn looper_ui(&mut self, ui: &mut Ui, r: Rect, id: Id, c: &Scheme, editor: bool) {
        let (w, h) = (r.width(), r.height());
        let s = self.sh.clone();
        let rec = s.record.load(Relaxed);
        let sound = self.extra.iter().find(|(k, _)| k.eq_ignore_ascii_case("soundFile")).map(|(_, v)| v.as_str()).unwrap_or("");
        file_box(ui, at(r, 0.0, 0.0, w - 84.0, 24.0), id.with("file"), c, sound, false);
        let playing = s.on.load(Relaxed);
        if icon_button(ui, at(r, w - 80.0, 0.0, 24.0, 24.0), id.with("play"), c, if playing { Icon::Pause } else { Icon::Play }, !rec, PLAY_TIP) {
            self.set_param(0, 1.0);
        }
        if icon_button(ui, at(r, w - 52.0, 0.0, 24.0, 24.0), id.with("rtz"), c, Icon::Rtz, true, RTZ_TIP) {
            self.set_param(1, 1.0);
        }
        if icon_button(ui, at(r, w - 24.0, 0.0, 24.0, 24.0), id.with("rec"), c, if rec { Icon::Stop } else { Icon::Record }, true, "Record/stop recording a loop") {
            self.set_param(2, 1.0);
        }
        // While a take is being recorded the display stays empty.
        let dr = at(r, 0.0, 28.0, w - 2.0, if editor { h - 155.0 } else { h - 48.0 });
        let frames = if rec { 0 } else { s.len.load(Relaxed) };
        let n = if rec { 0 } else { s.wave_n.load(Acquire).min(s.wave.len()) };
        let bin = |i: usize| s.wave[i].each_ref().map(getf);
        let pos = if frames > 0 { s.pos.load(Relaxed) as f32 / frames as f32 } else { 0.0 };
        let secs = frames as f64 / self.rate.max(1.0);
        waveform(ui.painter(), dr, c, &Wave { n, bin: &bin, block: s.wave_block.load(Relaxed).max(1), frames, secs, stereo: true }, Some(pos));
        if let Some(v) = wave_seek(ui, dr, id.with("wave")) {
            self.set_param(3, v);
        }
        let ty = h - 23.0;
        let sync = s.sync.load(Relaxed);
        if tick(ui, at(r, 0.0, ty, 168.0, 24.0), id.with("sync"), c, sync, SYNC_TEXT, PLAYER_SYNC_TIP) {
            self.set_param(4, b2f(!sync));
        }
        let stop = s.stop_bar.load(Relaxed);
        if tick(ui, at(r, 176.0, ty, 112.0, 24.0), id.with("stop"), c, stop, "Stop after bar", "Stop recording after a bar has elapsed.") {
            self.set_param(5, b2f(!stop));
        }
        if !editor {
            return;
        }
        let auto = s.auto_play.load(Relaxed);
        if tick(ui, at(r, 296.0, ty, 80.0, 24.0), id.with("auto"), c, auto, "Autoplay", "") {
            self.set_param(6, b2f(!auto));
        }
        label(ui.painter(), at(r, 0.0, h - 127.0, 80.0, 24.0), "Bar length:", c);
        let (text, edit) = (c.get(colours::TEXT), c.get(colours::TEXT_EDITOR));
        if let Some(t) = number_label(ui, at(r, 8.0, h - 103.0, 80.0, 80.0), id.with("num"), &s.num.load(Relaxed).to_string(), font(105.0), text, edit) {
            self.set_param(7, parse_num(&t));
        }
        let sr = at(r, 64.0, h - 111.0, 80.0, 80.0);
        text_at(&ui.painter().with_clip_rect(sr.intersect(ui.clip_rect())), sr.center().x, baseline(sr, 105.0), Align::Center, "/", bold(105.0), text);
        if let Some(t) = number_label(ui, at(r, 112.0, h - 103.0, 80.0, 80.0), id.with("den"), &s.den.load(Relaxed).to_string(), font(105.0), text, edit) {
            self.set_param(8, parse_num(&t));
        }
        for (k, x, name, gain) in [(9, w - 170.0, "Input level", &s.level), (10, w - 82.0, "Loop level", &s.level2)] {
            label(ui.painter(), at(r, x, h - 127.0, 78.0, 24.0), name, c);
            if let Some(v) = dial(ui, at(r, x, h - 103.0, 80.0, 80.0), id.with(("level", k)), c, getf(gain) * 0.5) {
                self.set_param(k, v);
            }
        }
    }
}

/// A rectangle placed relative to the control's top left corner.
fn at(o: Rect, x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(o.min + vec2(x, y), vec2(w, h))
}

/// The number at the start of the text, as JUCE's getFloatValue reads it (0 when there is none).
fn parse_num(t: &str) -> f32 {
    let t = t.trim_start();
    (1..=t.len()).rev().filter(|&n| t.is_char_boundary(n)).find_map(|n| t[..n].parse::<f32>().ok().filter(|v| v.is_finite())).unwrap_or(0.0)
}

/// The open dialog offers the original's wildcard "*.wav;*.aif" (0x1402813a0); saving stays WAV,
/// the only format the Recorder writes.
fn wav_dialog(current: &str, open: bool) -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    let d = if open { d.add_filter("*.wav;*.aif", &["wav", "aif"]) } else { d.add_filter("WAV file", &["wav"]) };
    match Path::new(current).parent().filter(|p| p.is_dir()) {
        Some(dir) => d.set_directory(dir),
        None => d,
    }
}

fn pick_wav(current: &str) -> Option<PathBuf> {
    crate::subwin::pick_file(wav_dialog(current, true))
}

fn save_wav(current: &str) -> Option<PathBuf> {
    let mut f = crate::subwin::save_file(wav_dialog(current, false))?;
    if f.extension().is_none() {
        f.set_extension("wav");
    }
    Some(f)
}

/// A JUCE label: 15 px text, left-aligned inside its 5 px border and centred vertically.
fn label(p: &Painter, r: Rect, text: &str, c: &Scheme) {
    text_fit(p, r.left() + 5.0, r.width() - 10.0, baseline(r, 15.0), Align::Min, text, font(15.0), c.get(colours::TEXT));
}

/// A tick box with its text (drawToggleButton 0x140024cf0 with the stock tick box): a glass
/// sphere in the Tick Box colour, a tick when on and the text fitted to the right of it.
/// Returns true when clicked.
fn tick(ui: &mut Ui, r: Rect, id: Id, c: &Scheme, on: bool, text: &str, tip: &str) -> bool {
    let resp = ui.interact(r, id, Sense::click());
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    let p = ui.painter();
    let fs = (r.height() * 0.75).min(15.0);
    let tw = fs * 1.1;
    let (bx, by) = (r.left() + 4.0, r.top() + (r.height() - tw) / 2.0);
    let thick = if resp.hovered() || resp.is_pointer_button_down_on() { 1.1 } else { 0.5 };
    let d = tw * 0.7;
    glass_sphere(p, bx, by + (tw - d) / 2.0, d, c.straight(colours::TICK_BOX), thick);
    if on {
        let k = tw / 9.0;
        let pts = [(1.5f32, 3.0f32), (3.0, 6.0), (6.0, 0.0)].map(|(x, y)| pos2(bx + x * k, by + y * k));
        p.add(Shape::line(pts.to_vec(), Stroke::new(2.5f32, c.get(colours::VECTOR))));
    }
    let tx = tw.trunc() + 5.0;
    text_fit(p, r.left() + tx, r.width() - tx - 2.0, baseline(r, fs), Align::Min, text, font(fs), c.get(colours::TEXT));
    resp.clicked()
}

/// JUCE's glass sphere: a white-tinted vertical gradient, a highlight near the top, a dark rim
/// that grows with `thick`, and a thin outline.
fn glass_sphere(p: &Painter, x: f32, y: f32, d: f32, col: [u8; 4], thick: f32) {
    if d <= thick {
        return;
    }
    let r = Rect::from_min_size(pos2(x, y), vec2(d, d));
    let (edge, mid) = (draw(overlaid(WHITE, mult_alpha(col, 0.3))), draw(overlaid(WHITE, col)));
    shade_ellipse(p, r, |q| {
        let k = ((q.y - y) / d).clamp(0.0, 1.0);
        if k < 0.4 {
            lerp(edge, mid, k / 0.4)
        } else {
            lerp(mid, edge, (k - 0.4) / 0.6)
        }
    });
    let (h0, h1) = (y + d * 0.06, y + d * 0.3);
    let hl = Rect::from_min_size(pos2(x + d * 0.2, y + d * 0.05), vec2(d * 0.6, d * 0.4));
    shade_ellipse(p, hl, |q| lerp(Color32::WHITE, Color32::TRANSPARENT, ((q.y - h0) / (h1 - h0)).clamp(0.0, 1.0)));
    let ca = col[3] as f32 / 255.0;
    let (a8, a10) = (with_alpha(BLACK, 0.1 * thick)[3] as f32, with_alpha(BLACK, 0.5 * thick * ca)[3] as f32);
    let centre = r.center();
    shade_ellipse(p, r, |q| {
        let k = (q.distance(centre) / (d / 2.0)).min(1.0);
        let a = if k <= 0.7 {
            0.0
        } else if k <= 0.8 {
            a8 * (k - 0.7) / 0.1
        } else {
            a8 + (a10 - a8) * (k - 0.8) / 0.2
        };
        Color32::from_black_alpha(a.round() as u8)
    });
    p.circle_stroke(centre, d / 2.0, Stroke::new(thick, draw(with_alpha(BLACK, 0.5 * ca))));
}

/// Points along a circle, angles clockwise from 12 o'clock as JUCE measures them.
fn arc(c: Pos2, rad: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = (((a1 - a0).abs() * rad / 3.0).ceil() as usize).clamp(2, 160);
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            pos2(c.x + rad * a.sin(), c.y - rad * a.cos())
        })
        .collect()
}

/// A rotary dial (drawRotarySlider 0x1401e9490): the value as a band from 7 o'clock and a pointer
/// in the Level Dial colour, inside a faint outline of the whole travel. Dragging up raises it
/// (250 px for the full range) and a double-click sets the middle. Returns the new 0..1 position
/// when it moved.
fn dial(ui: &mut Ui, r: Rect, id: Id, c: &Scheme, v: f32) -> Option<f32> {
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let nv = if resp.double_clicked() {
        0.5
    } else if resp.dragged() {
        (v - resp.drag_delta().y / 250.0).clamp(0.0, 1.0)
    } else {
        v
    };
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter();
    let rad = ((r.width() as i32 / 2).min(r.height() as i32 / 2) - 2) as f32;
    let cn = r.min + vec2(r.width() * 0.5, r.height() * 0.5);
    let (a0, a1) = (1.2 * PI, 2.8 * PI);
    let ang = a0 + nv * (a1 - a0);
    let fill = draw(with_alpha(c.straight(colours::LEVEL_DIAL), if hot { 1.0 } else { 0.7 }));
    if nv > 0.0 {
        p.add(Shape::line(arc(cn, rad * 0.85, a0, ang), Stroke::new(rad * 0.3, fill)));
    }
    // The pointer: a triangle 0.77 R long on a circle of 0.2 R, outlined from the tip round the
    // circle's far side.
    let (ir, tip) = (rad * 0.2, rad * 0.77);
    let t = (tip * tip - ir * ir) / (tip * tip + ir * ir);
    let th = (tip * (t - 1.0)).atan2(ir * t);
    let (sn, cs) = ang.sin_cos();
    let rot = |x: f32, y: f32| pos2(cn.x + x * cs - y * sn, cn.y + x * sn + y * cs);
    let mut pts = vec![rot(0.0, -tip)];
    pts.extend((0..=24).map(|i| th + (PI - 2.0 * th) * i as f32 / 24.0).map(|a| rot(ir * a.cos(), ir * a.sin())));
    p.add(Shape::convex_polygon(pts, fill, Stroke::NONE));
    let mut outline = arc(cn, rad, a0, a1);
    outline.extend(arc(cn, rad * 0.7, a1, a0));
    p.add(Shape::closed_line(outline, Stroke::new(if hot { 2.0f32 } else { 1.2 }, Color32::from_black_alpha(0x66))));
    (nv != v).then_some(nv)
}

#[derive(Clone, Copy)]
enum Icon {
    Play,
    Pause,
    Rtz,
    Record,
    Stop,
}

/// A 24 px transport button: the rounded button face with its picture in the Vector colour
/// (faded and inert when disabled). Returns true when clicked.
fn icon_button(ui: &mut Ui, r: Rect, id: Id, c: &Scheme, icon: Icon, enabled: bool, tip: &str) -> bool {
    let resp = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
    let resp = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    let p = ui.painter();
    button_face(p, r, 4.0, c, enabled && resp.is_pointer_button_down_on(), enabled && resp.hovered());
    let col = if enabled { c.get(colours::VECTOR) } else { draw(mult_alpha(c.straight(colours::VECTOR), 0.4)) };
    let m = r.center();
    let q = |x: f32, y: f32| m + vec2(x, y);
    let bar = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.rect_filled(Rect::from_min_max(q(x0, y0), q(x1, y1)), CornerRadius::ZERO, col);
    };
    match icon {
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![q(-4.0, -5.67), q(4.33, 0.0), q(-4.0, 5.67)], col, Stroke::NONE));
        }
        Icon::Pause => {
            bar(-4.33, -5.33, -1.0, 5.33);
            bar(1.0, -5.33, 4.33, 5.33);
        }
        Icon::Rtz => {
            p.add(Shape::convex_polygon(vec![q(-6.0, 0.0), q(3.0, -6.0), q(3.0, 6.0)], col, Stroke::NONE));
            bar(3.5, -6.0, 6.0, 6.0);
        }
        Icon::Record => {
            p.add(Shape::ellipse_filled(m, Vec2::splat(5.5), RECORD_RED));
        }
        Icon::Stop => bar(-5.0, -5.0, 5.0, 5.0),
    }
    enabled && resp.clicked()
}

/// The file row (FilenameComponent 0x1401ea3d0): the name in a combo-box face and a "..." button
/// as wide as its text plus the row height. Clicking either asks to browse; returns true then.
fn file_box(ui: &mut Ui, r: Rect, id: Id, c: &Scheme, path: &str, enabled: bool) -> bool {
    let p = ui.painter().clone();
    let h = r.height();
    let bf = (h * 0.6).min(15.0);
    let bw = (text_w(ui, "...", font(bf)) + h).trunc();
    let br = Rect::from_min_max(pos2(r.right() - bw, r.top()), r.max);
    let cr = Rect::from_min_max(r.min, pos2(br.left(), r.bottom()));
    let text_area = combo_face(&p, cr, c);
    let tp = p.with_clip_rect(text_area.intersect(p.clip_rect()));
    // The combo box's label sits 1 px in, (width + 3 − height) wide; the text has a 5 px border.
    let (lw, tb) = (cr.width() + 3.0 - h, baseline(cr.shrink2(vec2(0.0, 2.0)), 15.0));
    if path.is_empty() {
        text_fit(&tp, cr.left() + 3.0, lw - 4.0, tb, Align::Min, "<no file loaded>", font(15.0), draw(mult_alpha(c.straight(colours::TEXT), 0.5)));
    } else {
        text_fit(&tp, cr.left() + 6.0, lw - 10.0, tb, Align::Min, path, font(15.0), c.get(colours::TEXT));
    }
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let b = ui.interact(br, id.with("browse"), sense).on_hover_text(BROWSE_TIP);
    button_face(&p, br, 3.0, c, enabled && b.is_pointer_button_down_on(), enabled && b.hovered());
    text_at(&p, br.center().x, baseline(br, bf), Align::Center, "...", font(bf), c.get(colours::TEXT));
    let combo = ui.interact(cr, id.with("combo"), sense);
    if !enabled {
        p.rect_filled(r, CornerRadius::ZERO, draw(with_alpha(c.straight(colours::PLUGIN_BG), 0.5)));
    }
    enabled && (b.clicked() || combo.clicked())
}

/// What a waveform display shows: `n` bins of [min L, max L, min R, max R], `block` samples each.
struct Wave<'a> {
    n: usize,
    bin: &'a dyn Fn(usize) -> [f32; 4],
    block: usize,
    frames: u64,
    secs: f64,
    stereo: bool,
}

/// A waveform display (0x14008d050 with the read position, 0x14008d8b0 without): the window
/// colour in a faint frame, each channel in its own band drawn a pixel column at a time as
/// JUCE's thumbnail does (levels kept to 8 bits), and the length in the top right corner.
fn waveform(p: &Painter, r: Rect, c: &Scheme, w: &Wave, pos: Option<f32>) {
    p.rect_filled(r, CornerRadius::ZERO, c.get(colours::WINDOW_BG));
    p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, draw(with_alpha(BLACK, 0.25))), StrokeKind::Inside);
    let faint = draw(with_alpha(c.straight(colours::TEXT), 0.25));
    if w.frames > 0 && w.n > 0 {
        let inner = r.shrink(1.0);
        let (cols, ih) = (inner.width().max(0.0) as u64, inner.height().max(0.0) as i64);
        let nch: i64 = if w.stereo { 2 } else { 1 };
        let bands: Vec<(f32, f32)> = (0..nch).map(|i| (inner.top() + (i * ih / nch) as f32, inner.top() + ((i + 1) * ih / nch) as f32)).collect();
        let col = c.get(colours::WAVEFORM);
        let q = |v: f32| (v * 127.0).round().clamp(-128.0, 127.0);
        let mut m = Mesh::default();
        for x in 0..cols {
            let f0 = x * w.frames / cols;
            let f1 = ((x + 1) * w.frames / cols).max(f0 + 1);
            let b0 = (f0 as usize / w.block).min(w.n - 1);
            let b1 = ((f1 - 1) as usize / w.block).clamp(b0, w.n - 1);
            let mm = (b0..=b1).fold(EMPTY_BIN, |a, i| merged(a, &(w.bin)(i)));
            for (ch, &(y0, y1)) in bands.iter().enumerate() {
                let (lo, hi) = (q(mm[ch * 2]), q(mm[ch * 2 + 1]));
                if hi <= lo {
                    continue;
                }
                let (mid, vs) = ((y0 + y1) * 0.5, (y1 - y0) / 256.0);
                let xx = inner.left() + x as f32;
                m.add_colored_rect(Rect::from_min_max(pos2(xx, (mid - hi * vs - 0.3).max(y0)), pos2(xx + 1.0, (mid - lo * vs + 0.3).min(y1))), col);
            }
        }
        p.add(Shape::mesh(m));
        let s = w.secs.max(0.0) as u64;
        let tr = Rect::from_min_size(r.min + vec2(r.width() - 53.0, 1.0), vec2(50.0, 12.0));
        text_at(p, tr.right(), baseline(tr, 10.0), Align::Max, &format!("{}:{:02}", s / 60, s % 60), font(10.0), faint);
    }
    if let Some(pos) = pos {
        let x = r.left() + (r.width() - 2.0) * pos.clamp(0.0, 1.0) + 1.0;
        p.rect_filled(Rect::from_min_max(pos2(x, r.top() + 1.0), pos2(x + 1.0, r.bottom() - 1.0)), CornerRadius::ZERO, faint);
    }
}

/// Clicking or dragging on a full display moves the read position (0 < x < width − 1).
fn wave_seek(ui: &mut Ui, r: Rect, id: Id) -> Option<f32> {
    let resp = ui.interact(r, id, Sense::click_and_drag());
    let moved = resp.clicked() || resp.drag_started() || (resp.dragged() && resp.drag_delta() != Vec2::ZERO);
    let x = resp.interact_pointer_pos()?.x - r.left();
    (moved && x > 0.0 && x < r.width() - 1.0).then(|| x / (r.width() - 2.0))
}

/// The VU meter (0x1400613d0): a bar per channel that runs from the lower colour to the upper
/// one (dimmer when lower) as it rises, an over light above each at 0 dB, and lines and labels
/// at 0, 6, 12, 24 and 48 dB down. No background.
fn vu(p: &Painter, r: Rect, c: &Scheme, peaks: [f32; 2]) {
    let (w, h) = (r.width(), r.height());
    let top_h = if h < 128.0 { 10.0 } else { h * 0.078125 };
    let meter_h = h - top_h - 4.0;
    let bw = w / 2.0 - 2.0;
    let levels: Vec<(f32, f32, f32)> = peaks
        .iter()
        .enumerate()
        .map(|(ch, &pk)| {
            let db = if pk > 1e-3 { 20.0 * pk.log10() } else { -60.0 };
            (r.left() + if ch == 0 { 0.0 } else { w / 2.0 + 2.0 }, db, if db < 0.0 { db / 60.0 + 1.0 } else { 1.0 })
        })
        .collect();
    for &(x, db, _) in &levels {
        if db >= 0.0 {
            p.rect_filled(Rect::from_min_size(pos2(x, r.top()), vec2(bw, top_h)), CornerRadius::ZERO, c.get(colours::VU_OVER));
        }
    }
    let lower = c.get(colours::VU_LOWER);
    for &(x, db, fill) in &levels {
        if db <= -60.0 {
            continue;
        }
        // The gradient runs from the control's top (upper colour) down to the bar's height
        // (lower colour), so short bars show only the lower colour.
        let bh = meter_h * fill;
        let upper = draw(mult_brightness(c.straight(colours::VU_UPPER), fill));
        let col = |y: f32| lerp(upper, lower, ((y - r.top()) / bh).clamp(0.0, 1.0));
        let (y0, y2) = (r.bottom() - bh - 4.0, r.bottom() - 4.0);
        let mut m = Mesh::default();
        for y in [y0, (r.top() + bh).clamp(y0, y2), y2] {
            m.colored_vertex(pos2(x, y), col(y));
            m.colored_vertex(pos2(x + bw, y), col(y));
        }
        for i in [0u32, 2] {
            m.add_triangle(i, i + 1, i + 3);
            m.add_triangle(i, i + 3, i + 2);
        }
        p.add(Shape::mesh(m));
    }
    let line = draw(with_alpha(c.straight(colours::TEXT), 0.25));
    let text = draw(with_alpha(c.straight(colours::TEXT), 0.5));
    let fs = if w <= 64.0 { 8.0 } else if w <= 192.0 { w * 0.125 } else { 24.0 };
    let marks = [(0.0, "0dB"), (0.1, "6dB"), (0.2, "12dB"), (0.4, "24dB"), (0.8, "48dB")];
    for (k, _) in marks {
        p.rect_filled(Rect::from_min_size(pos2(r.left(), r.top() + top_h + meter_h * k), vec2(w, 1.0)), CornerRadius::ZERO, line);
    }
    for (k, t) in marks {
        let lr = Rect::from_center_size(pos2(r.center().x, r.top() + top_h + meter_h * k), vec2(w, fs));
        text_at(p, lr.center().x, baseline(lr, fs), Align::Center, t, font(fs), text);
    }
}

/// The Output Toggle's switch picture: a 48-unit drawing scaled to fit and centred, in the
/// background colour darkened halfway. Off routes the curve to the upper output, on to the lower.
/// Returns true when clicked.
fn toggle_picture(ui: &mut Ui, r: Rect, id: Id, on: bool, bg: Color32) -> bool {
    let resp = ui.interact(r, id, Sense::click());
    let p = ui.painter();
    let s = r.width().min(r.height()) / 48.0;
    let o = r.center() - vec2(24.0 * s, 24.0 * s);
    let pt = |x: f32, y: f32| o + vec2(x * s, y * s);
    let col = lerp(bg, Color32::BLACK, 0.5);
    let (curve, dot) = if on {
        ([(6.0f32, 24.5f32), (28.07, 24.5), (21.88, 38.5), (42.0, 38.5)], (42.0, 10.0))
    } else {
        ([(6.0, 24.0), (28.07, 24.0), (21.88, 10.0), (42.0, 10.0)], (42.0, 38.5))
    };
    let pts: Vec<Pos2> = (0..=32)
        .map(|i| {
            let t = i as f32 / 32.0;
            let u = 1.0 - t;
            let k = [u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t];
            let (x, y) = curve.iter().zip(k).fold((0.0, 0.0), |(ax, ay), (&(x, y), k)| (ax + x * k, ay + y * k));
            pt(x, y)
        })
        .collect();
    let ends = [pts[0], pts[32]];
    p.add(Shape::line(pts, Stroke::new(10.0 * s, col)));
    for e in ends.into_iter().chain([pt(dot.0, dot.1)]) {
        p.add(Shape::ellipse_filled(e, Vec2::splat(5.0 * s), col));
    }
    resp.clicked()
}

/// A number shown centred that turns into a text box on a single click, as the original's
/// editable labels do. Enter or clicking elsewhere hands back the typed text; Escape drops it.
fn number_label(ui: &mut Ui, r: Rect, id: Id, text: &str, f: FontId, col: Color32, edit_fill: Color32) -> Option<String> {
    let (buf_id, fresh_id, edit_id) = (id.with("buf"), id.with("fresh"), id.with("edit"));
    let clip = r.intersect(ui.clip_rect());
    let p = ui.painter().with_clip_rect(clip);
    let Some(mut buf) = ui.data(|d| d.get_temp::<String>(buf_id)) else {
        let resp = ui.interact(r, id, Sense::click());
        text_fit(&p, r.left(), r.width(), baseline(r, f.size), Align::Center, text, f, col);
        if resp.clicked() {
            ui.data_mut(|d| {
                d.insert_temp(buf_id, text.to_string());
                d.insert_temp(fresh_id, true);
            });
        }
        return None;
    };
    p.rect_filled(r, CornerRadius::ZERO, edit_fill);
    let row = ui.fonts(|fo| fo.row_height(&f));
    let mut eu = ui.new_child(UiBuilder::new().max_rect(Rect::from_center_size(r.center(), vec2(r.width(), row))).id_salt(edit_id));
    eu.set_clip_rect(clip);
    let out = TextEdit::singleline(&mut buf).id(edit_id).frame(false).font(f).text_color(col).horizontal_align(Align::Center).margin(vec2(0.0, 0.0)).desired_width(r.width()).show(&mut eu);
    if ui.data_mut(|d| d.remove_temp::<bool>(fresh_id)).unwrap_or(false) {
        out.response.request_focus();
        let mut st = out.state;
        st.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(0), CCursor::new(buf.chars().count()))));
        st.store(ui.ctx(), edit_id);
    } else if !out.response.has_focus() {
        ui.data_mut(|d| d.remove::<String>(buf_id));
        return (!ui.input(|i| i.key_pressed(egui::Key::Escape))).then_some(buf);
    }
    ui.data_mut(|d| d.insert_temp(buf_id, buf));
    None
}

#[cfg(test)]
mod tests {
    use super::parse_num;

    #[test]
    fn typed_numbers_read_like_juce() {
        assert_eq!(parse_num(" 7/8"), 7.0);
        assert_eq!(parse_num("1.5x"), 1.5);
        assert_eq!(parse_num("abc"), 0.0);
        assert_eq!(parse_num("inf"), 0.0);
    }
}
