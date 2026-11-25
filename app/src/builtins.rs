//! Built-in processors. Names, descriptions and settings roots come from the strings at
//! 0x1404156c0 onward; parameters, saved settings and the audio maths from the functions listed
//! in MECHANICS.md.

use crate::engine::{ProcessArgs, Processor};
use crate::pdl::El;
use egui::vec2;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed, Ordering::Release};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

mod view;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Level,
    FilePlayer,
    OutputToggle,
    VuMeter,
    Recorder,
    Metronome,
    Looper,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Level, Kind::FilePlayer, Kind::OutputToggle, Kind::VuMeter, Kind::Recorder, Kind::Metronome, Kind::Looper];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Level => "Level",
            Kind::FilePlayer => "File Player",
            Kind::OutputToggle => "Output Toggle",
            Kind::VuMeter => "VU Meter",
            Kind::Recorder => "Audio Recorder",
            Kind::Metronome => "Metronome",
            Kind::Looper => "Looper",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Kind::Level => "Simple level processor.",
            Kind::FilePlayer => "Simple audio file player.",
            Kind::OutputToggle => "Simple output toggle processor.",
            Kind::VuMeter => "Simple VU Meter.",
            Kind::Recorder => "Simple audio recorder.",
            Kind::Metronome => "Simple metronome.",
            Kind::Looper => "Simple looper processor.",
        }
    }

    pub fn from_name(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.name().eq_ignore_ascii_case(s))
    }

    fn settings_root(self) -> Option<&'static str> {
        match self {
            Kind::Level => Some("Pedalboard2LevelSettings"),
            Kind::FilePlayer => Some("Pedalboard2FilePlayerSettings"),
            Kind::OutputToggle => Some("Pedalboard2OutputToggleSettings"),
            Kind::Recorder => Some("Pedalboard2RecorderSettings"),
            Kind::Metronome => Some("Pedalboard2MetronomeSettings"),
            Kind::Looper => Some("Pedalboard2LooperSettings"),
            Kind::VuMeter => None,
        }
    }

    /// Saved-setting keys the rebuild reads and writes. Any other attribute (the editor window
    /// bounds, the Looper's sound file) is kept and written back unchanged. "sync" and "loop" are
    /// what earlier test builds of this rebuild wrote.
    fn state_keys(self) -> &'static [&'static str] {
        match self {
            Kind::Level => &["level"],
            Kind::OutputToggle => &["toggle"],
            Kind::FilePlayer => &["file", "looping", "syncToMainTransport", "loop", "sync"],
            Kind::Recorder => &["file", "syncToMainTransport", "sync"],
            Kind::Metronome => &["syncToMainTransport", "numerator", "denominator", "accentFile", "clickFile", "sync"],
            Kind::Looper => &["syncToMainTransport", "stopAfterBar", "autoPlay", "barNumerator", "barDenominator", "inputLevel", "loopLevel"],
            Kind::VuMeter => &[],
        }
    }

    /// Audio (inputs, outputs) as each constructor sets them: File Player 0x140063fa0, Output
    /// Toggle 0x1400660e0, VU Meter 0x140067350, Recorder 0x1400682c0, Metronome 0x14006a0c0.
    pub fn io(self) -> (usize, usize) {
        match self {
            Kind::Level => (2, 2),
            Kind::FilePlayer => (0, 2),
            Kind::OutputToggle => (1, 2),
            Kind::VuMeter => (2, 0),
            Kind::Recorder => (2, 0),
            Kind::Metronome => (0, 1),
            Kind::Looper => (2, 2),
        }
    }

    /// Size of the control drawn inside the box: each built-in's getSize (the last slot of its
    /// class table, offset 0x138), read from the class tables named by their type records.
    pub fn control_size(self) -> egui::Vec2 {
        match self {
            Kind::Level => vec2(64.0, 64.0),               // 0x140062bd0
            Kind::OutputToggle => vec2(48.0, 48.0),        // 0x140062ca0
            Kind::VuMeter => vec2(64.0, 128.0),            // 0x140062d60
            Kind::Metronome => vec2(170.0, 100.0),         // 0x140062e20
            Kind::FilePlayer | Kind::Recorder | Kind::Looper => vec2(300.0, 100.0), // 0x140062dc0, shared
        }
    }

    /// Parameter names in the original's order (each class's getParameterName, e.g. the Output
    /// Toggle's 0x140062d00 and the Looper's 0x14006ef10), so MIDI and OSC mappings saved in its
    /// patches reach the same controls. The Metronome's two time-signature entries have no name.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            Kind::Level => &["Level"],
            Kind::FilePlayer => &["Play", "Return to Zero", "Looping", "Read Position", "Sync to Main Transport", "Trigger"],
            Kind::OutputToggle => &["Output"],
            Kind::VuMeter => &[],
            Kind::Recorder => &["Record", "Sync to Main Transport"],
            Kind::Metronome => &["Play", "", "", "Sync to Main Transport"],
            Kind::Looper => &[
                "Play/Pause",
                "Return to Zero",
                "Record",
                "Read Position",
                "Sync to Main Transport",
                "Stop After Bar",
                "Auto-play",
                "Bar Numerator",
                "Bar Denominator",
                "Input Level",
                "Loop Level",
            ],
        }
    }
}

/// State shared between a built-in's control (UI thread) and its processor (audio thread).
#[derive(Default)]
pub struct Shared {
    /// Level: the 0..1 dial (0.5 is unity). Looper: the input gain.
    pub level: AtomicU32,
    /// Looper: the loop gain.
    pub level2: AtomicU32,
    pub on: AtomicBool,
    pub record: AtomicBool,
    pub writing: AtomicBool,
    pub sync: AtomicBool,
    pub looping: AtomicBool,
    pub stop_bar: AtomicBool,
    pub auto_play: AtomicBool,
    pub rtz: AtomicBool,
    pub trigger: AtomicBool,
    /// A pending Read Position (the f32 bits of a 0..1 fraction), or `NO_SEEK`.
    pub seek: AtomicU32,
    pub peak: [AtomicU32; 2],
    pub fade: AtomicU32,
    pub pos: AtomicU64,
    pub len: AtomicU64,
    pub num: AtomicU32,
    pub den: AtomicU32,
    pub beat: AtomicU32,
    pub error: AtomicU32,
    /// Looper: the take's waveform as bins of [min L, max L, min R, max R] (f32 bits), each
    /// `wave_block` samples long; `wave_n` of them are filled.
    pub wave: Vec<[AtomicU32; 4]>,
    pub wave_n: AtomicUsize,
    pub wave_block: AtomicUsize,
}

/// Bins in the Looper's take waveform.
const WAVE_BINS: usize = 16384;
/// Bins in a loaded sound's overview.
pub const OVERVIEW_BINS: usize = 4096;
/// The Recorder's waveform keeps at most this many bins, merging pairs when full.
const PEAK_BINS: usize = 8192;
/// A bin that has seen no samples yet.
pub const EMPTY_BIN: [f32; 4] = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];

fn add_sample(b: &mut [f32; 4], l: f32, r: f32) {
    b[0] = b[0].min(l);
    b[1] = b[1].max(l);
    b[2] = b[2].min(r);
    b[3] = b[3].max(r);
}

pub fn merged(a: [f32; 4], b: &[f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].max(b[3])]
}

/// Samples per overview bin for a sound of `frames` samples.
pub fn overview_block(frames: usize) -> usize {
    frames.div_ceil(OVERVIEW_BINS).max(1)
}

