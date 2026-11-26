//! The mappings editor (MIDI controllers and OSC addresses per box), the Patch Organiser and
//! the User Preset Management window.

use crate::app::{App, Cmd, Patch, UNTITLED};
use crate::bars::baseline;
use crate::builtins::Kind;
use crate::colours;
use crate::engine::{MidiMap, OscMap};
use crate::pdl::El;
use crate::plugins::PluginInfo;
use crate::presets;
use crate::widgets::{bevel, bold, font, gradient_rrect, rgb, scrollbar, text_at, text_fit};
use egui::*;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub struct MapDraft {
    /// The MIDI mapping (index into the graph's list) waiting for a controller to learn.
    pub learn: Option<usize>,
    params_for: Option<u32>,
    params: Vec<(i32, String)>,
}

/// A name made safe for a file or folder.
pub(crate) fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if "\\/:*?\"<>|".contains(c) || c.is_control() { '_' } else { c }).collect();
    let t = t.trim().trim_end_matches('.').to_string();
    if t.is_empty() { "_".into() } else { t }
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
    Import,
    Export,
    Up,
    Down,
}

/// The Patch Organiser's content size. Its constructor asks for 600 x 400 (0x14005d499), but the
/// menu command that opens it makes it 400 x 300 (0x1400384d6) before showing it.
const ORGANISER_SIZE: Vec2 = vec2(400.0, 300.0);
/// The smallest the rebuild lets it get, keeping Move Down (bottom at 240) in view. The original
/// has no limit (estimated).
const ORGANISER_MIN: Vec2 = vec2(200.0, 248.0);
/// The patch list: JUCE's ListBox rows are 22 high (0x140202755), each a JUCE Label with 15 px text
/// (0x14043394c), centred left, 5 px in from the sides and squeezed down to 70% when too long
/// (0x1401fffb0). The scroll bar is JUCE's usual 18 px (estimated).
const ROW_H: f32 = 22.0;
const ROW_FONT: f32 = 15.0;
const ROW_INDENT: f32 = 5.0;
const LIST_SB: f32 = 18.0;
/// The list's 1 px outline in JUCE's default ListBox outline colour (0x140424648), and the tint of
/// every second row (paintListBoxItem, 0x14005ee30).
const LIST_OUTLINE: [u8; 4] = [0x80, 0x80, 0x80, 0xb2];
const ODD_ROW: [u8; 4] = [0, 0, 0, 0x10];
/// A selected row fades from "List Selected Colour" brightened by 0.4 at its top to the same colour
/// darkened by 0.125 at its bottom (0x14005ee30, with brighter 0x140146b90 and darker 0x140146c30).
const SELECTED_TOP: f32 = 0.4;
const SELECTED_BOTTOM: f32 = 0.125;
/// Renaming a row in place (Label::showEditor 0x140200d40, createEditorComponent 0x1402012d0):
/// the text box's text sits its 1 px border plus 4 px indent in from the left and top, with a
/// 3 px border on the right and 1 below (TextEditor constructor, 0x14020c050). While it has the
/// keyboard it draws a 2 px outline and a 4 px inner shadow of 0x38000000 at 0.75 strength
/// (drawTextEditorOutline 0x1401e7b60, colour table 0x14042458c, 0.75 at 0x140433888).
const EDIT_TEXT_LEFT: i8 = 5;
const EDIT_TEXT_TOP: f32 = 5.0;
const EDIT_RIGHT: i8 = 3;
const EDIT_BOTTOM: i8 = 1;
const EDIT_OUTLINE: f32 = 2.0;
const EDIT_BEVEL: u32 = 4;
const EDIT_SHADOW: [u8; 4] = [0, 0, 0, 0x38];
const EDIT_SHADOW_FADE: f32 = 0.75;
/// Without the keyboard a text box has a 1 px outline in its outline colour and a 3 px inner
/// shadow at full strength (the same drawTextEditorOutline).
const EDIT_BEVEL_IDLE: u32 = 3;

/// The file choosers' titles and the box Import... opens (buttonClicked, 0x14005d870). Export...
/// is the rebuild's own, so its title is too.
const IMPORT_FILE_TITLE: &str = "Select file to import patch from...";
const IMPORT_TITLE: &str = "Select patch...";
const EXPORT_FILE_TITLE: &str = "Select file to export patch to...";

/// JUCE's AlertWindow as its updateLayout (0x140217bc0) sizes it: the title in the message font
/// x1.1 and bold (0x14043b888), at least 350 wide and at most 70% of what it opens over
/// (0x14041db74), the text 10 in from the edges, 40 more below it, 50 for each combo box and
/// 20 + the button height for the buttons; combo boxes 22 high at 10% of the width and 80% wide;
/// buttons 16 apart, centred, with their bottoms at 95% of the height (0x14043ce98). The message
/// font is 15 px plain (the original's look and feel, 0x140023670), and a button is 28 high
/// (0x1401e47d0) and its text width + 28 wide (changeWidthToFitText, 0x1401baa00).
const ALERT_FONT: f32 = 15.0;
const ALERT_TITLE_SCALE: f32 = 1.1;
const ALERT_MIN_W: f32 = 350.0;
const ALERT_MAX_PART: f32 = 0.7;
const ALERT_EDGE: f32 = 10.0;
const ALERT_TEXT_GAP: f32 = 40.0;
const ALERT_COMBO_ROW: f32 = 50.0;
const ALERT_COMBO_H: f32 = 22.0;
const ALERT_COMBO_X: f32 = 0.1;
const ALERT_COMBO_W: f32 = 0.8;
const ALERT_BUTTON_H: f32 = 28.0;
const ALERT_BUTTON_GAP: f32 = 16.0;
const ALERT_BUTTONS_BOTTOM: f32 = 0.95;

/// Where an AlertWindow puts a title, a message, one combo box and its buttons, from its top left.
struct AlertLayout {
    size: Vec2,
    /// Each line of text: its top, its text and its font. Lines are centred across the box.
    lines: Vec<(f32, String, FontId)>,
    combo: Rect,
    buttons: Vec<Rect>,
}

fn alert_layout(ctx: &Context, title: &str, message: &str, buttons: &[&str], parent: Vec2) -> AlertLayout {
    let (mf, tf) = (font(ALERT_FONT), bold(ALERT_FONT * ALERT_TITLE_SCALE));
    ctx.fonts(|f| {
        let width = |s: &str, ft: &FontId| f.layout_no_wrap(s.to_string(), ft.clone(), Color32::BLACK).size().x.round();
        let rows = |s: &str, ft: &FontId, wrap: f32| -> Vec<String> {
            let g = f.layout(s.to_string(), ft.clone(), Color32::BLACK, wrap);
            g.rows.iter().map(|r| r.glyphs.iter().map(|gl| gl.chr).collect::<String>().trim().to_string()).collect()
        };
        let max_w = (parent.x * ALERT_MAX_PART).floor();
        // The text wraps at 300 + 2 * sqrt(font height * the wider of title and message, both
        // measured in the message font). Its line balancing (0x140188e30) keeps that first width
        // in this JUCE version: it stops once longest / shortest line > 0.9, which is always.
        let widest = width(message, &mf).max(width(title, &mf));
        let wrap = (300.0 + 2.0 * (ALERT_FONT * widest).sqrt().floor()).min(max_w);
        let mut lines = Vec::new();
        let mut y = ALERT_EDGE;
        for r in rows(title, &tf, wrap) {
            lines.push((y, r, tf.clone()));
            y += tf.size;
        }
        // The title and the message are one text with a blank line between them.
        y += ALERT_FONT;
        for r in rows(message, &mf, wrap) {
            lines.push((y, r, mf.clone()));
            y += ALERT_FONT;
        }
        let text_w = lines.iter().map(|(_, s, ft)| width(s, ft)).fold(0.0, f32::max);
        let w = (text_w + 4.0 * ALERT_EDGE).max(ALERT_MIN_W).min(max_w);
        let text_bottom = (y - ALERT_EDGE).floor() + ALERT_TEXT_GAP;
        let h = (text_bottom + ALERT_COMBO_ROW + 20.0 + ALERT_BUTTON_H).min(parent.y - 50.0);
        let combo = Rect::from_min_size(pos2((w * ALERT_COMBO_X).round(), text_bottom), vec2((w * ALERT_COMBO_W).round(), ALERT_COMBO_H));
        let widths: Vec<f32> = buttons.iter().map(|b| width(b, &mf) + ALERT_BUTTON_H).collect();
        let total = widths.iter().sum::<f32>() + ALERT_BUTTON_GAP * widths.len().saturating_sub(1) as f32;
        let top = (h * ALERT_BUTTONS_BOTTOM).round() - ALERT_BUTTON_H;
        let mut x = ((w - total) / 2.0).floor();
        let buttons = widths
            .iter()
            .map(|bw| {
                let r = Rect::from_min_size(pos2(x, top), vec2(*bw, ALERT_BUTTON_H));
                x += bw + ALERT_BUTTON_GAP;
                r
            })
            .collect();
        AlertLayout { size: vec2(w, h), lines, combo, buttons }
    })
}

