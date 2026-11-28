//! Transfers selected input channels to an independently clocked output device.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Timing {
    input_us: AtomicU64,
    bridge_us: AtomicU64,
    output_us: AtomicU64,
    pub input_rate: f64,
}

#[derive(Clone, Copy, Default)]
pub struct Delay {
    pub input_ms: f64,
    pub bridge_ms: f64,
    pub output_ms: f64,
}

impl Delay {
    pub fn total_ms(self, plugins_ms: f64) -> f64 {
        self.input_ms + self.bridge_ms + self.output_ms + plugins_ms
    }
}

impl Timing {
    pub fn new(input_rate: f64, input_samples: u32, output_rate: f64, output_samples: u32) -> Arc<Self> {
        Arc::new(Self {
            input_us: AtomicU64::new((1e6 * input_samples as f64 / input_rate).round() as u64),
            output_us: AtomicU64::new((1e6 * output_samples as f64 / output_rate).round() as u64),
            bridge_us: AtomicU64::new(0),
            input_rate,
        })
    }

    pub fn set_input(&self, duration: Duration) {
        self.input_us.store(duration.as_micros() as u64, Relaxed);
    }

    pub fn set_output(&self, duration: Duration) {
        self.output_us.store(duration.as_micros() as u64, Relaxed);
    }

    pub fn delay(&self) -> Delay {
        Delay {
            input_ms: self.input_us.load(Relaxed) as f64 / 1000.0,
            bridge_ms: self.bridge_us.load(Relaxed) as f64 / 1000.0,
            output_ms: self.output_us.load(Relaxed) as f64 / 1000.0,
        }
    }
}

#[derive(Clone, Copy)]
struct Packet {
    first: u64,
    frames: usize,
    received: Instant,
}

pub struct Writer {
    samples: rtrb::Producer<f32>,
    packets: rtrb::Producer<Packet>,
    channels: usize,
    written: u64,
}

pub struct Reader {
    samples: rtrb::Consumer<f32>,
    packets: rtrb::Consumer<Packet>,
    pub timing: Arc<Timing>,
    channels: usize,
    capacity: usize,
    ratio: f64,
    consumed: u64,
    packet: Option<Packet>,
    phase: f64,
    correction: f64,
    primed: bool,
}

pub fn connect(channels: usize, output_rate: f64, block: usize, timing: Arc<Timing>) -> (Writer, Reader) {
    let ratio = timing.input_rate / output_rate;
    let capacity = 16384.max(block.saturating_mul(4)).max((block as f64 * ratio * 4.0).ceil() as usize);
    let (tx, rx) = rtrb::RingBuffer::new(capacity * channels.max(1));
    // At worst the producer delivers one frame per packet.
    let (ptx, prx) = rtrb::RingBuffer::new(capacity + 1);
    (
        Writer { samples: tx, packets: ptx, channels, written: 0 },
        Reader {
            samples: rx, packets: prx, timing, channels, capacity, ratio,
            consumed: 0, packet: None, phase: 0.0, correction: 0.0, primed: false,
        },
    )
}

impl Writer {
    /// Publish the timestamp before the corresponding complete, interleaved frames.
    pub fn write(&mut self, frames: usize, received: Instant, mut sample: impl FnMut(usize, usize) -> f32) {
        if self.channels == 0 {
            return;
        }
        let count = frames.min(self.samples.slots() / self.channels);
        if count == 0 || self.packets.push(Packet { first: self.written, frames, received }).is_err() {
            return;
        }
        let width = self.channels;
        if let Ok(chunk) = self.samples.write_chunk_uninit(count * width) {
            chunk.fill_from_iter((0..count * width).map(|i| sample(i / width, i % width)));
            self.written += count as u64;
        }
    }
}

impl Reader {
    fn locate_packet(&mut self) {
        while self.packets.peek().is_ok_and(|p| p.first <= self.consumed) {
            self.packet = self.packets.pop().ok();
        }
    }

