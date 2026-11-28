//! Transfers selected input channels to an independently clocked output device.
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::AtomicU64;
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
    dropped: bool,
    resume_at: Arc<AtomicU64>,
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
    render_period: usize,
    capture_period: usize,
    resume_at: Arc<AtomicU64>,
    fade: Fade,
}

// Smooth transitions to silence or to a new position after a real interruption.
// Keep the fade across callbacks, including callbacks shorter than the fade itself.
struct Fade {
    last: Vec<f32>,
    from: Vec<f32>,
    length: usize,
    left: usize,
}

impl Fade {
    fn begin(&mut self) {
        self.from.copy_from_slice(&self.last);
        self.left = self.length;
    }

    fn apply(&mut self, dst: &mut [Vec<f32>], start: usize, end: usize) {
        if start == end {
            return;
        }
        let count = self.left.min(end - start);
        for (ch, channel) in dst.iter_mut().enumerate() {
            for (i, sample) in channel[start..start + count].iter_mut().enumerate() {
                let old = (self.left - i - 1) as f32 / self.length as f32;
                *sample += old * (self.from[ch] - *sample);
            }
            self.last[ch] = channel[end - 1];
        }
        self.left -= count;
    }
}

pub fn connect(channels: usize, output_rate: f64, block: usize, timing: Arc<Timing>) -> (Writer, Reader) {
    let ratio = timing.input_rate / output_rate;
    let capacity = 16384.max(block.saturating_mul(4)).max((block as f64 * ratio * 4.0).ceil() as usize);
    let (tx, rx) = rtrb::RingBuffer::new(capacity * channels.max(1));
    // At worst the producer delivers one frame per packet.
    let (ptx, prx) = rtrb::RingBuffer::new(capacity + 1);
    let resume_at = Arc::new(AtomicU64::new(0));
    (
        Writer { samples: tx, packets: ptx, channels, written: 0, dropped: false, resume_at: resume_at.clone() },
        Reader {
            samples: rx, packets: prx, timing, channels, capacity, ratio,
            consumed: 0, packet: None, phase: 0.0, correction: 0.0, primed: false,
            capture_period: 0, render_period: 0, resume_at,
            fade: Fade {
                last: vec![0.0; channels],
                from: vec![0.0; channels],
                length: (output_rate * 0.002).round().max(1.0) as usize,
                left: 0,
            },
        },
    )
}

impl Writer {
    /// Publish the timestamp before the corresponding complete, interleaved frames.
    pub fn write(&mut self, frames: usize, received: Instant, mut sample: impl FnMut(usize, usize) -> f32) {
        if self.channels == 0 || frames == 0 {
            return;
        }
        let count = frames.min(self.samples.slots() / self.channels);
        if count == 0 || self.packets.push(Packet { first: self.written, frames: count, received }).is_err() {
            self.dropped = true;
            return;
        }
        let width = self.channels;
        if let Ok(chunk) = self.samples.write_chunk_uninit(count * width) {
            if self.dropped {
                self.resume_at.store(self.written, Release);
            }
            self.dropped = count < frames;
            chunk.fill_from_iter((0..count * width).map(|i| sample(i / width, i % width)));
            self.written += count as u64;
        }
    }
}

impl Reader {
    pub fn observe_render_period(&mut self, frames: usize) -> usize {
        let needed = (frames as f64 * self.ratio).ceil() as usize;
        self.render_period = self.render_period.max(needed);
        needed
    }