/// The top margin that puts the text of egui's text box in `r` where a JUCE TextEditor has it:
/// egui puts the text's top at the margin, and JUCE starts a `size` px line 5 px down.
fn edit_top(ui: &Ui, r: Rect, size: f32) -> i8 {
    let line = Rect::from_min_size(r.min + vec2(0.0, EDIT_TEXT_TOP), vec2(r.width(), size));
    let asc = ui
        .fonts(|f| f.layout_no_wrap("A".into(), font(size), Color32::BLACK))
        .rows
        .first()
        .and_then(|row| row.glyphs.first())
        .map_or(0.0, |g| g.pos.y);
    (baseline(line, size) - r.top() - asc).round().clamp(0.0, r.height()) as i8
}

/// Whether two paths name the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A button filling `r` with its label on one line, as the original's look draws button text
/// (drawButtonText, 0x140021bb0): in the scheme's text colour at half strength when the button is
/// off, 1 px right and down while it's held, 8 px in from each side for an 82 x 24 button and
/// squeezed when it doesn't fit.
fn fitted_button(ui: &mut Ui, r: Rect, label: &str, enabled: bool, text: Color32) -> Response {
    const INDENT: f32 = 8.0;
    let b = ui.scope_builder(UiBuilder::new().max_rect(r), |ui| ui.add_enabled(enabled, Button::new("").min_size(r.size()))).inner;
    let col = if enabled { text } else { text.gamma_multiply(0.5) };
    let nudge = if b.is_pointer_button_down_on() { 1.0 } else { 0.0 };
    let f = TextStyle::Button.resolve(ui.style());
    text_fit(ui.painter(), r.left() + INDENT + nudge, r.width() - 2.0 * INDENT, baseline(r, f.size) + nudge, Align::Center, label, f, col);
    b
}

/// The Patch Organiser's own state while it's open. Nothing is selected when it opens: the menu
/// command (0x1400384e4) builds a new organiser each time and never selects a row.
#[derive(Default)]
pub struct OrganiserState {
    /// The selected rows, lowest first, as JUCE's ListBox keeps them.
    rows: BTreeSet<usize>,
    /// The row whose name is being edited in place, its text so far, and whether it just opened.
    edit: Option<(usize, String, bool)>,
    scroll: f32,
    /// A row to scroll into view.
    reveal: Option<usize>,
}

/// What a patch's row shows: its name, as in the original (refreshComponentForRow, 0x14005f020).
/// The original names a new patch "N - <untitled>" (comboBoxChanged, 0x140035df0); the rebuild's
/// are "<untitled>" and numbered where they're listed, so they get their number here too.
fn row_text(i: usize, p: &Patch) -> String {
    if p.name == UNTITLED { format!("{} - {UNTITLED}", i + 1) } else { p.name.clone() }
}

/// A click on a row (listBoxItemClicked, 0x14005f2c0): only that row is selected, or with Ctrl it
/// joins the selection. Ctrl can't take a row out again, because the original selects it straight
/// after JUCE flips it, and Shift works like a plain click.
fn click_row(rows: &mut BTreeSet<usize>, row: usize, ctrl: bool) {
    if !ctrl {
        rows.clear();
    }
    rows.insert(row);
}

/// Copy: each selected row, lowest first, is copied to the end of the list as "<name> (copy)"
/// (0x14003cf90, called for each row by buttonClicked 0x14005d870). The selection and the playing
/// patch stay as they are. Returns whether anything was copied.
fn copy_rows(patches: &mut Vec<Patch>, rows: &BTreeSet<usize>) -> bool {
    let copies: Vec<Patch> = rows.iter().filter_map(|&r| patches.get(r)).map(|p| Patch { name: format!("{} (copy)", p.name), tempo: p.tempo, el: p.el.clone() }).collect();
    let any = !copies.is_empty();
    patches.extend(copies);
    any
}

/// Remove: the selected rows go from the last to the first while more than one patch is left
/// (buttonClicked, 0x14005d870). When the playing patch goes, the one above it plays instead (the
/// one below for the first row) as in the original; any other patch keeps playing. Returns whether
/// the playing patch changed.
fn remove_rows(patches: &mut Vec<Patch>, current: &mut usize, rows: &BTreeSet<usize>) -> bool {
    let mut switched = false;
    for &r in rows.iter().rev() {
        if patches.len() < 2 {
            break;
        }
        if r >= patches.len() {
            continue;
        }
        if r == *current {
            *current = if r > 0 { r - 1 } else { r + 1 };
            switched = true;
        }
        patches.remove(r);
        if *current > r {
            *current -= 1;
        }
    }
    switched
}

/// Move Up and Move Down: the lowest selected row swaps places with its neighbour and becomes the
/// only selected row (buttonClicked, 0x14005d870). The playing patch keeps playing wherever it ends
/// up. Returns the row the patch moved to.
fn move_row(patches: &mut [Patch], current: &mut usize, rows: &mut BTreeSet<usize>, up: bool) -> Option<usize> {
    let r = *rows.first()?;
    let to = if up { r.checked_sub(1)? } else { r + 1 };
    if to >= patches.len() || r >= patches.len() {
        return None;
    }
    patches.swap(r, to);
    if *current == r {
        *current = to;
    } else if *current == to {
        *current = r;
    }
    rows.clear();
    rows.insert(to);
    Some(to)
}