    /// Output drives the graph. Interpolate at the negotiated rate ratio and gently
    /// correct clock drift using the average queue depth. All storage is preallocated.
    pub fn read(&mut self, dst: &mut [Vec<f32>], n: usize, render_time: Instant) {
        if self.channels == 0 || n == 0 {
            return;
        }
        for channel in dst.iter_mut() {
            channel[..n].fill(0.0);
        }
        let mut available = self.samples.slots() / self.channels;
        self.locate_packet();
        let Some(packet) = self.packet else { return };
        let needed = n as f64 * self.ratio;
        let needed = needed.ceil() as usize;
        // Reserve at least one capture packet so its arrival phase can change, and
        // a quarter render period so slow input clocks have room to settle.
        let target = (needed + packet.frames.max(needed / 4).max(2)).min(self.capacity / 2);
        if available > target.saturating_mul(2) {
            let skip = available - target;
            if let Ok(chunk) = self.samples.read_chunk(skip * self.channels) {
                chunk.commit_all();
            }
            self.consumed += skip as u64;
            self.phase = 0.0;
            available -= skip;
            self.locate_packet();
        }
        if !self.primed && available < target {
            self.timing.bridge_us.store(0, Relaxed);
            return;
        }
        self.primed = true;
        let packet = self.packet.unwrap();
        // Both endpoint estimates refer to the first frame of a callback. Measure how
        // much later this particular frame is rendered, accounting for its packet offset.
        let offset = (self.consumed - packet.first) as f64 + self.phase;
        let wait = render_time.saturating_duration_since(packet.received).as_secs_f64() - offset / self.timing.input_rate;
        self.timing.bridge_us.store((wait.max(0.0) * 1e6).round() as u64, Relaxed);

        let error = (available as f64 - target as f64) / target as f64;
        self.correction += 0.02 * ((error * 0.005).clamp(-0.005, 0.005) - self.correction);
        let step = self.ratio * (1.0 + self.correction);
        let width = self.channels;
        let Ok(chunk) = self.samples.read_chunk(available * width) else { return };
        let (a, b) = chunk.as_slices();
        let at = |i: usize| if i < a.len() { a[i] } else { b[i - a.len()] };
        let mut position = self.phase;
        for f in 0..n {
            let index = position as usize;
            if index + 1 >= available || position + step > available as f64 {
                self.primed = false;
                break;
            }
            let frac = (position - index as f64) as f32;
            for (ch, channel) in dst.iter_mut().enumerate() {
                let i = index * width + ch;
                channel[f] = at(i) + frac * (at(i + width) - at(i));
            }
            position += step;
        }
        let used = position as usize;
        chunk.commit(used * width);
        self.consumed += used as u64;
        self.phase = position - used as f64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::latency::LatencyReading;

    #[test]
    fn converts_stereo_and_measures_the_wait_of_the_frame_actually_rendered() {
        let t = Instant::now();
        let timing = Timing::new(48000.0, 96, 24000.0, 72);
        let (mut tx, mut rx) = connect(2, 24000.0, 32, timing.clone());
        for packet in 0..3 {
            tx.write(48, t + Duration::from_millis(packet), |f, ch| (packet * 48 + f as u64 + 1000 * ch as u64) as f32);
        }
        let mut dst = vec![vec![0.0; 32]; 2];
        rx.read(&mut dst, 32, t + Duration::from_millis(5));
        for f in 0..32 {
            assert!((dst[0][f] - 2.0 * f as f32).abs() < 0.01);
            assert!((dst[1][f] - dst[0][f] - 1000.0).abs() < 0.001);
        }
        let delay = timing.delay();
        assert_eq!((delay.input_ms, delay.bridge_ms, delay.output_ms), (2.0, 5.0, 3.0));
        assert_eq!(LatencyReading { plugin_ms: 1.0, devices: Some(delay) }.total_ms(), 11.0);
        rx.read(&mut dst, 32, t + Duration::from_micros(6333));
        assert!((timing.delay().bridge_ms - 5.0).abs() < 0.002);
    }

    #[test]
    fn a_stalled_device_recovers_with_aligned_channels_and_recent_audio() {
        let t = Instant::now();
        let timing = Timing::new(48000.0, 0, 48000.0, 0);
        let (mut tx, mut rx) = connect(2, 48000.0, 48, timing);
        let mut dst = vec![vec![f32::NAN; 48]; 2];
        rx.read(&mut dst, 48, t);
        assert!(dst.iter().flatten().all(|&v| v == 0.0));
        for k in 0..400 {
            tx.write(48, t + Duration::from_millis(k), |_, ch| (k + 1000 * ch as u64) as f32);
        }
        rx.read(&mut dst, 48, t + Duration::from_millis(401));
        assert!(dst[0][0] > 300.0);
        for f in 0..48 {
            assert!((dst[1][f] - dst[0][f] - 1000.0).abs() < 0.001);
        }
        for k in 402..420 {
            tx.write(48, t + Duration::from_millis(k), |_, _| 0.25);
            rx.read(&mut dst, 48, t + Duration::from_millis(k));
        }
        assert!(dst.iter().flatten().all(|&v| v == 0.25));
    }

    #[test]
    fn independent_clocks_stay_in_step_with_unequal_rates_and_callback_sizes() {
        let start = Instant::now();
        for (input, output, packet, block, drift) in [
            (44100.0, 48000.0, 441, 64, 1.00025),
            (48000.0, 44100.0, 64, 441, 0.99975),
        ] {
            let timing = Timing::new(input, 0, output, 0);
            let (mut tx, mut rx) = connect(1, output, block, timing.clone());
            let mut next_input = packet as f64 / input;
            let mut dst = vec![vec![0.0; block]];
            for callback in 0..(30.0 * output / block as f64) as usize {
                let now = callback as f64 * block as f64 / output;
                while next_input <= now {
                    tx.write(packet, start + Duration::from_secs_f64(next_input), |_, _| 0.5);
                    next_input += packet as f64 / (input * drift);
                }
                rx.read(&mut dst, block, start + Duration::from_secs_f64(now));
                if now > 0.05 {
                    assert!(dst[0].iter().all(|&s| s == 0.5), "Gap at {now}s with {input} Hz input");
                    assert!(timing.delay().bridge_ms < 50.0);
                }
            }
        }
    }
}
