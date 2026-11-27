//! User presets: the "Presets:" bar above each plugin's window (the original's PresetBar,
//! 0x1400865f0) and the .fxp files behind it, one folder per plugin under the original's preset
//! folder (0x1400887b0).

use crate::app::App;
use crate::bars::{baseline, Row};
use crate::colours::{self, Scheme};
use crate::graph::{Node, NodeKind};
use crate::pdl::{b64_decode, xml_from_binary, xml_to_binary};
use crate::plugins::editor::{self, BarLook};
use crate::plugins::PluginInfo;
use crate::widgets::{button, combo_face, font, rgb, text_fit};
use egui::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The bar's size: PresetBar's constructor (0x1400865f0) makes it 396×32, and the window around
/// a plugin (EditorWrapper, 0x140076240) is never narrower than the bar.
pub const BAR_W: i32 = 396;
pub const BAR_H: i32 = 32;
/// Rectangles in the bar as x, y, width, height (PresetBar::resized, 0x140087310).
pub const LABEL_TEXT: [i32; 4] = [5, 5, 54, 22];
pub const COMBO: [i32; 4] = [64, 4, 272, 24];
pub const COMBO_TEXT: [i32; 4] = [65, 5, 251, 22];
/// The line the combo's text sits on, and the native text box laid over it.
pub const TEXT_LINE: [i32; 4] = [70, 6, 241, 20];
pub const EDIT: [i32; 4] = [70, 8, 241, 16];
pub const IMPORT_BUTTON: [i32; 4] = [340, 4, 24, 24];
pub const SAVE_BUTTON: [i32; 4] = [368, 4, 24, 24];
pub const LABEL: &str = "Presets:";
pub const FONT: f32 = 15.0;
/// The bar's fill and the line along its bottom (PresetBar::paint, 0x140087250): a 1 px stroke
/// of 0x20000000 along y 32, whose upper half covers row 31 and comes out 0xDFDDD3 there.
pub const BAR_BG: u32 = 0xEEECE1;
pub const BAR_LINE: u32 = 0xDFDDD3;
pub const LINE_Y: i32 = 31;
pub const TIP_IMPORT: &str = "Import preset from .fxp file";
pub const TIP_SAVE: &str = "Save current preset";
pub const IMPORT_TITLE: &str = "Select an .fxp file to import...";
/// What the combo shows with an empty list (0x140284010, set in the bar's constructor).
pub const NO_CHOICES: &str = "(no choices)";
pub const ERROR_TITLE: &str = "Preset Error";
const FOREIGN: &str = "This preset was saved by a different plugin.";
const REFUSED: &str = "The plugin did not accept this preset.";
/// Where an icon's 12×12 box sits in its button.
pub const ICON_AT: (i32, i32) = (6, 6);

/// One filled, convex shape of a button icon; a hole shows the button's face.
pub struct IconPart {
    pub pts: &'static [(i32, i32)],
    pub hole: bool,
}

const fn solid(pts: &'static [(i32, i32)]) -> IconPart {
    IconPart { pts, hole: false }
}

/// The import button's folder.
pub const FOLDER: &[IconPart] = &[
    solid(&[(0, 1), (4, 1), (6, 3), (0, 3)]),
    solid(&[(0, 3), (11, 3), (11, 5), (0, 5)]),
    solid(&[(0, 5), (2, 5), (2, 11), (0, 11)]),
    solid(&[(3, 6), (12, 6), (10, 11), (0, 11)]),
];

/// The save button's disk.
pub const DISK: &[IconPart] = &[
    solid(&[(0, 0), (9, 0), (12, 3), (12, 12), (0, 12)]),
    IconPart { pts: &[(3, 1), (8, 1), (8, 5), (3, 5)], hole: true },
    solid(&[(6, 2), (7, 2), (7, 4), (6, 4)]),
    IconPart { pts: &[(2, 7), (10, 7), (10, 11), (2, 11)], hole: true },
];

/// What the person did in a bar, for the app to carry out.
#[derive(Clone, Debug, PartialEq)]
pub enum BarEvent {
    /// An item of the list, counted from 0: the plugin's programs, then the presets.
    Pick(usize),
    /// New text typed into the list's box.
    Typed(String),
    Import,
    Save,
}

/// The folder all user presets live in: the original's `<application data>\Pedalboard2\presets`
/// (0x1400887b0), shared with it.
pub fn root() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Pedalboard2").join("presets")
}

/// The folder a plugin's presets go in, named as the original's host names the plugin (a VST
/// plugin by its file name without the extension) and made safe for a folder.
pub fn folder_name(info: &PluginInfo) -> String {
    let stem = if info.format.eq_ignore_ascii_case("VST") { Path::new(&info.file).file_stem().map(|s| s.to_string_lossy().into_owned()) } else { None };
    crate::editors::safe(stem.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&info.name))
}

pub fn folder(info: &PluginInfo) -> PathBuf {
    root().join(folder_name(info))
}

pub fn preset_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.fxp"))
}

fn fxp_stem(n: &str) -> Option<&str> {
    let k = n.len().checked_sub(4).filter(|&k| k > 0 && n.is_char_boundary(k))?;
    n[k..].eq_ignore_ascii_case(".fxp").then(|| &n[..k])
}

