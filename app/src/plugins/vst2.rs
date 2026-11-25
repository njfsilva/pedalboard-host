//! VST 2.4 hosting over the plugin's plain C interface (no SDK headers needed).

use super::editor::{client_size, fit_view, is_shown, plugin_view, resize_client, set_view_size, window_size, EditorWindow};
use super::{c_text, Loaded, PluginInfo, PluginUi};
use crate::engine::{in_audio_thread, ProcessArgs, Processor, MIDI_CAP};
use crate::watchdog::{self, Stage};
use std::cell::{Cell, UnsafeCell};
use std::ffi::{c_char, c_void, CStr};
use std::path::Path;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

const MAGIC: i32 = 0x5673_7450; // 'VstP'

type Dispatcher = unsafe extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize;
type ProcessFn = unsafe extern "C" fn(*mut AEffect, *mut *mut f32, *mut *mut f32, i32);
type ProcessDoubleFn = unsafe extern "C" fn(*mut AEffect, *mut *mut f64, *mut *mut f64, i32);
type SetParamFn = unsafe extern "C" fn(*mut AEffect, i32, f32);
type GetParamFn = unsafe extern "C" fn(*mut AEffect, i32) -> f32;
type HostCallback = unsafe extern "C" fn(*mut AEffect, i32, i32, isize, *mut c_void, f32) -> isize;
type MainFn = unsafe extern "C" fn(HostCallback) -> *mut AEffect;

#[repr(C)]
pub struct AEffect {
    magic: i32,
    dispatcher: Option<Dispatcher>,
    process: Option<ProcessFn>,
    set_parameter: Option<SetParamFn>,
    get_parameter: Option<GetParamFn>,
    num_programs: i32,
    num_params: i32,
    num_inputs: i32,
    num_outputs: i32,
    flags: i32,
    resvd1: isize,
    resvd2: isize,
    initial_delay: i32,
    real_qualities: i32,
    off_qualities: i32,
    io_ratio: f32,
    object: *mut c_void,
    user: *mut c_void,
    unique_id: i32,
    version: i32,
    process_replacing: Option<ProcessFn>,
    process_double_replacing: Option<ProcessDoubleFn>,
    future: [u8; 56],
}

const _: () = assert!(std::mem::offset_of!(AEffect, process_replacing) == 120);
const _: () = assert!(std::mem::offset_of!(AEffect, unique_id) == 112);

// Plugin opcodes.
const EFF_OPEN: i32 = 0;
const EFF_CLOSE: i32 = 1;
const EFF_SET_PROGRAM: i32 = 2;
const EFF_GET_PROGRAM: i32 = 3;
const EFF_SET_PROGRAM_NAME: i32 = 4;
const EFF_GET_PROGRAM_NAME: i32 = 5;
const EFF_GET_PARAM_DISPLAY: i32 = 7;
const EFF_GET_PARAM_NAME: i32 = 8;
const EFF_SET_SAMPLE_RATE: i32 = 10;
const EFF_SET_BLOCK_SIZE: i32 = 11;
const EFF_MAINS_CHANGED: i32 = 12;
const EFF_EDIT_GET_RECT: i32 = 13;
const EFF_EDIT_OPEN: i32 = 14;
const EFF_EDIT_CLOSE: i32 = 15;
const EFF_EDIT_IDLE: i32 = 19;
const EFF_GET_CHUNK: i32 = 23;
const EFF_SET_CHUNK: i32 = 24;
const EFF_PROCESS_EVENTS: i32 = 25;
const EFF_GET_PROGRAM_NAME_INDEXED: i32 = 29;
const EFF_GET_PLUG_CATEGORY: i32 = 35;
const EFF_GET_EFFECT_NAME: i32 = 45;
const EFF_GET_VENDOR_STRING: i32 = 47;
const EFF_GET_PRODUCT_STRING: i32 = 48;
const EFF_GET_VENDOR_VERSION: i32 = 49;
const EFF_BEGIN_SET_PROGRAM: i32 = 67;
const EFF_END_SET_PROGRAM: i32 = 68;
const EFF_SHELL_GET_NEXT_PLUGIN: i32 = 70;
const EFF_START_PROCESS: i32 = 71;
const EFF_STOP_PROCESS: i32 = 72;

const FLAG_HAS_EDITOR: i32 = 1;
const FLAG_CAN_REPLACING: i32 = 1 << 4;
const FLAG_PROGRAM_CHUNKS: i32 = 1 << 5;
const FLAG_IS_SYNTH: i32 = 1 << 8;

const CATEGORY_SHELL: isize = 10;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TimeInfo {
    sample_pos: f64,
    sample_rate: f64,
    nano_seconds: f64,
    ppq_pos: f64,
    tempo: f64,
    bar_start_pos: f64,
    cycle_start_pos: f64,
    cycle_end_pos: f64,
    time_sig_numerator: i32,
    time_sig_denominator: i32,
    smpte_offset: i32,
    smpte_frame_rate: i32,
    samples_to_next_clock: i32,
    flags: i32,
}

const _: () = assert!(std::mem::size_of::<TimeInfo>() == 88);

