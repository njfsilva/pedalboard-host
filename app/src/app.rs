//! The main window: patches and files, the audio device, and carrying out what the pedalboard,
//! the menus, the bars and incoming MIDI/OSC ask for.

use crate::bars::MenuState;
use crate::colours::{self, Scheme};
use crate::dialogs::Dialogs;
use crate::engine::{Engine, Plan};
use crate::field::{Action, Field};
use crate::graph::{Ctx, Graph, NodeKind};
use crate::pdl::El;
use crate::settings::{self, AppCommand, AudioConfig, Settings};
use crate::tempo::TapTempo;
use crate::watchdog::{self, Stage};
use crate::{midi, osc};
use egui::*;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const TITLE: &str = "Pedalboard Host";
pub const UNTITLED: &str = "<untitled>";
const MENU_H: f32 = 24.0;
const BAR_H: f32 = 40.0;
const LOG_LINES: usize = 2000;
/// A driver's reset request restarts the device this long after the last request (0x1400a7920).
const RESET_DELAY: Duration = Duration::from_millis(500);
/// At most this many automatic restarts within RESTART_SPAN (rebuild guard against a driver
/// that asks again after every restart).
const MAX_RESTARTS: usize = 4;
const RESTART_SPAN: Duration = Duration::from_secs(15);

/// What to do about a driver's restart requests this frame.
#[derive(Debug, PartialEq)]
enum Restart {
    No,
    After(Duration),
    Now,
    /// Too many restarts lately; `first` the first time since the device was opened by hand.
    HeldBack { first: bool },
}

/// Like the original's resetRequest timer (0x1400a7920 / 0x1400a5bb0), a request restarts the
/// device half a second after the last one; unlike it, never more than MAX_RESTARTS times
/// within RESTART_SPAN, so a driver that asks again after every restart can't take the
/// window over.
#[derive(Default)]
struct Restarts {
    due: Option<Instant>,
    done: Vec<Instant>,
    told: bool,
}

impl Restarts {
    fn poll(&mut self, now: Instant, asked: bool) -> Restart {
        if asked {
            self.due = Some(now + RESET_DELAY);
        }
        let Some(due) = self.due else { return Restart::No };
        if now < due {
            return Restart::After(due - now);
        }
        self.due = None;
        self.done.retain(|t| now.duration_since(*t) < RESTART_SPAN);
        if self.done.len() >= MAX_RESTARTS {
            let first = !self.told;
            self.told = true;
            return Restart::HeldBack { first };
        }
        self.done.push(now);
        Restart::Now
    }
}

pub struct Patch {
    pub name: String,
    pub tempo: f64,
    pub el: Option<El>,
}