/// The Recorder's waveform so far, built by its writer thread: one bin per `block` samples, the
/// last one still filling.
pub struct Peaks {
    pub block: usize,
    pub bins: Vec<[f32; 4]>,
    pub frames: u64,
    pub rate: f64,
    fill: usize,
    half: Option<f32>,
}

impl Peaks {
    fn new(rate: f64) -> Peaks {
        Peaks { block: 512, bins: Vec::new(), frames: 0, rate, fill: 0, half: None }
    }

    /// Interleaved stereo samples as the writer receives them.
    fn add(&mut self, samples: &[f32]) {
        for &s in samples {
            let Some(l) = self.half.take() else {
                self.half = Some(s);
                continue;
            };
            if self.fill == 0 {
                self.bins.push(EMPTY_BIN);
            }
            if let Some(b) = self.bins.last_mut() {
                add_sample(b, l, s);
            }
            self.fill += 1;
            self.frames += 1;
            if self.fill == self.block {
                self.fill = 0;
                if self.bins.len() >= PEAK_BINS {
                    self.bins = self.bins.chunks(2).map(|c| c.iter().fold(EMPTY_BIN, merged)).collect();
                    self.block *= 2;
                }
            }
        }
    }
}

const NO_SEEK: u32 = u32::MAX;
/// `Shared::error` value: the Looper ran out of room for its take.
const LOOPER_OUT_OF_MEMORY: u32 = 2;

fn getf(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Relaxed))
}

fn setf(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Relaxed);
}

fn b2f(on: bool) -> f32 {
    if on {
        1.0
    } else {
        0.0
    }
}

pub struct Clip {
    pub data: Vec<Vec<f32>>,
    pub rate: f64,
    pub frames: usize,
    /// [min L, max L, min R, max R] per `overview_block(frames)` samples; mono repeats L as R.
    pub overview: Vec<[f32; 4]>,
}

/// Reads a sound for the File Player or the Metronome. The original's file boxes offer
/// "*.wav;*.aif" (0x1402813a0), read by JUCE's WAV and AIFF formats; the file's own header
/// decides which reader is used here.
pub fn load_audio(path: &Path) -> Result<Clip, String> {
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).map_err(|e| e.to_string())?;
    let (data, rate) = if &head == b"FORM" { read_aiff(&std::fs::read(path).map_err(|e| e.to_string())?)? } else { read_wav(path)? };
    Ok(clip(data, rate))
}

fn read_wav(path: &Path) -> Result<(Vec<Vec<f32>>, f64), String> {
    let mut r = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = r.spec();
    let ch = (spec.channels as usize).max(1);
    let mut data = vec![Vec::with_capacity(r.duration() as usize); ch];
    match spec.sample_format {
        hound::SampleFormat::Float => {
            for (i, s) in r.samples::<f32>().enumerate() {
                data[i % ch].push(s.map_err(|e| e.to_string())?);
            }
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(8, 32) - 1)) as f32;
            for (i, s) in r.samples::<i32>().enumerate() {
                data[i % ch].push(s.map_err(|e| e.to_string())? as f32 * scale);
            }
        }
    }
    Ok((data, spec.sample_rate as f64))
}

/// AIFF, and AIFF-C when uncompressed ("NONE", "twos", "sowt", "fl32", "fl64"): the COMM chunk
/// gives channels, frames, bits and the rate (80-bit extended), SSND the big-endian samples.
fn read_aiff(b: &[u8]) -> Result<(Vec<Vec<f32>>, f64), String> {
    let bad = || "not a readable AIFF file".to_string();
    if b.len() < 12 || &b[..4] != b"FORM" {
        return Err(bad());
    }
    let aifc = match &b[8..12] {
        b"AIFF" => false,
        b"AIFC" => true,
        _ => return Err(bad()),
    };
    let u16_at = |o: usize| u16::from_be_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let (mut comm, mut ssnd) = (None, None);
    let mut pos = 12usize;
    while pos + 8 <= b.len() {
        let len = u32_at(pos + 4) as usize;
        let body = pos + 8;
        let end = body.saturating_add(len).min(b.len());
        match &b[pos..pos + 4] {
            b"COMM" if end - body >= 18 => {
                let tag: [u8; 4] = if aifc && end - body >= 22 { [b[body + 18], b[body + 19], b[body + 20], b[body + 21]] } else { *b"NONE" };
                comm = Some((u16_at(body) as usize, u32_at(body + 2) as usize, u16_at(body + 6) as usize, extended(&b[body + 8..body + 18]), tag));
            }
            b"SSND" if end - body >= 8 => ssnd = Some((body + 8).saturating_add(u32_at(body) as usize).min(end)..end),
            _ => {}
        }
        pos = body.saturating_add(len).saturating_add(len & 1);
    }
    let ((ch, frames, bits, rate, tag), data) = comm.zip(ssnd).ok_or_else(bad)?;
    if ch == 0 || !(rate.is_finite() && rate > 0.0) {
        return Err(bad());
    }
    let (bytes, read): (usize, fn(&[u8]) -> f32) = match (&tag, bits) {
        (b"NONE" | b"twos", 1..=8) => (1, |s| s[0] as i8 as f32 / 128.0),
        (b"NONE" | b"twos", 9..=16) => (2, |s| i16::from_be_bytes([s[0], s[1]]) as f32 / 32768.0),
        (b"NONE" | b"twos", 17..=24) => (3, |s| i32::from_be_bytes([s[0], s[1], s[2], 0]) as f32 / 2147483648.0),
        (b"NONE" | b"twos", 25..=32) => (4, |s| i32::from_be_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2147483648.0),
        (b"sowt", 9..=16) => (2, |s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0),
        (b"sowt", 17..=24) => (3, |s| i32::from_le_bytes([0, s[0], s[1], s[2]]) as f32 / 2147483648.0),
        (b"sowt", 25..=32) => (4, |s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2147483648.0),
        (b"fl32" | b"FL32", _) => (4, |s| f32::from_be_bytes([s[0], s[1], s[2], s[3]])),
        (b"fl64" | b"FL64", _) => (8, |s| f64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f32),
        _ => return Err(format!("unsupported AIFF sample format ({}, {bits} bits)", String::from_utf8_lossy(&tag))),
    };
    let frame = ch * bytes;
    let frames = frames.min(data.len() / frame);
    let mut out = vec![Vec::with_capacity(frames); ch];
    for f in b[data.start..data.start + frames * frame].chunks_exact(frame) {
        for (c, s) in f.chunks_exact(bytes).enumerate() {
            out[c].push(read(s));
        }
    }
    Ok((out, rate))
}

/// An 80-bit IEEE 754 extended number, as AIFF stores its sample rate.
fn extended(b: &[u8]) -> f64 {
    let exp = (((b[0] & 0x7f) as i32) << 8 | b[1] as i32) - 16383;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    let v = mant as f64 * 2f64.powi(exp - 63);
    if b[0] & 0x80 != 0 { -v } else { v }
}

fn clip(data: Vec<Vec<f32>>, rate: f64) -> Clip {
    let ch = data.len().max(1);
    let frames = data.iter().map(|d| d.len()).min().unwrap_or(0);
    let block = overview_block(frames);
    let overview = if frames == 0 {
        Vec::new()
    } else {
        let (l, r) = (&data[0], &data[1.min(ch - 1)]);
        (0..frames.div_ceil(block))
            .map(|b| {
                let mut m = EMPTY_BIN;
                for k in b * block..((b + 1) * block).min(frames) {
                    add_sample(&mut m, l[k], r[k]);
                }
                m
            })
            .collect()
    };
    Clip { data, rate, frames, overview }
}