/// Sorted as the original sorts its lists: ignoring case first, then by case.
pub fn sort_names(v: &mut [String]) {
    v.sort_by(|a, b| a.to_uppercase().cmp(&b.to_uppercase()).then_with(|| a.cmp(b)));
}

/// The presets in a folder: every .fxp file, named without the extension (0x140088480).
pub fn list(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| fxp_stem(&e.file_name().to_string_lossy()).map(str::to_string))
        .collect();
    sort_names(&mut v);
    v
}

/// The plugin folders in the preset folder.
pub fn folders(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    sort_names(&mut v);
    v
}

fn be(d: &[u8], at: usize) -> Option<i32> {
    d.get(at..at + 4).map(|b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The id an .fxp file gives its plugin: a VST plugin's unique id, the first eight digits of a
/// VST3 plugin's class id, and 0 for the built-in processors.
pub fn fx_id(info: &PluginInfo) -> i32 {
    let hex: String = match info.format.to_ascii_uppercase().as_str() {
        "VST" => info.uid.trim().to_string(),
        "VST3" => info.uid.trim().chars().take(8).collect(),
        _ => String::new(),
    };
    u32::from_str_radix(&hex, 16).map_or(0, |v| v as i32)
}

/// An .fxp file holding one program as a chunk ('FPCh'), as JUCE's VST host writes it.
pub fn fpch(name: &str, id: i32, version: i32, programs: i32, chunk: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(60 + chunk.len());
    v.extend_from_slice(b"CcnK");
    v.extend_from_slice(&((52 + chunk.len()) as i32).to_be_bytes());
    v.extend_from_slice(b"FPCh");
    v.extend_from_slice(&1i32.to_be_bytes());
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(&version.to_be_bytes());
    v.extend_from_slice(&programs.to_be_bytes());
    // 28 bytes for the name: at most 27 and a nul, cut where a character ends.
    let mut k = name.len().min(27);
    while !name.is_char_boundary(k) {
        k -= 1;
    }
    let mut n = [0u8; 28];
    n[..k].copy_from_slice(&name.as_bytes()[..k]);
    v.extend_from_slice(&n);
    v.extend_from_slice(&(chunk.len() as i32).to_be_bytes());
    v.extend_from_slice(chunk);
    v
}

/// The chunk inside an .fxp ('FPCh') or .fxb ('FBCh') file that holds one.
pub fn chunk_of(d: &[u8]) -> Option<&[u8]> {
    if d.get(0..4)? != b"CcnK" {
        return None;
    }
    let at = match d.get(8..12)? {
        b"FPCh" | b"hCPF" => 56,
        b"FBCh" | b"hCBF" => 156,
        _ => return None,
    };
    let n = usize::try_from(be(d, at)?).ok()?;
    d.get(at + 4..at + 4 + n)
}

/// True when an .fxp file names a plugin other than `own` (the original's check before loading
/// a preset, 0x140088010).
pub fn foreign(d: &[u8], own: i32) -> bool {
    own != 0 && d.starts_with(b"CcnK") && matches!(be(d, 16), Some(id) if id != 0 && id != own)
}

/// A preset name that can be a file name, trimmed.
pub fn check_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Type a name for the preset first.".into());
    }
    let base = n.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && matches!(base.as_bytes()[3], b'1'..=b'9'));
    if reserved || n.chars().any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)) {
        return Err(format!("\"{n}\" can't be used as a preset name."));
    }
    Ok(n.to_string())
}

/// Moves the presets this app kept in its own folder before (`<name>\<preset>.preset`, the box's
/// state in base64) into the original's folder as .fxp files. `known` turns an old folder name
/// into the new one. A preset whose new file already exists stays where it is. Returns how many
/// moved.
pub fn migrate_legacy(old: &Path, new: &Path, known: &HashMap<String, String>) -> usize {
    let mut moved = 0;
    let Ok(dirs) = std::fs::read_dir(old) else { return 0 };
    for d in dirs.flatten() {
        let dir = d.path();
        if !dir.is_dir() {
            continue;
        }
        let name = d.file_name().to_string_lossy().into_owned();
        let target = new.join(known.get(&name).unwrap_or(&name));
        for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = f.path();
            if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("preset")) {
                continue;
            }
            let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            let to = preset_path(&target, &stem);
            if to.exists() {
                continue;
            }
            let Some(data) = std::fs::read_to_string(&p).ok().and_then(|t| b64_decode(t.trim())) else { continue };
            if std::fs::create_dir_all(&target).is_ok() && std::fs::write(&to, data).is_ok() {
                let _ = std::fs::remove_file(&p);
                moved += 1;
            }
        }
        let _ = std::fs::remove_dir(&dir);
    }
    let _ = std::fs::remove_dir(old);
    moved
}

/// What a bar works on: a box's plugin or built-in processor.
pub trait Target {
    fn num_programs(&self) -> i32;
    fn program(&self) -> i32;
    fn set_program(&mut self, k: i32);
    fn program_name(&self, k: i32) -> String;
    fn rename_program(&mut self, k: i32, name: &str);
    /// The current program as an .fxp file's contents.
    fn program_state(&mut self) -> Vec<u8>;
    fn set_program_state(&mut self, d: &[u8]) -> Result<(), String>;
    fn fx_id(&self) -> i32;
}