const TIME_CHANGED: i32 = 1;
const TIME_PLAYING: i32 = 2;
const TIME_PPQ_VALID: i32 = 1 << 9;
const TIME_TEMPO_VALID: i32 = 1 << 10;
const TIME_BARS_VALID: i32 = 1 << 11;
const TIME_SIG_VALID: i32 = 1 << 13;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VstMidiEvent {
    kind: i32,
    byte_size: i32,
    delta_frames: i32,
    flags: i32,
    note_length: i32,
    note_offset: i32,
    midi_data: [u8; 4],
    detune: i8,
    note_off_velocity: i8,
    reserved1: i8,
    reserved2: i8,
}

const _: () = assert!(std::mem::size_of::<VstMidiEvent>() == 32);

#[repr(C)]
struct VstEvents {
    num_events: i32,
    reserved: isize,
    events: [*mut VstMidiEvent; MIDI_CAP],
}

#[repr(C)]
struct ERect {
    top: i16,
    left: i16,
    bottom: i16,
    right: i16,
}

/// Host-side data of one plugin instance, reachable from the callback through `resvd1`.
struct HostData {
    sample_rate: AtomicU64,
    block: AtomicU32,
    editor: AtomicIsize,
    shell_id: i32,
    /// Written by the audio thread right before each process call.
    time: UnsafeCell<TimeInfo>,
    tempo: AtomicU64,
    pos: AtomicI64,
    playing: AtomicBool,
}

unsafe impl Sync for HostData {}

impl HostData {
    fn new(sr: f64, block: usize, shell_id: i32) -> Box<HostData> {
        Box::new(HostData {
            sample_rate: AtomicU64::new(sr.to_bits()),
            block: AtomicU32::new(block as u32),
            editor: AtomicIsize::new(0),
            shell_id,
            time: UnsafeCell::new(TimeInfo::default()),
            tempo: AtomicU64::new(120f64.to_bits()),
            pos: AtomicI64::new(0),
            playing: AtomicBool::new(false),
        })
    }
}

thread_local! {
    static LOADING_SHELL_ID: Cell<i32> = const { Cell::new(0) };
    static OTHER_TIME: UnsafeCell<TimeInfo> = UnsafeCell::new(TimeInfo::default());
}

const HOST_NAME: &[u8] = b"Pedalboard Host\0";

unsafe extern "C" fn host_callback(effect: *mut AEffect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, _opt: f32) -> isize {
    let host: Option<&HostData> = if effect.is_null() { None } else { ((*effect).resvd1 as *const HostData).as_ref() };
    match opcode {
        // automate, idle: values are read back from the plugin when needed.
        0 | 3 => 0,
        // version
        1 => 2400,
        // current id (shell plugins ask which sub-plugin to create)
        2 => match host {
            Some(h) => h.shell_id as isize,
            None => LOADING_SHELL_ID.with(|c| c.get()) as isize,
        },
        // wantMidi (deprecated)
        6 => 1,
        // getTime
        7 => {
            if in_audio_thread() {
                match host {
                    Some(h) => h.time.get() as isize,
                    None => 0,
                }
            } else {
                OTHER_TIME.with(|t| {
                    let ti = &mut *t.get();
                    if let Some(h) = host {
                        let sr = f64::from_bits(h.sample_rate.load(Relaxed));
                        let tempo = f64::from_bits(h.tempo.load(Relaxed));
                        let pos = h.pos.load(Relaxed);
                        *ti = TimeInfo {
                            sample_pos: pos as f64,
                            sample_rate: sr,
                            ppq_pos: pos as f64 / sr * tempo / 60.0,
                            tempo,
                            time_sig_numerator: 4,
                            time_sig_denominator: 4,
                            flags: TIME_PPQ_VALID | TIME_TEMPO_VALID | TIME_SIG_VALID | if h.playing.load(Relaxed) { TIME_PLAYING } else { 0 },
                            ..Default::default()
                        };
                    } else {
                        *ti = TimeInfo { sample_rate: 44100.0, tempo: 120.0, time_sig_numerator: 4, time_sig_denominator: 4, ..Default::default() };
                    }
                    t.get() as isize
                })
            }
        }
        // processEvents (MIDI sent by the plugin isn't routed onwards)
        8 => 0,
        // sizeWindow
        15 => {
            if let Some(h) = host {
                let hwnd = h.editor.load(Relaxed);
                if hwnd != 0 && index > 0 && value > 0 {
                    resize_client(hwnd, index, value as i32);
                    return 1;
                }
            }
            0
        }
        16 => host.map_or(44100.0, |h| f64::from_bits(h.sample_rate.load(Relaxed))) as isize,
        17 => host.map_or(512, |h| h.block.load(Relaxed)) as isize,
        18 | 19 => 0,
        // current process level: 2 = realtime audio thread, 1 = user interface
        23 => {
            if in_audio_thread() {
                2
            } else {
                1
            }
        }
        // automation state: off
        24 => 1,
        32 | 33 => {
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(HOST_NAME.as_ptr(), ptr as *mut u8, HOST_NAME.len());
            }
            1
        }
        34 => 1000,
        37 => {
            if ptr.is_null() {
                return 0;
            }
            let s = CStr::from_ptr(ptr as *const c_char).to_bytes();
            match s {
                b"sendVstEvents" | b"sendVstMidiEvent" | b"sendVstTimeInfo" | b"receiveVstEvents" | b"receiveVstMidiEvent" | b"sizeWindow" | b"supplyIdle"
                | b"shellCategory" | b"supportShell" | b"startStopProcess" => 1,
                _ => 0,
            }
        }
        // language: English
        38 => 1,
        // updateDisplay, beginEdit, endEdit
        42..=44 => 1,
        _ => 0,
    }
}

