/// The values a program reads per frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameInputs {
    /// Seconds since the start.
    pub time: f64,
    /// Seconds since the previous frame.
    pub delta: f64,
    /// Index of the frame since the start.
    pub frame: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    /// Frames per second: the rate of fixed-rate playback and exports.
    pub fps: f64,
    /// Draw on every display refresh, time following the wall clock, rather than at `fps`.
    pub realtime: bool,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            fps: 60.0,
            realtime: true,
        }
    }
}

/// Longest step real-time playback takes, so a stall (e.g. a hidden window) doesn't jump ahead.
const MAX_REALTIME_DELTA: f64 = 0.1;
/// Longest catch-up fixed-rate playback does after a stall.
const MAX_FIXED_CATCH_UP: f64 = 1.0;

/// Project time. At a fixed rate, time is always `frame / fps` and delta
/// `1 / fps`, so a frame looks the same however it's reached (playback, a
/// seek, an export). In real time, time follows the wall clock.
#[derive(Clone, Debug)]
pub struct Clock {
    pub timing: Timing,
    pub time: f64,
    pub delta: f64,
    pub frame: u64,
    /// Wall-clock seconds since the current fixed-rate frame was due; negative when it was drawn early.
    carry: f64,
}

impl Clock {
    pub fn new(timing: Timing) -> Self {
        Self {
            timing,
            time: 0.0,
            delta: 0.0,
            frame: 0,
            carry: 0.0,
        }
    }

    pub fn inputs(&self) -> FrameInputs {
        FrameInputs {
            time: self.time,
            delta: self.delta,
            frame: self.frame,
        }
    }

    pub fn frame_duration(&self) -> f64 {
        1.0 / self.timing.fps
    }

    /// Advances by `elapsed` wall-clock seconds; returns whether a new frame is due.
    pub fn tick(&mut self, elapsed: f64) -> bool {
        if self.timing.realtime {
            self.delta = elapsed.clamp(0.0, MAX_REALTIME_DELTA);
            self.time += self.delta;
            self.frame += 1;
            return true;
        }
        let wall = elapsed.max(0.0);
        self.carry = (self.carry + wall).min(MAX_FIXED_CATCH_UP);
        // Draw the frame due nearest this animation frame, not the last one already due:
        // when animation frames come as often as frames are due, their jitter would
        // otherwise leave some just short of a frame, skipping that draw and doubling the
        // next step. Leading by half an interval keeps the phase away from that edge.
        let lead = wall.min(self.frame_duration()) / 2.0;
        // The epsilon absorbs float error in elapsed times summing to exactly a frame.
        let steps = ((self.carry + lead) * self.timing.fps + 1e-6).floor();
        if steps <= 0.0 {
            return false;
        }
        self.carry -= steps / self.timing.fps;
        self.set_frame(self.frame as i64 + steps as i64);
        true
    }

    /// Jumps to `time` seconds; at a fixed rate, to the frame showing it.
    pub fn seek(&mut self, time: f64) {
        self.carry = 0.0;
        let clamped = time.max(0.0);
        if self.timing.realtime {
            self.time = clamped;
            self.frame = (clamped * self.timing.fps).floor() as u64;
            self.delta = 0.0;
        } else {
            self.set_frame((clamped * self.timing.fps + 1e-6).floor() as i64);
        }
    }

    /// Moves by whole frames (of `fps`, in real time too).
    pub fn step(&mut self, frames: i64) {
        self.carry = 0.0;
        if self.timing.realtime {
            self.time = (self.time + frames as f64 * self.frame_duration()).max(0.0);
            self.frame = (self.frame as i64 + frames).max(0) as u64;
        } else {
            self.set_frame(self.frame as i64 + frames);
        }
    }

    /// Re-snaps to the new timing's frame grid.
    pub fn set_timing(&mut self, timing: Timing) {
        self.timing = timing;
        self.seek(self.time);
    }

    fn set_frame(&mut self, frame: i64) {
        self.frame = frame.max(0) as u64;
        self.time = self.frame as f64 / self.timing.fps;
        self.delta = self.frame_duration();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed(fps: f64) -> Clock {
        Clock::new(Timing {
            fps,
            realtime: false,
        })
    }

    #[test]
    fn keeps_time_on_the_frame_grid_at_a_fixed_rate() {
        let mut clock = fixed(30.0);
        assert!(!clock.tick(1.0 / 60.0));
        assert!(clock.tick(1.0 / 60.0));
        assert_eq!(clock.frame, 1);
        assert_eq!(clock.time, 1.0 / 30.0);
        assert_eq!(clock.delta, 1.0 / 30.0);
    }

    #[test]
    fn draws_on_every_animation_frame_at_the_frame_rate_despite_jitter() {
        let mut clock = fixed(60.0);
        for i in 1..=120 {
            assert!(clock.tick(if i % 2 == 0 { 0.0164 } else { 0.0169 }));
            assert_eq!(clock.frame, i);
        }
    }

    #[test]
    fn skips_frames_when_wall_time_runs_ahead() {
        let mut clock = fixed(24.0);
        clock.tick(0.25);
        assert_eq!(clock.frame, 6);
        assert_eq!(clock.time, 0.25);
    }

    #[test]
    fn follows_the_wall_clock_in_real_time_capping_stalls() {
        let mut clock = Clock::new(Timing {
            fps: 60.0,
            realtime: true,
        });
        clock.tick(0.02);
        clock.tick(5.0);
        assert_eq!(clock.frame, 2);
        assert!((clock.time - 0.12).abs() < 1e-9);
        assert!((clock.delta - 0.1).abs() < 1e-9);
    }

    #[test]
    fn seeks_to_the_frame_showing_a_time() {
        let mut clock = fixed(10.0);
        clock.seek(1.27);
        assert_eq!(clock.frame, 12);
        assert_eq!(clock.time, 1.2);
        clock.step(-20);
        assert_eq!(clock.frame, 0);
    }

    #[test]
    fn re_snaps_when_the_timing_changes() {
        let mut clock = Clock::new(Timing {
            fps: 60.0,
            realtime: true,
        });
        clock.seek(0.51);
        clock.set_timing(Timing {
            fps: 4.0,
            realtime: false,
        });
        assert_eq!(clock.frame, 2);
        assert_eq!(clock.time, 0.5);
    }
}
