use crate::clock::{Clock, FrameInputs, Timing};
use crate::frame_meter::FrameMeter;

/// Plays a `Clock` on the host's animation frames, drawing whenever there is
/// a frame to draw: on each new frame while playing, and once after a seek or
/// an `invalidate` (e.g. a new program) while paused.
///
/// The host calls `animation_frame` on each display refresh for as long as
/// `wants_animation_frame` holds. A draw that leaves GPU work in flight says
/// so, and the host calls `finished` once that work is done; frames wait for
/// it.
#[derive(Debug)]
pub struct Player {
    pub clock: Clock,
    /// The rate frames are drawn at while playing; reset on play and pause.
    pub meter: FrameMeter,
    /// Frames drawn but not finished yet before the next waits. A deeper queue
    /// lets a program too heavy for the display's rate pile up work, which
    /// the UI's own rendering then waits behind, so the whole window lags,
    /// not just the preview.
    pub max_frames_in_flight: usize,
    playing: bool,
    last_timestamp: Option<f64>,
    dirty: bool,
    frames_in_flight: usize,
}

impl Player {
    pub fn new(timing: Timing) -> Self {
        Self {
            clock: Clock::new(timing),
            meter: FrameMeter::default(),
            max_frames_in_flight: 1,
            playing: false,
            last_timestamp: None,
            dirty: true,
            frames_in_flight: 0,
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn play(&mut self) {
        if self.playing {
            return;
        }
        self.playing = true;
        self.last_timestamp = None;
        self.meter.reset();
    }

    pub fn pause(&mut self) {
        self.playing = false;
        self.meter.reset();
    }

    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    pub fn seek(&mut self, time: f64) {
        self.clock.seek(time);
        self.invalidate();
    }

    pub fn step(&mut self, frames: i64) {
        self.pause();
        self.clock.step(frames);
        self.invalidate();
    }

    /// Plays at `timing`, re-snapping time to its frame grid when it changes:
    /// in real time, frames count the display's refreshes, so a re-snap may
    /// move the frame.
    pub fn set_timing(&mut self, timing: Timing) {
        if self.clock.timing == timing {
            return;
        }
        self.clock.set_timing(timing);
        self.invalidate();
    }

    /// Draws the current frame again on the next animation frame.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn wants_animation_frame(&self) -> bool {
        self.playing || (self.dirty && self.frames_in_flight < self.max_frames_in_flight)
    }

    /// Runs an animation frame at `timestamp` seconds. `draw` is called with
    /// the frame to draw, if there is one, and returns whether it left work
    /// in flight.
    pub fn animation_frame(&mut self, timestamp: f64, draw: impl FnOnce(FrameInputs) -> bool) {
        if self.playing {
            let elapsed = self.last_timestamp.map_or(0.0, |last| timestamp - last);
            self.last_timestamp = Some(timestamp);
            if self.clock.tick(elapsed) {
                self.dirty = true;
            }
        }
        if self.dirty && self.frames_in_flight < self.max_frames_in_flight {
            if self.playing {
                self.meter.record(timestamp);
            }
            self.draw_now(draw);
        }
    }

    /// Draws the current frame right away rather than on the next animation frame.
    pub fn draw_now(&mut self, draw: impl FnOnce(FrameInputs) -> bool) {
        self.dirty = false;
        if draw(self.clock.inputs()) {
            self.frames_in_flight += 1;
        }
    }

    /// Releases a frame left in flight, whether it succeeded or not.
    pub fn finished(&mut self) {
        self.frames_in_flight = self.frames_in_flight.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REALTIME: Timing = Timing {
        fps: 60.0,
        realtime: true,
    };

    /// Drives a player like a display would, counting draws.
    struct Display {
        now: f64,
        drawn: u32,
    }

    impl Display {
        fn new() -> Self {
            Self { now: 0.0, drawn: 0 }
        }

        fn next_frame(&mut self, player: &mut Player, in_flight: bool) {
            self.now += 1.0 / 60.0;
            if !player.wants_animation_frame() {
                return;
            }
            player.animation_frame(self.now, |_| {
                self.drawn += 1;
                in_flight
            });
        }
    }

    #[test]
    fn doesnt_draw_a_frame_while_the_last_one_isnt_finished() {
        let mut player = Player::new(REALTIME);
        let mut display = Display::new();
        player.play();
        for _ in 0..3 {
            display.next_frame(&mut player, true);
        }
        assert_eq!(display.drawn, 1);

        player.finished();
        display.next_frame(&mut player, true);
        assert_eq!(display.drawn, 2);
    }

    #[test]
    fn draws_up_to_its_limit_of_frames_before_one_finishes() {
        let mut player = Player::new(REALTIME);
        let mut display = Display::new();
        player.max_frames_in_flight = 3;
        player.play();
        for _ in 0..5 {
            display.next_frame(&mut player, true);
        }
        assert_eq!(display.drawn, 3);
    }

    #[test]
    fn draws_what_was_invalidated_once_a_frame_finishes_even_paused() {
        let mut player = Player::new(REALTIME);
        let mut display = Display::new();
        display.next_frame(&mut player, true);
        player.invalidate();
        display.next_frame(&mut player, true);
        assert_eq!(display.drawn, 1);
        assert!(!player.wants_animation_frame());

        player.finished();
        assert!(player.wants_animation_frame());
        display.next_frame(&mut player, true);
        assert_eq!(display.drawn, 2);
    }

    #[test]
    fn rests_when_paused_and_nothing_changed() {
        let mut player = Player::new(REALTIME);
        let mut display = Display::new();
        display.next_frame(&mut player, false);
        assert!(!player.wants_animation_frame());
        player.seek(1.0);
        assert!(player.wants_animation_frame());
    }

    #[test]
    fn only_counts_drawn_frames_in_its_rate() {
        let mut player = Player::new(REALTIME);
        let mut display = Display::new();
        player.play();
        for _ in 0..120 {
            display.next_frame(&mut player, true);
        }
        assert!(player.meter.rate.map_or(0.0, |r| r.fps) < 2.0);
    }

    #[test]
    fn keeps_its_frame_when_given_the_timing_it_has() {
        let mut player = Player::new(REALTIME);
        // In real time, frames count the display's refreshes rather than follow time.
        player.clock.time = 1.0;
        player.clock.frame = 100;
        player.set_timing(REALTIME);
        assert_eq!(player.clock.frame, 100);
        player.set_timing(Timing {
            fps: 30.0,
            realtime: true,
        });
        assert_eq!(player.clock.frame, 30);
    }
}
