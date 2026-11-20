#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRate {
    /// Frames drawn per second.
    pub fps: f64,
    /// Mean time between frames, in seconds.
    pub frame_time: f64,
    /// Longest time between two frames, in seconds: a stutter the mean hides.
    pub worst_frame_time: f64,
    /// Mean time the GPU spent drawing a frame, in seconds, when it was timed.
    pub gpu_time: Option<f64>,
}

/// Measures the rate frames are drawn at, over windows of `window` seconds so
/// that the rate shown holds still long enough to be read.
#[derive(Clone, Debug)]
pub struct FrameMeter {
    pub rate: Option<FrameRate>,
    window: f64,
    start: Option<f64>,
    last: f64,
    count: u32,
    worst: f64,
    gpu_total: f64,
    gpu_count: u32,
}

impl Default for FrameMeter {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl FrameMeter {
    pub fn new(window: f64) -> Self {
        Self {
            rate: None,
            window,
            start: None,
            last: 0.0,
            count: 0,
            worst: 0.0,
            gpu_total: 0.0,
            gpu_count: 0,
        }
    }

    /// Counts a frame drawn at `timestamp` seconds.
    pub fn record(&mut self, timestamp: f64) {
        let Some(start) = self.start else {
            self.start = Some(timestamp);
            self.last = timestamp;
            return;
        };
        self.count += 1;
        self.worst = self.worst.max(timestamp - self.last);
        self.last = timestamp;
        let elapsed = timestamp - start;
        if elapsed < self.window {
            return;
        }
        self.rate = Some(FrameRate {
            fps: f64::from(self.count) / elapsed,
            frame_time: elapsed / f64::from(self.count),
            worst_frame_time: self.worst,
            gpu_time: (self.gpu_count > 0).then(|| self.gpu_total / f64::from(self.gpu_count)),
        });
        self.start = Some(timestamp);
        self.clear_window();
    }

    /// Counts the GPU time of a frame drawn in the current window, in seconds.
    pub fn record_gpu_time(&mut self, seconds: f64) {
        if self.start.is_none() {
            return;
        }
        self.gpu_total += seconds;
        self.gpu_count += 1;
    }

    pub fn reset(&mut self) {
        self.rate = None;
        self.start = None;
        self.clear_window();
    }

    fn clear_window(&mut self) {
        self.count = 0;
        self.worst = 0.0;
        self.gpu_total = 0.0;
        self.gpu_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn measures_the_rate_once_a_window_has_passed() {
        let mut meter = FrameMeter::new(1.0);
        for i in 0..=45 {
            meter.record(f64::from(i) / 45.0);
            if i < 45 {
                assert_eq!(meter.rate, None);
            }
        }
        let rate = meter.rate.unwrap();
        assert!(close(rate.fps, 45.0));
        assert!(close(rate.frame_time, 1.0 / 45.0));
    }

    #[test]
    fn keeps_the_longest_gap_between_frames() {
        let mut meter = FrameMeter::new(1.0);
        for t in [0.0, 0.1, 0.6, 0.7, 1.0] {
            meter.record(t);
        }
        let rate = meter.rate.unwrap();
        assert!(close(rate.fps, 4.0));
        assert!(close(rate.worst_frame_time, 0.5));
    }

    #[test]
    fn averages_the_gpu_time_of_the_windows_frames() {
        let mut meter = FrameMeter::new(1.0);
        meter.record_gpu_time(1.0);
        meter.record(0.0);
        meter.record_gpu_time(0.002);
        meter.record_gpu_time(0.004);
        assert_eq!(meter.rate, None);
        meter.record(1.0);
        assert!(close(meter.rate.unwrap().gpu_time.unwrap(), 0.003));
        meter.record(2.0);
        assert_eq!(meter.rate.unwrap().gpu_time, None);
    }

    #[test]
    fn starts_over_after_a_reset() {
        let mut meter = FrameMeter::new(1.0);
        meter.record(0.0);
        meter.record(1.0);
        meter.reset();
        assert_eq!(meter.rate, None);
        meter.record(5.0);
        meter.record(5.5);
        assert_eq!(meter.rate, None);
    }
}