/// A sound handed to a processor: (slot, sound). The File Player uses slot 0; the Metronome its
/// accent (0) and click (1) sounds, where `None` goes back to the beep.
type ClipMsg = (u8, Option<Arc<Clip>>);

/// UI-side handle of a built-in processor.
pub struct Builtin {
    pub kind: Kind,
    pub sh: Arc<Shared>,
    /// The File Player's sound file or the Recorder's target file.
    pub file: String,
    /// The Metronome's accent and click sound files ("" for the beep).
    pub sounds: [String; 2],
    clips: [Option<Arc<Clip>>; 2],
    retired: Vec<Arc<Clip>>,
    clip_tx: Option<rtrb::Producer<ClipMsg>>,
    rec_cons: Option<rtrb::Consumer<f32>>,
    rec_thread: Option<(JoinHandle<rtrb::Consumer<f32>>, Arc<AtomicBool>)>,
    /// Saved settings the rebuild doesn't use, written back as they were read.
    extra: Vec<(String, String)>,
    /// (title, message) for the app to show.
    pub error: Option<(String, String)>,
    /// The device sample rate, refreshed every frame.
    pub rate: f64,
    /// The Recorder's waveform of what it has written.
    pub peaks: Arc<Mutex<Peaks>>,
}

const FILE_BUSY: &str = "Have you got the file open elsewhere? (e.g. in another File Player)";

pub fn create(kind: Kind) -> (Builtin, Box<dyn Processor>) {
    let sh = Arc::new(if kind == Kind::Looper { Shared { wave: (0..WAVE_BINS).map(|_| Default::default()).collect(), ..Default::default() } } else { Shared::default() });
    // Level: 0.5 is unity. Looper: input and loop gains of 1, bar 4/4, auto-play on.
    setf(&sh.level, if kind == Kind::Looper { 1.0 } else { 0.5 });
    setf(&sh.level2, 1.0);
    sh.num.store(4, Relaxed);
    sh.den.store(4, Relaxed);
    sh.auto_play.store(true, Relaxed);
    sh.seek.store(NO_SEEK, Relaxed);
    let mut b = Builtin {
        kind,
        sh: sh.clone(),
        file: String::new(),
        sounds: Default::default(),
        clips: [None, None],
        retired: Vec::new(),
        clip_tx: None,
        rec_cons: None,
        rec_thread: None,
        extra: Vec::new(),
        error: None,
        rate: 44100.0,
        peaks: Arc::new(Mutex::new(Peaks::new(44100.0))),
    };
    let p: Box<dyn Processor> = match kind {
        Kind::Level => Box::new(LevelProc { sh }),
        Kind::OutputToggle => Box::new(ToggleProc { sh, fade: 0.0 }),
        Kind::VuMeter => Box::new(VuProc { sh, peak: [0.0; 2] }),
        Kind::FilePlayer => {
            let (tx, rx) = rtrb::RingBuffer::new(8);
            b.clip_tx = Some(tx);
            Box::new(PlayerProc { sh, rx, clip: None, pos: 0.0, sr: 44100.0 })
        }
        Kind::Recorder => {
            let (tx, rx) = rtrb::RingBuffer::new(48000 * 2 * 6);
            b.rec_cons = Some(rx);
            Box::new(RecorderProc { sh, tx })
        }
        Kind::Metronome => {
            let (tx, rx) = rtrb::RingBuffer::new(8);
            b.clip_tx = Some(tx);
            Box::new(MetronomeProc {
                sh,
                rx,
                sounds: [None, None],
                voice: [f64::INFINITY; 2],
                sr: 44100.0,
                count: 0.0,
                beat: 0,
                last: -1,
                env: 0.0,
                x: 0.0,
                y: 0.0,
                coef: 0.0,
            })
        }
        Kind::Looper => Box::new(LooperProc::new(sh)),
    };
    (b, p)
}

impl Builtin {
    /// getParameter, as decoded for every built-in (vtable slot 19: Level 0x140062c40, File Player
    /// 0x140064ce0, Output Toggle 0x140062d40, Recorder 0x140069060, Metronome 0x14006b110, Looper
    /// 0x14006f4d0): buttons read 0, switches 1 or 0, the bar as numbers, Looper levels as gain / 2.
    pub fn param_value(&self, i: i32) -> f32 {
        let s = &self.sh;
        let b = |x: &AtomicBool| b2f(x.load(Relaxed));
        match (self.kind, i) {
            (Kind::Level, 0) => getf(&s.level),
            (Kind::OutputToggle, 0) => b(&s.on),
            (Kind::FilePlayer, 2) => b(&s.looping),
            (Kind::FilePlayer | Kind::Looper, 4) | (Kind::Recorder, 1) | (Kind::Metronome, 3) => b(&s.sync),
            (Kind::Looper, 5) => b(&s.stop_bar),
            (Kind::Looper, 6) => b(&s.auto_play),
            (Kind::Metronome, 1) | (Kind::Looper, 7) => s.num.load(Relaxed) as f32,
            (Kind::Metronome, 2) | (Kind::Looper, 8) => s.den.load(Relaxed) as f32,
            (Kind::Looper, 9) => getf(&s.level) * 0.5,
            (Kind::Looper, 10) => getf(&s.level2) * 0.5,
            _ => 0.0,
        }
    }

    /// Same meaning as the processor's `set_parameter`, from the UI thread.
    pub fn set_param(&mut self, i: i32, v: f32) {
        apply_param(self.kind, &self.sh, i, v);
    }

    /// The settings element under the original's names (each class's getStateInformation);
    /// switches are written as 1 or 0.
    pub fn get_state(&self) -> Option<El> {
        let root = self.kind.settings_root()?;
        let s = &self.sh;
        let b = |x: &AtomicBool| x.load(Relaxed) as i32;
        let mut e = El::new(root);
        match self.kind {
            Kind::Level => e.set("level", getf(&s.level)),
            Kind::OutputToggle => e.set("toggle", b(&s.on)),
            Kind::FilePlayer => {
                e.set("file", &self.file);
                e.set("looping", b(&s.looping));
                e.set("syncToMainTransport", b(&s.sync));
            }
            Kind::Recorder => {
                e.set("file", &self.file);
                e.set("syncToMainTransport", b(&s.sync));
            }
            Kind::Metronome => {
                e.set("syncToMainTransport", b(&s.sync));
                e.set("numerator", s.num.load(Relaxed));
                e.set("denominator", s.den.load(Relaxed));
                e.set("accentFile", &self.sounds[0]);
                e.set("clickFile", &self.sounds[1]);
            }
            Kind::Looper => {
                e.set("syncToMainTransport", b(&s.sync));
                e.set("stopAfterBar", b(&s.stop_bar));
                e.set("autoPlay", b(&s.auto_play));
                e.set("barNumerator", s.num.load(Relaxed));
                e.set("barDenominator", s.den.load(Relaxed));
                e.set("inputLevel", getf(&s.level));
                e.set("loopLevel", getf(&s.level2));
            }
            Kind::VuMeter => {}
        }
        for (k, v) in &self.extra {
            if e.get(k).is_none() {
                e.set(k, v);
            }
        }
        Some(e)
    }

