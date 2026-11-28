//! Windows Audio (WASAPI shared mode) endpoints for native and mixed-system routes.

use super::{bridge, AudioRuntime, DeviceInfo};
use crate::settings::AudioConfig;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SCRATCH: usize = 4096;
const COMMON_RATES: [u32; 8] = [22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000];
/// Input device setting that turns audio input off.
pub const NO_INPUT: &str = "<none>";

fn host() -> cpal::Host {
    cpal::default_host()
}

pub fn output_names() -> Vec<String> {
    host().output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

pub fn input_names() -> Vec<String> {
    host().input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
}

fn find(input: bool, name: &str, exact: bool) -> Option<cpal::Device> {
    let h = host();
    if !name.is_empty() {
        let found = if input {
            h.input_devices().ok().and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == name)))
        } else {
            h.output_devices().ok().and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == name)))
        };
        if exact || found.is_some() {
            return found;
        }
    }
    if input {
        h.default_input_device()
    } else {
        h.default_output_device()
    }
}

fn pick_channels(want: &[u32], total: usize) -> Vec<u32> {
    if want.is_empty() {
        return Vec::new();
    }
    let v: Vec<u32> = want.iter().copied().filter(|&c| (c as usize) < total).collect();
    if v.is_empty() {
        (0..total.min(2) as u32).collect()
    } else {
        v
    }
}

fn input_config(d: &cpal::Device, sr: u32) -> Option<cpal::SupportedStreamConfig> {
    let def = d.default_input_config().ok()?;
    if def.sample_rate().0 == sr {
        return Some(def);
    }
    d.supported_input_configs()
        .ok()?
        .find(|c| c.min_sample_rate().0 <= sr && sr <= c.max_sample_rate().0)
        .map(|c| c.with_sample_rate(cpal::SampleRate(sr)))
}

struct OutCb {
    runtime: Arc<Mutex<AudioRuntime>>,
    ring: Option<rtrb::Consumer<f32>>,
    mixed: Option<bridge::Reader>,
    in_width: usize,
    in_active: Vec<usize>,
    out_width: usize,
    out_active: Vec<usize>,
    scratch_in: Vec<Vec<f32>>,
    scratch_out: Vec<Vec<f32>>,
    in_ptrs: Vec<*const f32>,
    out_ptrs: Vec<*mut f32>,
}

// The pointers refer to the heap buffers inside `scratch_*`, which never move or resize.
unsafe impl Send for OutCb {}

impl OutCb {
    fn render<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T], info: &cpal::OutputCallbackInfo, rate: f64) {
        let callback_time = self.mixed.as_ref().map(|reader| {
            let stamp = info.timestamp();
            if let Some(delay) = stamp.playback.duration_since(&stamp.callback) {
                reader.timing.set_output(delay);
            }
            Instant::now()
        });
        let ow = self.out_width.max(1);
        let total = data.len() / ow;
        data.fill(T::EQUILIBRIUM);
        if let Some(r) = self.ring.as_mut() {
            // Keep input close to real time: drop anything older than about one spare callback.
            let iw = self.in_width;
            let keep = (total + total.max(256)) * iw;
            let avail = r.slots();
            if avail > keep {
                if let Ok(c) = r.read_chunk((avail - keep) / iw * iw) {
                    c.commit_all();
                }
            }
        }
        let mut done = 0;
        while done < total {
            let n = (total - done).min(SCRATCH);
            if let (Some(reader), Some(start)) = (&mut self.mixed, callback_time) {
                reader.read(&mut self.scratch_in, n, start + Duration::from_secs_f64(done as f64 / rate));
            }
            if let Some(r) = self.ring.as_mut() {
                let iw = self.in_width;
                let have = (r.slots() / iw).min(n);
                if let Ok(c) = r.read_chunk(have * iw) {
                    let (a, b) = c.as_slices();
                    for (k, &ch) in self.in_active.iter().enumerate() {
                        let dst = &mut self.scratch_in[k];
                        for (f, d) in dst[..have].iter_mut().enumerate() {
                            let i = f * iw + ch;
                            *d = if i < a.len() { a[i] } else { b[i - a.len()] };
                        }
                    }
                    c.commit_all();
                }
                for d in &mut self.scratch_in {
                    d[have..n].fill(0.0);
                }
            }
            match self.runtime.try_lock() {
                Ok(mut rt) => unsafe { rt.process(&self.in_ptrs, &self.out_ptrs, n) },
                Err(_) => self.scratch_out.iter_mut().for_each(|b| b[..n].fill(0.0)),
            }
            for (k, &ch) in self.out_active.iter().enumerate() {
                for (f, &v) in self.scratch_out[k][..n].iter().enumerate() {
                    data[(done + f) * ow + ch] = T::from_sample(v);
                }
            }
            done += n;
        }
    }
}

fn push_input<T: SizedSample>(p: &mut rtrb::Producer<f32>, data: &[T], width: usize)
where
    f32: FromSample<T>,
{
    let n = data.len().min(p.slots()) / width * width;
    if let Ok(c) = p.write_chunk_uninit(n) {
        c.fill_from_iter(data[..n].iter().map(|&s| s.to_sample::<f32>()));
    }
}