impl Patch {
    pub fn untitled(tempo: f64) -> Patch {
        Patch { name: UNTITLED.into(), tempo, el: None }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmd {
    New,
    Open,
    Save,
    SaveAs,
    SaveAsDefault,
    ResetDefault,
    Exit,
    DeleteConnections,
    OrganisePatches,
    UserPresets,
    MiscSettings,
    AudioSettings,
    PluginList,
    ColourSchemes,
    AppMappings,
    Documentation,
    EventLog,
    About,
    PrevPatch,
    NextPatch,
    SelectPatch(usize),
    NewPatch,
    PlayPause,
    Rtz,
    TapTempo,
}

impl From<AppCommand> for Cmd {
    fn from(c: AppCommand) -> Cmd {
        match c {
            AppCommand::NextPatch => Cmd::NextPatch,
            AppCommand::PreviousPatch => Cmd::PrevPatch,
            AppCommand::PlayPause => Cmd::PlayPause,
            AppCommand::ReturnToZero => Cmd::Rtz,
            AppCommand::TapTempo => Cmd::TapTempo,
        }
    }
}

/// What the MIDI and OSC threads pass to the window (they feed the audio thread directly).
pub enum Incoming {
    Midi(Vec<u8>),
    Osc { address: String, tags: String, args: Vec<osc::Arg> },
}

pub struct App {
    pub settings: Settings,
    pub scheme: Scheme,
    pub engine: Engine,
    pub graph: Graph,
    pub field: Field,
    pub patches: Vec<Patch>,
    pub current: usize,
    pub file: Option<PathBuf>,
    pub dirty: bool,
    pub tap: TapTempo,
    pub tempo_text: String,
    pub menu: MenuState,
    pub combo_open: bool,
    /// Where the patch list was last drawn; the program change notice points at it.
    pub patch_combo: Rect,
    /// A program number past the last patch, and when it arrived.
    pub pc_notice: Option<(u8, f64)>,
    pub pending: Vec<Cmd>,
    pub dlg: Dialogs,
    pub log: VecDeque<String>,
    pub tx: mpsc::Sender<Incoming>,
    rx: mpsc::Receiver<Incoming>,
    pub midi: midi::Inputs,
    pub osc: Option<osc::Server>,
    pub hwnd: isize,
    pub egui_ctx: Context,
    pub shot: Option<PathBuf>,
    frames: u64,
    started: bool,
    start_file: Option<PathBuf>,
    pub window: Vec2,
    title: String,
    pub allow_close: bool,
    style_for: Option<Scheme>,
    pub tray: Option<crate::tray::Tray>,
    pub tray_hide: u8,
    restarts: Restarts,
}

fn hwnd_of(cc: &eframe::CreationContext<'_>) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(w)) => w.hwnd.get(),
        _ => 0,
    }
}

pub fn default_file() -> PathBuf {
    settings::dir().join("default.pdl")
}

/// JUCE writes channel sets as binary strings, highest channel first ("11" = channels 1 and 2).
fn chan_bits(v: &[u32]) -> String {
    let n = v.iter().max().map_or(0, |m| m + 1);
    (0..n).rev().map(|i| if v.contains(&i) { '1' } else { '0' }).collect()
}

fn chans_from(s: &str) -> Vec<u32> {
    s.chars().rev().enumerate().filter(|(_, c)| *c == '1').map(|(i, _)| i as u32).collect()
}

fn device_el(a: &AudioConfig) -> El {
    El::new("DEVICESETUP")
        .attr("deviceType", &a.backend)
        .attr("audioOutputDeviceName", &a.device)
        .attr("audioInputDeviceName", &a.input_device)
        .attr("audioDeviceRate", a.sample_rate)
        .attr("audioDeviceBufferSize", a.buffer_size)
        .attr("audioDeviceInChans", chan_bits(&a.input_channels))
        .attr("audioDeviceOutChans", chan_bits(&a.output_channels))
}

fn device_from(e: &El) -> AudioConfig {
    let s = |k: &str, d: &str| e.get(k).unwrap_or(d).to_string();
    AudioConfig {
        backend: s("deviceType", "Windows Audio"),
        device: s("audioOutputDeviceName", ""),
        input_device: s("audioInputDeviceName", ""),
        sample_rate: e.get_f("audioDeviceRate", 0.0),
        buffer_size: e.get_i("audioDeviceBufferSize", 0).max(0) as u32,
        input_channels: chans_from(&s("audioDeviceInChans", "11")),
        output_channels: chans_from(&s("audioDeviceOutChans", "11")),
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shot: Option<PathBuf>, start_file: Option<PathBuf>) -> App {
        // The check picture always shows a fresh, plugin-free setup.
        let settings = if shot.is_some() { Settings::default() } else { Settings::load() };
        let scheme = crate::dialogs::load_scheme(&settings.colour_scheme).unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            settings,
            scheme,
            engine: Engine::new(),
            graph: Graph::default(),
            field: Field::default(),
            patches: vec![Patch::untitled(120.0)],
            current: 0,
            file: None,
            dirty: false,
            tap: TapTempo::default(),
            tempo_text: "120.00".into(),
            menu: MenuState::default(),
            combo_open: false,
            patch_combo: Rect::NOTHING,
            pc_notice: None,
            pending: Vec::new(),
            dlg: Dialogs::default(),
            log: VecDeque::new(),
            tx,
            rx,
            midi: midi::Inputs::default(),
            osc: None,
            hwnd: hwnd_of(cc),
            egui_ctx: cc.egui_ctx.clone(),
            shot,
            frames: 0,
            started: false,
            start_file,
            window: vec2(1010.0, 573.0),
            title: String::new(),
            allow_close: false,
            style_for: None,
            tray: None,
            tray_hide: 0,
            restarts: Restarts::default(),
        };
        if app.shot.is_none() {
            if let Err(e) = app.open_device() {
                app.message("Audio Device Error", &e);
            }
            app.open_midi();
            app.open_osc();
            watchdog::watch(app.hwnd);
        }
        app
    }