    /// Reads the settings as each class's setStateInformation does: missing switches are off
    /// (the Looper's auto-play on), missing numbers 0 and missing Looper levels a gain of 0.5.
    pub fn set_state(&mut self, e: &El) {
        let s = self.sh.clone();
        let keys = self.kind.state_keys();
        self.extra = e.attrs.iter().filter(|(k, _)| !keys.iter().any(|o| o.eq_ignore_ascii_case(k))).cloned().collect();
        let sync = e.get_b("syncToMainTransport", e.get_b("sync", false));
        // A bar of 0 beats would never end; the rebuild keeps it at 1 to 32.
        let bar = |k: &str| e.get_i(k, 0).clamp(1, 32) as u32;
        match self.kind {
            Kind::Level => setf(&s.level, e.get_f("level", 0.5) as f32),
            Kind::OutputToggle => s.on.store(e.get_b("toggle", false), Relaxed),
            Kind::FilePlayer => {
                s.looping.store(e.get_b("looping", e.get_b("loop", false)), Relaxed);
                s.sync.store(sync, Relaxed);
                let f = e.get("file").unwrap_or_default().to_string();
                if !f.is_empty() {
                    if let Err(m) = self.load_file(Path::new(&f)) {
                        self.file = f;
                        self.error = Some(("File Player Error".into(), m));
                    }
                }
            }
            Kind::Recorder => {
                s.sync.store(sync, Relaxed);
                self.file = e.get("file").unwrap_or_default().to_string();
            }
            Kind::Metronome => {
                s.sync.store(sync, Relaxed);
                s.num.store(bar("numerator"), Relaxed);
                s.den.store(bar("denominator"), Relaxed);
                for (slot, key) in [(0, "accentFile"), (1, "clickFile")] {
                    let f = e.get(key).unwrap_or_default().to_string();
                    let res = if f.is_empty() { self.set_sound(slot, None) } else { self.set_sound(slot, Some(Path::new(&f))) };
                    if let Err(m) = res {
                        self.sounds[slot] = f;
                        self.error = Some(("Metronome Error".into(), m));
                    }
                }
            }
            Kind::Looper => {
                s.sync.store(sync, Relaxed);
                s.stop_bar.store(e.get_b("stopAfterBar", false), Relaxed);
                s.auto_play.store(e.get_b("autoPlay", true), Relaxed);
                s.num.store(bar("barNumerator"), Relaxed);
                s.den.store(bar("barDenominator"), Relaxed);
                setf(&s.level, e.get_f("inputLevel", 0.5) as f32);
                setf(&s.level2, e.get_f("loopLevel", 0.5) as f32);
            }
            Kind::VuMeter => {}
        }
    }

    /// Hands a sound to the processor; the old one is freed here once the audio thread lets go.
    fn send_clip(&mut self, slot: usize, clip: Option<Arc<Clip>>) -> Result<(), String> {
        let tx = self.clip_tx.as_mut().ok_or("This box has no sound to set")?;
        tx.push((slot as u8, clip.clone())).map_err(|_| "The audio engine is busy; please try again.".to_string())?;
        if let Some(old) = std::mem::replace(&mut self.clips[slot], clip) {
            self.retired.push(old);
        }
        Ok(())
    }

    pub fn load_file(&mut self, path: &Path) -> Result<(), String> {
        let clip = Arc::new(load_audio(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?);
        self.send_clip(0, Some(clip))?;
        self.file = path.display().to_string();
        Ok(())
    }

    /// Sets the Metronome's accent (0) or click (1) sound; `None` goes back to the beep.
    pub fn set_sound(&mut self, slot: usize, path: Option<&Path>) -> Result<(), String> {
        if path.is_none() && self.sounds[slot].is_empty() && self.clips[slot].is_none() {
            return Ok(());
        }
        let clip = match path {
            Some(p) => Some(Arc::new(load_audio(p).map_err(|e| format!("Could not open {}: {e}", p.display()))?)),
            None => None,
        };
        self.send_clip(slot, clip)?;
        self.sounds[slot] = path.map(|p| p.display().to_string()).unwrap_or_default();
        Ok(())
    }

    /// Starts the Recorder's file: 16-bit stereo WAV at the device rate, like the original's
    /// writer (0x140068610 asks for 16 bits and 2 channels).
    fn start_writer(&mut self, sample_rate: f64) -> Result<(), String> {
        let mut cons = self.rec_cons.take().ok_or("Recorder busy")?;
        while let Ok(c) = cons.read_chunk(cons.slots()) {
            if c.is_empty() {
                break;
            }
            c.commit_all();
        }
        let spec = hound::WavSpec { channels: 2, sample_rate: sample_rate as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = match hound::WavWriter::create(&self.file, spec) {
            Ok(w) => w,
            Err(_) => {
                self.rec_cons = Some(cons);
                return Err(format!("Could not create {}.\n{FILE_BUSY}", self.file));
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        *self.peaks.lock().unwrap_or_else(|e| e.into_inner()) = Peaks::new(sample_rate);
        let peaks = self.peaks.clone();
        let h = std::thread::spawn(move || {
            loop {
                let n = cons.slots();
                if n > 0 {
                    if let Ok(c) = cons.read_chunk(n) {
                        let (a, b) = c.as_slices();
                        if let Ok(mut pk) = peaks.lock() {
                            pk.add(a);
                            pk.add(b);
                        }
                        for &s in a.iter().chain(b) {
                            let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32767.0).round() as i16);
                        }
                        c.commit_all();
                    }
                } else if stop2.load(Relaxed) {
                    break;
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            let _ = w.finalize();
            cons
        });
        self.rec_thread = Some((h, stop));
        self.sh.writing.store(true, Relaxed);
        Ok(())
    }

    fn stop_writer(&mut self) {
        self.sh.writing.store(false, Relaxed);
        if let Some((h, stop)) = self.rec_thread.take() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            stop.store(true, Relaxed);
            if let Ok(c) = h.join() {
                self.rec_cons = Some(c);
            }
        }
    }

    /// Housekeeping on the UI thread, once per frame.
    pub fn tick(&mut self, sample_rate: f64) {
        self.rate = sample_rate;
        self.retired.retain(|c| Arc::strong_count(c) > 1);
        match self.kind {
            Kind::Recorder => {
                let want = self.sh.record.load(Relaxed);
                if want && self.rec_thread.is_none() {
                    if self.file.is_empty() {
                        self.sh.record.store(false, Relaxed);
                        self.error = Some(("Audio Recorder".into(), "Choose a file to record to first.".into()));
                    } else if let Err(m) = self.start_writer(sample_rate) {
                        self.sh.record.store(false, Relaxed);
                        self.error = Some(("Audio Recorder Error".into(), m));
                    }
                } else if !want && self.rec_thread.is_some() {
                    self.stop_writer();
                }
            }
            Kind::Looper => {
                if self.sh.error.swap(0, Relaxed) == LOOPER_OUT_OF_MEMORY {
                    self.error = Some(("Looper Error".into(), "Not enough memory to continue. Recording stopped.".into()));
                }
            }
            _ => {}
        }
    }
}

impl Drop for Builtin {
    fn drop(&mut self) {
        self.stop_writer();
    }
}

/// Each built-in's setParameter. Buttons act above 0.5 (DAT_14042ee44) and Return to Zero on any
/// value; the Looper's is 0x14006f5a0, the Output Toggle's 0x140066680.
fn apply_param(kind: Kind, s: &Shared, i: i32, v: f32) {
    let on = v > 0.5;
    let flip = |a: &AtomicBool| {
        if on {
            a.fetch_xor(true, Relaxed);
        }
    };
    // A bar of 0 beats would never end; the rebuild keeps it at 1 to 32.
    let bar = |a: &AtomicU32| a.store((v as i32).clamp(1, 32) as u32, Relaxed);
    match (kind, i) {
        (Kind::Level, 0) => setf(&s.level, v.clamp(0.0, 1.0)),
        (Kind::OutputToggle, 0) => s.on.store(on, Relaxed),
        (Kind::FilePlayer | Kind::Metronome | Kind::Looper, 0) => flip(&s.on),
        (Kind::Recorder, 0) => flip(&s.record),
        (Kind::FilePlayer | Kind::Looper, 1) => s.rtz.store(true, Relaxed),
        (Kind::FilePlayer, 2) => s.looping.store(on, Relaxed),
        (Kind::FilePlayer | Kind::Looper, 3) => s.seek.store(v.clamp(0.0, 1.0).to_bits(), Relaxed),
        (Kind::FilePlayer | Kind::Looper, 4) | (Kind::Recorder, 1) | (Kind::Metronome, 3) => s.sync.store(on, Relaxed),
        (Kind::FilePlayer, 5) => {
            if on {
                s.trigger.store(true, Relaxed);
            }
        }
        (Kind::Metronome, 1) | (Kind::Looper, 7) => bar(&s.num),
        (Kind::Metronome, 2) | (Kind::Looper, 8) => bar(&s.den),
        (Kind::Looper, 2) => {
            if on {
                // Record stops the loop playing, then starts a new take or ends the one running.
                s.on.store(false, Relaxed);
                s.record.fetch_xor(true, Relaxed);
            }
        }
        (Kind::Looper, 5) => s.stop_bar.store(on, Relaxed),
        (Kind::Looper, 6) => s.auto_play.store(on, Relaxed),
        (Kind::Looper, 9) => setf(&s.level, v.clamp(0.0, 1.0) * 2.0),
        (Kind::Looper, 10) => setf(&s.level2, v.clamp(0.0, 1.0) * 2.0),
        _ => {}
    }
}

// ---------------------------------------------------------------- processors

struct LevelProc {
    sh: Arc<Shared>,
}

impl Processor for LevelProc {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn process(&mut self, a: &ProcessArgs) {
        // 0x140063200: out = in * level * 2 (0.5 is unity).
        let g = getf(&self.sh.level) * 2.0;
        for ch in 0..a.outputs.len().min(a.inputs.len()) {
            let (i, o) = (a.input(ch), a.output(ch));
            o.iter_mut().zip(i).for_each(|(o, i)| *o = *i * g);
        }
    }
    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::Level, &self.sh, i, v);
    }
}

struct ToggleProc {
    sh: Arc<Shared>,
    fade: f32,
}

impl Processor for ToggleProc {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn process(&mut self, a: &ProcessArgs) {
        // 0x140066410: crossfade between the two outputs, 0.001 per sample.
        let target = if self.sh.on.load(Relaxed) { 1.0 } else { 0.0 };
        let (i, o1, o2) = (a.input(0), a.output(0), a.output(1));
        for k in 0..a.frames {
            if self.fade < target {
                self.fade = (self.fade + 0.001).min(1.0);
            } else if self.fade > target {
                self.fade = (self.fade - 0.001).max(0.0);
            }
            let x = i[k];
            o1[k] = x * (1.0 - self.fade);
            o2[k] = x * self.fade;
        }
        setf(&self.sh.fade, self.fade);
    }
    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::OutputToggle, &self.sh, i, v);
    }
}