unsafe fn open_effect(path: &Path, shell_id: i32) -> Result<*mut AEffect, String> {
    let lib = libloading::Library::new(path).map_err(|e| format!("Could not load {}: {e}", path.display()))?;
    let main: MainFn = match lib.get::<MainFn>(b"VSTPluginMain\0") {
        Ok(f) => *f,
        Err(_) => *lib.get::<MainFn>(b"main\0").map_err(|_| "This file isn't a VST plugin.".to_string())?,
    };
    // Plugins are never unloaded: many crash when their library is freed and loaded again.
    std::mem::forget(lib);
    LOADING_SHELL_ID.with(|c| c.set(shell_id));
    let e = main(host_callback);
    LOADING_SHELL_ID.with(|c| c.set(0));
    if e.is_null() || (*e).magic != MAGIC {
        return Err("The plugin could not be created.".into());
    }
    Ok(e)
}

struct Inst {
    e: *mut AEffect,
    host: Box<HostData>,
    running: AtomicBool,
}

unsafe impl Send for Inst {}
unsafe impl Sync for Inst {}

impl Inst {
    unsafe fn new(path: &Path, shell_id: i32, sr: f64, block: usize) -> Result<Inst, String> {
        let e = open_effect(path, shell_id)?;
        let host = HostData::new(sr, block, shell_id);
        (*e).resvd1 = &*host as *const HostData as isize;
        let inst = Inst { e, host, running: AtomicBool::new(false) };
        inst.call(EFF_OPEN, 0, 0, null_mut(), 0.0);
        Ok(inst)
    }

    fn call(&self, op: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
        unsafe {
            match (*self.e).dispatcher {
                Some(d) => d(self.e, op, index, value, ptr, opt),
                None => 0,
            }
        }
    }

    fn text(&self, op: i32, index: i32) -> String {
        // Generous buffer: plenty of plugins ignore the documented length limits.
        let mut buf = [0u8; 512];
        self.call(op, index, 0, buf.as_mut_ptr() as *mut c_void, 0.0);
        c_text(&buf)
    }

    fn fx(&self) -> &AEffect {
        unsafe { &*self.e }
    }

    fn get_param(&self, i: i32) -> f32 {
        match self.fx().get_parameter {
            Some(f) if i >= 0 && i < self.fx().num_params => unsafe { f(self.e, i) },
            _ => 0.0,
        }
    }

    fn set_param(&self, i: i32, v: f32) {
        if let Some(f) = self.fx().set_parameter {
            if i >= 0 && i < self.fx().num_params {
                unsafe { f(self.e, i, v) };
            }
        }
    }

    fn program(&self) -> i32 {
        self.call(EFF_GET_PROGRAM, 0, 0, null_mut(), 0.0) as i32
    }

    fn set_program(&self, i: i32) {
        if i >= 0 && i < self.fx().num_programs {
            self.call(EFF_BEGIN_SET_PROGRAM, 0, 0, null_mut(), 0.0);
            self.call(EFF_SET_PROGRAM, 0, i as isize, null_mut(), 0.0);
            self.call(EFF_END_SET_PROGRAM, 0, 0, null_mut(), 0.0);
        }
    }

    fn start(&self, sr: f64, block: usize) {
        if self.running.swap(false, Relaxed) {
            self.call(EFF_STOP_PROCESS, 0, 0, null_mut(), 0.0);
            self.call(EFF_MAINS_CHANGED, 0, 0, null_mut(), 0.0);
        }
        self.host.sample_rate.store(sr.to_bits(), Relaxed);
        self.host.block.store(block as u32, Relaxed);
        self.call(EFF_SET_SAMPLE_RATE, 0, 0, null_mut(), sr as f32);
        self.call(EFF_SET_BLOCK_SIZE, 0, block as isize, null_mut(), 0.0);
        self.call(EFF_MAINS_CHANGED, 0, 1, null_mut(), 0.0);
        self.call(EFF_START_PROCESS, 0, 0, null_mut(), 0.0);
        self.running.store(true, Relaxed);
    }

    fn describe(&self, path: &Path, shell_name: Option<String>) -> PluginInfo {
        let fx = self.fx();
        let mut name = shell_name.unwrap_or_default();
        if name.is_empty() {
            name = self.text(EFF_GET_EFFECT_NAME, 0);
        }
        if name.is_empty() {
            name = self.text(EFF_GET_PRODUCT_STRING, 0);
        }
        if name.is_empty() {
            name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        }
        let cat = self.call(EFF_GET_PLUG_CATEGORY, 0, 0, null_mut(), 0.0);
        let mut v = self.call(EFF_GET_VENDOR_VERSION, 0, 0, null_mut(), 0.0) as u32;
        if v == 0 || v == u32::MAX {
            v = fx.version as u32;
        }
        PluginInfo {
            name,
            format: "VST".into(),
            category: category_name(cat).into(),
            manufacturer: self.text(EFF_GET_VENDOR_STRING, 0),
            version: version_text(v),
            file: path.display().to_string(),
            uid: format!("{:x}", fx.unique_id as u32),
            is_instrument: fx.flags & FLAG_IS_SYNTH != 0,
            num_inputs: fx.num_inputs.max(0) as u32,
            num_outputs: fx.num_outputs.max(0) as u32,
        }
    }
}

