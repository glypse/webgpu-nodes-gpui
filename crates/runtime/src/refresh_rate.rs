/// The display's refresh rate in Hz, from the median time between animation
/// frames. Animation frames slow down while the GPU falls behind, so this
/// only holds when measured with little else drawing.
#[derive(Clone, Debug)]
pub struct RefreshRateMeter {
    frames: usize,
    stamps: Vec<f64>,
}

impl RefreshRateMeter {
    /// Measures over `frames` intervals.
    pub fn new(frames: usize) -> Self {
        Self {
            frames: frames.max(1),
            stamps: Vec::new(),
        }
    }

    /// Counts an animation frame at `timestamp` seconds; returns the rate
    /// once enough have been, starting over after.
    pub fn record(&mut self, timestamp: f64) -> Option<f64> {
        self.stamps.push(timestamp);
        if self.stamps.len() <= self.frames {
            return None;
        }
        let mut intervals: Vec<f64> = self.stamps.windows(2).map(|w| w[1] - w[0]).collect();
        self.stamps.clear();
        intervals.sort_by(f64::total_cmp);
        let median = intervals[intervals.len() / 2];
        Some(if median > 0.0 { 1.0 / median } else { 60.0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_median_interval() {
        let mut meter = RefreshRateMeter::new(4);
        let mut rate = None;
        for t in [0.0, 1.0 / 120.0, 2.0 / 120.0, 0.5, 0.5 + 1.0 / 120.0] {
            rate = meter.record(t);
        }
        assert!((rate.unwrap() - 120.0).abs() < 1e-6);
        assert_eq!(meter.record(1.0), None);
    }
}