    pub fn message(&mut self, title: &str, text: &str) {
        self.dlg.messages.push((title.to_string(), text.to_string()));
    }

    pub fn log(&mut self, s: String) {
        if self.log.len() >= LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(s);
    }

    pub fn ctx(&self) -> Ctx {
        Ctx {
            sample_rate: self.engine.sample_rate(),
            block: self.engine.block(),
            dev_ins: self.engine.active_ins(),
            dev_outs: self.engine.active_outs(),
            window: self.window,
            audio_in: self.settings.audio_input,
            midi_in: self.settings.midi_input,
            osc_in: self.settings.osc_input,
        }
    }

    /// Hands the audio thread a fresh, pre-built processing plan for the current patch.
    pub fn rebuild(&mut self) {
        let plan = Plan::build(&self.graph.desc(), self.engine.block(), self.engine.active_ins(), self.engine.active_outs());
        self.engine.set_plan(plan);
    }

    /// (Re)opens the configured audio device and starts it.
    pub fn open_device(&mut self) -> Result<(), String> {
        // Opened by hand (or by a patch file): automatic restarts may start over.
        self.restarts = Restarts::default();
        self.start_device()
    }

    fn start_device(&mut self) -> Result<(), String> {
        let _stage = watchdog::enter(Stage::AudioDevice);
        let r = self.engine.open(&self.settings.audio, self.hwnd);
        self.graph.set_device_channels(self.engine.active_ins(), self.engine.active_outs());
        // Nothing is running now, so every processor can be prepared for the new rate and size.
        unsafe { self.graph.prepare_all(self.engine.sample_rate(), self.engine.block()) };
        self.rebuild();
        r?;
        self.engine.start()
    }

    /// Restarts the device when its driver asks for it (see Restarts).
    fn check_device(&mut self, ctx: &Context) {
        let asked = self.engine.running && self.engine.device_lost();
        match self.restarts.poll(Instant::now(), asked) {
            Restart::No => {}
            Restart::After(wait) => ctx.request_repaint_after(wait),
            Restart::Now => {
                self.log("The audio driver asked for a restart; restarting the audio device.".into());
                if let Err(e) = self.start_device() {
                    self.message("Audio Device Error", &e);
                }
            }
            Restart::HeldBack { first } => {
                if first {
                    self.log("The audio driver keeps asking to be restarted; restarts are spaced out.".into());
                    self.message(
                        "Audio Device Error",
                        "The audio driver keeps asking to be restarted, so restarts are now spaced out to keep the program responsive. If the sound stops, press Apply in the Audio Settings.",
                    );
                }
            }
        }
    }

    pub fn open_midi(&mut self) {
        self.midi = midi::Inputs::default();
        let (inputs, errors) = midi::Inputs::open(&self.settings.midi_inputs, self.engine.midi_tx.clone(), self.tx.clone(), self.egui_ctx.clone());
        self.midi = inputs;
        for e in errors {
            self.log(e);
        }
    }

    /// (Re)starts OSC listening with the current port and multicast address. Like the original
    /// (0x14003b190), it only listens while the OSC Input box is shown.
    pub fn open_osc(&mut self) {
        self.osc = None;
        if !self.settings.osc_input {
            return;
        }
        let port = self.settings.osc_port;
        let group = self.settings.osc_multicast.trim().to_string();
        let multicast = match group.parse() {
            Ok(a) => Some(a),
            Err(_) if group.is_empty() => None,
            Err(_) => {
                self.log(format!("OSC: \"{group}\" is not an IPv4 address; listening without multicast"));
                None
            }
        };
        match osc::Server::start(port, multicast, self.engine.osc_tx.clone(), self.tx.clone(), self.egui_ctx.clone()) {
            Ok(s) => {
                if let Some(w) = &s.warning {
                    self.log(format!("OSC: {w}"));
                }
                self.osc = Some(s);
            }
            Err(e) => self.log(format!("OSC: could not listen on port {port}: {e}")),
        }
    }

    // ---- patches ----

    pub fn store_current(&mut self) {
        let Some(p) = self.patches.get(self.current) else { return };
        let (name, tempo) = (p.name.clone(), p.tempo);
        let el = self.graph.to_patch_el(&name, tempo);
        self.patches[self.current].el = Some(el);
    }

