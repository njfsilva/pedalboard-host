use super::asio::{AsioDevice, Use};
use super::bridge::{self, Timing};
use super::wasapi::{InputSetup, InputSink, OutputSetup};
use super::{AudioRuntime, DeviceInfo};
use crate::settings::AudioConfig;
use cpal::traits::StreamTrait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub struct MixedDevice {
    pub asio: AsioDevice,
    windows: Option<cpal::Stream>,
    failed: Arc<AtomicBool>,
    asio_input: bool,
}

impl MixedDevice {
    pub fn open(cfg: &AudioConfig, hwnd: isize, runtime: Arc<Mutex<AudioRuntime>>) -> Result<(Self, DeviceInfo), String> {
        let asio_input = cfg.input_asio();
        let usage = if asio_input { Use::Capture } else { Use::Playback };
        let (mut asio, mut info) = AsioDevice::open(cfg, usage, hwnd, runtime.clone())?;
        let failed = Arc::new(AtomicBool::new(false));
        let windows = if asio_input {
            let mut output = OutputSetup::probe(cfg, Some(info.sample_rate))?;
            let timing = Timing::new(
                info.sample_rate,
                if info.active_ins.is_empty() { 0 } else { info.in_latency },
                output.info.sample_rate, output.info.out_latency,
            );
            let (writer, reader) = bridge::connect(info.active_ins.len(), output.info.sample_rate, info.block.max(super::wasapi::SCRATCH), timing.clone());
            asio.capture_to(writer);
            output.info.in_names = info.in_names;
            output.info.active_ins = info.active_ins;
            let stream = output.build(runtime, None, Some(reader), failed.clone())?;
            output.info.name = format!("{} → {}", info.name, output.info.name);
            output.info.backend = "ASIO → WASAPI".into();
            output.info.rates = info.rates;
            output.info.blocks = info.blocks;
            output.info.mixed = Some(timing);
            info = output.info;
            Some(stream)
        } else {
            let input = InputSetup::probe(cfg, info.sample_rate as u32, true)?;
            let rate = input.as_ref().map_or(info.sample_rate, |i| i.rate());
            info.active_ins = input.as_ref().map_or_else(Vec::new, |i| i.active.clone());
            info.in_names = input.as_ref().map_or_else(Vec::new, |i| i.names());
            let timing = Timing::new(rate, 0, info.sample_rate, info.out_latency);
            let (writer, reader) = bridge::connect(info.active_ins.len(), info.sample_rate, info.block, timing.clone());
            asio.render_from(reader, info.active_ins.len());
            info.name = format!("{} → {}", input.as_ref().map_or("No input", |i| i.name.as_str()), info.name);
            info.backend = "WASAPI → ASIO".into();
            info.mixed = Some(timing.clone());
            match input {
                Some(i) if !i.active.is_empty() => Some(i.build(InputSink::Mixed(writer, i.active.clone(), timing), failed.clone())?),
                _ => None,
            }
        };
        Ok((Self { asio, windows, failed, asio_input }, info))
    }

    pub fn start(&mut self) -> Result<(), String> {
        if self.asio_input {
            self.asio.start()?;
        }
        if let Some(s) = &self.windows {
            s.play().map_err(|e| e.to_string())?;
        }
        if !self.asio_input {
            self.asio.start()?;
        }
        Ok(())
    }

    pub fn device_lost(&self) -> bool {
        super::asio::reset_requested() || self.failed.swap(false, Ordering::Relaxed)
    }

    pub fn delay(&self, info: &DeviceInfo) -> Option<bridge::Delay> {
        let timing = info.mixed.as_ref()?;
        if let Some((input, output)) = self.asio.updated_latencies() {
            if self.asio_input {
                let samples = if info.active_ins.is_empty() { 0 } else { input };
                timing.set_input(std::time::Duration::from_secs_f64(samples as f64 / timing.input_rate));
            } else {
                timing.set_output(std::time::Duration::from_secs_f64(output as f64 / info.sample_rate));
            }
        }
        Some(timing.delay())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore = "requires an ASIO capture device and WASAPI output"]
    fn live_asio_to_wasapi() {
        check_live_route(true);
    }

    #[test]
    #[ignore = "requires an accessible WASAPI capture device and ASIO output"]
    fn live_wasapi_to_asio() {
        check_live_route(false);
    }

    fn check_live_route(asio_input: bool) {
        static DRIVER: Mutex<()> = Mutex::new(());
        let _exclusive = DRIVER.lock().unwrap_or_else(|e| e.into_inner());
        let mut engine = Engine::new();
        let mut cfg = AudioConfig::default();
        let driver = std::env::var("MIXED_ASIO_DRIVER").unwrap_or_default();
        cfg.set_systems(asio_input, !asio_input);
        if asio_input {
            cfg.input_device = driver;
            cfg.device = std::env::var("MIXED_WINDOWS_OUTPUT").unwrap_or_default();
        } else {
            cfg.device = driver;
            cfg.input_device = std::env::var("MIXED_WINDOWS_INPUT").unwrap_or_default();
        }
        let info = engine.open(&cfg, 0).unwrap();
        let timing = info.mixed.unwrap();
        let before = engine.shared.callbacks.load(Ordering::Relaxed);
        engine.start().unwrap(); // The empty patch produces silence.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && (timing.delay().bridge_ms == 0.0 || engine.shared.callbacks.load(Ordering::Relaxed) < before + 20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let delay = timing.delay();
        eprintln!("{} ({}) input {:.2} ms, bridge {:.2} ms, output {:.2} ms", info.name, info.backend, delay.input_ms, delay.bridge_ms, delay.output_ms);
        assert!(engine.shared.callbacks.load(Ordering::Relaxed) >= before + 20);
        assert!(delay.bridge_ms > 0.0 && delay.output_ms > 0.0);
        engine.close();
    }
}