/// The organiser's patch list (paintListBoxItem 0x14005ee30, refreshComponentForRow 0x14005f020):
/// rows of names on "Dialog Inner Background", every second row tinted, selected rows shaded with
/// white text, and a 1 px outline drawn over them. Double-clicking a row edits its name in place:
/// Enter or a click elsewhere keeps it and Escape drops it, as a JUCE Label does. A click below the
/// rows clears the selection (backgroundClicked 0x14005f370, deselectAllRows 0x1402030e0).
fn organiser_list(ui: &mut Ui, list: Rect, patches: &[Patch], c: &colours::Scheme, st: &mut OrganiserState) -> Option<(usize, String)> {
    let p = ui.painter().clone();
    p.rect_filled(list, CornerRadius::ZERO, c.get(colours::DIALOG_INNER));
    let view = list.shrink(1.0);
    let total = patches.len() as f32 * ROW_H;
    let id = Id::new("patchList");
    if ui.rect_contains_pointer(view) {
        st.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    if let Some(r) = st.reveal.take() {
        let top = r as f32 * ROW_H;
        st.scroll = st.scroll.max(top + ROW_H - view.height()).min(top);
    }
    let max_scroll = (total - view.height()).max(0.0);
    st.scroll = st.scroll.clamp(0.0, max_scroll);
    let rows_w = if max_scroll > 0.0 {
        scrollbar(ui, Rect::from_min_max(pos2(view.right() - LIST_SB, view.top()), view.max), true, &mut st.scroll, view.height(), total, id.with("sb"));
        st.scroll = st.scroll.clamp(0.0, max_scroll);
        view.width() - LIST_SB
    } else {
        view.width()
    };
    let rows_r = Rect::from_min_size(view.min, vec2(rows_w, view.height()));
    let clip = rows_r.intersect(ui.clip_rect());
    let rp = p.with_clip_rect(clip);
    let row_rect = |i: usize, scroll: f32| Rect::from_min_size(pos2(rows_r.left(), rows_r.top() + i as f32 * ROW_H - scroll), vec2(rows_w, ROW_H));
    // Rows are hit before the background under them.
    if ui.interact(rows_r, id.with("bg"), Sense::click()).clicked() {
        st.rows.clear();
    }
    let ls = c.get(colours::LIST_SELECTED);
    let first = (st.scroll / ROW_H) as usize;
    let last = (((st.scroll + rows_r.height()) / ROW_H).ceil() as usize).min(patches.len());
    for i in first..last {
        let r = row_rect(i, st.scroll);
        let resp = ui.interact(r.intersect(rows_r), id.with(("row", i)), Sense::click());
        if resp.clicked() || resp.secondary_clicked() {
            click_row(&mut st.rows, i, ui.input(|inp| inp.modifiers.command));
            st.reveal = Some(i);
            if resp.double_clicked() {
                st.edit = Some((i, row_text(i, &patches[i]), true));
            }
        }
        let sel = st.rows.contains(&i);
        if sel {
            gradient_rrect(&rp, r, 0.0, colours::brighter(ls, SELECTED_TOP), colours::darker(ls, SELECTED_BOTTOM));
        } else if i % 2 == 1 {
            rp.rect_filled(r, CornerRadius::ZERO, colours::draw(ODD_ROW));
        }
        if !matches!(st.edit, Some((k, _, _)) if k == i) {
            let col = if sel { Color32::WHITE } else { Color32::BLACK };
            text_fit(&rp, r.left() + ROW_INDENT, r.width() - 2.0 * ROW_INDENT, baseline(r, ROW_FONT), Align::Min, &row_text(i, &patches[i]), font(ROW_FONT), col);
        }
    }
    let mut renamed = None;
    let mut done = false;
    let scroll = st.scroll;
    if let Some((k, buf, fresh)) = st.edit.as_mut() {
        let r = row_rect(*k, scroll);
        let box_clip = r.intersect(clip);
        let mut tu = ui.new_child(UiBuilder::new().max_rect(r).id_salt("organiser_rename"));
        tu.set_clip_rect(box_clip);
        // The label hands its text box its own black text on a clear background; the original's
        // look puts "Button Highlight" behind selected text (0x1400211e0) and leaves the caret
        // and the selected text at the default table's black (0x140424594, 0x140424574).
        tu.visuals_mut().selection.bg_fill = c.get(colours::BUTTON_HIGHLIGHT);
        tu.visuals_mut().text_cursor.stroke.color = Color32::BLACK;
        let top = edit_top(ui, r, ROW_FONT);
        let edit_id = id.with("rename");
        let out = TextEdit::singleline(buf)
            .id(edit_id)
            .font(font(ROW_FONT))
            .text_color(Color32::BLACK)
            .frame(false)
            .margin(Margin { left: EDIT_TEXT_LEFT, right: EDIT_RIGHT, top, bottom: EDIT_BOTTOM })
            .desired_width(f32::INFINITY)
            .min_size(r.size())
            .show(&mut tu);
        if *fresh || out.response.has_focus() {
            // Its outline is "Menu Selection Colour" in the original's look (0x1400211e0); the
            // shadow is drawn 2 px taller than the box, so its bottom edge mostly falls outside.
            let ep = ui.painter().with_clip_rect(box_clip);
            ep.rect_stroke(r, CornerRadius::ZERO, Stroke::new(EDIT_OUTLINE, c.get(colours::MENU_SELECTION)), StrokeKind::Inside);
            bevel(&ep, Rect::from_min_size(r.min, r.size() + vec2(0.0, 2.0)), EDIT_BEVEL, colours::draw(EDIT_SHADOW).gamma_multiply(EDIT_SHADOW_FADE));
        }
        if *fresh {
            *fresh = false;
            out.response.request_focus();
            let mut state = out.state;
            state.cursor.set_char_range(Some(text::CCursorRange::two(text::CCursor::new(0), text::CCursor::new(buf.chars().count()))));
            state.store(ui.ctx(), edit_id);
        } else if !out.response.has_focus() {
            if !ui.input(|i| i.key_pressed(Key::Escape)) && *buf != row_text(*k, &patches[*k]) {
                renamed = Some((*k, buf.clone()));
            }
            done = true;
        }
    }
    if done {
        st.edit = None;
    }
    p.rect_stroke(list, CornerRadius::ZERO, Stroke::new(1.0f32, colours::draw(LIST_OUTLINE)), StrokeKind::Inside);
    renamed
}

/// The patches of the file Import... opened, and the one picked in "Select patch...".
pub struct ImportPick {
    file_name: String,
    patches: Vec<Patch>,
    sel: usize,
}

/// The patches Import... finds in a file's text (buttonClicked, 0x14005d870): every child of the
/// root tagged "Patch" in any case (the root's own tag isn't checked), named by its "name".
pub fn patches_in(text: &str) -> Result<Vec<Patch>, String> {
    let root = El::parse(text)?;
    Ok(root
        .children
        .iter()
        .filter(|c| c.name.eq_ignore_ascii_case("Patch"))
        .map(|c| {
            let mut el = c.clone();
            el.name = "Patch".into();
            Patch { name: c.get("name").unwrap_or(UNTITLED).to_string(), tempo: c.get_f("tempo", 120.0), el: Some(el) }
        })
        .collect())
}

/// The file Export... writes: a patch file holding just these patches, which Open and Import... read.
pub fn export_document(patches: &[&Patch]) -> String {
    let mut root = El::new("Pedalboard2PatchFile");
    root.children.extend(patches.iter().map(|p| p.to_el()));
    root.to_document()
}

/// User Preset Management as the menu command opens it (0x1400385d9): 400 x 300 in the middle,
/// and it can be resized.
const PRESETS_TITLE: &str = "User Preset Management";
const PRESETS_SIZE: Vec2 = vec2(400.0, 300.0);
/// The smallest the rebuild lets it get, keeping Export... (bottom at 160) in view. The original
/// has no limit (estimated).
const PRESETS_MIN: Vec2 = vec2(200.0, 168.0);
/// Its paint (0x14008b2e0): the whole window in 0xEEECE1, whatever the colour scheme, and a
/// 0x40000000 shade 1 px larger than the tree all round.
const PRESETS_BG: u32 = 0xEEECE1;
const TREE_SHADE: [u8; 4] = [0, 0, 0, 0x40];
/// The tree (resized 0x14008b360): JUCE's TreeView rows are 20 high (0x14001bc50) and each level
/// is 24 px further in. Its root is hidden, so the plugin folders start 24 px in, with their
/// open/close box in the 24 px before them, and their presets 48 px in.
const TREE_ROW: f32 = 20.0;
const TREE_INDENT: f32 = 24.0;
/// Names are 16 px (0x14042ef7c): a folder's bold (PluginItem::paint 0x140089e60), a preset's plain
/// and 4 px in (PresetItem::paint 0x140089980). The selected preset sits on a box 4 px narrower than
/// its row with 4 px corners, shaded like a selected patch.
const TREE_FONT: f32 = 16.0;
const PRESET_TEXT_X: f32 = 4.0;
const PRESET_SEL_GAP: f32 = 4.0;
const PRESET_SEL_RADIUS: f32 = 4.0;
/// The lines joining presets to their folder: 1 px lines on half-pixel positions in JUCE's
/// default line colour 0x4C000000 (linesColourId 0x1000501, 0x1404245c0; paint 0x140215a20),
/// which cover two pixels at half strength each.
const TREE_LINE: [u8; 4] = [0, 0, 0, 0x26];
/// The open/close box (0x1401e6110 via 0x140215980): 11 px, centred in the 24 x 20 before the
/// folder, filled 0xE5FFFFFF with a 0x80000000 outline, a minus and, while closed, a plus.
const TREE_BOX: f32 = 11.0;
const TREE_BOX_FILL: [u8; 4] = [255, 255, 255, 0xE5];
const TREE_BOX_MARK: [u8; 4] = [0, 0, 0, 0x80];

#[derive(Clone, Copy)]
enum PresetOp {
    Rename,
    Copy,
    Remove,
    Import,
    Export,
}

/// The manager's buttons: 82 x 24, 90 px in from the right edge (resized 0x14008b360), always
/// on, with the original's tooltips (constructor 0x14008a320).
const PRESET_BUTTONS: [(f32, &str, &str, PresetOp); 5] = [
    (8.0, "Rename", "Rename the selected preset", PresetOp::Rename),
    (40.0, "Copy", "Duplicate the selected preset", PresetOp::Copy),
    (72.0, "Remove", "Delete the selected preset", PresetOp::Remove),
    (104.0, "Import...", "Import a preset from an .fxp file", PresetOp::Import),
    (136.0, "Export...", "Export the selected preset to an .fxp file", PresetOp::Export),
];

/// The prompts' buttons, Cancel first, with Return on Ok and no key on Cancel (0x14008b450).
const PROMPT_BUTTONS: [&str; 2] = ["Cancel", "Ok"];
const IMPORT_PRESET_TITLE: &str = "Import preset";
const EXPORT_PRESET_TITLE: &str = "Export preset";
const DELETE_ERROR_TITLE: &str = "Preset Deletion Error";
const DELETE_ERROR: &str = "Could not delete preset from the filesystem. Check your permissions.";
const IMPORT_ERROR_TITLE: &str = "Preset Import Error";
const IMPORT_ERROR: &str = "Could not create a directory for this plugin. Check your permissions.";
/// The rebuild's own: the original writes over a preset of the same name.
const PRESET_EXISTS: &str = "A preset with that name already exists.";

#[derive(Clone, Copy, PartialEq, Debug)]
enum PromptKind {
    Copy,
    Rename,
    Import,
}

impl PromptKind {
    /// The box's title and message (buttonClicked, 0x14008b450).
    fn words(self) -> (&'static str, &'static str) {
        match self {
            PromptKind::Copy => ("Copy Preset", "Enter a name for the duplicate preset:"),
            PromptKind::Rename => ("Rename Preset", "Enter a new name for the selected preset:"),
            PromptKind::Import => ("Import Preset", "Select which plugin this preset is intended for:"),
        }
    }
}

/// An open prompt: the preset it's for as (folder, name), the name typed so far (empty at first,
/// as in the original), the plugin folders Import offers and the one picked (the first at first),
/// and whether it just opened.
struct PresetPrompt {
    kind: PromptKind,
    target: Option<(String, String)>,
    text: String,
    choices: Vec<String>,
    pick: usize,
    fresh: bool,
}

impl PresetPrompt {
    fn new(kind: PromptKind) -> PresetPrompt {
        PresetPrompt { kind, target: None, text: String::new(), choices: Vec::new(), pick: 0, fresh: true }
    }
}

/// User Preset Management's own state while it's open: the preset folder as plugin folders and
/// their presets, the folders the person closed (all open at first, as each PluginItem opens itself,
/// 0x140089bc0), the selected preset as (folder, name), the scroll and an open prompt.
pub struct PresetManager {
    tree: Vec<(String, Vec<String>)>,
    listed: bool,
    closed: BTreeSet<String>,
    sel: Option<(String, String)>,
    scroll: f32,
    prompt: Option<PresetPrompt>,
    /// Which opening this is, so each is a fresh window in the middle at the original's size.
    opens: u32,
}

impl Default for PresetManager {
    fn default() -> PresetManager {
        static OPENS: AtomicU32 = AtomicU32::new(0);
        PresetManager { tree: Vec::new(), listed: false, closed: BTreeSet::new(), sel: None, scroll: 0.0, prompt: None, opens: OPENS.fetch_add(1, Ordering::Relaxed) }
    }
}

impl PresetManager {
    /// The preset files changed: the tree is read again before it's drawn next.
    pub fn refresh(&mut self) {
        self.listed = false;
    }

    pub fn prompting(&self) -> bool {
        self.prompt.is_some()
    }

    /// Reads the preset folder: each plugin folder and its .fxp files (RootItem 0x140089fd0,
    /// PluginItem 0x140089bc0). What's gone stops being selected or closed.
    fn relist(&mut self, root: &Path) {
        self.tree = presets::folders(root)
            .into_iter()
            .map(|f| {
                let names = presets::list(&root.join(&f));
                (f, names)
            })
            .collect();
        let tree = &self.tree;
        if self.sel.as_ref().is_some_and(|(f, n)| !tree.iter().any(|(tf, l)| tf == f && l.contains(n))) {
            self.sel = None;
        }
        self.closed.retain(|c| tree.iter().any(|(f, _)| f == c));
        self.listed = true;
    }
}

/// A row of the tree: a plugin folder, or a preset in one as (folder, preset).
#[derive(Clone, Copy, PartialEq, Debug)]
enum TreeRow {
    Folder(usize),
    Preset(usize, usize),
}

/// The rows shown: every folder, followed by its presets while it's open.
fn tree_rows(tree: &[(String, Vec<String>)], closed: &BTreeSet<String>) -> Vec<TreeRow> {
    let mut rows = Vec::new();
    for (i, (f, names)) in tree.iter().enumerate() {
        rows.push(TreeRow::Folder(i));
        if !closed.contains(f) {
            rows.extend((0..names.len()).map(|k| TreeRow::Preset(i, k)));
        }
    }
    rows
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TreeHit {
    Select,
    Toggle,
    Nothing,
}

/// What a press `x` px from the tree's left edge does to a row (TreeView mouseDown, 0x140212370):
/// on the item it selects it, in the 24 px before a folder it opens or closes it, and anywhere
/// else it does nothing, as presets have no box.
fn tree_hit(row: TreeRow, x: f32) -> TreeHit {
    let item_x = match row {
        TreeRow::Folder(_) => TREE_INDENT,
        TreeRow::Preset(..) => 2.0 * TREE_INDENT,
    };
    if x >= item_x {
        TreeHit::Select
    } else if x >= item_x - TREE_INDENT && matches!(row, TreeRow::Folder(_)) {
        TreeHit::Toggle
    } else {
        TreeHit::Nothing
    }
}

/// The tree of plugin folders and their presets (paintRecursively 0x140215d40). A press picks a
/// row or opens or closes a folder from its box, and a double-click on a folder's name opens or
/// closes it too (itemDoubleClicked 0x140215a70). Picking a folder leaves no preset selected.
fn preset_tree(ui: &mut Ui, view: Rect, root: &Path, c: &colours::Scheme, m: &mut PresetManager) {
    let id = Id::new(("presetTree", m.opens));
    let mut rows = tree_rows(&m.tree, &m.closed);
    if ui.rect_contains_pointer(view) {
        m.scroll -= ui.input(|i| i.smooth_scroll_delta.y);
    }
    let total = rows.len() as f32 * TREE_ROW;
    let max_scroll = (total - view.height()).max(0.0);
    m.scroll = m.scroll.clamp(0.0, max_scroll);
    let rows_w = if max_scroll > 0.0 {
        scrollbar(ui, Rect::from_min_max(pos2(view.right() - LIST_SB, view.top()), view.max), true, &mut m.scroll, view.height(), total, id.with("sb"));
        m.scroll = m.scroll.clamp(0.0, max_scroll);
        view.width() - LIST_SB
    } else {
        view.width()
    };
    let rows_r = Rect::from_min_size(view.min, vec2(rows_w, view.height()));
    let clip = rows_r.intersect(ui.clip_rect());
    // Dragging on the tree mustn't move the window.
    let resp = ui.interact(rows_r, id.with("rows"), Sense::click_and_drag());
    let row_under = |p: Pos2, scroll: f32, rows: &[TreeRow]| -> Option<TreeRow> {
        let k = ((p.y - rows_r.top() + scroll) / TREE_ROW).floor();
        if !clip.contains(p) || k < 0.0 {
            return None;
        }
        rows.get(k as usize).copied()
    };
    let mut toggle = None;
    if resp.hovered() && ui.input(|i| i.pointer.any_pressed()) {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            if let Some(row) = row_under(p, m.scroll, &rows) {
                match tree_hit(row, p.x - rows_r.left()) {
                    TreeHit::Select => {
                        m.sel = match row {
                            TreeRow::Preset(f, k) => Some((m.tree[f].0.clone(), m.tree[f].1[k].clone())),
                            TreeRow::Folder(_) => None,
                        }
                    }
                    TreeHit::Toggle => toggle = Some(row),
                    TreeHit::Nothing => {}
                }
            }
        }
    }
    if resp.double_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(row @ TreeRow::Folder(_)) = row_under(p, m.scroll, &rows) {
                if tree_hit(row, p.x - rows_r.left()) == TreeHit::Select {
                    toggle = Some(row);
                }
            }
        }
    }
    if let Some(TreeRow::Folder(f)) = toggle {
        let name = m.tree[f].0.clone();
        if !m.closed.remove(&name) {
            // Closing a folder drops its presets, and with them the selection (0x140089ce0).
            if m.sel.as_ref().is_some_and(|(sf, _)| *sf == name) {
                m.sel = None;
            }
            m.closed.insert(name);
        }
        // Opening a folder reads its presets again (itemOpennessChanged, 0x140089ce0).
        m.relist(root);
        rows = tree_rows(&m.tree, &m.closed);
        let total = rows.len() as f32 * TREE_ROW;
        m.scroll = m.scroll.clamp(0.0, (total - view.height()).max(0.0));
        ui.ctx().request_repaint();
    }
    let p = ui.painter().with_clip_rect(clip);
    let (ls, text) = (c.get(colours::LIST_SELECTED), c.get(colours::TEXT));
    let (line, box_fill, mark) = (colours::draw(TREE_LINE), colours::draw(TREE_BOX_FILL), colours::draw(TREE_BOX_MARK));
    let first = (m.scroll / TREE_ROW) as usize;
    let last = (((m.scroll + rows_r.height()) / TREE_ROW).ceil() as usize).min(rows.len());
    for (i, row) in rows.iter().enumerate().take(last).skip(first) {
        let top = rows_r.top() + i as f32 * TREE_ROW - m.scroll;
        let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(rows_r.left() + x, top + y), vec2(w, h));
        match *row {
            TreeRow::Folder(f) => {
                let b = at(((TREE_INDENT - TREE_BOX) / 2.0).floor(), ((TREE_ROW - TREE_BOX) / 2.0).floor(), TREE_BOX, TREE_BOX);
                p.rect_filled(b, CornerRadius::ZERO, box_fill);
                p.rect_stroke(b, CornerRadius::ZERO, Stroke::new(1.0f32, mark), StrokeKind::Inside);
                let (mid, bar) = ((TREE_BOX / 2.0).floor(), (TREE_BOX / 2.0).floor() + 1.0);
                p.rect_filled(Rect::from_min_size(b.min + vec2((TREE_BOX - bar) / 2.0, mid), vec2(bar, 1.0)), CornerRadius::ZERO, mark);
                if m.closed.contains(&m.tree[f].0) {
                    p.rect_filled(Rect::from_min_size(b.min + vec2(mid, (TREE_BOX - bar) / 2.0), vec2(1.0, bar)), CornerRadius::ZERO, mark);
                }
                let item = at(TREE_INDENT, 0.0, rows_w - TREE_INDENT, TREE_ROW);
                text_at(&p.with_clip_rect(item), item.left(), baseline(item, TREE_FONT), Align::Min, &m.tree[f].0, bold(TREE_FONT), text);
            }
            TreeRow::Preset(f, k) => {
                // A line down from the folder (half way for its last preset) and one across.
                let x = 1.5 * TREE_INDENT;
                let down = if k + 1 == m.tree[f].1.len() { TREE_ROW / 2.0 } else { TREE_ROW };
                p.rect_filled(at(x - 1.0, 0.0, 2.0, down), CornerRadius::ZERO, line);
                p.rect_filled(at(x, TREE_ROW / 2.0 - 1.0, TREE_INDENT / 2.0, 2.0), CornerRadius::ZERO, line);
                let item = at(2.0 * TREE_INDENT, 0.0, rows_w - 2.0 * TREE_INDENT, TREE_ROW);
                let name = &m.tree[f].1[k];
                if m.sel.as_ref().is_some_and(|(sf, sn)| *sf == m.tree[f].0 && sn == name) {
                    let r = Rect::from_min_size(item.min, vec2(item.width() - PRESET_SEL_GAP, TREE_ROW));
                    gradient_rrect(&p, r, PRESET_SEL_RADIUS, colours::brighter(ls, SELECTED_TOP), colours::darker(ls, SELECTED_BOTTOM));
                }
                text_at(&p.with_clip_rect(item), item.left() + PRESET_TEXT_X, baseline(item, TREE_FONT), Align::Min, name, font(TREE_FONT), text);
            }
        }
    }
}