    pub fn load_current(&mut self) {
        if self.patches.is_empty() {
            self.patches.push(Patch::untitled(120.0));
        }
        self.current = self.current.min(self.patches.len() - 1);
        let ctx = self.ctx();
        let p = &self.patches[self.current];
        let tempo = p.tempo;
        let stored = p.el.is_some();
        let (g, errs) = match &p.el {
            Some(el) => Graph::from_patch_el(el, &ctx),
            None => (Graph::new_default(&ctx), Vec::new()),
        };
        let mut old = std::mem::replace(&mut self.graph, g);
        for n in &mut old.nodes {
            if let Some(pl) = n.plugin() {
                pl.close_editor();
            }
        }
        self.field.selected.clear();
        self.field.stop_rename();
        self.field.close_popups();
        // switchPatch (0x140039720) clears the field's hint after loading a stored patch.
        if stored {
            self.field.hide_hint();
        }
        self.dlg.mappings = None;
        self.dlg.editors.clear();
        self.dlg.raise = None;
        self.rebuild();
        // The old processors stay alive inside the old plan until the audio thread lets go of it.
        drop(old);
        self.apply_tempo(tempo);
        if !errs.is_empty() {
            self.message("Patch loading", &errs.join("\n"));
        }
    }

    pub fn select_patch(&mut self, i: usize) {
        if i >= self.patches.len() || i == self.current {
            return;
        }
        self.store_current();
        self.current = i;
        self.load_current();
    }

    pub fn apply_tempo(&mut self, bpm: f64) {
        self.engine.shared.set_tempo(bpm);
        self.tempo_text = format!("{bpm:.2}");
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        let bpm = bpm.clamp(1.0, 999.0);
        if let Some(p) = self.patches.get_mut(self.current) {
            if p.tempo != bpm {
                p.tempo = bpm;
                self.dirty = true;
            }
        }
        self.apply_tempo(bpm);
    }

    // ---- files ----

    pub fn document(&mut self) -> El {
        self.store_current();
        let mut root = El::new("Pedalboard2PatchFile");
        for p in &self.patches {
            let tempo = format!("{:.2}", p.tempo);
            root.children.push(match &p.el {
                Some(e) => {
                    let mut e = e.clone();
                    e.set("name", &p.name);
                    e.set("tempo", &tempo);
                    e
                }
                None => El::new("Patch").attr("name", &p.name).attr("tempo", &tempo),
            });
        }
        if self.settings.pdl_audio_settings {
            root.children.push(device_el(&self.settings.audio));
        }
        root
    }