impl Drop for Inst {
    fn drop(&mut self) {
        if self.running.swap(false, Relaxed) {
            self.call(EFF_STOP_PROCESS, 0, 0, null_mut(), 0.0);
            self.call(EFF_MAINS_CHANGED, 0, 0, null_mut(), 0.0);
        }
        self.call(EFF_CLOSE, 0, 0, null_mut(), 0.0);
    }
}

fn category_name(c: isize) -> &'static str {
    match c {
        1 => "Effect",
        2 => "Synth",
        3 => "Analysis",
        4 => "Mastering",
        5 => "Spacial",
        6 => "Reverb",
        7 => "Surround",
        8 => "Restoration",
        9 => "Offline",
        10 => "Shell",
        11 => "Generator",
        _ => "",
    }
}

/// JUCE style: one number per byte, most significant first ("V1.2.0").
fn version_text(mut v: u32) -> String {
    if v == 0 {
        return String::new();
    }
    let mut parts = Vec::new();
    while v != 0 {
        parts.push(v & 0xff);
        v >>= 8;
    }
    parts.reverse();
    format!("V{}", parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("."))
}

fn parse_uid(s: &str) -> i32 {
    u32::from_str_radix(s.trim(), 16).unwrap_or(0) as i32
}

pub fn scan(path: &Path) -> Result<Vec<PluginInfo>, String> {
    unsafe {
        let inst = Inst::new(path, 0, 44100.0, 512)?;
        let cat = inst.call(EFF_GET_PLUG_CATEGORY, 0, 0, null_mut(), 0.0);
        if cat != CATEGORY_SHELL {
            return Ok(vec![inst.describe(path, None)]);
        }
        let mut subs = Vec::new();
        loop {
            let mut buf = [0u8; 512];
            let id = inst.call(EFF_SHELL_GET_NEXT_PLUGIN, 0, 0, buf.as_mut_ptr() as *mut c_void, 0.0) as i32;
            if id == 0 || subs.len() > 4096 {
                break;
            }
            subs.push((id, c_text(&buf)));
        }
        let mut out = Vec::new();
        for (id, name) in subs {
            if let Ok(sub) = Inst::new(path, id, 44100.0, 512) {
                let mut d = sub.describe(path, Some(name));
                d.uid = format!("{:x}", id as u32);
                out.push(d);
            }
        }
        Ok(out)
    }
}

pub fn load(info: &PluginInfo, sr: f64, block: usize) -> Result<Loaded, String> {
    let path = Path::new(&info.file);
    let inst = unsafe { Inst::new(path, parse_uid(&info.uid), sr, block)? };
    let fx = inst.fx();
    if fx.process_replacing.is_none() && fx.process.is_none() {
        return Err("The plugin has no audio processing.".into());
    }
    inst.start(sr, block);
    let ins = fx.num_inputs.clamp(0, 64) as usize;
    let outs = fx.num_outputs.clamp(0, 64) as usize;
    let mut name = info.name.clone();
    if name.is_empty() {
        name = inst.text(EFF_GET_EFFECT_NAME, 0);
    }
    let never_resizable = name == NEVER_RESIZABLE || path.file_stem().is_some_and(|s| s == NEVER_RESIZABLE);
    let inst = Arc::new(inst);
    let mut p = V2Proc {
        inst: inst.clone(),
        scratch: Vec::new(),
        in_ptrs: Vec::new(),
        out_scratch: Vec::new(),
        out_ptrs: Vec::new(),
        cap: 0,
        midi: vec![VstMidiEvent::default(); MIDI_CAP].into_boxed_slice(),
        events: Box::new(VstEvents { num_events: 0, reserved: 0, events: [null_mut(); MIDI_CAP] }),
        was_playing: false,
        ins,
        outs,
    };
    p.alloc(block);
    Ok(Loaded { ui: Box::new(V2Ui { inst, name, never_resizable, editor: None }), proc_: Box::new(p), ins, outs })
}

struct V2Proc {
    inst: Arc<Inst>,
    scratch: Vec<Vec<f32>>,
    in_ptrs: Vec<*mut f32>,
    out_scratch: Vec<Vec<f32>>,
    out_ptrs: Vec<*mut f32>,
    cap: usize,
    midi: Box<[VstMidiEvent]>,
    events: Box<VstEvents>,
    was_playing: bool,
    ins: usize,
    outs: usize,
}

unsafe impl Send for V2Proc {}

impl V2Proc {
    fn alloc(&mut self, block: usize) {
        self.cap = block.max(16);
        self.scratch = (0..self.ins).map(|_| vec![0.0; self.cap]).collect();
        self.in_ptrs = self.scratch.iter_mut().map(|b| b.as_mut_ptr()).collect();
        self.out_scratch = (0..self.outs).map(|_| vec![0.0; self.cap]).collect();
        self.out_ptrs = self.out_scratch.iter_mut().map(|b| b.as_mut_ptr()).collect();
        if self.out_ptrs.is_empty() {
            self.out_ptrs.push(null_mut());
        }
    }
}

impl Processor for V2Proc {
    fn prepare(&mut self, sample_rate: f64, max_block: usize) {
        self.inst.start(sample_rate, max_block.max(16));
        self.alloc(max_block);
    }