/// A prompt's text box ("presetName") as the original's look draws a TextEditor: white, 15 px black
/// text, "Button Highlight" behind selected text (0x1400211e0) and everything selected when it
/// gets the keyboard. With the keyboard it has a 2 px "Menu Selection" outline and a 4 px shadow at
/// 0.75, without it a 1 px outline and a 3 px shadow (drawTextEditorOutline 0x1401e7b60). As the
/// box's only text box it keeps the keyboard.
fn name_box(ui: &mut Ui, r: Rect, text: &mut String, fresh: &mut bool, c: &colours::Scheme) {
    let id = Id::new("presetName");
    let p = ui.painter().with_clip_rect(r);
    p.rect_filled(r, CornerRadius::ZERO, Color32::WHITE);
    if *fresh {
        text_edit::TextEditState::default().store(ui.ctx(), id);
    }
    let top = edit_top(ui, r, ALERT_FONT);
    let out = ui
        .scope_builder(UiBuilder::new().max_rect(r), |ui| {
            ui.visuals_mut().selection.bg_fill = c.get(colours::BUTTON_HIGHLIGHT);
            ui.visuals_mut().text_cursor.stroke.color = Color32::BLACK;
            TextEdit::singleline(text)
                .id(id)
                .font(font(ALERT_FONT))
                .text_color(Color32::BLACK)
                .frame(false)
                .margin(Margin { left: EDIT_TEXT_LEFT, right: EDIT_RIGHT, top, bottom: EDIT_BOTTOM })
                .desired_width(f32::INFINITY)
                .min_size(r.size())
                .show(ui)
        })
        .inner;
    if *fresh || (!out.response.has_focus() && !ui.input(|i| i.pointer.any_down())) {
        out.response.request_focus();
    }
    *fresh = false;
    if out.response.gained_focus() {
        let mut state = out.state;
        state.cursor.set_char_range(Some(text::CCursorRange::two(text::CCursor::new(0), text::CCursor::new(text.chars().count()))));
        state.store(ui.ctx(), id);
    }
    let shadow = colours::draw(EDIT_SHADOW);
    let tall = Rect::from_min_size(r.min, r.size() + vec2(0.0, 2.0));
    if ui.memory(|m| m.has_focus(id)) {
        p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(EDIT_OUTLINE, c.get(colours::MENU_SELECTION)), StrokeKind::Inside);
        bevel(&p, tall, EDIT_BEVEL, shadow.gamma_multiply(EDIT_SHADOW_FADE));
    } else {
        p.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0f32, colours::draw(LIST_OUTLINE)), StrokeKind::Inside);
        bevel(&p, tall, EDIT_BEVEL_IDLE, shadow);
    }
}