fn build_output(dev: &cpal::Device, conf: &cpal::StreamConfig, fmt: SampleFormat, cb: OutCb, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    fn go<T: SizedSample + FromSample<f32>>(dev: &cpal::Device, conf: &cpal::StreamConfig, mut cb: OutCb, failed: Arc<AtomicBool>) -> Result<cpal::Stream, cpal::BuildStreamError> {
        let rate = conf.sample_rate.0 as f64;
        dev.build_output_stream::<T, _, _>(conf, move |data, info| cb.render(data, info, rate), move |_| failed.store(true, Ordering::Relaxed), None)
    }
    match fmt {
        SampleFormat::F32 => go::<f32>(dev, conf, cb, failed),
        SampleFormat::I16 => go::<i16>(dev, conf, cb, failed),
        SampleFormat::I32 => go::<i32>(dev, conf, cb, failed),
        SampleFormat::U16 => go::<u16>(dev, conf, cb, failed),
        f => return Err(format!("The output uses an unsupported sample format ({f:?}).")),
    }
    .map_err(|e| e.to_string())
}

pub(super) enum InputSink {
    Native(rtrb::Producer<f32>),
    Mixed(bridge::Writer, Vec<u32>, Arc<bridge::Timing>),
}

fn build_input(dev: &cpal::Device, conf: &cpal::StreamConfig, fmt: SampleFormat, sink: InputSink, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    fn go<T: SizedSample>(dev: &cpal::Device, conf: &cpal::StreamConfig, mut sink: InputSink, failed: Arc<AtomicBool>) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        f32: FromSample<T>,
    {
        let width = conf.channels as usize;
        dev.build_input_stream::<T, _, _>(conf, move |data, info| match &mut sink {
            InputSink::Native(prod) => push_input(prod, data, width),
            InputSink::Mixed(writer, selected, timing) => {
                let now = Instant::now();
                let stamp = info.timestamp();
                if let Some(delay) = stamp.callback.duration_since(&stamp.capture) {
                    timing.set_input(delay);
                }
                writer.write(data.len() / width, now, |frame, ch| data[frame * width + selected[ch] as usize].to_sample::<f32>());
            }
        }, move |_| failed.store(true, Ordering::Relaxed), None)
    }
    match fmt {
        SampleFormat::F32 => go::<f32>(dev, conf, sink, failed),
        SampleFormat::I16 => go::<i16>(dev, conf, sink, failed),
        SampleFormat::I32 => go::<i32>(dev, conf, sink, failed),
        SampleFormat::U16 => go::<u16>(dev, conf, sink, failed),
        f => return Err(format!("The input uses an unsupported sample format ({f:?}).")),
    }.map_err(|e| e.to_string())
}

pub struct CpalDevice {
    out_stream: cpal::Stream,
    in_stream: Option<cpal::Stream>,
    failed: Arc<AtomicBool>,
}

pub(super) struct OutputSetup {
    dev: cpal::Device,
    conf: cpal::StreamConfig,
    fmt: SampleFormat,
    pub info: DeviceInfo,
}

impl OutputSetup {
    pub fn probe(cfg: &AudioConfig, capture_rate: Option<f64>) -> Result<Self, String> {
        let mixed = capture_rate.is_some();
        let out_dev = find(false, &cfg.device, mixed).ok_or_else(|| format!("Windows audio output '{}' is unavailable.", cfg.device))?;
        let name = out_dev.name().unwrap_or_else(|_| "Output".into());
        let def = out_dev.default_output_config().map_err(|e| format!("{name}: {e}"))?;
        let ranges: Vec<cpal::SupportedStreamConfigRange> = out_dev.supported_output_configs().map(|r| r.collect()).unwrap_or_default();
        let fits = |r: u32| {
            ranges.iter().any(|c| c.channels() == def.channels() && c.sample_format() == def.sample_format() && c.min_sample_rate().0 <= r && r <= c.max_sample_rate().0)
        };
        let mut rates: Vec<f64> = COMMON_RATES.iter().copied().filter(|&r| fits(r)).map(|r| r as f64).collect();
        let def_rate = def.sample_rate().0;
        if !rates.contains(&(def_rate as f64)) {
            rates.push(def_rate as f64);
            rates.sort_by(f64::total_cmp);
        }
        let preferred = capture_rate.unwrap_or(cfg.sample_rate);
        let sr = if preferred > 0.0 && fits(preferred as u32) { preferred as u32 } else { def_rate };
        let fmt = def.sample_format();
        let out_width = def.channels() as usize;
        let (blocks, fixed_ok): (Vec<u32>, Box<dyn Fn(u32) -> bool>) = match *def.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => (
                [32u32, 64, 128, 256, 480, 512, 1024, 2048, 4096].into_iter().filter(|&b| b >= min && b <= max).collect(),
                Box::new(move |b| b >= min && b <= max),
            ),
            cpal::SupportedBufferSize::Unknown => (vec![64, 128, 256, 480, 512, 1024, 2048], Box::new(|_| true)),
        };
        // In a mixed route the buffer selector controls the ASIO driver.
        let fixed = !mixed && cfg.buffer_size > 0 && fixed_ok(cfg.buffer_size);
        let block = if fixed { cfg.buffer_size as usize } else { 1024 };