    fn process(&mut self, a: &ProcessArgs) {
        let n = a.frames;
        let outs = a.outputs.len().min(self.outs);
        if n > self.cap || n == 0 {
            for i in 0..a.outputs.len() {
                a.output(i).fill(0.0);
            }
            return;
        }
        // Plugins may process in place, so they get private copies of their inputs.
        for (i, buf) in self.scratch.iter_mut().enumerate() {
            let d = &mut buf[..n];
            if i < a.inputs.len() {
                d.copy_from_slice(a.input(i));
            } else {
                d.fill(0.0);
            }
        }
        // Outputs the graph doesn't use still need somewhere to go.
        for i in 0..self.outs {
            self.out_ptrs[i] = if i < outs { a.outputs[i] } else { self.out_scratch[i].as_mut_ptr() };
        }
        if !a.midi_in.is_empty() {
            let count = a.midi_in.len().min(MIDI_CAP);
            for (k, ev) in a.midi_in[..count].iter().enumerate() {
                self.midi[k] = VstMidiEvent {
                    kind: 1,
                    byte_size: 32,
                    delta_frames: (ev.frame as i32).min(n as i32 - 1),
                    flags: 1,
                    midi_data: [ev.data[0], ev.data[1], ev.data[2], 0],
                    ..Default::default()
                };
                self.events.events[k] = &mut self.midi[k];
            }
            self.events.num_events = count as i32;
            self.inst.call(EFF_PROCESS_EVENTS, 0, 0, &mut *self.events as *mut VstEvents as *mut c_void, 0.0);
        }
        let t = a.transport;
        let h = &self.inst.host;
        h.tempo.store(t.tempo.to_bits(), Relaxed);
        h.pos.store(t.sample_pos, Relaxed);
        h.playing.store(t.playing, Relaxed);
        let ppq = t.ppq();
        let bar_len = t.num.max(1) as f64 * 4.0 / t.den.max(1) as f64;
        unsafe {
            *h.time.get() = TimeInfo {
                sample_pos: t.sample_pos as f64,
                sample_rate: t.sample_rate,
                ppq_pos: ppq,
                tempo: t.tempo,
                bar_start_pos: (ppq / bar_len).floor() * bar_len,
                time_sig_numerator: t.num,
                time_sig_denominator: t.den,
                flags: TIME_PPQ_VALID
                    | TIME_TEMPO_VALID
                    | TIME_BARS_VALID
                    | TIME_SIG_VALID
                    | if t.playing { TIME_PLAYING } else { 0 }
                    | if t.playing != self.was_playing { TIME_CHANGED } else { 0 },
                ..Default::default()
            };
        }
        self.was_playing = t.playing;
        let fx = self.inst.fx();
        unsafe {
            if let Some(pr) = fx.process_replacing.filter(|_| fx.flags & FLAG_CAN_REPLACING != 0 || fx.process.is_none()) {
                pr(self.inst.e, self.in_ptrs.as_mut_ptr(), self.out_ptrs.as_mut_ptr(), n as i32);
            } else if let Some(p) = fx.process {
                for i in 0..self.outs {
                    std::slice::from_raw_parts_mut(self.out_ptrs[i], n).fill(0.0);
                }
                p(self.inst.e, self.in_ptrs.as_mut_ptr(), self.out_ptrs.as_mut_ptr(), n as i32);
            }
        }
        for i in outs..a.outputs.len() {
            a.output(i).fill(0.0);
        }
    }

    fn set_parameter(&mut self, index: i32, value: f32) {
        self.inst.set_param(index, value);
    }
}

// Plugin windows follow the original's VST window: opening in 0x1400fdf40, its timer in
// 0x1400fde40, the size check in 0x1400fe480 and moves in 0x1400fdbf0.

/// Time between a plugin window's idle calls. The original waits 18 + a random 0-4 ms;
/// 10 ms (about 100 calls a second on the window's own ticker) keeps these windows above
/// 60 pictures a second, as the person asked.
const EDITOR_IDLE_MS: u32 = 10;
/// Idle calls between checks that the window still fits the plugin's own window.
const SIZE_CHECK_TICKS: i32 = 10;
/// Smallest plugin area each way.
const EDITOR_MIN: i32 = 32;
/// A plugin window's first size, before the plugin's editor sizes it (0x140075950).
const FIRST_SIZE: (i32, i32) = (400, 300);
/// Plugin area when the plugin doesn't make a window of its own.
const NO_VIEW_SIZE: (i32, i32) = (300, 150);
/// The one plugin whose window the original never lets the person resize (0x140075950).
const NEVER_RESIZABLE: &str = "VAZPlusVSTi";

thread_local! {
    /// Set while a plugin window's idle call runs: an idle call that would start inside
    /// another is skipped, as the original's one shared flag does.
    static IN_EDITOR_IDLE: Cell<bool> = const { Cell::new(false) };
}

/// The original's choice of plugin area when a window opens: the plugin's own window size,
/// unless the size the plugin reports is believable and more than 350 px away from it. Then
/// the plugin's window is set to the reported size, which the second value asks for.
fn view_size(actual: (i32, i32), reported: Option<(i32, i32)>) -> ((i32, i32), bool) {
    let (w, h) = actual;
    if let Some((rw, rh)) = reported {
        let believable = rw > 50 && rh > 50 && rw < 2000 && rh < 2000 && rw != w && rh != h;
        let missing = (w == 0 && rw > 0) || (h == 0 && rh > 0);
        if (believable || missing) && ((rw - w).abs() > 350 || (rh - h).abs() > 350) {
            return ((rw, rh), true);
        }
    }
    ((w, h), false)
}