    fn locate_packet(&mut self) {
        while self.packets.peek().is_ok_and(|p| p.first <= self.consumed) {
            let packet = self.packets.pop().unwrap();
            self.capture_period = self.capture_period.max(packet.frames);
            self.packet = Some(packet);
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
        // WASAPI can request its entire device buffer before capture has started.
        // Size the reserve from callbacks that actually transfer audio.
        if self.packet.is_none() {
            self.render_period = 0;
            return;
        }
        let needed = self.observe_render_period(n);
        // Keep a full endpoint period for scheduling jitter. A short render call
        // must not shrink the reserve and cause a jump over healthy queued audio.
        let reserve = self.render_period.max(self.capture_period).max((self.timing.input_rate * 0.003) as usize)
            + self.capture_period / 2 + 2;
        let prime = (self.render_period + reserve).min(self.capacity / 2);
        // Skip only at a known capture loss or a backlog several periods deep.
        // Keep the lookahead frame until then so the interpolator never splices across a loss.
        let mut skip = self.resume_at.load(Acquire).saturating_sub(self.consumed).min(available as u64) as usize;
        let backlog_limit = (prime * 3).max((self.timing.input_rate * 0.080) as usize).min(self.capacity * 3 / 4);
        if available >= backlog_limit || !self.primed && available > prime + self.render_period.max(self.capture_period) {
            skip = skip.max(available - prime);
        }
        if skip > 0 {
            let Ok(chunk) = self.samples.read_chunk(skip * self.channels) else { return };
            chunk.commit_all();
            self.consumed += skip as u64;
            self.phase = 0.0;
            available -= skip;
            self.locate_packet();
            self.primed = false;
            self.correction = 0.0;
            self.fade.begin();
        }
        if !self.primed && available < prime {
            self.timing.bridge_us.store(0, Relaxed);
            self.fade.apply(dst, 0, n);
            return;
        }
        if !self.primed {
            self.fade.begin();
        }
        self.primed = true;
        let packet = self.packet.unwrap();
        // Both endpoint estimates refer to the first frame of a callback. Measure how
        // much later this particular frame is rendered, accounting for its packet offset.
        let offset = (self.consumed - packet.first) as f64 + self.phase;
        let wait = render_time.saturating_duration_since(packet.received).as_secs_f64() - offset / self.timing.input_rate;
        self.timing.bridge_us.store((wait.max(0.0) * 1e6).round() as u64, Relaxed);

        let target = (needed + reserve).min(self.capacity / 2);
        let error = (available as f64 - target as f64) / target as f64;
        // Use elapsed audio time so a run of short callbacks does not steer the clock faster.
        let weight = (needed as f64 / (0.5 * self.timing.input_rate)).min(1.0);
        self.correction += weight * ((error * 0.005).clamp(-0.005, 0.005) - self.correction);
        let step = self.ratio * (1.0 + self.correction);
        let width = self.channels;
        let Ok(chunk) = self.samples.read_chunk(available * width) else { return };
        let (a, b) = chunk.as_slices();
        let at = |i: usize| if i < a.len() { a[i] } else { b[i - a.len()] };
        let mut position = self.phase;
        let mut produced = 0;
        for f in 0..n {
            let index = position as usize;
            if index + 1 >= available || position + step >= available as f64 {
                break;
            }
            let frac = (position - index as f64) as f32;
            for (ch, channel) in dst.iter_mut().enumerate() {
                let i = index * width + ch;
                channel[f] = at(i) + frac * (at(i + width) - at(i));
            }
            position += step;
            produced += 1;
        }
        // A stalled capture can resume with an unrelated waveform. Retire its old
        // interpolation tail so the first new samples fade in from silence.
        let used = if produced < n { available } else { position as usize };
        chunk.commit(used * width);
        self.consumed += used as u64;
        self.phase = if produced < n { 0.0 } else { position - used as f64 };
        self.fade.apply(dst, 0, produced);
        if produced < n {
            self.primed = false;
            self.correction = 0.0;
            self.fade.begin();
            self.fade.apply(dst, produced, n);
        }
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
        for packet in 0..6 {
            tx.write(48, t + Duration::from_millis(packet), |f, ch| (packet * 48 + f as u64 + 1000 * ch as u64) as f32);
        }
        let mut dst = vec![vec![0.0; 32]; 2];
        rx.read(&mut dst, 32, t + Duration::from_millis(5));
        for f in 0..32 {
            let gain = (f + 1) as f32 / 48.0;
            assert!((dst[0][f] - gain * 2.0 * f as f32).abs() < 0.01);
            assert!((dst[1][f] - dst[0][f] - gain * 1000.0).abs() < 0.001);
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
        let mut dst = vec![vec![f32::NAN; 128]; 2];
        rx.read(&mut dst, 128, t);
        assert!(dst.iter().flatten().all(|&v| v == 0.0));
        for k in 0..400 {
            tx.write(48, t + Duration::from_millis(k), |_, ch| (k + 1000 * ch as u64) as f32);
        }
        rx.read(&mut dst, 128, t + Duration::from_millis(401));
        assert!(dst[0][96] > 300.0);
        for f in 96..128 {
            assert!((dst[1][f] - dst[0][f] - 1000.0).abs() < 0.001);
        }
        for k in 402..420 {
            tx.write(48, t + Duration::from_millis(k), |_, _| 0.25);
            rx.read(&mut dst, 48, t + Duration::from_millis(k));
        }
        assert!(dst.iter().all(|ch| ch[..48].iter().all(|&v| v == 0.25)));
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

    #[test]
    fn capture_stall_and_restart_fade_across_short_callbacks() {
        let t = Instant::now();
        let (mut tx, mut rx) = connect(1, 48000.0, 32, Timing::new(48000.0, 0, 48000.0, 0));
        let mut dst = vec![vec![0.0; 32]];
        let mut last = 0.0;
        for block in 0..80 {
            if block < 20 || block >= 50 {
                tx.write(32, t, |_, _| if block < 20 { 0.8 } else { -0.8 });
            }
            rx.read(&mut dst, 32, t);
            for &sample in &dst[0] {
                assert!((sample - last).abs() < 0.02, "jump at block {block}");
                last = sample;
            }
            match block {
                19 => assert_eq!(last, 0.8),
                49 => assert_eq!(last, 0.0),
                79 => assert_eq!(last, -0.8),
                _ => {}
            }
        }
    }

    #[test]
    fn variable_render_blocks_keep_a_recorded_stereo_tone_continuous() {
        use crate::builtins::{create, Kind};
        use crate::engine::{ProcessArgs, Transport};

        let start = Instant::now();
        let path = std::env::temp_dir().join(format!("pedalhost-bridge-tone-{}.wav", std::process::id()));
        for (input, output, packet) in [(48000.0, 48000.0, 128), (44100.0, 48000.0, 128), (48000.0, 44100.0, 480)] {
            let timing = Timing::new(input, 0, output, 0);
            let (mut tx, mut rx) = connect(2, output, 1024, timing);
            let mut dst = vec![vec![0.0; 960]; 2];
            let mut written = 0;
            let mut rendered = 0;
            let (mut recorder, mut processor) = create(Kind::Recorder);
            recorder.file = path.to_string_lossy().into_owned();
            processor.set_parameter(0, 1.0);
            recorder.tick(output);
            let mut transport = Transport { playing: true, tempo: 120.0, sample_rate: output, sample_pos: 0, num: 4, den: 4 };
            let tone = std::f64::consts::TAU * 311.0;
            for block in [480, 480, 96, 864, 480, 480, 960, 32, 448].into_iter().cycle().take(400) {
                rendered += block;
                let now = rendered as f64 / output;
                // Some capture callbacks arrive together, as they can under scheduler jitter.
                while (written + 2 * packet) as f64 <= now * input * 1.0002 {
                    for _ in 0..2 {
                        tx.write(packet, start + Duration::from_secs_f64(now), |f, ch| {
                            let angle = tone * (written + f) as f64 / input;
                            if ch == 0 { angle.sin() as f32 } else { angle.cos() as f32 }
                        });
                        written += packet;
                    }
                }
                rx.read(&mut dst, block, start + Duration::from_secs_f64(now));
                processor.process(&ProcessArgs {
                    inputs: &[dst[0].as_ptr(), dst[1].as_ptr()], outputs: &[], frames: block, midi_in: &[], transport: &transport,
                });
                transport.sample_pos += block as i64;
            }
            processor.set_parameter(0, 1.0);
            recorder.tick(output);
            let bytes = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            let mut wav = hound::WavReader::new(&bytes[..]).unwrap();
            assert_eq!(wav.spec().sample_rate, output as u32);
            assert_eq!(wav.spec().channels, 2);
            assert_eq!(wav.duration() as usize, rendered);
            let samples: Vec<_> = wav.samples::<i16>().map(|s| s.unwrap() as f32 / 32767.0).collect();
            let mut previous = None;
            for (f, frame) in samples.chunks_exact(2).enumerate().skip((output * 0.15) as usize) {
                let (l, r) = (frame[0], frame[1]);
                assert!((l * l + r * r - 1.0).abs() < 0.002, "recording gap at frame {f}");
                if let Some((last_l, last_r)) = previous {
                    let advance: f32 = l * last_r - r * last_l;
                    assert!((advance - (tone / output).sin() as f32).abs() < 0.001, "recording jump at frame {f}: {advance}");
                }
                previous = Some((l, r));
            }
        }
    }
}