    pub fn load_document(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("Could not locate file: {} ({e})", path.display()))?;
        let root = El::parse(&text)?;
        let mut patches: Vec<Patch> = root
            .named("Patch")
            .map(|p| Patch {
                name: p.get("name").or_else(|| p.get("Name")).unwrap_or(UNTITLED).to_string(),
                tempo: p.get_f("tempo", 120.0),
                el: Some(p.clone()),
            })
            .collect();
        if patches.is_empty() {
            if root.name != "Patch" && root.child("FILTERGRAPH").is_none() {
                return Err(format!("{} doesn't contain any patches.", path.display()));
            }
            patches.push(Patch { name: UNTITLED.into(), tempo: root.get_f("tempo", 120.0), el: Some(root.clone()) });
        }
        if self.settings.pdl_audio_settings {
            if let Some(cfg) = root.child("DEVICESETUP").map(device_from) {
                if cfg != self.settings.audio {
                    let old = std::mem::replace(&mut self.settings.audio, cfg);
                    if self.open_device().is_err() {
                        self.message("Audio Device Error", "Could not initialise audio settings loaded from .pdl file");
                        self.settings.audio = old;
                        let _ = self.open_device();
                    }
                }
            }
        }
        self.patches = patches;
        self.current = 0;
        self.load_current();
        Ok(())
    }

    pub fn new_file(&mut self) {
        self.file = None;
        let def = default_file();
        let loaded = self.shot.is_none() && def.exists() && self.load_document(&def).is_ok();
        if !loaded {
            self.patches = vec![Patch::untitled(120.0)];
            self.current = 0;
            self.load_current();
        }
        self.dirty = false;
    }

    pub fn open_path(&mut self, p: PathBuf) {
        match self.load_document(&p) {
            Ok(()) => {
                self.settings.last_file = p.display().to_string();
                self.settings.save();
                self.file = Some(p);
                self.dirty = false;
            }
            Err(e) => self.message("File error", &e),
        }
    }

    fn pdl_dialog(&self) -> rfd::FileDialog {
        let mut d = rfd::FileDialog::new().add_filter("Patch files", &["pdl"]);
        if let Some(dir) = Path::new(&self.settings.last_file).parent().filter(|d| d.is_dir()) {
            d = d.set_directory(dir);
        }
        d
    }

    pub fn save(&mut self, as_new: bool) -> bool {
        let path = match (&self.file, as_new) {
            (Some(f), false) => Some(f.clone()),
            _ => self.pdl_dialog().set_file_name("Untitled.pdl").save_file(),
        };
        let Some(mut path) = path else { return false };
        if path.extension().is_none() {
            path.set_extension("pdl");
        }
        let doc = self.document().to_document();
        match std::fs::write(&path, doc) {
            Ok(()) => {
                self.settings.last_file = path.display().to_string();
                self.settings.save();
                self.file = Some(path);
                self.dirty = false;
                true
            }
            Err(e) => {
                self.message("File error", &format!("Could not save {}: {e}", path.display()));
                false
            }
        }
    }

    // ---- commands ----

    /// Runs a command, first asking about unsaved changes where one would be lost.
    pub fn run(&mut self, cmd: Cmd) {
        if matches!(cmd, Cmd::New | Cmd::Open | Cmd::Exit) && self.dirty && self.shot.is_none() {
            self.dlg.confirm = Some(cmd);
            return;
        }
        self.run_now(cmd);
    }

    pub fn run_now(&mut self, cmd: Cmd) {
        let sh = self.engine.shared.clone();
        match cmd {
            Cmd::New => self.new_file(),
            Cmd::Open => {
                if let Some(p) = self.pdl_dialog().pick_file() {
                    self.open_path(p);
                }
            }
            Cmd::Save => {
                self.save(false);
            }
            Cmd::SaveAs => {
                self.save(true);
            }
            Cmd::SaveAsDefault => {
                let _ = std::fs::create_dir_all(settings::dir());
                let doc = self.document().to_document();
                if let Err(e) = std::fs::write(default_file(), doc) {
                    self.message("File error", &format!("Could not save the default patch file: {e}"));
                }
            }
            Cmd::ResetDefault => {
                let _ = std::fs::remove_file(default_file());
            }
            Cmd::Exit => {
                self.allow_close = true;
                self.egui_ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            Cmd::DeleteConnections => self.delete_selected(),
            Cmd::OrganisePatches => self.dlg.organiser = true,
            Cmd::UserPresets => self.dlg.presets = Some(Default::default()),
            Cmd::MiscSettings => self.dlg.misc = true,
            Cmd::AudioSettings => self.dlg.open_audio(&self.settings, &self.engine),
            Cmd::PluginList => self.dlg.plugins = Some(Default::default()),
            Cmd::ColourSchemes => self.dlg.open_colours(&self.scheme),
            Cmd::AppMappings => self.dlg.app_maps = true,
            Cmd::Documentation => self.dlg.docs = true,
            Cmd::EventLog => self.dlg.log = true,
            Cmd::About => self.dlg.about = true,
            Cmd::PrevPatch | Cmd::NextPatch => {
                let n = self.patches.len();
                let i = match (cmd == Cmd::NextPatch, self.current) {
                    (true, c) if c + 1 < n => Some(c + 1),
                    (true, _) if self.settings.loop_patches => Some(0),
                    (false, c) if c > 0 => Some(c - 1),
                    (false, _) if self.settings.loop_patches => Some(n - 1),
                    _ => None,
                };
                if let Some(i) = i {
                    self.select_patch(i);
                }
            }
            Cmd::SelectPatch(i) => self.select_patch(i),
            Cmd::NewPatch => {
                self.store_current();
                self.patches.push(Patch::untitled(sh.tempo()));
                self.current = self.patches.len() - 1;
                self.load_current();
                self.dirty = true;
            }
            Cmd::PlayPause => {
                sh.playing.fetch_xor(true, Ordering::Relaxed);
            }
            Cmd::Rtz => sh.rtz.store(true, Ordering::Relaxed),
            Cmd::TapTempo => {
                if let Some(bpm) = self.tap.tap() {
                    self.set_tempo(bpm);
                }
            }
        }
    }

    pub fn delete_selected(&mut self) {
        let sel = std::mem::take(&mut self.field.selected);
        if sel.is_empty() {
            return;
        }
        self.graph.conns.retain(|c| !sel.contains(c));
        self.rebuild();
        self.dirty = true;
    }

    pub fn open_editor(&mut self, id: u32) {
        let (owner, top, fixed) = (self.hwnd, self.settings.windows_on_top, self.settings.fixed_size_windows);
        let Some(n) = self.graph.node_mut(id) else { return };
        // A plugin's own editor opens in a window of its own; built-ins and plugins without one
        // get theirs in a window inside this one (the generic editor for plugins).
        let r = match &mut n.kind {
            NodeKind::Plugin(p) if p.has_editor() => {
                let _stage = watchdog::enter(Stage::PluginEditor);
                p.open_editor(owner, top, fixed)
            }
            NodeKind::Builtin(_) | NodeKind::Plugin(_) => {
                if !self.dlg.editors.contains(&id) {
                    self.dlg.editors.push(id);
                }
                self.dlg.raise = Some(id);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = r {
            self.message("Plugin editor", &e);
        }
    }

    fn apply(&mut self, acts: Vec<Action>) {
        let mut changed = false;
        for a in acts {
            match a {
                Action::AddBuiltin(k, pos) => {
                    let ctx = self.ctx();
                    self.graph.add_builtin(k, pos, &ctx);
                    self.field.hide_hint();
                    changed = true;
                }
                Action::AddPlugin(i, pos) => {
                    let Some(info) = self.settings.plugins.get(i).cloned() else { continue };
                    let ctx = self.ctx();
                    match self.graph.add_plugin(&info, pos, &ctx) {
                        Ok(_) => {
                            self.field.hide_hint();
                            changed = true;
                        }
                        // addFilter (0x14002c6b0) titles its failure message this way.
                        Err(e) => self.message("Couldn't create filter", &format!("Could not load {}:\n{e}", info.name)),
                    }
                }
                Action::Connected(_, dst, param) => {
                    changed = true;
                    if param && self.settings.auto_mappings_window {
                        self.dlg.mappings = Some(dst);
                    }
                }
                Action::Moved | Action::Renamed => self.dirty = true,
                Action::OpenEditor(id) => self.open_editor(id),
                Action::OpenMappings(id) => self.dlg.mappings = Some(id),
                Action::Bypass(id) => {
                    if let Some(n) = self.graph.node(id) {
                        n.set_bypass(!n.bypassed());
                        self.dirty = true;
                    }
                }
                Action::Delete(id) => {
                    if let Some(mut n) = self.graph.remove(id) {
                        if let Some(p) = n.plugin() {
                            p.close_editor();
                        }
                        self.rebuild();
                        self.dirty = true;
                        self.dlg.editors.retain(|e| *e != id);
                        if self.dlg.mappings == Some(id) {
                            self.dlg.mappings = None;
                        }
                    }
                }
                Action::DeleteConns => self.delete_selected(),
                Action::SetProgram(id, k) => {
                    if let Some(p) = self.graph.node_mut(id).and_then(|n| n.plugin()) {
                        p.set_program(k);
                        self.dirty = true;
                    }
                }
                Action::UserPresets(id) => self.dlg.presets = Some(crate::editors::PresetState::for_node(id)),
            }
        }
        if changed {
            self.rebuild();
            self.dirty = true;
        }
    }

    // ---- MIDI and OSC arriving at the window ----

    fn on_midi(&mut self, b: &[u8]) {
        let Some(&s) = b.first() else { return };
        let ch = (s & 0x0F) + 1;
        match (s & 0xF0, b.len()) {
            (0xB0, 3..) => {
                self.log(format!("MIDI CC message received: CC={} val={} chan={ch}", b[1], b[2]));
                self.learn_cc(b[1]);
                if b[2] > 0 {
                    let hits: Vec<Cmd> = self.settings.app_mappings.iter().filter(|m| !m.osc && m.cc == b[1] && (m.channel == 0 || m.channel == ch)).map(|m| m.command.into()).collect();
                    self.pending.extend(hits);
                }
            }
            (0x90, 3..) if b[2] > 0 => self.log(format!("MIDI Note On message received: note={} vel={} chan={ch}", b[1], b[2])),
            (0x80 | 0x90, 3..) => self.log(format!("MIDI Note Off message received: note={} chan={ch}", b[1])),
            (0xC0, 2..) => {
                self.log(format!("MIDI Program Change message received: prog={} chan={ch}", b[1]));
                if self.settings.midi_program_change {
                    if (b[1] as usize) < self.patches.len() {
                        self.pc_notice = None;
                        self.pending.push(Cmd::SelectPatch(b[1] as usize));
                    } else {
                        self.pc_notice = Some((b[1], self.egui_ctx.input(|i| i.time)));
                    }
                }
            }
            (0xF0, 6..) if s == 0xF0 && b[1] == 0x7F && b[3] == 0x06 => {
                self.log(format!("MIDI Machine Control message received: command={}", b[4]));
                if self.settings.mmc_transport {
                    let sh = &self.engine.shared;
                    match b[4] {
                        0x01 | 0x09 => sh.playing.store(false, Ordering::Relaxed),
                        0x02 | 0x03 => sh.playing.store(true, Ordering::Relaxed),
                        0x05 => sh.rtz.store(true, Ordering::Relaxed),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn on_osc(&mut self, address: String, tags: String, args: Vec<osc::Arg>) {
        let list = |f: &dyn Fn(&osc::Arg) -> Option<String>| args.iter().filter_map(f).collect::<Vec<_>>().join(",");
        let floats = list(&|a| if let osc::Arg::F(v) = a { Some(v.to_string()) } else { None });
        let ints = list(&|a| if let osc::Arg::I(v) = a { Some(v.to_string()) } else { None });
        let strs = list(&|a| if let osc::Arg::S(v) = a { Some(v.clone()) } else { None });
        self.log(format!("OSC Message received. address={address} typetag={tags} float value(s)={floats} int value(s)={ints} string value(s)={strs}"));
        let v = args.iter().find_map(|a| a.value()).unwrap_or(1.0);
        if v > 0.0 {
            let hits: Vec<Cmd> = self.settings.app_mappings.iter().filter(|m| m.osc && m.address == address).map(|m| m.command.into()).collect();
            self.pending.extend(hits);
        }
    }

    fn shortcuts(&mut self, ctx: &Context) {
        if ctx.wants_keyboard_input() {
            return;
        }
        let mut cmds = Vec::new();
        ctx.input_mut(|i| {
            for (m, k, c) in [
                (Modifiers::COMMAND | Modifiers::SHIFT, Key::S, Cmd::SaveAs),
                (Modifiers::COMMAND, Key::S, Cmd::Save),
                (Modifiers::COMMAND, Key::N, Cmd::New),
                (Modifiers::COMMAND, Key::O, Cmd::Open),
                (Modifiers::NONE, Key::F1, Cmd::Documentation),
            ] {
                if i.consume_key(m, k) {
                    cmds.push(c);
                }
            }
        });
        self.pending.extend(cmds);
    }

    fn startup(&mut self) {
        self.started = true;
        self.new_file();
        if let Some(f) = self.start_file.take() {
            if self.shot.is_none() {
                self.open_path(f);
            } else if self.load_document(&f).is_ok() {
                // A picture of a given patch: show it without remembering it as the last file.
                self.file = Some(f);
            }
        }
        self.update_tray();
        if self.tray.is_some() && self.settings.start_in_tray {
            self.tray_hide = 3;
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let t = match self.file.as_ref().and_then(|f| f.file_stem()) {
            Some(n) => format!("{TITLE} - {}", n.to_string_lossy()),
            None => TITLE.to_string(),
        };
        if t != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(t.clone()));
            self.title = t;
        }
    }

    /// `--screenshot`: draw about 120 frames, save the window's picture, then quit.
    fn screenshot(&mut self, ctx: &Context) {
        ctx.request_repaint();
        self.frames += 1;
        let img = ctx.input(|i| i.raw.events.iter().find_map(|e| if let Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
        if let (Some(img), Some(path)) = (img, self.shot.clone()) {
            if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Err(e) = image::save_buffer(&path, &bytes, img.size[0] as u32, img.size[1] as u32, image::ExtendedColorType::Rgba8) {
                eprintln!("Could not save {}: {e}", path.display());
            }
            self.allow_close = true;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        } else if self.frames >= 120 && (self.frames - 120) % 60 == 0 {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
        }
        if self.frames > 3000 {
            eprintln!("No screenshot arrived.");
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        watchdog::set_stage(Stage::Frame);
        self.window = ctx.screen_rect().size();
        if !self.started {
            self.startup();
        }
        if self.style_for.as_ref() != Some(&self.scheme) {
            crate::dialogs::apply_style(ctx, &self.scheme);
            self.style_for = Some(self.scheme.clone());
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Incoming::Midi(b) => self.on_midi(&b),
                Incoming::Osc { address, tags, args } => self.on_osc(address, tags, args),
            }
        }
        self.engine.collect_garbage();
        self.check_device(ctx);
        let sr = self.engine.sample_rate();
        let mut errors = Vec::new();
        {
            let _stage = watchdog::enter(Stage::PluginIdle);
            for n in &mut self.graph.nodes {
                match &mut n.kind {
                    NodeKind::Builtin(b) => {
                        b.tick(sr);
                        errors.extend(b.error.take());
                    }
                    NodeKind::Plugin(p) => p.idle(),
                    _ => {}
                }
            }
        }
        for (title, text) in errors {
            self.message(&title, &text);
        }
        self.shortcuts(ctx);
        self.tray_frame(ctx);

        let bg = self.scheme.get(colours::WINDOW_BG);
        CentralPanel::default().frame(Frame::NONE.fill(bg)).show(ctx, |ui| {
            let full = ui.max_rect();
            let menu_r = Rect::from_min_size(full.min, vec2(full.width(), MENU_H));
            let panel = Rect::from_min_max(pos2(full.left(), menu_r.bottom()), full.max);
            // The view is (0, 0, W, H-40) in the original's layout code, but its drawn edges
            // (scrollbar rims, corner) sit one pixel lower in the original's picture.
            let field_r = Rect::from_min_max(panel.min, pos2(panel.right(), panel.bottom() - BAR_H + 1.0));
            let acts = self.field.show(ui, field_r, &mut self.graph, &self.scheme, &self.settings.plugins);
            self.apply(acts);
            self.bottom_bar(ui, panel);
            self.menu_bar(ui, menu_r);
        });
        self.windows(ctx);
        self.program_notice(ctx);
        for c in std::mem::take(&mut self.pending) {
            self.run(c);
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.dirty && !self.allow_close && self.shot.is_none() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.dlg.confirm = Some(Cmd::Exit);
        }
        self.update_title(ctx);
        if self.shot.is_some() {
            self.screenshot(ctx);
        } else if self.engine.running {
            // Meters and the CPU display move while audio runs.
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        watchdog::set_stage(Stage::Idle);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.stop();
        self.tray = None;
        self.midi = midi::Inputs::default();
        self.osc = None;
        if self.shot.is_none() {
            self.settings.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restart_follows_half_a_second_after_the_last_request() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut r = Restarts::default();
        assert_eq!(r.poll(t0, false), Restart::No);
        assert_eq!(r.poll(t0, true), Restart::After(RESET_DELAY));
        // Another request starts the wait over.
        assert_eq!(r.poll(ms(300), true), Restart::After(RESET_DELAY));
        assert_eq!(r.poll(ms(799), false), Restart::After(Duration::from_millis(1)));
        assert_eq!(r.poll(ms(800), false), Restart::Now);
        assert_eq!(r.poll(ms(900), false), Restart::No);
    }

    #[test]
    fn a_driver_that_keeps_asking_is_held_back_for_a_while() {
        let t0 = Instant::now();
        let mut r = Restarts::default();
        let mut t = t0;
        let mut ask_and_wait = |r: &mut Restarts| {
            r.poll(t, true);
            t += RESET_DELAY;
            r.poll(t, false)
        };
        for _ in 0..MAX_RESTARTS {
            assert_eq!(ask_and_wait(&mut r), Restart::Now);
        }
        assert_eq!(ask_and_wait(&mut r), Restart::HeldBack { first: true });
        assert_eq!(ask_and_wait(&mut r), Restart::HeldBack { first: false });
        // Once the earlier restarts are RESTART_SPAN old, a request is honoured again.
        let later = t0 + RESTART_SPAN + RESET_DELAY * (MAX_RESTARTS as u32 + 1);
        r.poll(later, true);
        assert_eq!(r.poll(later + RESET_DELAY, false), Restart::Now);
    }
}
