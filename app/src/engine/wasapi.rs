//! Windows Audio (WASAPI shared mode) through cpal, for machines without an ASIO driver.
//! The output callback drives the patch; the input device feeds it through a lock-free ring
//! that is trimmed every callback so input latency never builds up.

use super::{AudioRuntime, DeviceInfo};
use crate::settings::AudioConfig;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

fn find(input: bool, name: &str) -> Option<cpal::Device> {
    let h = host();
    if !name.is_empty() {
        let found = if input {
            h.input_devices().ok().and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == name)))
        } else {
            h.output_devices().ok().and_then(|mut it| it.find(|d| d.name().is_ok_and(|n| n == name)))
        };
        if found.is_some() {
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
    fn render<T: SizedSample + FromSample<f32>>(&mut self, data: &mut [T]) {
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

fn push_input<T: SizedSample>(p: &mut rtrb::Producer<f32>, data: &[T])
where
    f32: FromSample<T>,
{
    let n = data.len().min(p.slots());
    if let Ok(c) = p.write_chunk_uninit(n) {
        c.fill_from_iter(data[..n].iter().map(|&s| s.to_sample::<f32>()));
    }
}

fn build_output(dev: &cpal::Device, conf: &cpal::StreamConfig, fmt: SampleFormat, cb: OutCb, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    fn go<T: SizedSample + FromSample<f32>>(dev: &cpal::Device, conf: &cpal::StreamConfig, mut cb: OutCb, failed: Arc<AtomicBool>) -> Result<cpal::Stream, cpal::BuildStreamError> {
        dev.build_output_stream::<T, _, _>(conf, move |data: &mut [T], _: &cpal::OutputCallbackInfo| cb.render(data), move |_| failed.store(true, Ordering::Relaxed), None)
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

fn build_input(dev: &cpal::Device, conf: &cpal::StreamConfig, fmt: SampleFormat, prod: rtrb::Producer<f32>, failed: Arc<AtomicBool>) -> Option<cpal::Stream> {
    fn go<T: SizedSample>(dev: &cpal::Device, conf: &cpal::StreamConfig, mut prod: rtrb::Producer<f32>, failed: Arc<AtomicBool>) -> Option<cpal::Stream>
    where
        f32: FromSample<T>,
    {
        dev.build_input_stream::<T, _, _>(conf, move |data: &[T], _: &cpal::InputCallbackInfo| push_input(&mut prod, data), move |_| failed.store(true, Ordering::Relaxed), None)
            .ok()
    }
    match fmt {
        SampleFormat::F32 => go::<f32>(dev, conf, prod, failed),
        SampleFormat::I16 => go::<i16>(dev, conf, prod, failed),
        SampleFormat::I32 => go::<i32>(dev, conf, prod, failed),
        SampleFormat::U16 => go::<u16>(dev, conf, prod, failed),
        _ => None,
    }
}

pub struct CpalDevice {
    out_stream: cpal::Stream,
    in_stream: Option<cpal::Stream>,
    failed: Arc<AtomicBool>,
}

impl CpalDevice {
    pub fn open(cfg: &AudioConfig, runtime: Arc<Mutex<AudioRuntime>>) -> Result<(CpalDevice, DeviceInfo), String> {
        let out_dev = find(false, &cfg.device).ok_or("No Windows audio output device was found.")?;
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
        let sr = if cfg.sample_rate > 0.0 && rates.contains(&cfg.sample_rate) && fits(cfg.sample_rate as u32) { cfg.sample_rate as u32 } else { def_rate };
        let fmt = def.sample_format();
        let out_width = def.channels() as usize;
        let (blocks, fixed_ok): (Vec<u32>, Box<dyn Fn(u32) -> bool>) = match *def.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } => (
                [32u32, 64, 128, 256, 480, 512, 1024, 2048, 4096].into_iter().filter(|&b| b >= min && b <= max).collect(),
                Box::new(move |b| b >= min && b <= max),
            ),
            cpal::SupportedBufferSize::Unknown => (vec![64, 128, 256, 480, 512, 1024, 2048], Box::new(|_| true)),
        };
        let fixed = cfg.buffer_size > 0 && fixed_ok(cfg.buffer_size);
        let block = if fixed { cfg.buffer_size as usize } else { 1024 };

        let in_dev = if cfg.input_device == NO_INPUT { None } else { find(true, &cfg.input_device) };
        let in_conf = in_dev.as_ref().and_then(|d| input_config(d, sr));
        let in_width = in_conf.as_ref().map_or(0, |c| c.channels() as usize);
        let failed = Arc::new(AtomicBool::new(false));
        let mut in_active = pick_channels(&cfg.input_channels, in_width);
        let mut in_stream = None;
        let mut ring_rx = None;
        if let (Some(d), Some(c)) = (in_dev.as_ref(), in_conf.as_ref()) {
            if !in_active.is_empty() {
                let (tx, rx) = rtrb::RingBuffer::new(in_width * SCRATCH * 4);
                let conf = cpal::StreamConfig { channels: c.channels(), sample_rate: cpal::SampleRate(sr), buffer_size: cpal::BufferSize::Default };
                in_stream = build_input(d, &conf, c.sample_format(), tx, failed.clone());
                if in_stream.is_some() {
                    ring_rx = Some(rx);
                }
            }
        }
        if ring_rx.is_none() {
            in_active.clear();
        }
        let out_active = pick_channels(&cfg.output_channels, out_width);
        let mut scratch_in = vec![vec![0f32; SCRATCH]; in_active.len()];
        let mut scratch_out = vec![vec![0f32; SCRATCH]; out_active.len()];
        let cb = OutCb {
            runtime,
            ring: ring_rx,
            in_width,
            in_active: in_active.iter().map(|&c| c as usize).collect(),
            out_width,
            out_active: out_active.iter().map(|&c| c as usize).collect(),
            in_ptrs: scratch_in.iter_mut().map(|b| b.as_ptr()).collect(),
            out_ptrs: scratch_out.iter_mut().map(|b| b.as_mut_ptr()).collect(),
            scratch_in,
            scratch_out,
        };
        let conf = cpal::StreamConfig {
            channels: def.channels(),
            sample_rate: cpal::SampleRate(sr),
            buffer_size: if fixed { cpal::BufferSize::Fixed(cfg.buffer_size) } else { cpal::BufferSize::Default },
        };
        let out_stream = build_output(&out_dev, &conf, fmt, cb, failed.clone()).map_err(|e| format!("{name}: {e}"))?;
        let info = DeviceInfo {
            name,
            backend: "Windows Audio".into(),
            sample_rate: sr as f64,
            block,
            in_names: (1..=in_width).map(|i| format!("Input {i}")).collect(),
            out_names: (1..=out_width).map(|i| format!("Output {i}")).collect(),
            active_ins: in_active,
            active_outs: out_active,
            rates,
            blocks,
            in_latency: block as u32,
            out_latency: block as u32,
        };
        Ok((CpalDevice { out_stream, in_stream, failed }, info))
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