/// The original's size check: when the plugin has changed the size of its own window, the
/// editor window follows. `stuck` holds a size Windows wouldn't give the window, so it isn't
/// asked for again on every check. Returns true when it resized the window.
fn follow_view_size(hwnd: isize, stuck: &mut Option<(i32, i32)>) -> bool {
    if !is_shown(hwnd) {
        return false;
    }
    let view = plugin_view(hwnd);
    if view == 0 {
        return false;
    }
    let size = window_size(view);
    if size.0 <= 0 || size.1 <= 0 {
        return false;
    }
    if size == client_size(hwnd) {
        *stuck = None;
        return false;
    }
    if *stuck == Some(size) {
        return false;
    }
    resize_client(hwnd, size.0, size.1);
    if client_size(hwnd) != size {
        *stuck = Some(size);
    }
    true
}

struct V2Ui {
    inst: Arc<Inst>,
    name: String,
    never_resizable: bool,
    editor: Option<EditorWindow>,
}

impl V2Ui {
    /// The size the plugin reports for its window, as given (it may be empty).
    fn reported_rect(&self) -> Option<(i32, i32)> {
        let mut r: *mut ERect = null_mut();
        self.inst.call(EFF_EDIT_GET_RECT, 0, 0, &mut r as *mut *mut ERect as *mut c_void, 0.0);
        let r = unsafe { r.as_ref()? };
        Some((i32::from(r.right) - i32::from(r.left), i32::from(r.bottom) - i32::from(r.top)))
    }

    fn rect(&self) -> Option<(i32, i32)> {
        self.reported_rect().filter(|&(w, h)| w > 0 && h > 0)
    }

    fn program_block(&self, out: &mut Vec<u8>) {
        let fx = self.inst.fx();
        let np = fx.num_params.max(0);
        let len = 56 + 4 * np as usize;
        out.extend_from_slice(b"CcnK");
        out.extend_from_slice(&((len - 8) as i32).to_be_bytes());
        out.extend_from_slice(b"FxCk");
        out.extend_from_slice(&1i32.to_be_bytes());
        out.extend_from_slice(&fx.unique_id.to_be_bytes());
        out.extend_from_slice(&fx.version.to_be_bytes());
        out.extend_from_slice(&np.to_be_bytes());
        let mut name = [0u8; 28];
        let pn = self.inst.text(EFF_GET_PROGRAM_NAME, 0);
        for (d, s) in name.iter_mut().zip(pn.bytes().take(27)) {
            *d = s;
        }
        out.extend_from_slice(&name);
        for i in 0..np {
            out.extend_from_slice(&self.inst.get_param(i).to_be_bytes());
        }
    }

    fn restore_program(&self, d: &[u8]) -> bool {
        if d.len() < 56 || &d[0..4] != b"CcnK" || &d[8..12] != b"FxCk" {
            return false;
        }
        let mut name = d[28..56].to_vec();
        name.push(0);
        self.inst.call(EFF_SET_PROGRAM_NAME, 0, 0, name.as_mut_ptr() as *mut c_void, 0.0);
        let np = be(d, 24).unwrap_or(0).max(0) as usize;
        for i in 0..np {
            match d.get(56 + 4 * i..60 + 4 * i) {
                Some(b) => self.inst.set_param(i as i32, f32::from_be_bytes(b.try_into().unwrap())),
                None => break,
            }
        }
        true
    }
}

fn be(d: &[u8], off: usize) -> Option<i32> {
    d.get(off..off + 4).map(|b| i32::from_be_bytes(b.try_into().unwrap()))
}

impl PluginUi for V2Ui {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn num_params(&self) -> i32 {
        self.inst.fx().num_params.max(0)
    }

    fn param_name(&self, i: i32) -> String {
        let n = self.inst.text(EFF_GET_PARAM_NAME, i);
        if n.is_empty() {
            format!("Parameter {}", i + 1)
        } else {
            n
        }
    }

    fn param_value(&self, i: i32) -> f32 {
        self.inst.get_param(i)
    }

    fn param_text(&self, i: i32) -> String {
        self.inst.text(EFF_GET_PARAM_DISPLAY, i).trim().to_string()
    }

    fn set_param(&mut self, i: i32, v: f32) {
        self.inst.set_param(i, v);
    }

    fn num_programs(&self) -> i32 {
        self.inst.fx().num_programs.max(0)
    }

    fn program(&self) -> i32 {
        self.inst.program()
    }

    fn set_program(&mut self, i: i32) {
        self.inst.set_program(i);
    }

    fn program_name(&self, i: i32) -> String {
        let mut buf = [0u8; 512];
        let ok = self.inst.call(EFF_GET_PROGRAM_NAME_INDEXED, i, -1, buf.as_mut_ptr() as *mut c_void, 0.0) != 0;
        let mut n = if ok { c_text(&buf) } else { String::new() };
        if n.is_empty() && i == self.inst.program() {
            n = self.inst.text(EFF_GET_PROGRAM_NAME, 0);
        }
        if n.is_empty() {
            format!("Program {}", i + 1)
        } else {
            n
        }
    }