/// The plugins Import offers, each by the name of its preset folder (for a VST plugin its file
/// name, as the original's plugin list names it), and the built-in processors, sorted once each.
fn plugin_folders(plugins: &[PluginInfo]) -> Vec<String> {
    let mut v: Vec<String> = plugins.iter().map(presets::folder_name).chain(Kind::ALL.into_iter().map(|k| safe(k.name()))).collect();
    presets::sort_names(&mut v);
    v.dedup_by(|a, b| a.to_uppercase() == b.to_uppercase());
    v
}

/// Copy: the preset again under a new name in the same folder (0x14008b4ce).
fn copy_preset(dir: &Path, from: &str, to: &str) -> Result<String, String> {
    let to = presets::check_name(to)?;
    let dest = presets::preset_path(dir, &to);
    if dest.exists() {
        return Err(PRESET_EXISTS.into());
    }
    std::fs::copy(presets::preset_path(dir, from), &dest).map_err(|e| format!("Could not copy the preset \"{from}\": {e}"))?;
    Ok(to)
}

/// Rename: the preset's file takes the new name (0x14008c926); changing only its case works too.
fn rename_preset(dir: &Path, from: &str, to: &str) -> Result<String, String> {
    let to = presets::check_name(to)?;
    if to == from {
        return Ok(to);
    }
    let (src, dest) = (presets::preset_path(dir, from), presets::preset_path(dir, &to));
    if dest.exists() && !same_file(&src, &dest) {
        return Err(PRESET_EXISTS.into());
    }
    std::fs::rename(&src, &dest).map_err(|e| format!("Could not rename the preset \"{from}\": {e}"))?;
    Ok(to)
}