struct VuProc {
    sh: Arc<Shared>,
    peak: [f32; 2],
}

impl Processor for VuProc {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn process(&mut self, a: &ProcessArgs) {
        // 0x1400676b0: peak follower that falls 1e-5 per sample.
        for ch in 0..a.inputs.len().min(2) {
            let mut p = self.peak[ch];
            for &x in a.input(ch) {
                let v = x.abs();
                p = if v > p { v } else { (p - 1e-5).max(0.0) };
            }
            self.peak[ch] = p;
            setf(&self.sh.peak[ch], p);
        }
    }
}

struct PlayerProc {
    sh: Arc<Shared>,
    rx: rtrb::Consumer<ClipMsg>,
    clip: Option<Arc<Clip>>,
    pos: f64,
    sr: f64,
}

impl Processor for PlayerProc {
    fn prepare(&mut self, sr: f64, _: usize) {
        self.sr = sr;
    }
    fn process(&mut self, a: &ProcessArgs) {
        while let Ok((_, c)) = self.rx.pop() {
            self.clip = c;
            self.pos = 0.0;
        }
        let s = &self.sh;
        if s.rtz.swap(false, Relaxed) {
            self.pos = 0.0;
        }
        if s.trigger.swap(false, Relaxed) {
            // Trigger: back to the start, playing.
            self.pos = 0.0;
            s.on.store(true, Relaxed);
        }
        let seek = s.seek.swap(NO_SEEK, Relaxed);
        let sync = s.sync.load(Relaxed);
        let looping = s.looping.load(Relaxed);
        let mut playing = if sync { a.transport.playing } else { s.on.load(Relaxed) };
        let (o0, o1) = (a.output(0), a.output(1));
        let Some(clip) = self.clip.as_ref().filter(|c| c.frames > 1) else {
            o0.fill(0.0);
            o1.fill(0.0);
            return;
        };
        let ratio = clip.rate / self.sr.max(1.0);
        let frames = clip.frames as f64;
        if seek != NO_SEEK {
            // Read Position: a fraction of the file.
            self.pos = f32::from_bits(seek).clamp(0.0, 1.0) as f64 * frames;
        }
        if sync {
            self.pos = a.transport.sample_pos as f64 * ratio;
            if looping {
                self.pos %= frames;
            }
        }
        let nch = clip.data.len();
        let (l, r) = (&clip.data[0], &clip.data[1.min(nch - 1)]);
        for k in 0..a.frames {
            if !playing {
                o0[k] = 0.0;
                o1[k] = 0.0;
                continue;
            }
            if self.pos >= frames - 1.0 {
                if looping {
                    self.pos = 0.0;
                } else {
                    playing = false;
                    if !sync {
                        s.on.store(false, Relaxed);
                        self.pos = 0.0;
                    }
                    o0[k] = 0.0;
                    o1[k] = 0.0;
                    continue;
                }
            }
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            o0[k] = l[i] + (l[i + 1] - l[i]) * f;
            o1[k] = r[i] + (r[i + 1] - r[i]) * f;
            self.pos += ratio;
        }
        s.pos.store(self.pos as u64, Relaxed);
    }
    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::FilePlayer, &self.sh, i, v);
    }
}

struct RecorderProc {
    sh: Arc<Shared>,
    tx: rtrb::Producer<f32>,
}

impl Processor for RecorderProc {
    fn prepare(&mut self, _: f64, _: usize) {}
    fn process(&mut self, a: &ProcessArgs) {
        let s = &self.sh;
        let go = s.record.load(Relaxed) && s.writing.load(Relaxed) && (!s.sync.load(Relaxed) || a.transport.playing);
        if !go || a.inputs.is_empty() {
            return;
        }
        let (l, r) = (a.input(0), a.input(1.min(a.inputs.len() - 1)));
        let n = (a.frames * 2).min(self.tx.slots() / 2 * 2);
        if let Ok(c) = self.tx.write_chunk_uninit(n) {
            c.fill_from_iter((0..n / 2).flat_map(|k| [l[k], r[k]]));
        }
    }
    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::Recorder, &self.sh, i, v);
    }
}

struct MetronomeProc {
    sh: Arc<Shared>,
    rx: rtrb::Consumer<ClipMsg>,
    /// The accent and click sound files, and where each is playing (in its own samples).
    sounds: [Option<Arc<Clip>>; 2],
    voice: [f64; 2],
    sr: f64,
    count: f64,
    beat: u32,
    last: i64,
    env: f32,
    x: f32,
    y: f32,
    coef: f32,
}