    /// Saves like JUCE's VST host does, so patch files stay interchangeable: an 'FBCh' bank chunk
    /// for plugins that use chunks, otherwise an 'FxBk' bank of every program's parameters.
    fn get_state(&mut self) -> Vec<u8> {
        let fx = self.inst.fx();
        let (uid, ver, np) = (fx.unique_id, fx.version, fx.num_programs.max(0));
        let cur = self.inst.program();
        let mut v = Vec::new();
        v.extend_from_slice(b"CcnK");
        v.extend_from_slice(&0i32.to_be_bytes());
        if fx.flags & FLAG_PROGRAM_CHUNKS != 0 {
            let mut ptr: *mut c_void = null_mut();
            let size = self.inst.call(EFF_GET_CHUNK, 0, 0, &mut ptr as *mut *mut c_void as *mut c_void, 0.0);
            let chunk: &[u8] = if size > 0 && !ptr.is_null() { unsafe { std::slice::from_raw_parts(ptr as *const u8, size as usize) } } else { &[] };
            v.extend_from_slice(b"FBCh");
            v.extend_from_slice(&1i32.to_be_bytes());
            v.extend_from_slice(&uid.to_be_bytes());
            v.extend_from_slice(&ver.to_be_bytes());
            v.extend_from_slice(&np.to_be_bytes());
            v.extend_from_slice(&cur.to_be_bytes());
            v.extend_from_slice(&[0u8; 124]);
            v.extend_from_slice(&(chunk.len() as i32).to_be_bytes());
            v.extend_from_slice(chunk);
        } else {
            v.extend_from_slice(b"FxBk");
            v.extend_from_slice(&1i32.to_be_bytes());
            v.extend_from_slice(&uid.to_be_bytes());
            v.extend_from_slice(&ver.to_be_bytes());
            v.extend_from_slice(&np.to_be_bytes());
            v.extend_from_slice(&cur.to_be_bytes());
            v.extend_from_slice(&[0u8; 124]);
            let n = np.max(1) as usize;
            let mut progs = vec![Vec::new(); n];
            let cur_i = (cur.max(0) as usize).min(n - 1);
            self.program_block(&mut progs[cur_i]);
            let saved: Vec<f32> = (0..fx.num_params.max(0)).map(|i| self.inst.get_param(i)).collect();
            for (i, p) in progs.iter_mut().enumerate() {
                if i != cur_i {
                    self.inst.set_program(i as i32);
                    self.program_block(p);
                }
            }
            if np > 0 {
                self.inst.set_program(cur_i as i32);
            }
            for (i, val) in saved.iter().enumerate() {
                self.inst.set_param(i as i32, *val);
            }
            for p in progs {
                v.extend_from_slice(&p);
            }
        }
        let size = (v.len() - 8) as i32;
        v[4..8].copy_from_slice(&size.to_be_bytes());
        v
    }

    fn set_state(&mut self, d: &[u8]) -> bool {
        if d.len() < 28 || &d[0..4] != b"CcnK" {
            return false;
        }
        match &d[8..12] {
            b"FxBk" => {
                let np = be(d, 24).unwrap_or(0);
                if d.len() < 156 + 56 {
                    return false;
                }
                let params = be(d, 156 + 24).unwrap_or(0).max(0) as usize;
                let len = 56 + 4 * params;
                let cur = self.inst.program().max(0);
                for i in 0..np.max(0) {
                    if i == cur {
                        continue;
                    }
                    let off = 156 + i as usize * len;
                    let Some(p) = d.get(off..off + len) else { return false };
                    self.inst.set_program(i);
                    if !self.restore_program(p) {
                        return false;
                    }
                }
                if np > 0 {
                    self.inst.set_program(cur);
                }
                let off = 156 + cur as usize * len;
                d.get(off..off + len).is_some_and(|p| self.restore_program(p))
            }
            b"FxCk" => self.restore_program(d),
            b"FBCh" | b"hCBF" => {
                let size = be(d, 156).unwrap_or(-1);
                if size < 0 || 160 + size as usize > d.len() {
                    return false;
                }
                let mut chunk = d[160..160 + size as usize].to_vec();
                self.inst.call(EFF_SET_CHUNK, 0, size as isize, chunk.as_mut_ptr() as *mut c_void, 0.0);
                true
            }
            b"FPCh" | b"hCPF" => {
                let size = be(d, 56).unwrap_or(-1);
                if size < 0 || 60 + size as usize > d.len() {
                    return false;
                }
                let mut chunk = d[60..60 + size as usize].to_vec();
                self.inst.call(EFF_SET_CHUNK, 1, size as isize, chunk.as_mut_ptr() as *mut c_void, 0.0);
                let mut name = d[28..56].to_vec();
                name.push(0);
                self.inst.call(EFF_SET_PROGRAM_NAME, 0, 0, name.as_mut_ptr() as *mut c_void, 0.0);
                true
            }
            _ => false,
        }
    }

    fn has_editor(&self) -> bool {
        self.inst.fx().flags & FLAG_HAS_EDITOR != 0
    }