        let conf = cpal::StreamConfig {
            channels: def.channels(),
            sample_rate: cpal::SampleRate(sr),
            buffer_size: if fixed { cpal::BufferSize::Fixed(cfg.buffer_size) } else { cpal::BufferSize::Default },
        };
        let info = DeviceInfo {
            name, backend: "Windows Audio".into(), sample_rate: sr as f64, block,
            out_names: (1..=out_width).map(|i| format!("Output {i}")).collect(),
            active_outs: pick_channels(&cfg.output_channels, out_width),
            rates, blocks, in_latency: block as u32, out_latency: block as u32,
            ..Default::default()
        };
        Ok(Self { dev: out_dev, conf, fmt, info })
    }

    pub fn build(&self, runtime: Arc<Mutex<AudioRuntime>>, ring: Option<rtrb::Consumer<f32>>,
                 mixed: Option<bridge::Reader>, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
        let info = &self.info;
        let mut scratch_in = vec![vec![0f32; SCRATCH]; info.active_ins.len()];
        let mut scratch_out = vec![vec![0f32; SCRATCH]; info.active_outs.len()];
        let cb = OutCb {
            runtime, ring, mixed,
            in_width: info.in_names.len(),
            in_active: info.active_ins.iter().map(|&c| c as usize).collect(),
            out_width: self.conf.channels as usize,
            out_active: info.active_outs.iter().map(|&c| c as usize).collect(),
            in_ptrs: scratch_in.iter_mut().map(|b| b.as_ptr()).collect(),
            out_ptrs: scratch_out.iter_mut().map(|b| b.as_mut_ptr()).collect(),
            scratch_in, scratch_out,
        };
        build_output(&self.dev, &self.conf, self.fmt, cb, failed).map_err(|e| format!("{}: {e}", info.name))
    }
}

pub(super) struct InputSetup {
    dev: cpal::Device,
    config: cpal::SupportedStreamConfig,
    pub name: String,
    pub active: Vec<u32>,
}

impl InputSetup {
    pub fn probe(cfg: &AudioConfig, rate: u32, mixed: bool) -> Result<Option<Self>, String> {
        if cfg.input_device == NO_INPUT {
            return Ok(None);
        }
        let dev = find(true, &cfg.input_device, mixed).ok_or_else(|| format!("Windows audio input '{}' is unavailable.", cfg.input_device))?;
        let name = dev.name().unwrap_or_else(|_| "Windows audio input".into());
        let config = input_config(&dev, rate).or_else(|| {
            if mixed { dev.default_input_config().ok() } else { None }
        }).ok_or_else(|| format!("Could not open the input format for {name}."))?;
        let active = pick_channels(&cfg.input_channels, config.channels() as usize);
        Ok(Some(Self { dev, config, name, active }))
    }

    pub fn rate(&self) -> f64 {
        self.config.sample_rate().0 as f64
    }

    pub fn names(&self) -> Vec<String> {
        (1..=self.config.channels()).map(|i| format!("Input {i}")).collect()
    }

    pub fn build(&self, sink: InputSink, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
        build_input(&self.dev, &self.config.config(), self.config.sample_format(), sink, failed)
            .map_err(|e| format!("{}: {e}", self.name))
    }
}

impl CpalDevice {
    pub fn open(cfg: &AudioConfig, runtime: Arc<Mutex<AudioRuntime>>) -> Result<(CpalDevice, DeviceInfo), String> {
        let mut output = OutputSetup::probe(cfg, None)?;
        let failed = Arc::new(AtomicBool::new(false));
        let mut in_stream = None;
        let mut ring = None;
        if let Ok(Some(input)) = InputSetup::probe(cfg, output.info.sample_rate as u32, false) {
            output.info.in_names = input.names();
            if !input.active.is_empty() {
                let (tx, rx) = rtrb::RingBuffer::new(output.info.in_names.len() * SCRATCH * 4);
                if let Ok(stream) = input.build(InputSink::Native(tx), failed.clone()) {
                    output.info.active_ins = input.active;
                    in_stream = Some(stream);
                    ring = Some(rx);
                }
            }
        }
        let out_stream = output.build(runtime, ring, None, failed.clone())?;
        Ok((CpalDevice { out_stream, in_stream, failed }, output.info))
    }

    pub fn start(&mut self) -> Result<(), String> {
        if let Some(s) = &self.in_stream {
            s.play().map_err(|e| e.to_string())?;
        }
        self.out_stream.play().map_err(|e| e.to_string())
    }

    /// True once after the stream reports an error, so a restart is asked for only once per failure.
    pub fn failed(&self) -> bool {
        self.failed.swap(false, Ordering::Relaxed)
    }
}
