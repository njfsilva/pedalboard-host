//! Real-time audio engine.
//!
//! The UI thread compiles the patch into a `Plan` (processing order, buffer pool and pointer
//! tables) and hands it to the audio thread through a lock-free queue. The audio callback never
//! allocates, never blocks and never frees: replaced plans travel back to the UI thread to be
//! dropped there.

pub mod asio;
pub mod wasapi;

use crate::settings::AudioConfig;
use std::cell::{Cell, UnsafeCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const MIDI_CAP: usize = 1024;
pub const OSC_CAP: usize = 256;

thread_local! {
    static AUDIO_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// True while running inside an audio callback (plugins ask via "process level").
pub fn in_audio_thread() -> bool {
    AUDIO_THREAD.with(|c| c.get())
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MidiEvent {
    pub frame: u32,
    pub len: u8,
    pub data: [u8; 3],
}

impl MidiEvent {
    pub fn new(bytes: &[u8]) -> Option<MidiEvent> {
        if bytes.is_empty() || bytes.len() > 3 || bytes[0] < 0x80 {
            return None;
        }
        let mut data = [0u8; 3];
        data[..bytes.len()].copy_from_slice(bytes);
        Some(MidiEvent { frame: 0, len: bytes.len() as u8, data })
    }
    pub fn status(&self) -> u8 {
        self.data[0] & 0xF0
    }
    /// 1..=16
    pub fn channel(&self) -> u8 {
        (self.data[0] & 0x0F) + 1
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct OscEvent {
    pub hash: u64,
    pub value: f32,
}

pub fn osc_hash(addr: &str) -> u64 {
    // FNV-1a: stable, allocation free, good enough for a handful of addresses.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in addr.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[derive(Clone, Copy, Debug)]
pub struct Transport {
    pub playing: bool,
    pub tempo: f64,
    pub sample_pos: i64,
    pub sample_rate: f64,
    pub num: i32,
    pub den: i32,
}

impl Transport {
    pub fn ppq(&self) -> f64 {
        self.sample_pos as f64 / self.sample_rate * self.tempo / 60.0
    }
}

pub struct ProcessArgs<'a> {
    pub inputs: &'a [*const f32],
    pub outputs: &'a [*mut f32],
    pub frames: usize,
    pub midi_in: &'a [MidiEvent],
    pub transport: &'a Transport,
}

impl<'a> ProcessArgs<'a> {
    #[inline]
    pub fn input(&self, i: usize) -> &[f32] {
        unsafe { std::slice::from_raw_parts(self.inputs[i], self.frames) }
    }
    /// Each output channel is a distinct buffer; callers must not hold two views of one channel.
    #[inline]
    #[allow(clippy::mut_from_ref)]
    pub fn output(&self, i: usize) -> &mut [f32] {
        unsafe { std::slice::from_raw_parts_mut(self.outputs[i], self.frames) }
    }
}

pub trait Processor: Send {
    /// Called on the UI thread while the node isn't running.
    fn prepare(&mut self, sample_rate: f64, max_block: usize);
    fn process(&mut self, a: &ProcessArgs);
    /// Called on the audio thread for MIDI/OSC mappings (value 0..1).
    fn set_parameter(&mut self, _index: i32, _value: f32) {}
}

/// A node shared between the UI (identity, bypass flag) and the audio thread (processor).
pub struct NodeCell {
    pub id: u32,
    pub bypass: AtomicBool,
    proc_: UnsafeCell<Option<Box<dyn Processor>>>,
}

unsafe impl Sync for NodeCell {}
unsafe impl Send for NodeCell {}

impl NodeCell {
    pub fn new(id: u32, p: Option<Box<dyn Processor>>) -> Arc<NodeCell> {
        Arc::new(NodeCell { id, bypass: AtomicBool::new(false), proc_: UnsafeCell::new(p) })
    }

    /// # Safety
    /// Only while no running plan contains this node (device stopped or node not yet added),
    /// or from the audio thread itself.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn processor(&self) -> Option<&mut Box<dyn Processor>> {
        (*self.proc_.get()).as_mut()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Proc,
    AudioIn,
    AudioOut,
    MidiIn,
    OscIn,
}

pub struct NodeDesc {
    pub cell: Arc<NodeCell>,
    pub kind: StepKind,
    pub ins: usize,
    pub outs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiMap {
    pub node: u32,
    pub param: i32,
    pub cc: u8,
    pub channel: u8,
    pub latch: bool,
    pub lower: f32,
    pub upper: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OscMap {
    pub node: u32,
    pub param: i32,
    pub address: String,
}

pub const MIDI_PIN: u32 = 0x1000;

#[derive(Default)]
pub struct GraphDesc {
    pub nodes: Vec<NodeDesc>,
    /// (src node, src channel, dst node, dst channel); MIDI uses channel 0x1000.
    pub conns: Vec<(u32, u32, u32, u32)>,
    pub midi_maps: Vec<MidiMap>,
    pub osc_maps: Vec<OscMap>,
}

struct Step {
    cell: Arc<NodeCell>,
    kind: StepKind,
    /// Per input channel: source buffers (0 = silent).
    sources: Vec<Vec<usize>>,
    mix: Vec<usize>,
    in_ptrs: Vec<*const f32>,
    out_ptrs: Vec<*mut f32>,
    outs: Vec<usize>,
    gets_midi: bool,
}

struct ActiveMap {
    step: usize,
    m: MidiMap,
    toggled: bool,
}

pub struct Plan {
    steps: Vec<Step>,
    pool: Vec<f32>,
    max_block: usize,
    dev_in: Vec<usize>,
    dev_out: Vec<(Vec<usize>, usize)>,
    maps: Vec<ActiveMap>,
    osc: Vec<(usize, u64, i32)>,
}

unsafe impl Send for Plan {}

impl Plan {
    /// Compiles a graph. `dev_ins`/`dev_outs` are the active device channel counts.
    pub fn build(g: &GraphDesc, max_block: usize, dev_ins: usize, dev_outs: usize) -> Box<Plan> {
        let max_block = max_block.max(16);
        let idx: HashMap<u32, usize> = g.nodes.iter().enumerate().map(|(i, n)| (n.cell.id, i)).collect();
        // Topological order (Kahn). Edges that would form a loop are ignored.
        let n = g.nodes.len();
        let mut indeg = vec![0usize; n];
        let mut adj = vec![Vec::new(); n];
        for &(s, _, d, _) in &g.conns {
            if let (Some(&a), Some(&b)) = (idx.get(&s), idx.get(&d)) {
                if a != b {
                    adj[a].push(b);
                    indeg[b] += 1;
                }
            }
        }
        let mut order: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut head = 0;
        while head < order.len() {
            let a = order[head];
            head += 1;
            for &b in &adj[a] {
                indeg[b] -= 1;
                if indeg[b] == 0 {
                    order.push(b);
                }
            }
        }
        let placed: Vec<bool> = {
            let mut p = vec![false; n];
            order.iter().for_each(|&i| p[i] = true);
            p
        };
        let pos_of: Vec<usize> = {
            let mut p = vec![usize::MAX; n];
            for (k, &i) in order.iter().enumerate() {
                p[i] = k;
            }
            p
        };

        // Buffer 0 is always silent; every output channel gets its own buffer.
        let mut nbuf = 1usize;
        let mut out_buf: Vec<Vec<usize>> = Vec::with_capacity(n);
        for nd in &g.nodes {
            let outs = if nd.kind == StepKind::AudioIn { dev_ins } else { nd.outs };
            out_buf.push((0..outs).map(|k| nbuf + k).collect());
            nbuf += outs;
        }
        let mut steps = Vec::new();
        let mut dev_out: Vec<(Vec<usize>, usize)> = vec![(Vec::new(), 0); dev_outs];
        let mut dev_in = Vec::new();
        for &i in &order {
            let nd = &g.nodes[i];
            let ins = if nd.kind == StepKind::AudioOut { dev_outs } else { nd.ins };
            let mut sources = vec![Vec::new(); ins];
            let mut gets_midi = false;
            for &(s, sc, d, dc) in &g.conns {
                if d != nd.cell.id {
                    continue;
                }
                let Some(&si) = idx.get(&s) else { continue };
                if !placed[si] || pos_of[si] >= pos_of[i] {
                    continue;
                }
                if dc == MIDI_PIN || sc == MIDI_PIN {
                    if dc == MIDI_PIN && sc == MIDI_PIN && g.nodes[si].kind == StepKind::MidiIn {
                        gets_midi = true;
                    }
                    continue;
                }
                if let (Some(src), Some(list)) = (out_buf[si].get(sc as usize), sources.get_mut(dc as usize)) {
                    list.push(*src);
                }
            }
            let mut mix = vec![0usize; ins];
            for (ch, list) in sources.iter().enumerate() {
                if list.len() > 1 {
                    mix[ch] = nbuf;
                    nbuf += 1;
                }
            }
            if nd.kind == StepKind::AudioOut {
                for (ch, list) in sources.iter().enumerate() {
                    dev_out[ch] = (list.clone(), mix[ch]);
                }
            }
            if nd.kind == StepKind::AudioIn {
                dev_in = out_buf[i].clone();
            }
            steps.push(Step {
                cell: nd.cell.clone(),
                kind: nd.kind,
                sources,
                mix,
                in_ptrs: Vec::new(),
                out_ptrs: Vec::new(),
                outs: out_buf[i].clone(),
                gets_midi,
            });
        }
        let mut pool = vec![0.0f32; nbuf * max_block];
        let base = pool.as_mut_ptr();
        let ptr = |b: usize| unsafe { base.add(b * max_block) };
        for st in &mut steps {
            st.in_ptrs = st
                .sources
                .iter()
                .zip(&st.mix)
                .map(|(l, &m)| match l.len() {
                    0 => ptr(0) as *const f32,
                    1 => ptr(l[0]) as *const f32,
                    _ => ptr(m) as *const f32,
                })
                .collect();
            st.out_ptrs = st.outs.iter().map(|&b| ptr(b)).collect();
        }
        let step_of: HashMap<u32, usize> = steps.iter().enumerate().map(|(k, s)| (s.cell.id, k)).collect();
        let maps = g
            .midi_maps
            .iter()
            .filter_map(|m| step_of.get(&m.node).map(|&step| ActiveMap { step, m: *m, toggled: false }))
            .collect();
        let osc = g
            .osc_maps
            .iter()
            .filter_map(|m| step_of.get(&m.node).map(|&s| (s, osc_hash(&m.address), m.param)))
            .collect();
        Box::new(Plan { steps, pool, max_block, dev_in, dev_out, maps, osc })
    }

    #[inline]
    fn buf(&mut self, b: usize) -> *mut f32 {
        unsafe { self.pool.as_mut_ptr().add(b * self.max_block) }
    }

    fn set_param(&mut self, step: usize, param: i32, value: f32) {
        let st = &self.steps[step];
        if param == -1 {
            // Mapping index -1 is the bypass switch (0x140042290: on above 0.5).
            st.cell.bypass.store(value > 0.5, Ordering::Relaxed);
        } else if let Some(p) = unsafe { st.cell.processor() } {
            p.set_parameter(param, value);
        }
    }

    fn apply_midi_maps(&mut self, events: &[MidiEvent]) {
        for ev in events {
            if ev.status() != 0xB0 {
                continue;
            }
            let (cc, val, ch) = (ev.data[1], ev.data[2] as i32, ev.channel());
            for k in 0..self.maps.len() {
                let am = &mut self.maps[k];
                if am.m.cc != cc || (am.m.channel != 0 && am.m.channel != ch) {
                    continue;
                }
                // MidiMapping::ccReceived (0x140052ba0).
                let v = if am.m.latch {
                    if val == 0 {
                        continue;
                    }
                    am.toggled = !am.toggled;
                    if am.toggled { 1.0 } else { 0.0 }
                } else {
                    val as f32 / 127.0
                };
                let value = am.m.lower + (am.m.upper - am.m.lower) * v;
                let (step, param) = (am.step, am.m.param);
                self.set_param(step, param, value);
            }
        }
    }

    fn run(&mut self, frames: usize, t: &Transport, midi: &[MidiEvent]) {
        let mb = self.max_block;
        for si in 0..self.steps.len() {
            // Sum inputs that have more than one source.
            for ch in 0..self.steps[si].sources.len() {
                if self.steps[si].sources[ch].len() > 1 {
                    let dst = self.buf(self.steps[si].mix[ch]);
                    let base = self.pool.as_ptr();
                    let d = unsafe { std::slice::from_raw_parts_mut(dst, frames) };
                    d.fill(0.0);
                    for &s in &self.steps[si].sources[ch] {
                        let src = unsafe { std::slice::from_raw_parts(base.add(s * mb), frames) };
                        d.iter_mut().zip(src).for_each(|(a, b)| *a += *b);
                    }
                }
            }
            let st = &self.steps[si];
            if st.kind != StepKind::Proc {
                continue;
            }
            let args = ProcessArgs {
                inputs: &st.in_ptrs,
                outputs: &st.out_ptrs,
                frames,
                midi_in: if st.gets_midi { midi } else { &[] },
                transport: t,
            };
            if st.cell.bypass.load(Ordering::Relaxed) {
                for (o, &op) in st.out_ptrs.iter().enumerate() {
                    let d = unsafe { std::slice::from_raw_parts_mut(op, frames) };
                    match st.in_ptrs.get(o) {
                        Some(&ip) => d.copy_from_slice(unsafe { std::slice::from_raw_parts(ip, frames) }),
                        None => d.fill(0.0),
                    }
                }
            } else if let Some(p) = unsafe { st.cell.processor() } {
                p.process(&args);
            }
        }
    }
}

/// State visible to the UI without locking.
pub struct Shared {
    pub cpu: AtomicU32,
    pub playing: AtomicBool,
    pub tempo: AtomicU64,
    pub rtz: AtomicBool,
    pub sample_pos: AtomicI64,
    pub callbacks: AtomicU64,
}

impl Shared {
    pub fn tempo(&self) -> f64 {
        f64::from_bits(self.tempo.load(Ordering::Relaxed))
    }
    pub fn set_tempo(&self, bpm: f64) {
        self.tempo.store(bpm.to_bits(), Ordering::Relaxed);
    }
}

pub struct AudioRuntime {
    plan: Box<Plan>,
    plan_rx: rtrb::Consumer<Box<Plan>>,
    garbage_tx: rtrb::Producer<Box<Plan>>,
    midi_rx: rtrb::Consumer<MidiEvent>,
    osc_rx: rtrb::Consumer<OscEvent>,
    shared: Arc<Shared>,
    pub sample_rate: f64,
    midi: Vec<MidiEvent>,
    load: f32,
}

impl AudioRuntime {
    fn take_new_plans(&mut self) {
        while let Ok(p) = self.plan_rx.pop() {
            let old = std::mem::replace(&mut self.plan, p);
            if let Err(rtrb::PushError::Full(old)) = self.garbage_tx.push(old) {
                // Never free on the audio thread; leaking a rare overflow is the lesser evil.
                std::mem::forget(old);
            }
        }
    }

    /// Runs the patch for one device callback. `ins`/`outs` point to `frames` f32 samples each.
    ///
    /// # Safety
    /// Every pointer must be valid for `frames` samples.
    pub unsafe fn process(&mut self, ins: &[*const f32], outs: &[*mut f32], frames: usize) {
        let started = Instant::now();
        AUDIO_THREAD.with(|c| c.set(true));
        set_denormals_off();
        self.take_new_plans();
        self.midi.clear();
        while let Ok(ev) = self.midi_rx.pop() {
            if self.midi.len() < MIDI_CAP {
                self.midi.push(ev);
            }
        }
        let plan = &mut *self.plan;
        plan.apply_midi_maps(&self.midi);
        while let Ok(o) = self.osc_rx.pop() {
            for k in 0..plan.osc.len() {
                let (step, hash, param) = plan.osc[k];
                if hash == o.hash {
                    plan.set_param(step, param, o.value);
                }
            }
        }
        let sh = &self.shared;
        if sh.rtz.swap(false, Ordering::Relaxed) {
            sh.sample_pos.store(0, Ordering::Relaxed);
        }
        let playing = sh.playing.load(Ordering::Relaxed);
        let mut t = Transport {
            playing,
            tempo: sh.tempo().clamp(1.0, 999.0),
            sample_pos: sh.sample_pos.load(Ordering::Relaxed),
            sample_rate: self.sample_rate,
            num: 4,
            den: 4,
        };
        let mb = plan.max_block;
        let mut done = 0;
        while done < frames {
            let n = mb.min(frames - done);
            for k in 0..plan.dev_in.len() {
                let d = std::slice::from_raw_parts_mut(plan.buf(plan.dev_in[k]), n);
                match ins.get(k) {
                    Some(&p) => d.copy_from_slice(std::slice::from_raw_parts(p.add(done), n)),
                    None => d.fill(0.0),
                }
            }
            let midi: &[MidiEvent] = if done == 0 { &self.midi } else { &[] };
            plan.run(n, &t, midi);
            let base = plan.pool.as_ptr();
            for (k, &op) in outs.iter().enumerate() {
                let d = std::slice::from_raw_parts_mut(op.add(done), n);
                match plan.dev_out.get(k) {
                    Some((srcs, mix)) if !srcs.is_empty() => {
                        let b = if srcs.len() == 1 { srcs[0] } else { *mix };
                        d.copy_from_slice(std::slice::from_raw_parts(base.add(b * mb), n));
                    }
                    _ => d.fill(0.0),
                }
            }
            if playing {
                t.sample_pos += n as i64;
            }
            done += n;
        }
        if playing {
            sh.sample_pos.store(t.sample_pos, Ordering::Relaxed);
        }
        let budget = frames as f64 / self.sample_rate;
        let used = started.elapsed().as_secs_f64();
        let inst = (used / budget.max(1e-6)) as f32;
        self.load = if inst > self.load { inst } else { self.load * 0.9 + inst * 0.1 };
        sh.cpu.store(self.load.to_bits(), Ordering::Relaxed);
        sh.callbacks.fetch_add(1, Ordering::Relaxed);
    }
}

#[inline]
fn set_denormals_off() {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    pub name: String,
    pub backend: String,
    pub sample_rate: f64,
    pub block: usize,
    pub in_names: Vec<String>,
    pub out_names: Vec<String>,
    pub active_ins: Vec<u32>,
    pub active_outs: Vec<u32>,
    pub rates: Vec<f64>,
    pub blocks: Vec<u32>,
    pub in_latency: u32,
    pub out_latency: u32,
}

pub enum Device {
    Asio(asio::AsioDevice),
    Cpal(wasapi::CpalDevice),
}

pub struct Engine {
    pub runtime: Arc<Mutex<AudioRuntime>>,
    plan_tx: rtrb::Producer<Box<Plan>>,
    garbage_rx: rtrb::Consumer<Box<Plan>>,
    pub midi_tx: Arc<Mutex<rtrb::Producer<MidiEvent>>>,
    pub osc_tx: Arc<Mutex<rtrb::Producer<OscEvent>>>,
    pub shared: Arc<Shared>,
    pub device: Option<Device>,
    pub info: Option<DeviceInfo>,
    pub running: bool,
}

impl Engine {
    pub fn new() -> Engine {
        let (plan_tx, plan_rx) = rtrb::RingBuffer::new(32);
        let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(64);
        let (midi_tx, midi_rx) = rtrb::RingBuffer::new(4096);
        let (osc_tx, osc_rx) = rtrb::RingBuffer::new(1024);
        let shared = Arc::new(Shared {
            cpu: AtomicU32::new(0),
            playing: AtomicBool::new(false),
            tempo: AtomicU64::new(120f64.to_bits()),
            rtz: AtomicBool::new(false),
            sample_pos: AtomicI64::new(0),
            callbacks: AtomicU64::new(0),
        });
        let rt = AudioRuntime {
            plan: Plan::build(&GraphDesc::default(), 512, 0, 0),
            plan_rx,
            garbage_tx,
            midi_rx,
            osc_rx,
            shared: shared.clone(),
            sample_rate: 44100.0,
            midi: Vec::with_capacity(MIDI_CAP),
            load: 0.0,
        };
        Engine {
            runtime: Arc::new(Mutex::new(rt)),
            plan_tx,
            garbage_rx,
            midi_tx: Arc::new(Mutex::new(midi_tx)),
            osc_tx: Arc::new(Mutex::new(osc_tx)),
            shared,
            device: None,
            info: None,
            running: false,
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.info.as_ref().map_or(44100.0, |i| i.sample_rate)
    }

    pub fn block(&self) -> usize {
        self.info.as_ref().map_or(512, |i| i.block)
    }

    pub fn active_ins(&self) -> usize {
        self.info.as_ref().map_or(2, |i| i.active_ins.len())
    }

    pub fn active_outs(&self) -> usize {
        self.info.as_ref().map_or(2, |i| i.active_outs.len())
    }

    pub fn cpu(&self) -> f32 {
        f32::from_bits(self.shared.cpu.load(Ordering::Relaxed))
    }

    /// Opens (but doesn't start) the configured device.
    pub fn open(&mut self, cfg: &AudioConfig, hwnd: isize) -> Result<DeviceInfo, String> {
        self.stop();
        let (dev, info) = if cfg.backend == "ASIO" {
            let (d, i) = asio::AsioDevice::open(cfg, hwnd, self.runtime.clone())?;
            (Device::Asio(d), i)
        } else {
            let (d, i) = wasapi::CpalDevice::open(cfg, self.runtime.clone())?;
            (Device::Cpal(d), i)
        };
        if let Ok(mut rt) = self.runtime.lock() {
            rt.sample_rate = info.sample_rate;
        }
        self.device = Some(dev);
        self.info = Some(info.clone());
        Ok(info)
    }

    pub fn start(&mut self) -> Result<(), String> {
        let r = match self.device.as_mut() {
            Some(Device::Asio(d)) => d.start(),
            Some(Device::Cpal(d)) => d.start(),
            None => Err("No audio device is open.".into()),
        };
        self.running = r.is_ok();
        r
    }

    pub fn stop(&mut self) {
        self.device = None;
        self.running = false;
        self.shared.cpu.store(0, Ordering::Relaxed);
    }

    pub fn close(&mut self) {
        self.stop();
        self.info = None;
    }

    pub fn control_panel(&self) {
        if let Some(Device::Asio(d)) = &self.device {
            d.control_panel();
        }
    }

    pub fn device_lost(&self) -> bool {
        match &self.device {
            Some(Device::Asio(_)) => asio::reset_requested(),
            Some(Device::Cpal(d)) => d.failed(),
            None => false,
        }
    }

    pub fn set_plan(&mut self, plan: Box<Plan>) {
        if self.running {
            if let Err(rtrb::PushError::Full(p)) = self.plan_tx.push(plan) {
                if let Ok(mut rt) = self.runtime.lock() {
                    rt.take_new_plans();
                    rt.plan = p;
                }
            }
        } else if let Ok(mut rt) = self.runtime.lock() {
            rt.take_new_plans();
            rt.plan = plan;
        }
        self.collect_garbage();
    }

    pub fn collect_garbage(&mut self) {
        while let Ok(p) = self.garbage_rx.pop() {
            drop(p);
        }
    }
}