impl Target for Node {
    fn num_programs(&self) -> i32 {
        match &self.kind {
            NodeKind::Plugin(p) => p.num_programs().max(0),
            _ => 0,
        }
    }

    fn program(&self) -> i32 {
        Node::program(self)
    }

    fn set_program(&mut self, k: i32) {
        if let NodeKind::Plugin(p) = &mut self.kind {
            p.set_program(k);
        }
    }

    fn program_name(&self, k: i32) -> String {
        match &self.kind {
            NodeKind::Plugin(p) => p.program_name(k),
            _ => String::new(),
        }
    }

    fn rename_program(&mut self, k: i32, name: &str) {
        if let NodeKind::Plugin(p) = &mut self.kind {
            p.rename_program(k, name);
        }
    }

    fn program_state(&mut self) -> Vec<u8> {
        let id = fx_id(&self.info);
        let vst3 = self.info.format.eq_ignore_ascii_case("VST3");
        match &mut self.kind {
            // A VST3 plugin's state goes in the chunk of an 'FPCh' file.
            NodeKind::Plugin(p) if vst3 => {
                let np = p.num_programs().max(0);
                let name = if np > 0 { p.program_name(p.program()) } else { String::new() };
                let s = p.get_state();
                fpch(&name, id, 1, np, &s)
            }
            NodeKind::Plugin(p) => p.program_state(),
            NodeKind::Builtin(b) => b.get_state().map(|e| xml_to_binary(&e)).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn set_program_state(&mut self, d: &[u8]) -> Result<(), String> {
        let id = fx_id(&self.info);
        if foreign(d, id) {
            return Err(FOREIGN.into());
        }
        let vst3 = self.info.format.eq_ignore_ascii_case("VST3");
        let ok = match &mut self.kind {
            NodeKind::Plugin(p) if vst3 => p.set_state(chunk_of(d).unwrap_or(d)),
            NodeKind::Plugin(p) => p.set_state(d),
            NodeKind::Builtin(_) if d.is_empty() => true,
            NodeKind::Builtin(b) => match xml_from_binary(d) {
                Some(e) => {
                    b.set_state(&e);
                    true
                }
                None => false,
            },
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(REFUSED.into())
        }
    }

    fn fx_id(&self) -> i32 {
        fx_id(&self.info)
    }
}

/// What a box's program had before a preset was loaded over it, by program number. The
/// original keeps this on the box (PluginComponent+0x1e0, filled by 0x140073ba0), so it lasts
/// while the box does, also with its window closed.
pub type Cache = HashMap<i32, Vec<u8>>;

/// A program's name as the list shows it. JUCE's combo box takes no empty item, so the original
/// lists an unnamed program as " " (fill 0x1400879d0, string 0x1404180a4).
fn listed(t: &dyn Target, k: i32) -> String {
    let n = t.program_name(k);
    if n.is_empty() {
        " ".into()
    } else {
        n
    }
}

/// The bar's list and text (PresetBar's fill, 0x1400879d0, and comboBoxChanged, 0x140087470).
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Bar {
    /// The plugin's programs, then the presets.
    pub items: Vec<String>,
    pub programs: usize,
    /// The ticked item, counted from 1; 0 for none.
    pub current: usize,
    pub text: String,
    /// The program number the bar last saw, to notice changes made elsewhere.
    seen: i32,
}

impl Bar {
    pub fn fill(&mut self, t: &dyn Target, dir: &Path) {
        let np = t.num_programs().max(0);
        self.programs = np as usize;
        self.items = (0..np).map(|k| listed(t, k)).collect();
        self.items.extend(list(dir));
        self.seen = t.program();
        if np > 0 {
            let k = t.program().clamp(0, np - 1) as usize;
            self.current = k + 1;
            self.text = self.items[k].clone();
        } else {
            self.current = 0;
            self.text.clear();
        }
    }

    pub fn ticked(&self) -> Option<usize> {
        self.current.checked_sub(1)
    }

    fn program_item(&self, k: i32) -> Option<usize> {
        usize::try_from(k).ok().filter(|&i| i < self.programs)
    }

    /// Picks an item. A program brings back what it had before a preset was loaded over it; a
    /// preset is loaded over the current program, which is kept first. Returns true when the
    /// plugin changed.
    pub fn pick(&mut self, t: &mut dyn Target, idx: usize, dir: &Path, cache: &mut Cache) -> Result<bool, String> {
        let Some(name) = self.items.get(idx).cloned() else { return Ok(false) };
        // The item already chosen, with its own text showing: nothing changes (JUCE's combo box).
        if self.current == idx + 1 && self.text == name {
            return Ok(false);
        }
        if idx < self.programs {
            let k = idx as i32;
            let kept = cache.remove(&k);
            t.set_program(k);
            self.seen = k;
            self.current = idx + 1;
            self.text = name;
            if let Some(s) = kept {
                t.set_program_state(&s)?;
            }
            return Ok(true);
        }
        let d = std::fs::read(preset_path(dir, &name)).map_err(|e| format!("Could not read the preset \"{name}\": {e}"))?;
        if foreign(&d, t.fx_id()) {
            return Err(FOREIGN.into());
        }
        let key = t.program();
        // The program is kept (0x140073ba0) only when the item picked before was a program or
        // nothing (comboBoxChanged 0x140087470), and only by the first preset over it, as
        // std::map::insert does (0x1400776e0).
        let after_program = self.current <= self.programs;
        let kept = self.programs > 0 && after_program && !cache.contains_key(&key);
        if kept {
            cache.insert(key, t.program_state());
        }
        if let Err(e) = t.set_program_state(&d) {
            if kept {
                cache.remove(&key);
            }
            return Err(e);
        }
        // A plugin may move to another program number with the preset; the preset stays ticked.
        self.seen = t.program();
        self.current = idx + 1;
        self.text = name;
        Ok(true)
    }

    /// New text in the box: it renames the current program (a blank name doesn't) and is the
    /// name the next save uses. Returns true when the program's name changed.
    pub fn typed(&mut self, t: &mut dyn Target, s: &str) -> bool {
        self.text = s.to_string();
        if s.trim().is_empty() {
            return false;
        }
        let k = t.program();
        let Some(i) = self.program_item(k) else { return false };
        if self.items[i] != s {
            t.rename_program(k, s);
        }
        let now = listed(t, k);
        let changed = now != self.items[i];
        self.items[i] = now;
        if self.items[i] == self.text {
            self.current = i + 1;
        }
        changed
    }

    /// Saves the current program under the name in the box, which then shows the ticked item's
    /// name again. Returns the name used.
    pub fn save(&mut self, t: &mut dyn Target, dir: &Path) -> Result<String, String> {
        let name = check_name(&self.text)?;
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not create the folder for this plugin's presets: {e}"))?;
        let d = t.program_state();
        std::fs::write(preset_path(dir, &name), d).map_err(|e| format!("Could not save the preset \"{name}\": {e}"))?;
        // Then the original fills the list again and ticks the same item (0x1400876e0), so the
        // box shows that item's name, read afresh, or nothing when none is ticked.
        for k in 0..self.programs {
            self.items[k] = listed(t, k as i32);
        }
        self.text = self.ticked().and_then(|i| self.items.get(i)).cloned().unwrap_or_default();
        Ok(name)
    }

    /// Loads an .fxp file over the current program without adding it to the presets. The box
    /// then shows the current program's name (buttonClicked 0x1400876e0, through
    /// ComboBox::setText 0x1401fec80), so with no program to show nothing stays ticked.
    pub fn import(&mut self, t: &mut dyn Target, file: &Path) -> Result<(), String> {
        let d = std::fs::read(file).map_err(|e| format!("Could not read {}: {e}", file.display()))?;
        t.set_program_state(&d)?;
        let k = t.program();
        self.seen = k;
        if let Some(i) = self.program_item(k) {
            self.items[i] = listed(t, k);
            self.current = i + 1;
            self.text = self.items[i].clone();
        } else {
            self.current = 0;
            self.text.clear();
        }
        Ok(())
    }

    /// Lists the presets again after files changed; a ticked preset that's gone is unticked.
    pub fn relist(&mut self, dir: &Path) {
        let was = self.ticked().filter(|&i| i >= self.programs).and_then(|i| self.items.get(i).cloned());
        self.items.truncate(self.programs);
        let presets = list(dir);
        if let Some(name) = was {
            self.current = presets.iter().position(|p| *p == name).map_or(0, |i| self.programs + i + 1);
        }
        self.items.extend(presets);
    }

    /// Follows a program chosen somewhere else. Returns true when the bar changed.
    pub fn follow(&mut self, t: &dyn Target) -> bool {
        let k = t.program();
        if k == self.seen {
            return false;
        }
        self.seen = k;
        let Some(i) = self.program_item(k) else { return false };
        self.items[i] = listed(t, k);
        self.current = i + 1;
        self.text = self.items[i].clone();
        true
    }
}

/// A box's bar with what the app keeps for it while the box exists.
#[derive(Default)]
pub struct BarSlot {
    pub bar: Bar,
    pub cache: Cache,
    /// Text being typed in the app-drawn bar.
    draft: Option<String>,
    /// The bar's list of presets, while it's open.
    pub dd: crate::subwin::DropDown,
}

/// The bar's colours for the native windows.
pub fn look(c: &Scheme) -> BarLook {
    BarLook {
        button: c.get(colours::BUTTON).to_array(),
        highlight: c.get(colours::BUTTON_HIGHLIGHT).to_array(),
        editor_bg: c.get(colours::TEXT_EDITOR).to_array(),
        text: c.get(colours::TEXT).to_array(),
    }
}

/// Draws a bar in `r` (32 px tall) for the windows the app draws itself and returns what the
/// person did.
pub fn bar_ui(ui: &mut Ui, r: Rect, id: Id, slot: &mut BarSlot, c: &Scheme) -> Vec<BarEvent> {
    let mut ev = Vec::new();
    let at = |a: [i32; 4]| Rect::from_min_size(r.min + vec2(a[0] as f32, a[1] as f32), vec2(a[2] as f32, a[3] as f32));
    let p = ui.painter_at(r);
    p.rect_filled(r, CornerRadius::ZERO, rgb(BAR_BG));
    p.hline(r.x_range(), r.top() + LINE_Y as f32 + 0.5, Stroke::new(1.0f32, rgb(BAR_LINE)));
    let lr = at(LABEL_TEXT);
    text_fit(&p, lr.left(), lr.width(), baseline(lr, FONT), Align::Min, LABEL, font(FONT), Color32::BLACK);
    let cr = at(COMBO);
    combo_face(&p, cr, c);
    // Under the text box, so only the arrow opens the list.
    let combo = ui.interact(cr, id.with("combo"), Sense::click());

    let tr = at(COMBO_TEXT);
    let edit_id = id.with("text");
    let focused = ui.memory(|m| m.has_focus(edit_id));
    let mut s = match (&slot.draft, focused) {
        (Some(d), true) => d.clone(),
        _ => slot.bar.text.clone(),
    };
    let f = font(FONT);
    let asc = ui.fonts(|fo| fo.layout_no_wrap("A".into(), f.clone(), Color32::BLACK).rows[0].glyphs[0].pos.y);
    let top = (baseline(at(TEXT_LINE), FONT) - tr.top() - asc).round().clamp(0.0, 20.0) as i8;
    let out = ui
        .scope_builder(UiBuilder::new().max_rect(tr), |ui| {
            let v = ui.visuals_mut();
            v.selection.bg_fill = c.get(colours::BUTTON_HIGHLIGHT);
            v.text_cursor.stroke.color = Color32::BLACK;
            TextEdit::singleline(&mut s)
                .id(edit_id)
                .font(f.clone())
                .text_color(c.get(colours::TEXT))
                .frame(false)
                .margin(Margin { left: 5, right: 2, top, bottom: 0 })
                .desired_width(tr.width())
                .min_size(tr.size())
                .show(ui)
        })
        .inner;
    if out.response.gained_focus() {
        // Clicking into the box selects its text, as the original's label editor does.
        let mut st = out.state.clone();
        st.cursor.set_char_range(Some(text::CCursorRange::two(text::CCursor::new(0), text::CCursor::new(s.chars().count()))));
        st.store(ui.ctx(), edit_id);
        slot.dd.close();
    }
    if out.response.lost_focus() {
        let esc = ui.input(|i| i.key_pressed(Key::Escape));
        let typed = if focused { s.clone() } else { slot.draft.clone().unwrap_or_else(|| s.clone()) };
        slot.draft = None;
        if !esc && typed != slot.bar.text {
            ev.push(BarEvent::Typed(typed));
        }
    } else if out.response.has_focus() {
        slot.draft = Some(s.clone());
    } else {
        slot.draft = None;
    }

    if combo.clicked() {
        slot.dd.toggle(ui, cr);
    }

    let l = look(c);
    for (k, (a, tip, icon)) in [(IMPORT_BUTTON, TIP_IMPORT, FOLDER), (SAVE_BUTTON, TIP_SAVE, DISK)].into_iter().enumerate() {
        let br = at(a);
        let resp = button(ui, br, id.with(("button", k)), 4.0, c, tip);
        let (hole, solid) = editor::icon_colours(&l, resp.hovered());
        for part in icon {
            let pts = part.pts.iter().map(|&(x, y)| br.min + vec2((ICON_AT.0 + x) as f32, (ICON_AT.1 + y) as f32)).collect();
            let col = if part.hole { hole } else { solid };
            p.add(Shape::convex_polygon(pts, Color32::from_rgb(col[0], col[1], col[2]), Stroke::NONE));
        }
        if resp.clicked() {
            ev.push(if k == 0 { BarEvent::Import } else { BarEvent::Save });
        }
    }
    ev
}

/// The bar's list of presets, open under its box as a window of its own that belongs to the
/// editor window `owner`. Returns the preset picked.
pub fn bar_list(ctx: &Context, vid: ViewportId, owner: ViewportId, slot: &mut BarSlot, c: &Scheme) -> Option<BarEvent> {
    slot.dd.open?;
    let rows: Vec<Row> = if slot.bar.items.is_empty() {
        vec![(NO_CHOICES.to_string(), String::new(), false, false)]
    } else {
        slot.bar.items.iter().enumerate().map(|(i, n)| (n.clone(), String::new(), true, slot.bar.current == i + 1)).collect()
    };
    let k = crate::subwin::drop_list(ctx, vid, owner, &rows, &mut slot.dd, c, COMBO[2] as f32)?;
    (!slot.bar.items.is_empty()).then_some(BarEvent::Pick(k))
}

/// A chooser for one .fxp file, in front of `parent` (0: the window in front).
pub fn choose_fxp(title: &str, parent: isize, save_as: Option<&str>) -> Option<PathBuf> {
    let d = rfd::FileDialog::new().set_title(title).add_filter("fxp", &["fxp"]);
    match save_as {
        Some(name) => crate::subwin::chooser(parent, d.set_file_name(name), |d| d.save_file()),
        None => crate::subwin::chooser(parent, d, |d| d.pick_file()),
    }
}

/// Shows a bar's list and text in the box's native plugin window, if it has one open.
fn show(n: &Node, bar: &Bar) {
    if let NodeKind::Plugin(p) = &n.kind {
        if let Some(w) = p.editor_window() {
            w.set_bar(&bar.items, bar.ticked(), &bar.text);
        }
    }
}

fn native_window(n: &Node) -> isize {
    match &n.kind {
        NodeKind::Plugin(p) => p.editor_window().map_or(0, |w| w.hwnd as isize),
        _ => 0,
    }
}

impl App {
    /// Sets the bars up: their colours, how they wake the app and, once, moving the presets
    /// this app kept in its own folder before into the original's.
    pub fn start_presets(&mut self) {
        editor::set_bar_look(look(&self.scheme));
        let ctx = self.egui_ctx.clone();
        editor::set_waker(Box::new(move || {
            ctx.request_repaint();
            crate::tray::wake();
        }));
        if self.shot.is_some() {
            return;
        }
        let known: HashMap<String, String> = self.settings.plugins.iter().map(|i| (crate::editors::safe(&i.name), folder_name(i))).collect();
        let moved = migrate_legacy(&crate::settings::dir().join("presets"), &root(), &known);
        if moved > 0 {
            self.log(format!("Moved {moved} user preset(s) to {}", root().display()));
        }
    }

    /// A box's window opened: its bar lists the programs and presets afresh, keeping what the
    /// box had before presets were loaded over its programs.
    pub fn open_bar(&mut self, id: u32) {
        let Some(n) = self.graph.node(id) else { return };
        let slot = self.preset_bars.entry(id).or_default();
        slot.bar.fill(n, &folder(&n.info));
        slot.draft = None;
        slot.dd.close();
        show(n, &slot.bar);
    }

    /// A program was chosen somewhere other than the bar.
    pub fn sync_bar(&mut self, id: u32) {
        let (Some(n), Some(slot)) = (self.graph.node(id), self.preset_bars.get_mut(&id)) else { return };
        slot.bar.follow(n);
        show(n, &slot.bar);
    }

    /// Carries out what the person did in the native windows' bars, and follows programs chosen
    /// in the plugins' own windows or by MIDI.
    pub fn bar_events(&mut self) {
        let mut todo = Vec::new();
        for (&id, slot) in self.preset_bars.iter_mut() {
            let Some(n) = self.graph.node(id) else { continue };
            if let NodeKind::Plugin(p) = &n.kind {
                if let Some(w) = p.editor_window() {
                    if slot.bar.follow(n) {
                        w.set_bar(&slot.bar.items, slot.bar.ticked(), &slot.bar.text);
                    }
                    todo.extend(w.take_bar_events().into_iter().map(|e| (id, e)));
                }
            }
        }
        for (id, e) in todo {
            self.bar_event(id, e);
        }
    }

    pub fn bar_event(&mut self, id: u32, e: BarEvent) {
        let Some((dir, parent)) = self.graph.node(id).map(|n| (folder(&n.info), native_window(n))) else { return };
        let file = if e == BarEvent::Import {
            match choose_fxp(IMPORT_TITLE, parent, None) {
                Some(f) => Some(f),
                None => return,
            }
        } else {
            None
        };
        let (Some(n), Some(slot)) = (self.graph.node_mut(id), self.preset_bars.get_mut(&id)) else { return };
        let saved = e == BarEvent::Save;
        let r = match e {
            BarEvent::Pick(k) => slot.bar.pick(n, k, &dir, &mut slot.cache),
            BarEvent::Typed(s) => Ok(slot.bar.typed(n, &s)),
            BarEvent::Import => match &file {
                Some(f) => slot.bar.import(n, f).map(|_| true),
                None => Ok(false),
            },
            BarEvent::Save => slot.bar.save(n, &dir).map(|_| false),
        };
        show(n, &slot.bar);
        match r {
            Ok(true) => self.dirty = true,
            Ok(false) => {}
            Err(msg) => self.message(ERROR_TITLE, &msg),
        }
        if saved {
            self.presets_changed();
        }
    }

    /// Preset files changed: every bar lists them again and the manager reads them again.
    pub fn presets_changed(&mut self) {
        for (id, slot) in self.preset_bars.iter_mut() {
            if let Some(n) = self.graph.node(*id) {
                slot.bar.relist(&folder(&n.info));
                show(n, &slot.bar);
            }
        }
        if let Some(m) = self.dlg.presets.as_mut() {
            m.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mock {
        names: Vec<String>,
        prog: i32,
        states: Vec<Vec<u8>>,
        id: i32,
    }

    impl Target for Mock {
        fn num_programs(&self) -> i32 {
            self.names.len() as i32
        }
        fn program(&self) -> i32 {
            self.prog
        }
        fn set_program(&mut self, k: i32) {
            self.prog = k;
        }
        fn program_name(&self, k: i32) -> String {
            self.names[k as usize].clone()
        }
        fn rename_program(&mut self, k: i32, name: &str) {
            self.names[k as usize] = name.into();
        }
        fn program_state(&mut self) -> Vec<u8> {
            fpch(&self.names[self.prog as usize], self.id, 1, 2, &self.states[self.prog as usize])
        }
        fn set_program_state(&mut self, d: &[u8]) -> Result<(), String> {
            if foreign(d, self.id) {
                return Err(FOREIGN.into());
            }
            self.states[self.prog as usize] = chunk_of(d).ok_or(REFUSED)?.to_vec();
            Ok(())
        }
        fn fx_id(&self) -> i32 {
            self.id
        }
    }

    fn mock() -> Mock {
        Mock { names: vec!["One".into(), "Two".into()], prog: 0, states: vec![b"1".to_vec(), b"2".to_vec()], id: 7 }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pedalhost-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn presets_are_the_fxp_files_sorted_as_the_original_lists_them() {
        let d = temp("list");
        for f in ["Bass.fxp", "alpha.FXP", "Clean.fxp", "notes.txt", ".fxp"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        std::fs::create_dir(d.join("Folder.fxp")).unwrap();
        assert_eq!(list(&d), ["alpha", "Bass", "Clean"]);
        assert_eq!(folders(&d), ["Folder.fxp"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_that_cant_be_files_are_refused() {
        assert_eq!(check_name("  Lead "), Ok("Lead".to_string()));
        assert_eq!(check_name("Console"), Ok("Console".to_string()));
        assert_eq!(check_name("COM10"), Ok("COM10".to_string()));
        for bad in ["", "   ", "a/b", "a:b", "CON", "com1.txt", "Nul ", "x?"] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_vst_plugins_presets_go_under_its_file_name() {
        let vst = PluginInfo { name: "Synth".into(), format: "VST".into(), file: r"C:\Plugins\My Synth.dll".into(), uid: "4142434A".into(), ..Default::default() };
        assert_eq!(folder_name(&vst), "My Synth");
        assert_eq!(fx_id(&vst), 0x4142434A);
        let vst3 = PluginInfo { name: "Amp: Red".into(), format: "VST3".into(), file: r"C:\x\Amp.vst3".into(), uid: "ABCDEF0123456789ABCDEF0123456789".into(), ..Default::default() };
        assert_eq!(folder_name(&vst3), "Amp_ Red");
        assert_eq!(fx_id(&vst3), 0xABCDEF01u32 as i32);
        assert_eq!(fx_id(&PluginInfo { name: "Level".into(), format: "Internal".into(), ..Default::default() }), 0);
    }

    #[test]
    fn an_fxp_holds_its_chunk_and_names_its_plugin() {
        let d = fpch("Lead", 0x41424344, 1, 2, b"xyz");
        assert_eq!(&d[8..12], b"FPCh");
        assert_eq!(be(&d, 4), Some(55));
        assert_eq!(chunk_of(&d), Some(&b"xyz"[..]));
        assert!(!foreign(&d, 0x41424344));
        assert!(foreign(&d, 5));
        assert!(!foreign(&d, 0));
        assert!(!foreign(b"not an fxp file", 5));
        // A long name is cut to 27 bytes where a character ends.
        let d = fpch(&"é".repeat(20), 1, 1, 0, b"");
        assert_eq!(std::str::from_utf8(&d[28..54]), Ok("ééééééééééééé"));
        assert!(d[54..56].iter().all(|&b| b == 0));
    }

    #[test]
    fn a_preset_loads_over_the_program_and_picking_the_program_gives_it_back() {
        let dir = temp("pick");
        let (mut m, mut bar, mut cache) = (mock(), Bar::default(), Cache::new());
        std::fs::write(preset_path(&dir, "Warm"), fpch("Warm", 7, 1, 2, b"W")).unwrap();
        bar.fill(&m, &dir);
        assert_eq!((bar.items.clone(), bar.current, bar.text.as_str()), (vec!["One".to_string(), "Two".into(), "Warm".into()], 1, "One"));
        // The ticked item again does nothing.
        assert_eq!(bar.pick(&mut m, 0, &dir, &mut cache), Ok(false));
        assert_eq!(bar.pick(&mut m, 2, &dir, &mut cache), Ok(true));
        assert_eq!((m.states[0].as_slice(), bar.current, bar.text.as_str()), (&b"W"[..], 3, "Warm"));
        // A second preset over the same program keeps what the program had before the first.
        bar.current = 0;
        assert_eq!(bar.pick(&mut m, 2, &dir, &mut cache), Ok(true));
        assert_eq!(cache.len(), 1);
        assert_eq!(bar.pick(&mut m, 1, &dir, &mut cache), Ok(true));
        assert_eq!((m.prog, m.states[1].as_slice()), (1, &b"2"[..]));
        assert_eq!(bar.pick(&mut m, 0, &dir, &mut cache), Ok(true));
        assert_eq!((m.prog, m.states[0].as_slice(), cache.len()), (0, &b"1"[..], 0));
        // Another plugin's preset is refused before anything is kept or changed.
        std::fs::write(preset_path(&dir, "Other"), fpch("Other", 9, 1, 2, b"O")).unwrap();
        bar.relist(&dir);
        assert_eq!(bar.items[2], "Other");
        assert_eq!(bar.pick(&mut m, 2, &dir, &mut cache), Err(FOREIGN.to_string()));
        assert_eq!((m.states[0].as_slice(), cache.len(), bar.current), (&b"1"[..], 0, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn typing_renames_the_program_and_names_the_saved_preset() {
        let dir = temp("save");
        let (mut m, mut bar) = (mock(), Bar::default());
        bar.fill(&m, &dir);
        assert!(bar.typed(&mut m, "Bright"));
        assert_eq!((m.names[0].as_str(), bar.items[0].as_str(), bar.current), ("Bright", "Bright", 1));
        assert!(!bar.typed(&mut m, "  "));
        assert_eq!(m.names[0], "Bright");
        assert!(bar.save(&mut m, &dir).is_err());
        bar.typed(&mut m, "Bright");
        assert_eq!(bar.save(&mut m, &dir), Ok("Bright".to_string()));
        let saved = std::fs::read(preset_path(&dir, "Bright")).unwrap();
        assert_eq!(chunk_of(&saved), Some(&b"1"[..]));
        bar.relist(&dir);
        assert_eq!(bar.items, ["Bright", "Two", "Bright"]);
        assert_eq!(bar.current, 1);
        // An .fxp from anywhere loads over the current program without becoming a preset.
        let other = temp("import");
        std::fs::write(other.join("x.fxp"), fpch("Two", 7, 1, 2, b"I")).unwrap();
        bar.import(&mut m, &other.join("x.fxp")).unwrap();
        assert_eq!(m.states[0], b"I");
        assert_eq!(list(&dir), ["Bright"]);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn importing_without_programs_unticks_the_preset() {
        let dir = temp("import0");
        let mut m = Mock { names: Vec::new(), prog: 0, states: vec![Vec::new()], id: 7 };
        let (mut bar, mut cache) = (Bar::default(), Cache::new());
        std::fs::write(preset_path(&dir, "Warm"), fpch("Warm", 7, 1, 2, b"W")).unwrap();
        bar.fill(&m, &dir);
        assert_eq!(bar.pick(&mut m, 0, &dir, &mut cache), Ok(true));
        assert_eq!((bar.current, bar.text.as_str()), (1, "Warm"));
        bar.import(&mut m, &preset_path(&dir, "Warm")).unwrap();
        assert_eq!((bar.current, bar.text.as_str()), (0, ""));
        // So the preset can be picked again.
        assert_eq!(bar.pick(&mut m, 0, &dir, &mut cache), Ok(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unnamed_program_is_listed_as_a_space() {
        let dir = temp("unnamed");
        let (mut m, mut bar) = (mock(), Bar::default());
        m.names[1].clear();
        bar.fill(&m, &dir);
        assert_eq!(bar.items, ["One", " "]);
        m.prog = 1;
        assert!(bar.follow(&m));
        assert_eq!((bar.current, bar.text.as_str()), (2, " "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_preset_picked_after_a_preset_keeps_no_program() {
        let dir = temp("keep");
        let (mut m, mut bar, mut cache) = (mock(), Bar::default(), Cache::new());
        for n in ["A", "B"] {
            std::fs::write(preset_path(&dir, n), fpch(n, 7, 1, 2, n.as_bytes())).unwrap();
        }
        bar.fill(&m, &dir);
        assert_eq!(bar.pick(&mut m, 2, &dir, &mut cache), Ok(true));
        assert_eq!(cache.keys().copied().collect::<Vec<_>>(), [0]);
        // The plugin is on its second program now, but a preset was picked last (0x140087470).
        m.prog = 1;
        assert_eq!(bar.pick(&mut m, 3, &dir, &mut cache), Ok(true));
        assert_eq!((cache.len(), m.states[1].as_slice()), (1, &b"B"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn after_saving_the_box_shows_the_ticked_item_again() {
        let dir = temp("resave");
        let (mut m, mut bar) = (mock(), Bar::default());
        bar.fill(&m, &dir);
        // A name the program didn't take.
        bar.text = "Lead".into();
        assert_eq!(bar.save(&mut m, &dir), Ok("Lead".to_string()));
        assert_eq!((bar.text.as_str(), m.names[0].as_str()), ("One", "One"));
        // Nothing ticked: the box empties.
        bar.current = 0;
        bar.text = "Pad".into();
        assert_eq!(bar.save(&mut m, &dir), Ok("Pad".to_string()));
        assert_eq!(bar.text, "");
        assert_eq!(list(&dir), ["Lead", "Pad"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presets_kept_in_the_old_folder_move_once_into_the_original_one() {
        let base = temp("migrate");
        let (old, new) = (base.join("old"), base.join("new"));
        std::fs::create_dir_all(old.join("Synth")).unwrap();
        std::fs::write(old.join("Synth").join("Lead.preset"), crate::pdl::b64_encode(b"LEAD")).unwrap();
        std::fs::write(old.join("Synth").join("Pad.preset"), crate::pdl::b64_encode(b"NEW")).unwrap();
        std::fs::create_dir_all(new.join("My Synth")).unwrap();
        std::fs::write(new.join("My Synth").join("Pad.fxp"), b"KEEP").unwrap();
        let known = HashMap::from([("Synth".to_string(), "My Synth".to_string())]);
        assert_eq!(migrate_legacy(&old, &new, &known), 1);
        assert_eq!(std::fs::read(new.join("My Synth").join("Lead.fxp")).unwrap(), b"LEAD");
        assert_eq!(std::fs::read(new.join("My Synth").join("Pad.fxp")).unwrap(), b"KEEP");
        assert!(!old.join("Synth").join("Lead.preset").exists());
        assert!(old.join("Synth").join("Pad.preset").exists());
        assert_eq!(migrate_legacy(&old, &new, &known), 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
