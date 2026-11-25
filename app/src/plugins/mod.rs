//! Plugin hosting: the shared plugin description, the UI-side handle, loading and scanning.
//!
//! Scanning runs each file in a short-lived child process (`--scan-plugin <file>`), so a plugin
//! that crashes or hangs while being examined can't take the host down with it.

pub mod editor;
pub mod vst2;
pub mod vst3;

use crate::engine::Processor;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// One plugin as listed in the Plugin List and saved in patch files (JUCE `PluginDescription`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PluginInfo {
    pub name: String,
    /// "VST", "VST3" or "Internal".
    pub format: String,
    pub category: String,
    pub manufacturer: String,
    pub version: String,
    pub file: String,
    /// VST: the unique id in hex. VST3: the 32-digit class id.
    pub uid: String,
    pub is_instrument: bool,
    pub num_inputs: u32,
    pub num_outputs: u32,
}

impl PluginInfo {
    pub fn same_plugin(&self, o: &PluginInfo) -> bool {
        self.format.eq_ignore_ascii_case(&o.format)
            && self.uid.eq_ignore_ascii_case(&o.uid)
            && (self.file.eq_ignore_ascii_case(&o.file) || file_name(&self.file).eq_ignore_ascii_case(&file_name(&o.file)))
    }

    /// The heading the plugin appears under in the add-processor menu.
    pub fn menu_category(&self) -> String {
        let c = self.category.split('|').next().unwrap_or("").trim();
        if c.is_empty() {
            if self.is_instrument { "Instrument".into() } else { "Effect".into() }
        } else {
            c.to_string()
        }
    }
}

pub fn file_name(f: &str) -> String {
    Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// The UI-thread side of a loaded plugin. Parameter indices are 0-based; the processor side
/// receives the same indices from MIDI/OSC mappings.
pub trait PluginUi {
    fn name(&self) -> String;
    fn num_params(&self) -> i32;
    fn param_name(&self, i: i32) -> String;
    fn param_value(&self, i: i32) -> f32;
    /// The plugin's own text for the parameter's current value (what the generic editor shows).
    fn param_text(&self, i: i32) -> String;
    fn set_param(&mut self, i: i32, v: f32);
    fn num_programs(&self) -> i32;
    fn program(&self) -> i32;
    fn set_program(&mut self, i: i32);
    fn program_name(&self, i: i32) -> String;
    fn get_state(&mut self) -> Vec<u8>;
    fn set_state(&mut self, data: &[u8]) -> bool;
    fn has_editor(&self) -> bool;
    fn open_editor(&mut self, owner: isize, on_top: bool, fixed_size: bool) -> Result<(), String>;
    fn editor_open(&self) -> bool;
    fn close_editor(&mut self);
    fn set_editor_on_top(&mut self, _on: bool) {}
    /// Called every UI frame: editor housekeeping and parameter echo from the audio thread.
    fn idle(&mut self);
    fn latency(&self) -> u32;
}

pub struct Loaded {
    pub ui: Box<dyn PluginUi>,
    pub proc_: Box<dyn Processor>,
    pub ins: usize,
    pub outs: usize,
}

/// Loads and starts a plugin on the UI thread, ready for `sample_rate`/`block`.
pub fn instantiate(info: &PluginInfo, sample_rate: f64, block: usize) -> Result<Loaded, String> {
    if !Path::new(&info.file).exists() {
        return Err(format!("Could not locate file: {}", info.file));
    }
    let block = block.max(16);
    match info.format.as_str() {
        "VST3" => vst3::load(info, sample_rate, block),
        "VST" => vst2::load(info, sample_rate, block),
        f => Err(format!("Unsupported plugin format: {f}")),
    }
}

fn is_vst3(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("vst3"))
}

/// Examines one file in this process (used by the `--scan-plugin` child).
pub fn scan_file(path: &Path) -> Result<Vec<PluginInfo>, String> {
    if is_vst3(path) {
        vst3::scan(path)
    } else {
        vst2::scan(path)
    }
}

const MARK: &str = "@@PEDALBOARD-SCAN@@";

/// Entry point of the `--scan-plugin <file>` child process.
pub fn scan_child_main(path: &str) {
    let r = scan_file(Path::new(path));
    let text = serde_json::to_string(&r).unwrap_or_else(|_| "{\"Err\":\"encoding\"}".into());
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{MARK}{text}");
    let _ = out.flush();
}

/// All plugin files under the given folders: `*.dll` for VST, `*.vst3` (file or bundle) for VST3.
pub fn candidates(vst2: &[String], vst3: &[String]) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, want_vst3: bool, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if want_vst3 && ext == "vst3" {
                out.push(p);
            } else if p.is_dir() {
                if depth < 8 && ext != "vst3" {
                    walk(&p, depth + 1, want_vst3, out);
                }
            } else if !want_vst3 && ext == "dll" {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for d in vst2 {
        walk(Path::new(d), 0, false, &mut out);
    }
    for d in vst3 {
        walk(Path::new(d), 0, true, &mut out);
    }
    out.dedup();
    out
}

pub enum ScanMsg {
    /// (file, index, total)
    Scanning(String, usize, usize),
    Found(Vec<PluginInfo>),
    Failed(String, String),
    Done,
}

pub struct Scan {
    pub rx: mpsc::Receiver<ScanMsg>,
    pub cancel: Arc<AtomicBool>,
}

pub fn start_scan(files: Vec<PathBuf>, repaint: impl Fn() + Send + 'static) -> Scan {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        let total = files.len();
        for (i, f) in files.iter().enumerate() {
            if c.load(Ordering::Relaxed) {
                break;
            }
            let _ = tx.send(ScanMsg::Scanning(f.display().to_string(), i, total));
            repaint();
            match scan_in_child(f) {
                Ok(list) => {
                    let _ = tx.send(ScanMsg::Found(list));
                }
                Err(e) => {
                    let _ = tx.send(ScanMsg::Failed(f.display().to_string(), e));
                }
            }
        }
        let _ = tx.send(ScanMsg::Done);
        repaint();
    });
    Scan { rx, cancel }
}

fn scan_in_child(path: &Path) -> Result<Vec<PluginInfo>, String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg("--scan-plugin")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let started = Instant::now();
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > Duration::from_secs(20) => {
                let _ = child.kill();
                let _ = child.wait();
                timed_out = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(_) => break,
        }
    }
    let text = reader.join().unwrap_or_default();
    if let Some(line) = text.lines().rev().find_map(|l| l.find(MARK).map(|i| &l[i + MARK.len()..])) {
        return serde_json::from_str::<Result<Vec<PluginInfo>, String>>(line.trim()).map_err(|e| e.to_string())?;
    }
    Err(if timed_out { "The plugin took too long to load.".into() } else { "The plugin crashed while being examined.".into() })
}

/// Reads a NUL-terminated byte string written by a plugin.
pub(crate) fn c_text(buf: &[u8]) -> String {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).trim().to_string()
}
