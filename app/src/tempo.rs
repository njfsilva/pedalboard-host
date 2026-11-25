//! Tap tempo (0x140089060 / 0x140088e30): the last four taps, reset after a 2 s pause,
//! tempo = 60 / average gap.

use std::time::Instant;

pub struct TapTempo {
    start: Instant,
    taps: Vec<f64>,
}

impl Default for TapTempo {
    fn default() -> Self {
        TapTempo { start: Instant::now(), taps: Vec::with_capacity(4) }
    }
}

impl TapTempo {
    /// Registers a tap; returns the new tempo once there are two or more taps.
    pub fn tap(&mut self) -> Option<f64> {
        let now = self.start.elapsed().as_secs_f64();
        if self.taps.last().map_or(false, |&t| now - t > 2.0) {
            self.taps.clear();
        }
        if self.taps.len() == 4 {
            self.taps.remove(0);
        }
        self.taps.push(now);
        let gaps: Vec<f64> = self.taps.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 0.0).collect();
        if gaps.is_empty() {
            return None;
        }
        let avg = gaps.iter().sum::<f64>() / gaps.len() as f64;
        let bpm = 60.0 / avg;
        (bpm > 0.0).then_some(bpm)
    }
}