impl Processor for MetronomeProc {
    fn prepare(&mut self, sr: f64, _: usize) {
        self.sr = sr;
    }
    fn process(&mut self, a: &ProcessArgs) {
        // 0x14006ab90: one beat = (4/den)·60/bpm; 1760 Hz accent on beat 1, 880 Hz otherwise;
        // magic-circle sine with an envelope falling 1e-4 per sample. A chosen accent or click
        // file plays from its start in place of the beep, read from its first channel.
        while let Ok((slot, c)) = self.rx.pop() {
            let i = (slot as usize).min(1);
            self.sounds[i] = c;
            self.voice[i] = f64::INFINITY;
        }
        if a.outputs.is_empty() {
            return;
        }
        let o = a.output(0);
        let s = &self.sh;
        let t = a.transport;
        let sr = self.sr;
        let num = s.num.load(Relaxed).max(1);
        let den = s.den.load(Relaxed).max(1);
        let beat_len = (4.0 / den as f64) * 60.0 / t.tempo.max(1.0) * sr;
        let sync = s.sync.load(Relaxed);
        let playing = if sync { t.playing } else { s.on.load(Relaxed) };
        if !playing {
            self.count = 0.0;
            self.beat = 0;
            self.last = -1;
        }
        for k in 0..a.frames {
            let mut trigger = None;
            if playing {
                if sync {
                    let b = ((t.sample_pos as f64 + k as f64) / beat_len).floor() as i64;
                    if b != self.last {
                        self.last = b;
                        let in_bar = b.rem_euclid(num as i64) as u32;
                        trigger = Some(in_bar == 0);
                        s.beat.store(in_bar, Relaxed);
                    }
                } else {
                    if self.count <= 0.0 {
                        trigger = Some(self.beat == 0);
                        s.beat.store(self.beat, Relaxed);
                        self.beat = (self.beat + 1) % num;
                        self.count += beat_len;
                    }
                    self.count -= 1.0;
                }
            }
            if let Some(accent) = trigger {
                let slot = if accent { 0 } else { 1 };
                if self.sounds[slot].is_some() {
                    self.voice[slot] = 0.0;
                } else {
                    let f = if accent { 1760.0 } else { 880.0 };
                    self.coef = (2.0 * (std::f64::consts::PI * f / sr).sin()) as f32;
                    self.env = 1.0;
                    self.x = 1.0;
                    self.y = 0.0;
                }
            }
            let mut v = if self.env > 0.0 {
                self.x -= self.coef * self.y;
                self.y += self.coef * self.x;
                let o = self.y * self.env;
                self.env -= 1e-4;
                o
            } else {
                0.0
            };
            for (snd, at) in self.sounds.iter().zip(self.voice.iter_mut()) {
                if let Some(c) = snd {
                    if *at < c.frames as f64 - 1.0 {
                        let i = *at as usize;
                        let d = &c.data[0];
                        v += d[i] + (d[i + 1] - d[i]) * (*at - i as f64) as f32;
                        *at += c.rate / sr;
                    }
                }
            }
            o[k] = v;
        }
    }
    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::Metronome, &self.sh, i, v);
    }
}

/// Longest loop, in seconds: the rebuild's own limit. The original streams its loop to a file
/// through a threaded writer (0x1400accf0), so only the disk limits it; the rebuild keeps loops in
/// memory, allocated here so the audio thread never allocates or waits on the disk.
const LOOP_SECONDS: f64 = 120.0;
/// Crossfade length at each end of the loop, in samples (0x14006dce0).
const XFADE: usize = 128;

struct LooperProc {
    sh: Arc<Shared>,
    buf: [Vec<f32>; 2],
    len: usize,
    pos: usize,
    sr: f64,
    recording: bool,
    /// Stop After Bar: seconds left in the current beat, seconds per sample, beats begun.
    remaining: f64,
    sps: f64,
    beats: u32,
    /// The last 128 raw input samples (circular), with the copies taken just before the take
    /// began (pre-roll) and just after it ended (post-roll).
    ring: [[f32; XFADE]; 2],
    ring_at: usize,
    pre: [[f32; XFADE]; 2],
    post: [[f32; XFADE]; 2],
    /// Samples gathered into the post-roll since the take ended; -1 once it's complete.
    post_count: i32,
    /// The loop fades in over the first stretch it plays after an auto-played take.
    ramp_pending: bool,
    /// The take's waveform bin being gathered, samples in it, samples per bin, bins stored.
    wacc: [f32; 4],
    wfill: usize,
    wblock: usize,
    wn: usize,
}

/// Publishes the Looper's gathered waveform bin (no allocation: the bins were made up front).
fn store_bin(s: &Shared, wn: &mut usize, acc: &mut [f32; 4], fill: &mut usize) {
    if let Some(slot) = s.wave.get(*wn) {
        for (a, v) in slot.iter().zip(*acc) {
            a.store(v.to_bits(), Relaxed);
        }
        *wn += 1;
        s.wave_n.store(*wn, Release);
    }
    *acc = EMPTY_BIN;
    *fill = 0;
}

impl LooperProc {
    fn new(sh: Arc<Shared>) -> LooperProc {
        LooperProc {
            sh,
            buf: [Vec::new(), Vec::new()],
            len: 0,
            pos: 0,
            sr: 44100.0,
            recording: false,
            remaining: 0.0,
            sps: 0.0,
            beats: 0,
            ring: [[0.0; XFADE]; 2],
            ring_at: 0,
            pre: [[0.0; XFADE]; 2],
            post: [[0.0; XFADE]; 2],
            post_count: -1,
            ramp_pending: false,
            wacc: EMPTY_BIN,
            wfill: 0,
            wblock: 1,
            wn: 0,
        }
    }
}

impl Processor for LooperProc {
    fn prepare(&mut self, sr: f64, _: usize) {
        self.sr = sr;
        let cap = (sr * LOOP_SECONDS) as usize;
        if self.buf[0].len() != cap {
            self.buf = [vec![0.0; cap], vec![0.0; cap]];
            self.len = 0;
            self.pos = 0;
            self.wn = 0;
            self.sh.wave_n.store(0, Release);
        }
        self.wblock = cap.div_ceil(WAVE_BINS).max(1);
        self.sh.wave_block.store(self.wblock, Relaxed);
    }