/// Import: a copy of an .fxp file in a plugin's folder under the file's own name.
fn import_preset(dir: &Path, file: &Path) -> Result<String, String> {
    let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = presets::check_name(&stem)?;
    let dest = presets::preset_path(dir, &name);
    if same_file(file, &dest) {
        return Ok(name);
    }
    if dest.exists() {
        return Err(PRESET_EXISTS.into());
    }
    std::fs::copy(file, &dest).map_err(|e| format!("Could not import {}: {e}", file.display()))?;
    Ok(name)
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

    /// The Patch Organiser as the original lays it out (resized, 0x14005d760; paint, 0x14005d660):
    /// the patch list at (8, 8) filling all but the right 106 px and the bottom 15, and 82 x 24
    /// buttons 90 px in from the right edge with the original's tooltips (constructor, 0x14005c440).
    /// It opens at 400 x 300 in the middle each time and can be resized (0x1400384e4, 0x140030940).
    /// Picking rows only selects them; the buttons act on the selected rows. Export... is the
    /// rebuild's own.
    pub fn organiser_window(&mut self, ctx: &Context) {
        let mut st = std::mem::take(&mut self.dlg.organiser_state);
        let n = self.patches.len();
        st.rows.retain(|&r| r < n);
        if st.edit.as_ref().is_some_and(|e| e.0 >= n) {
            st.edit = None;
        }
        let (mut open, mut op, mut renamed) = (true, None, None);
        let patches = &self.patches;
        let c = &self.scheme;
        // Centred like the original; the window's frame and title add about 12 x 40.
        let screen = ctx.screen_rect();
        let pos = (screen.center() - (ORGANISER_SIZE + vec2(12.0, 40.0)) / 2.0).max(screen.min);
        Window::new("Patch Organiser")
            .id(Id::new(("organiser", self.dlg.organiser_opens)))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(pos)
            .default_size(ORGANISER_SIZE)
            .min_size(ORGANISER_MIN)
            .show(ctx, |ui| {
                let area = ui.available_rect_before_wrap();
                ui.allocate_rect(area, Sense::hover());
                let (w, h) = (area.width(), area.height());
                let at = |x: f32, y: f32, rw: f32, rh: f32| Rect::from_min_size(area.min + vec2(x, y), vec2(rw, rh));
                renamed = organiser_list(ui, at(8.0, 8.0, w - 106.0, h - 15.0), patches, c, &mut st);
                let first = st.rows.first().copied();
                let any = first.is_some();
                // Export... sits under Import..., so the line and the move buttons are 32 px lower
                // than the original's (line at 140, Move Up at 152, Move Down at 184).
                let buttons = [
                    (8.0, "Add", "Add a new patch", Op::Add, true),
                    (40.0, "Copy", "Duplicate selected patch", Op::Copy, any),
                    (72.0, "Remove", "Delete selected patch", Op::Remove, any && n > 1),
                    (104.0, "Import...", "Import patch from another .pdl file", Op::Import, true),
                    (136.0, "Export...", "Export selected patch to a .pdl file", Op::Export, any),
                    (184.0, "Move Up", "Move patch up", Op::Up, first.is_some_and(|r| r > 0)),
                    (216.0, "Move Down", "Move patch down", Op::Down, first.is_some_and(|r| r + 1 < n)),
                ];
                let text = c.get(colours::TEXT);
                for (y, label, tip, o, on) in buttons {
                    let r = at(w - 90.0, y, 82.0, 24.0);
                    if fitted_button(ui, r, label, on, text).on_hover_text(tip).on_disabled_hover_text(tip).clicked() {
                        op = Some(o);
                    }
                }
                ui.painter().rect_filled(at(w - 89.0, 172.0, 80.0, 1.0), CornerRadius::ZERO, Color32::from_black_alpha(0x40));
            });
        // labelTextChanged (0x14005f380) renames the patch, which the main window's list shows.
        if let Some((k, name)) = renamed {
            if let Some(p) = self.patches.get_mut(k) {
                p.name = name;
                self.dirty = true;
            }
        }
        match op {
            // Add picks "<new patch>" in the main window's patch list (comboBoxChanged,
            // 0x140035df0): a new patch at the end, switched to.
            Some(Op::Add) => self.pending.push(Cmd::NewPatch),
            Some(Op::Copy) => {
                // 0x14003cf90 saves the playing patch first, so its latest changes are copied too.
                self.store_current();
                if copy_rows(&mut self.patches, &st.rows) {
                    self.dirty = true;
                }
            }
            Some(Op::Remove) => {
                let before = self.patches.len();
                if remove_rows(&mut self.patches, &mut self.current, &st.rows) {
                    self.load_current();
                }
                self.dirty |= self.patches.len() != before;
                // JUCE keeps the selected row numbers that are still in the list.
                st.rows.retain(|&r| r < self.patches.len());
            }
            Some(o @ (Op::Up | Op::Down)) => {
                if let Some(to) = move_row(&mut self.patches, &mut self.current, &mut st.rows, matches!(o, Op::Up)) {
                    st.reveal = Some(to);
                    self.dirty = true;
                }
            }
            Some(Op::Import) => self.import_patch(ctx),
            Some(Op::Export) => self.export_patches(&st.rows),
            None => {}
        }
        self.dlg.organiser_state = st;
        self.dlg.organiser = open;
    }

    /// Import...: asks for a patch file and opens "Select patch..." on the patches in it
    /// (buttonClicked, 0x14005d870). A file it can't read says so; the original did nothing.
    fn import_patch(&mut self, ctx: &Context) {
        let Some(path) = self.pdl_dialog().set_title(IMPORT_FILE_TITLE).pick_file() else { return };
        match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| patches_in(&text)) {
            Ok(patches) => {
                let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.dlg.import = Some(ImportPick { file_name, patches, sel: 0 });
                // The box takes the keyboard from here on.
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
                ctx.request_repaint();
            }
            Err(e) => self.message("File error", &format!("Could not import a patch from {}: {e}", path.display())),
        }
    }

    /// Export...: the selected patches on their own in a new patch file, which Open and Import...
    /// both read. The rebuild's own: the original organiser has no export.
    fn export_patches(&mut self, rows: &BTreeSet<usize>) {
        self.store_current();
        let chosen: Vec<&Patch> = rows.iter().filter_map(|&r| self.patches.get(r)).collect();
        let stem = match chosen.as_slice() {
            [] => return,
            [p] if p.name.trim().is_empty() || p.name == UNTITLED => "Untitled".to_string(),
            [p] => safe(&p.name),
            _ => "Patches".to_string(),
        };
        let doc = export_document(&chosen);
        let Some(mut path) = self.pdl_dialog().set_title(EXPORT_FILE_TITLE).set_file_name(format!("{stem}.pdl")).save_file() else { return };
        if path.extension().is_none() {
            path.set_extension("pdl");
        }
        // Writing one patch over the open file would lose its other patches.
        if self.file.as_deref().is_some_and(|f| same_file(f, &path)) {
            self.message("File error", &format!("{} is the open patch file. Export the patch to another file.", path.display()));
            return;
        }
        if let Err(e) = std::fs::write(&path, doc) {
            self.message("File error", &format!("Could not save {}: {e}", path.display()));
        }
    }

    /// "Select patch...", the box Import... opens: the patches of the file in a combo box, OK
    /// (Return) and Cancel (Escape). OK adds the chosen patch at the end of the list and marks the
    /// file changed, but stays on the current patch. Nothing else takes input while it's open.
    pub fn import_window(&mut self, ctx: &Context) {
        let Some(pick) = self.dlg.import.as_mut() else { return };
        let empty = pick.patches.is_empty();
        let (mut ok, mut cancel) = (false, false);
        // An open list takes Return and Escape itself, and a focused button or list takes Return.
        if !ctx.memory(|m| m.any_popup_open()) {
            let free = ctx.memory(|m| m.focused().is_none());
            ctx.input_mut(|i| {
                ok = free && i.consume_key(Modifiers::NONE, Key::Enter) && !empty;
                cancel = i.consume_key(Modifiers::NONE, Key::Escape);
            });
        }
        let message = format!("Patches in file: {}", pick.file_name);
        let lay = alert_layout(ctx, IMPORT_TITLE, &message, &["OK", "Cancel"], ctx.screen_rect().size());
        // The window colour is the original look's (0x1400211e0); the text keeps JUCE's default
        // black, which that look leaves alone (0x1404246b0).
        let (bg, text) = (self.scheme.get(colours::WINDOW_BG), Color32::BLACK);
        let frame = Frame::NONE.fill(bg).shadow(ctx.style().visuals.window_shadow);
        Modal::new(Id::new("import_patch")).backdrop_color(Color32::TRANSPARENT).frame(frame).show(ctx, |ui| {
            let (area, _) = ui.allocate_exact_size(lay.size, Sense::hover());
            let p = ui.painter().clone();
            p.rect_stroke(area, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_gray(0x66)), StrokeKind::Inside);
            for (top, s, f) in &lay.lines {
                let r = Rect::from_min_size(pos2(area.left(), area.top() + top), vec2(area.width(), f.size));
                text_at(&p, area.center().x, baseline(r, f.size), Align::Center, s, f.clone(), text);
            }
            let combo = lay.combo.translate(area.min.to_vec2());
            ui.scope_builder(UiBuilder::new().max_rect(combo), |ui| {
                ui.spacing_mut().interact_size.y = combo.height();
                let shown = pick.patches.get(pick.sel).map(|p| p.name.clone()).unwrap_or_default();
                ComboBox::from_id_salt("patchName").width(combo.width()).truncate().selected_text(shown).show_ui(ui, |ui| {
                    for (i, p) in pick.patches.iter().enumerate() {
                        ui.selectable_value(&mut pick.sel, i, p.name.as_str());
                    }
                });
            });
            for (k, (label, r)) in ["OK", "Cancel"].into_iter().zip(&lay.buttons).enumerate() {
                let r = r.translate(area.min.to_vec2());
                let on = k == 1 || !empty;
                let b = ui.scope_builder(UiBuilder::new().max_rect(r), |ui| ui.add_enabled(on, Button::new(RichText::new(label).font(font(ALERT_FONT))).min_size(r.size()))).inner;
                if b.clicked() {
                    ok |= k == 0;
                    cancel |= k == 1;
                }
            }
        });
        if !(ok || cancel) {
            return;
        }
        if let Some(mut pick) = self.dlg.import.take() {
            if ok && pick.sel < pick.patches.len() {
                self.patches.push(pick.patches.swap_remove(pick.sel));
                self.dirty = true;
            }
        }
        ctx.memory_mut(|m| m.close_popup());
        ctx.request_repaint();
    }

    /// User Preset Management from the menu or a mapping. The original builds a new window each
    /// time (0x1400385d9); one that's still open here comes to the front instead.
    pub fn open_presets(&mut self) {
        match &self.dlg.presets {
            Some(m) => self.egui_ctx.move_to_top(LayerId::new(Order::Middle, Id::new(("presets", m.opens)))),
            None => self.dlg.presets = Some(PresetManager::default()),
        }
    }

    /// User Preset Management as the original lays it out (constructor 0x14008a320, resized
    /// 0x14008b360, paint 0x14008b2e0): the tree at (8, 8) filling all but the right 106 px and
    /// the bottom 15, on its shade, and the buttons 90 px in from the right. The buttons act on
    /// the selected preset.
    pub fn presets_window(&mut self, ctx: &Context) {
        let Some(mut m) = self.dlg.presets.take() else { return };
        let root = presets::root();
        if !m.listed {
            m.relist(&root);
        }
        let (mut open, mut op) = (true, None);
        let c = &self.scheme;
        // Centred like the original; the window's frame and title add about 12 x 40.
        let screen = ctx.screen_rect();
        let pos = (screen.center() - (PRESETS_SIZE + vec2(12.0, 40.0)) / 2.0).max(screen.min);
        Window::new(PRESETS_TITLE)
            .id(Id::new(("presets", m.opens)))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_pos(pos)
            .default_size(PRESETS_SIZE)
            .min_size(PRESETS_MIN)
            .show(ctx, |ui| {
                let area = ui.available_rect_before_wrap();
                ui.allocate_rect(area, Sense::hover());
                // The window's own colour reaches out to its frame at the sides and bottom.
                let margin = ui.style().spacing.window_margin;
                let fill = Rect::from_min_max(
                    pos2(area.left() - f32::from(margin.left), area.top()),
                    pos2(area.right() + f32::from(margin.right), area.bottom() + f32::from(margin.bottom)),
                );
                let rad = ui.visuals().window_corner_radius;
                let corners = CornerRadius { nw: 0, ne: 0, sw: rad.sw.saturating_sub(1), se: rad.se.saturating_sub(1) };
                let mut bg = ui.painter().clone();
                bg.set_clip_rect(fill);
                bg.rect_filled(fill, corners, rgb(PRESETS_BG));
                let (w, h) = (area.width(), area.height());
                let at = |x: f32, y: f32, rw: f32, rh: f32| Rect::from_min_size(area.min + vec2(x, y), vec2(rw, rh));
                ui.painter().rect_filled(at(7.0, 7.0, w - 104.0, h - 13.0), CornerRadius::ZERO, colours::draw(TREE_SHADE));
                preset_tree(ui, at(8.0, 8.0, w - 106.0, h - 15.0), &root, c, &mut m);
                let text = c.get(colours::TEXT);
                for (y, label, tip, o) in PRESET_BUTTONS {
                    if fitted_button(ui, at(w - 90.0, y, 82.0, 24.0), label, true, text).on_hover_text(tip).clicked() {
                        op = Some(o);
                    }
                }
            });
        if let Some(o) = op {
            self.preset_op(ctx, &mut m, o);
        }
        if open {
            self.dlg.presets = Some(m);
        }
    }

    /// A manager button (buttonClicked, 0x14008b450). Each but Import needs a selected preset.
    fn preset_op(&mut self, ctx: &Context, m: &mut PresetManager, op: PresetOp) {
        let root = presets::root();
        let sel = m.sel.clone();
        match (op, sel) {
            (PresetOp::Rename | PresetOp::Copy, Some(target)) => {
                let mut pr = PresetPrompt::new(if matches!(op, PresetOp::Rename) { PromptKind::Rename } else { PromptKind::Copy });
                pr.target = Some(target);
                m.prompt = Some(pr);
            }
            // The original asks for the plugin first and then for the file.
            (PresetOp::Import, _) => {
                let mut pr = PresetPrompt::new(PromptKind::Import);
                pr.choices = plugin_folders(&self.settings.plugins);
                m.prompt = Some(pr);
            }
            // Remove deletes the file straight away, and the plugin's folder once nothing is left
            // in it (0x14008b8ec).
            (PresetOp::Remove, Some((f, n))) => {
                let dir = root.join(&f);
                match std::fs::remove_file(presets::preset_path(&dir, &n)) {
                    Ok(()) => {
                        if std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_none()) {
                            let _ = std::fs::remove_dir(&dir);
                        }
                        m.sel = None;
                    }
                    Err(_) => self.message(DELETE_ERROR_TITLE, DELETE_ERROR),
                }
                m.refresh();
                self.presets_changed();
            }
            // Export copies the preset's file to one the person names, always ending in .fxp
            // (withFileExtension, 0x14008b450); the chooser asks before writing over a file.
            (PresetOp::Export, Some((f, n))) => {
                let src = presets::preset_path(&root.join(&f), &n);
                let Some(mut dest) = presets::choose_fxp(EXPORT_PRESET_TITLE, self.hwnd, Some(&format!("{n}.fxp"))) else { return };
                let mut added = false;
                if !dest.extension().is_some_and(|e| e.eq_ignore_ascii_case("fxp")) {
                    let mut s = dest.into_os_string();
                    s.push(".fxp");
                    dest = PathBuf::from(s);
                    added = true;
                }
                // The chooser only asked about the name as it was typed, so a file the added .fxp
                // lands on is never written over.
                if !same_file(&src, &dest) {
                    if added && dest.exists() {
                        self.message(presets::ERROR_TITLE, &format!("{} already exists.", dest.display()));
                    } else if let Err(e) = std::fs::copy(&src, &dest) {
                        self.message(presets::ERROR_TITLE, &format!("Could not export the preset \"{n}\": {e}"));
                    }
                }
                m.refresh();
                self.presets_changed();
            }
            _ => {}
        }
        if m.prompt.is_some() {
            // The prompt takes the keyboard from here on.
            ctx.memory_mut(|mm| {
                if let Some(id) = mm.focused() {
                    mm.surrender_focus(id);
                }
            });
            ctx.request_repaint();
        }
    }

    /// The manager's prompts (buttonClicked, 0x14008b450): Copy Preset and Rename Preset ask for a
    /// name in a text box, Import Preset for the plugin in a list, each with Cancel and Ok. Return
    /// is Ok and Escape does nothing, as no button has it. Nothing else takes input meanwhile.
    pub fn preset_prompt(&mut self, ctx: &Context) {
        let Some(mut m) = self.dlg.presets.take() else { return };
        let Some(mut pr) = m.prompt.take() else {
            self.dlg.presets = Some(m);
            return;
        };
        let (mut ok, mut cancel) = (false, false);
        if pr.kind != PromptKind::Import {
            // The text box would let go of the keyboard on Return or Escape.
            ctx.input_mut(|i| {
                ok = i.consume_key(Modifiers::NONE, Key::Enter);
                i.consume_key(Modifiers::NONE, Key::Escape);
            });
        } else if !ctx.memory(|mm| mm.any_popup_open() || mm.focused().is_some()) {
            // An open list takes Return itself, and a focused button or list takes it too.
            ok = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
        }
        let (title, message) = pr.kind.words();
        let lay = alert_layout(ctx, title, message, &PROMPT_BUTTONS, ctx.screen_rect().size());
        let (bg, text) = (self.scheme.get(colours::WINDOW_BG), Color32::BLACK);
        let frame = Frame::NONE.fill(bg).shadow(ctx.style().visuals.window_shadow);
        let c = &self.scheme;
        Modal::new(Id::new("preset_prompt")).backdrop_color(Color32::TRANSPARENT).frame(frame).show(ctx, |ui| {
            let (area, _) = ui.allocate_exact_size(lay.size, Sense::hover());
            let p = ui.painter().clone();
            p.rect_stroke(area, CornerRadius::ZERO, Stroke::new(1.0f32, Color32::from_gray(0x66)), StrokeKind::Inside);
            for (top, s, f) in &lay.lines {
                let r = Rect::from_min_size(pos2(area.left(), area.top() + top), vec2(area.width(), f.size));
                text_at(&p, area.center().x, baseline(r, f.size), Align::Center, s, f.clone(), text);
            }
            let field = lay.combo.translate(area.min.to_vec2());
            if pr.kind == PromptKind::Import {
                ui.scope_builder(UiBuilder::new().max_rect(field), |ui| {
                    ui.spacing_mut().interact_size.y = field.height();
                    let shown = pr.choices.get(pr.pick).cloned().unwrap_or_default();
                    ComboBox::from_id_salt("plugins").width(field.width()).truncate().selected_text(shown).show_ui(ui, |ui| {
                        for (i, n) in pr.choices.iter().enumerate() {
                            ui.selectable_value(&mut pr.pick, i, n.as_str());
                        }
                    });
                });
            } else {
                name_box(ui, field, &mut pr.text, &mut pr.fresh, c);
            }
            for (k, (label, r)) in PROMPT_BUTTONS.into_iter().zip(&lay.buttons).enumerate() {
                let r = r.translate(area.min.to_vec2());
                let b = ui.scope_builder(UiBuilder::new().max_rect(r), |ui| ui.add(Button::new(RichText::new(label).font(font(ALERT_FONT))).min_size(r.size()))).inner;
                if b.clicked() {
                    cancel |= k == 0;
                    ok |= k == 1;
                }
            }
        });
        if ok {
            self.finish_prompt(&mut m, pr);
        } else if !cancel {
            m.prompt = Some(pr);
        }
        if ok || cancel {
            ctx.memory_mut(|mm| mm.close_popup());
            ctx.request_repaint();
        }
        self.dlg.presets = Some(m);
    }

    /// Ok in a prompt. Copy and Rename act in the preset's own folder; Import asks for the file
    /// and copies it into the picked plugin's folder, making the folder if needed.
    fn finish_prompt(&mut self, m: &mut PresetManager, pr: PresetPrompt) {
        let root = presets::root();
        let done = match (pr.kind, pr.target) {
            (PromptKind::Copy, Some((f, n))) => copy_preset(&root.join(&f), &n, &pr.text).map(|_| ()),
            (PromptKind::Rename, Some((f, n))) => rename_preset(&root.join(&f), &n, &pr.text).map(|to| m.sel = Some((f, to))),
            (PromptKind::Import, _) => {
                let Some(f) = pr.choices.get(pr.pick).cloned() else { return };
                let Some(file) = presets::choose_fxp(IMPORT_PRESET_TITLE, self.hwnd, None) else { return };
                let dir = root.join(&f);
                if std::fs::create_dir_all(&dir).is_err() {
                    self.message(IMPORT_ERROR_TITLE, IMPORT_ERROR);
                    return;
                }
                import_preset(&dir, &file).map(|n| {
                    m.closed.remove(&f);
                    m.sel = Some((f, n));
                })
            }
            _ => Ok(()),
        };
        if let Err(e) = done {
            self.message(presets::ERROR_TITLE, &e);
        }
        m.refresh();
        self.presets_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_finds_patches_under_any_root_in_any_case() {
        let text = r#"<Something>
            <Patch name="Clean" tempo="90.00"><FILTERGRAPH/></Patch>
            <DEVICESETUP/>
            <PATCH name="Fuzz"/>
            <patch/>
        </Something>"#;
        let found = patches_in(text).unwrap();
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Clean", "Fuzz", UNTITLED]);
        assert_eq!(found[0].tempo, 90.0);
        assert_eq!(found[1].tempo, 120.0);
        assert!(found.iter().all(|p| p.el.as_ref().is_some_and(|e| e.name == "Patch")));
        assert!(found[0].el.as_ref().unwrap().child("FILTERGRAPH").is_some());
    }

    #[test]
    fn import_rejects_text_that_isnt_xml() {
        assert!(patches_in("not a patch file").is_err());
        assert!(patches_in("<Pedalboard2PatchFile/>").unwrap().is_empty());
    }

    #[test]
    fn exported_patches_import_as_they_were() {
        let mut el = El::new("patch");
        el.children.push(El::new("FILTERGRAPH").with_child(El::new("FILTER").attr("uid", 7)));
        let p = Patch { name: "Lead & <Solo>".into(), tempo: 133.5, el: Some(el) };
        let back = patches_in(&export_document(&[&p, &Patch::untitled(90.0)])).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].name, p.name);
        assert_eq!(back[0].tempo, 133.5);
        let graph = back[0].el.as_ref().unwrap().child("FILTERGRAPH").unwrap();
        assert_eq!(graph.child("FILTER").and_then(|f| f.get("uid")), Some("7"));
        assert_eq!((back[1].name.as_str(), back[1].tempo), (UNTITLED, 90.0));
    }

    fn named(names: &[&str]) -> Vec<Patch> {
        names.iter().map(|n| Patch { name: n.to_string(), tempo: 120.0, el: None }).collect()
    }

    fn names(patches: &[Patch]) -> Vec<&str> {
        patches.iter().map(|p| p.name.as_str()).collect()
    }

    fn set(rows: &[usize]) -> BTreeSet<usize> {
        rows.iter().copied().collect()
    }

    #[test]
    fn ctrl_click_adds_a_row_but_never_takes_one_out() {
        let mut rows = set(&[2]);
        click_row(&mut rows, 0, true);
        click_row(&mut rows, 0, true);
        assert_eq!(rows, set(&[0, 2]));
        click_row(&mut rows, 1, false);
        assert_eq!(rows, set(&[1]));
    }

    #[test]
    fn copies_go_to_the_end_in_list_order() {
        let mut p = named(&["A", "B", "C"]);
        assert!(copy_rows(&mut p, &set(&[2, 0])));
        assert_eq!(names(&p), ["A", "B", "C", "A (copy)", "C (copy)"]);
        assert!(!copy_rows(&mut p, &set(&[])));
    }

    #[test]
    fn remove_keeps_the_playing_patch_or_plays_its_neighbour() {
        let mut p = named(&["A", "B", "C", "D"]);
        let mut cur = 2;
        assert!(!remove_rows(&mut p, &mut cur, &set(&[0, 3])));
        assert_eq!((names(&p), cur), (vec!["B", "C"], 1));
        let mut p = named(&["A", "B", "C"]);
        let mut cur = 1;
        assert!(remove_rows(&mut p, &mut cur, &set(&[1])));
        assert_eq!((names(&p), cur), (vec!["A", "C"], 0));
        let mut cur = 0;
        assert!(remove_rows(&mut p, &mut cur, &set(&[0])));
        assert_eq!((names(&p), cur), (vec!["C"], 0));
        // The last patch always stays.
        let mut p = named(&["A", "B", "C"]);
        let mut cur = 2;
        assert!(remove_rows(&mut p, &mut cur, &set(&[0, 1, 2])));
        assert_eq!((names(&p), cur), (vec!["A"], 0));
    }

    #[test]
    fn moving_the_lowest_selected_row_keeps_the_playing_patch() {
        let mut p = named(&["A", "B", "C"]);
        let (mut cur, mut rows) = (1, set(&[1, 2]));
        assert_eq!(move_row(&mut p, &mut cur, &mut rows, true), Some(0));
        assert_eq!((names(&p), cur, &rows), (vec!["B", "A", "C"], 0, &set(&[0])));
        let mut rows = set(&[2]);
        assert_eq!(move_row(&mut p, &mut cur, &mut rows, true), Some(1));
        assert_eq!((names(&p), cur), (vec!["B", "C", "A"], 0));
        let mut rows = set(&[1]);
        assert_eq!(move_row(&mut p, &mut cur, &mut rows, true), Some(0));
        assert_eq!((names(&p), cur), (vec!["C", "B", "A"], 1));
        // Nothing moves past either end or with nothing selected.
        assert_eq!(move_row(&mut p, &mut cur, &mut set(&[0]), true), None);
        assert_eq!(move_row(&mut p, &mut cur, &mut set(&[2]), false), None);
        assert_eq!(move_row(&mut p, &mut cur, &mut set(&[]), false), None);
        assert_eq!(names(&p), ["C", "B", "A"]);
    }

    #[test]
    fn tree_presses_pick_items_and_toggle_folders_from_their_box() {
        assert_eq!(tree_hit(TreeRow::Folder(0), 0.0), TreeHit::Toggle);
        assert_eq!(tree_hit(TreeRow::Folder(0), 23.5), TreeHit::Toggle);
        assert_eq!(tree_hit(TreeRow::Folder(0), 24.0), TreeHit::Select);
        assert_eq!(tree_hit(TreeRow::Preset(0, 0), 30.0), TreeHit::Nothing);
        assert_eq!(tree_hit(TreeRow::Preset(0, 0), 5.0), TreeHit::Nothing);
        assert_eq!(tree_hit(TreeRow::Preset(0, 0), 48.0), TreeHit::Select);
        let tree = vec![("A".to_string(), vec!["x".to_string()]), ("B".to_string(), vec!["y".to_string()])];
        let closed: BTreeSet<String> = ["A".to_string()].into();
        assert_eq!(tree_rows(&tree, &closed), [TreeRow::Folder(0), TreeRow::Folder(1), TreeRow::Preset(1, 0)]);
    }

    #[test]
    fn manager_copies_renames_and_imports_without_writing_over_presets() {
        let dir = std::env::temp_dir().join(format!("pedalhost-test-manager-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let other = dir.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(presets::preset_path(&dir, "Lead"), b"lead").unwrap();
        std::fs::write(presets::preset_path(&dir, "Pad"), b"pad").unwrap();
        assert_eq!(copy_preset(&dir, "Lead", " Lead 2 ").unwrap(), "Lead 2");
        assert_eq!(std::fs::read(presets::preset_path(&dir, "Lead 2")).unwrap(), b"lead");
        assert!(copy_preset(&dir, "Lead", "Pad").is_err());
        assert!(rename_preset(&dir, "Lead", "Pad").is_err());
        assert!(rename_preset(&dir, "Lead", "a/b").is_err());
        assert_eq!(rename_preset(&dir, "Lead", "LEAD").unwrap(), "LEAD");
        assert_eq!(presets::list(&dir), ["LEAD", "Lead 2", "Pad"]);
        std::fs::write(other.join("Pad.fxp"), b"other pad").unwrap();
        std::fs::write(other.join("Bass.fxp"), b"bass").unwrap();
        assert!(import_preset(&dir, &other.join("Pad.fxp")).is_err());
        assert_eq!(std::fs::read(presets::preset_path(&dir, "Pad")).unwrap(), b"pad");
        assert_eq!(import_preset(&dir, &other.join("Bass.fxp")).unwrap(), "Bass");
        assert_eq!(std::fs::read(presets::preset_path(&dir, "Bass")).unwrap(), b"bass");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