    fn open_editor(&mut self, owner: isize, on_top: bool, fixed_size: bool) -> Result<(), String> {
        if let Some(w) = &self.editor {
            w.show();
            return Ok(());
        }
        if !self.has_editor() {
            return Err("This plugin has no editor.".into());
        }
        let (w, h) = self.rect().unwrap_or(FIRST_SIZE);
        let resizable = !fixed_size && !self.never_resizable;
        let mut win = EditorWindow::new(&self.name, w, h, owner, on_top, resizable)?;
        let hwnd = win.hwnd as isize;
        self.inst.host.editor.store(hwnd, Relaxed);
        // The original's order: open, ask for the size again, then ask for the program.
        self.inst.call(EFF_EDIT_OPEN, 0, 0, win.hwnd as *mut c_void, 0.0);
        let reported = self.reported_rect();
        self.inst.call(EFF_GET_PROGRAM, 0, 0, null_mut(), 0.0);
        let view = plugin_view(hwnd);
        // A plugin that won't take the size it reported keeps its own size from then on.
        let mut keep_size = false;
        let (w, h) = if view == 0 {
            NO_VIEW_SIZE
        } else {
            let ((w, h), force) = view_size(window_size(view), reported);
            if force {
                set_view_size(view, w, h);
                keep_size = window_size(view) != (w, h);
            }
            (w.max(EDITOR_MIN), h.max(EDITOR_MIN))
        };
        // Whenever the window changes size, the plugin's window is laid over all of it.
        win.set_on_size(Box::new(move |cw, ch| {
            let view = plugin_view(hwnd);
            if view != 0 {
                fit_view(view, cw, ch);
            }
        }));
        resize_client(hwnd, w, h);
        let inst = self.inst.clone();
        let mut countdown = 0;
        let mut stuck = None;
        win.set_timer(
            EDITOR_IDLE_MS,
            Box::new(move || {
                countdown -= 1;
                if countdown <= 0 {
                    countdown = SIZE_CHECK_TICKS;
                    // After a change, check again on the next tick.
                    if !keep_size && follow_view_size(hwnd, &mut stuck) {
                        countdown = 0;
                    }
                }
                if !IN_EDITOR_IDLE.with(|b| b.replace(true)) {
                    let _stage = watchdog::enter(Stage::PluginWindowIdle);
                    inst.call(EFF_EDIT_IDLE, 0, 0, null_mut(), 0.0);
                    IN_EDITOR_IDLE.with(|b| b.set(false));
                }
            }),
        );
        win.show();
        self.editor = Some(win);
        Ok(())
    }

    fn editor_open(&self) -> bool {
        self.editor.is_some()
    }

    fn close_editor(&mut self) {
        if let Some(w) = self.editor.take() {
            self.inst.call(EFF_EDIT_CLOSE, 0, 0, null_mut(), 0.0);
            self.inst.host.editor.store(0, Relaxed);
            drop(w);
        }
    }

    fn set_editor_on_top(&mut self, on: bool) {
        if let Some(w) = &self.editor {
            w.set_topmost(on);
        }
    }

    fn idle(&mut self) {
        if self.editor.as_ref().is_some_and(|w| w.closed()) {
            self.close_editor();
        }
    }

    fn latency(&self) -> u32 {
        self.inst.fx().initial_delay.max(0) as u32
    }
}

impl Drop for V2Ui {
    fn drop(&mut self) {
        self.close_editor();
    }
}

#[cfg(test)]
mod tests {
    use super::view_size;

    #[test]
    fn a_window_takes_the_plugins_own_size_unless_the_reported_size_is_far_off() {
        // Close to the plugin's own window: its own size wins.
        assert_eq!(view_size((600, 400), Some((620, 410))), ((600, 400), false));
        // Believable and more than 350 px away: the reported size is used and set on it.
        assert_eq!(view_size((200, 100), Some((800, 500))), ((800, 500), true));
        // Too big to believe: its own size wins.
        assert_eq!(view_size((200, 100), Some((2400, 500))), ((200, 100), false));
        // A window with no width yet takes a reported width that is far enough off.
        assert_eq!(view_size((0, 300), Some((400, 300))), ((400, 300), true));
        // Nothing reported.
        assert_eq!(view_size((640, 480), None), ((640, 480), false));
    }

    #[test]
    fn an_editor_window_and_the_plugins_window_keep_the_same_size() {
        use super::super::editor::{client_size, fit_view, resize_client, set_view_size, wide, window_size, EditorWindow};
        use super::follow_view_size;
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        let mut win = EditorWindow::new("test", 300, 150, 0, false, false).unwrap();
        let hwnd = win.hwnd as isize;
        let class = wide("STATIC");
        unsafe {
            // Off screen and without taking focus.
            SetWindowPos(win.hwnd, null_mut(), -20000, -20000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            ShowWindow(win.hwnd, SW_SHOWNOACTIVATE);
            let view = CreateWindowExW(0, class.as_ptr(), null(), WS_CHILD | WS_VISIBLE, 0, 0, 500, 300, win.hwnd, null_mut(), null_mut(), null());
            assert!(!view.is_null());
            let view = view as isize;
            // The window follows the plugin's own window.
            let mut stuck = None;
            assert!(follow_view_size(hwnd, &mut stuck));
            assert_eq!(client_size(hwnd), (500, 300));
            assert!(!follow_view_size(hwnd, &mut stuck));
            set_view_size(view, 700, 450);
            assert!(follow_view_size(hwnd, &mut stuck));
            assert_eq!(client_size(hwnd), (700, 450));
            // A size Windows won't give a window with a title bar is asked for once, not on every check.
            set_view_size(view, 4, 4);
            assert!(follow_view_size(hwnd, &mut stuck));
            assert!(!follow_view_size(hwnd, &mut stuck));
            // The plugin's window is laid over the whole window when the window changes size.
            win.set_on_size(Box::new(move |cw, ch| fit_view(view, cw, ch)));
            resize_client(hwnd, 640, 480);
            assert_eq!(window_size(view), (640, 480));
        }
    }
}