    /// 0x14006dce0, with the take start of the record thread (0x14006d060).
    fn process(&mut self, a: &ProcessArgs) {
        let s = &self.sh;
        let n = a.frames;
        let cap = self.buf[0].len();
        if s.rtz.swap(false, Relaxed) {
            self.pos = 0;
        }
        let seek = s.seek.swap(NO_SEEK, Relaxed);
        if seek != NO_SEEK && self.len > 0 {
            self.pos = ((f32::from_bits(seek).clamp(0.0, 1.0) as f64 * self.len as f64) as usize).min(self.len - 1);
        }
        let want = s.record.load(Relaxed);
        let mut stop_at = None;
        if want && !self.recording {
            // A new take: counters to zero, the 128 samples before it become the pre-roll, and
            // the loop stops playing.
            self.recording = true;
            self.len = 0;
            self.pos = 0;
            self.remaining = 0.0;
            self.sps = 0.0;
            self.beats = 0;
            self.post_count = -1;
            self.wn = 0;
            self.wfill = 0;
            self.wacc = EMPTY_BIN;
            s.wave_n.store(0, Release);
            if s.auto_play.load(Relaxed) {
                self.ramp_pending = true;
            }
            for ch in 0..2 {
                for i in 0..XFADE {
                    self.pre[ch][i] = self.ring[ch][(self.ring_at + i) % XFADE];
                }
            }
            s.on.store(false, Relaxed);
        } else if !want && self.recording {
            stop_at = Some(0);
        }
        let mut playing = s.on.load(Relaxed);
        let in_gain = getf(&s.level);
        let loop_gain = getf(&s.level2);
        let stop_bar = s.stop_bar.load(Relaxed);
        let auto = s.auto_play.load(Relaxed);
        let num = s.num.load(Relaxed).max(1);
        let den = s.den.load(Relaxed).max(1);
        let bpm = if a.transport.tempo > 0.0 { a.transport.tempo } else { 120.0 };
        let beat_secs = (4.0 / den as f64) * (60.0 / bpm);
        // Fade-in of the loop: (first sample, length); none unless it's due.
        let mut ramp = (usize::MAX, 1usize);
        if playing && !self.recording && self.len > 0 && self.ramp_pending {
            self.ramp_pending = false;
            ramp = (0, n.max(1));
        }
        let (i0, i1) = (a.input(0), a.input(1));
        let (o0, o1) = (a.output(0), a.output(1));
        for k in 0..n {
            let x = [i0[k], i1[k]];
            if self.recording && stop_at.is_none() {
                if stop_bar {
                    // Counts beats from the take's first sample; the sample that would begin
                    // beat num + 1 ends the take, so the loop is exactly one bar.
                    self.remaining -= self.sps;
                    if self.remaining <= 0.0 {
                        self.beats += 1;
                        if self.beats > num {
                            stop_at = Some(k);
                        } else {
                            self.sps = 1.0 / self.sr;
                            self.remaining += beat_secs;
                        }
                    }
                }
                if stop_at.is_none() {
                    if self.len < cap {
                        self.buf[0][self.len] = x[0];
                        self.buf[1][self.len] = x[1];
                        self.len += 1;
                        add_sample(&mut self.wacc, x[0], x[1]);
                        self.wfill += 1;
                        if self.wfill >= self.wblock {
                            store_bin(s, &mut self.wn, &mut self.wacc, &mut self.wfill);
                        }
                    } else {
                        stop_at = Some(k);
                        s.error.store(LOOPER_OUT_OF_MEMORY, Relaxed);
                    }
                }
            }
            if stop_at == Some(k) && self.recording {
                self.recording = false;
                if self.wfill > 0 {
                    store_bin(s, &mut self.wn, &mut self.wacc, &mut self.wfill);
                }
                s.record.store(false, Relaxed);
                if auto {
                    playing = true;
                    s.on.store(true, Relaxed);
                }
                self.pos = 0;
                self.post_count = 0;
                if playing && self.len > 0 && self.ramp_pending {
                    self.ramp_pending = false;
                    ramp = (k, n - k);
                }
            }
            // The input gain only scales what passes through; the take is the raw input.
            let mut y = [x[0] * in_gain, x[1] * in_gain];
            if playing && !self.recording && self.len > 0 {
                let j = self.pos.min(self.len - 1);
                let g = if k >= ramp.0 { loop_gain * (k - ramp.0) as f32 / ramp.1 as f32 } else { loop_gain };
                let e = self.len - 1 - j;
                for ch in 0..2 {
                    let mut v = self.buf[ch][j] * g;
                    if self.post_count == -1 && j < XFADE {
                        v += self.post[ch][j] * (1.0 - j as f32 / 128.0) * loop_gain;
                    }
                    if e < XFADE {
                        v += self.pre[ch][XFADE - 1 - e] * (1.0 - e as f32 / 127.0) * loop_gain;
                    }
                    y[ch] += v;
                }
                self.pos = if j + 1 >= self.len { 0 } else { j + 1 };
            }
            if self.post_count >= XFADE as i32 {
                self.post_count = -1;
                for ch in 0..2 {
                    for i in 0..XFADE {
                        self.post[ch][i] = self.ring[ch][(self.ring_at + i) % XFADE];
                    }
                }
            }
            self.ring[0][self.ring_at] = x[0];
            self.ring[1][self.ring_at] = x[1];
            self.ring_at = (self.ring_at + 1) % XFADE;
            if self.post_count >= 0 {
                self.post_count += 1;
            }
            o0[k] = y[0];
            o1[k] = y[1];
        }
        s.pos.store(self.pos as u64, Relaxed);
        s.len.store(self.len as u64, Relaxed);
    }

    fn set_parameter(&mut self, i: i32, v: f32) {
        apply_param(Kind::Looper, &self.sh, i, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Transport;

    /// 1024 Hz keeps the beat maths exact: at 120 bpm a quarter note is 512 samples.
    const SR: f64 = 1024.0;

    fn transport() -> Transport {
        Transport { playing: false, tempo: 120.0, sample_pos: 0, sample_rate: SR, num: 4, den: 4 }
    }

    /// One block through `p`; returns its output channels.
    fn run(p: &mut dyn Processor, ins: &[Vec<f32>], outs: usize, frames: usize) -> Vec<Vec<f32>> {
        let mut o = vec![vec![0.0f32; frames]; outs];
        let ip: Vec<*const f32> = ins.iter().map(|v| v.as_ptr()).collect();
        let op: Vec<*mut f32> = o.iter_mut().map(|v| v.as_mut_ptr()).collect();
        p.process(&ProcessArgs { inputs: &ip, outputs: &op, frames, midi_in: &[], transport: &transport() });
        o
    }

    fn looper() -> (Builtin, Box<dyn Processor>) {
        let (b, mut p) = create(Kind::Looper);
        p.prepare(SR, 256);
        (b, p)
    }

    fn stereo(v: f32, frames: usize) -> Vec<Vec<f32>> {
        vec![vec![v; frames]; 2]
    }

    #[test]
    fn looper_stop_after_bar_records_one_bar_and_auto_plays() {
        let (b, mut p) = looper();
        p.set_parameter(5, 1.0); // Stop After Bar
        p.set_parameter(2, 1.0); // Record
        for _ in 0..12 {
            run(p.as_mut(), &stereo(0.1, 256), 2, 256);
        }
        assert_eq!(b.sh.len.load(Relaxed), 4 * 512);
        assert!(!b.sh.record.load(Relaxed));
        assert!(b.sh.on.load(Relaxed));
    }

    #[test]
    fn looper_takes_the_raw_input_and_scales_only_what_passes_through() {
        let (b, mut p) = looper();
        p.set_parameter(9, 0.0); // Input Level: gain 0
        p.set_parameter(2, 1.0);
        let out = run(p.as_mut(), &stereo(0.5, 300), 2, 300);
        assert!(out[0].iter().all(|&v| v == 0.0));
        p.set_parameter(2, 1.0); // end the take; auto-play fades the loop in
        run(p.as_mut(), &stereo(0.0, 300), 2, 300);
        let out = run(p.as_mut(), &stereo(0.0, 300), 2, 300);
        assert_eq!(b.sh.len.load(Relaxed), 300);
        assert!(out[1].iter().all(|&v| v == 0.5), "{:?}", &out[1][..4]);
    }

    #[test]
    fn looper_record_while_playing_starts_a_new_take() {
        let (b, mut p) = looper();
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(1.0, 100), 2, 100);
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(0.0, 100), 2, 100);
        assert!(b.sh.on.load(Relaxed));
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(0.25, 50), 2, 50);
        assert!(b.sh.record.load(Relaxed) && !b.sh.on.load(Relaxed));
        assert_eq!(b.sh.len.load(Relaxed), 50);
    }

    #[test]
    fn looper_without_auto_play_stays_stopped() {
        let (b, mut p) = looper();
        p.set_parameter(6, 0.0); // Auto-play off
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(0.3, 64), 2, 64);
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(0.0, 64), 2, 64);
        assert_eq!(b.sh.len.load(Relaxed), 64);
        assert!(!b.sh.on.load(Relaxed));
    }

    #[test]
    fn settings_use_the_original_keys() {
        let (mut b, _p) = create(Kind::Looper);
        let e = El::new("Pedalboard2LooperSettings")
            .attr("soundFile", "C:\\loops\\a.wav")
            .attr("stopAfterBar", "1")
            .attr("autoPlay", "0")
            .attr("barNumerator", "3")
            .attr("barDenominator", "8")
            .attr("loopLevel", "1.5")
            .attr("editorX", "10");
        b.set_state(&e);
        assert_eq!([b.param_value(5), b.param_value(6), b.param_value(7), b.param_value(8)], [1.0, 0.0, 3.0, 8.0]);
        assert_eq!(b.param_value(9), 0.25, "a missing inputLevel is a gain of 0.5");
        assert_eq!(b.param_value(10), 0.75);
        let out = b.get_state().unwrap();
        assert_eq!(out.get("soundFile"), Some("C:\\loops\\a.wav"));
        assert_eq!(out.get("editorX"), Some("10"));
        assert_eq!(out.get("autoPlay"), Some("0"));

        let (mut t, _p) = create(Kind::OutputToggle);
        t.set_state(&El::new("Pedalboard2OutputToggleSettings").attr("toggle", "1"));
        assert!(t.sh.on.load(Relaxed));
        assert_eq!(t.get_state().unwrap().get("toggle"), Some("1"));

        let (mut f, _p) = create(Kind::FilePlayer);
        f.set_state(&El::new("Pedalboard2FilePlayerSettings").attr("looping", "1").attr("syncToMainTransport", "1"));
        assert!(f.sh.looping.load(Relaxed) && f.sh.sync.load(Relaxed));
    }

    #[test]
    fn looper_take_waveform_has_one_bin_per_block_and_keeps_the_last_part() {
        let (b, mut p) = looper();
        let block = b.sh.wave_block.load(Relaxed);
        assert_eq!(block, ((SR * LOOP_SECONDS) as usize).div_ceil(WAVE_BINS));
        p.set_parameter(2, 1.0);
        let ins = vec![(0..block * 2 + 3).map(|i| if i == 5 { 0.75 } else { -0.25 }).collect::<Vec<f32>>(), vec![0.5; block * 2 + 3]];
        run(p.as_mut(), &ins, 2, block * 2 + 3);
        assert_eq!(b.sh.wave_n.load(Relaxed), 2, "only whole bins while recording");
        p.set_parameter(2, 1.0);
        run(p.as_mut(), &stereo(0.0, 4), 2, 4);
        assert_eq!(b.sh.wave_n.load(Relaxed), 3, "the part-filled bin is kept when the take ends");
        let bin = |i: usize| b.sh.wave[i].each_ref().map(getf);
        assert_eq!(bin(0), [-0.25, 0.75, 0.5, 0.5]);
        assert_eq!(bin(2), [-0.25, -0.25, 0.5, 0.5]);
    }

    #[test]
    fn recorder_waveform_merges_pairs_when_full() {
        let mut pk = Peaks { block: 1, ..Peaks::new(SR) };
        let samples: Vec<f32> = (0..PEAK_BINS).flat_map(|i| [i as f32, -(i as f32)]).collect();
        pk.add(&samples[..3]);
        pk.add(&samples[3..]);
        assert_eq!((pk.bins.len(), pk.block, pk.frames), (PEAK_BINS / 2, 2, PEAK_BINS as u64));
        assert_eq!(pk.bins[1], [2.0, 3.0, -3.0, -2.0]);
    }

    #[test]
    fn metronome_plays_a_chosen_accent_sound() {
        let (mut b, mut p) = create(Kind::Metronome);
        p.prepare(SR, 64);
        let clip = Clip { data: vec![vec![0.25; 200]], rate: SR, frames: 200, overview: Vec::new() };
        b.send_clip(0, Some(Arc::new(clip))).unwrap();
        p.set_parameter(0, 1.0); // Play
        let out = run(p.as_mut(), &[], Kind::Metronome.io().1, 64);
        assert!(out[0].iter().all(|&v| v == 0.25));
    }

    #[test]
    fn file_player_read_position_and_trigger() {
        let (mut b, mut p) = create(Kind::FilePlayer);
        p.prepare(SR, 16);
        let ramp: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        b.send_clip(0, Some(Arc::new(Clip { data: vec![ramp.clone(), ramp], rate: SR, frames: 1000, overview: Vec::new() }))).unwrap();
        p.set_parameter(3, 0.5); // Read Position
        p.set_parameter(0, 1.0); // Play
        assert_eq!(run(p.as_mut(), &[], 2, 4)[0][0], 0.5);
        p.set_parameter(5, 1.0); // Trigger
        assert_eq!(run(p.as_mut(), &[], 2, 4)[0][0], 0.0);
    }

    /// A FORM file with a COMM chunk (`comm_tail` holds the AIFF-C compression tag) and an SSND
    /// chunk holding `samples`, at 44100 Hz.
    fn aiff(kind: &[u8; 4], ch: u16, frames: u32, bits: u16, comm_tail: &[u8], samples: &[u8]) -> Vec<u8> {
        let mut comm = Vec::new();
        comm.extend(ch.to_be_bytes());
        comm.extend(frames.to_be_bytes());
        comm.extend(bits.to_be_bytes());
        comm.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        comm.extend(comm_tail);
        let mut body = kind.to_vec();
        for (id, data) in [(b"COMM", comm), (b"SSND", [&[0u8; 8][..], samples].concat())] {
            body.extend(id);
            body.extend((data.len() as u32).to_be_bytes());
            let odd = data.len() % 2 == 1;
            body.extend(data);
            if odd {
                body.push(0);
            }
        }
        [&b"FORM"[..], &(body.len() as u32).to_be_bytes()[..], &body[..]].concat()
    }

    #[test]
    fn aiff_files_read_like_wav() {
        // 16-bit stereo, big-endian: frame 0 = (0.5, -1), frame 1 = (-0.5, 0).
        let (d, rate) = read_aiff(&aiff(b"AIFF", 2, 2, 16, &[], &[0x40, 0, 0x80, 0, 0xC0, 0, 0, 0])).unwrap();
        assert_eq!(rate, 44100.0);
        assert_eq!(d, vec![vec![0.5, -0.5], vec![-1.0, 0.0]]);
        // AIFF-C little-endian 16-bit mono; a frame count beyond the data is cut to what's there.
        let (d, _) = read_aiff(&aiff(b"AIFC", 1, 9, 16, b"sowt\0", &[0, 0x40, 0, 0xC0])).unwrap();
        assert_eq!(d, vec![vec![0.5, -0.5]]);
        // 8-bit AIFF samples are signed.
        let (d, _) = read_aiff(&aiff(b"AIFF", 1, 1, 8, &[], &[0x80])).unwrap();
        assert_eq!(d, vec![vec![-1.0]]);
        assert!(read_aiff(&aiff(b"AIFC", 1, 1, 16, b"ima4\0", &[0, 0])).is_err());
    }
}
