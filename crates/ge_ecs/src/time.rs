//! Frame time and the fixed simulation step (lore §6.3, CLAUDE.md §7 "Determinism").
//!
//! `Time` is the variable frame clock. `FixedTime` is an accumulator that the engine drains in
//! whole steps to run the `FixedUpdate` tick group; whatever is left over becomes the
//! interpolation fraction for rendering between two fixed states.

use std::time::Duration;

use crate::prelude::*;

/// Longest frame delta the simulation will see. A longer frame (debugger break, window drag,
/// shader compile) is treated as a hitch, so the game slows down instead of teleporting.
pub const MAX_DELTA: Duration = Duration::from_millis(250);

#[derive(Resource, Clone, Debug, Default)]
pub struct Time {
    delta: Duration,
    elapsed: Duration,
    frame: u64,
}

impl Time {
    /// Advances one frame. Returns the (clamped) delta the simulation will use.
    pub fn advance(&mut self, raw_delta: Duration) -> Duration {
        self.delta = raw_delta.min(MAX_DELTA);
        self.elapsed += self.delta;
        self.frame += 1;
        self.delta
    }

    pub fn delta(&self) -> Duration {
        self.delta
    }

    pub fn delta_secs(&self) -> f32 {
        self.delta.as_secs_f32()
    }

    /// Simulation time since startup (sum of clamped deltas).
    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }

    /// Frames advanced so far.
    pub fn frame(&self) -> u64 {
        self.frame
    }
}

#[derive(Resource, Clone, Debug)]
pub struct FixedTime {
    step: Duration,
    accumulator: Duration,
    max_steps_per_frame: u32,
    steps: u64,
}

impl Default for FixedTime {
    fn default() -> Self {
        Self::from_hz(60.0)
    }
}

impl FixedTime {
    pub fn from_hz(hz: f64) -> Self {
        Self {
            step: Duration::from_secs_f64(1.0 / hz.max(1.0)),
            accumulator: Duration::ZERO,
            max_steps_per_frame: 5,
            steps: 0,
        }
    }

    pub fn set_hz(&mut self, hz: f64) {
        self.step = Duration::from_secs_f64(1.0 / hz.max(1.0));
    }

    pub fn step(&self) -> Duration {
        self.step
    }

    /// The fixed delta every `FixedUpdate` system should integrate with.
    pub fn step_secs(&self) -> f32 {
        self.step.as_secs_f32()
    }

    /// Fixed steps run since startup.
    pub fn steps(&self) -> u64 {
        self.steps
    }

    /// Adds a frame's delta. If more than `max_steps_per_frame` steps would be owed, the excess
    /// is dropped (spiral-of-death guard) and the dropped time is returned.
    pub fn accumulate(&mut self, delta: Duration) -> Duration {
        self.accumulator += delta;
        let cap = self.step * self.max_steps_per_frame;
        if self.accumulator > cap {
            let dropped = self.accumulator - cap;
            self.accumulator = cap;
            dropped
        } else {
            Duration::ZERO
        }
    }

    /// Consumes one step if enough time has accumulated.
    pub fn expend(&mut self) -> bool {
        if self.accumulator >= self.step {
            self.accumulator -= self.step;
            self.steps += 1;
            true
        } else {
            false
        }
    }

    /// How far (0..1) the frame is between the last fixed state and the next one.
    pub fn overstep_fraction(&self) -> f32 {
        (self.accumulator.as_secs_f64() / self.step.as_secs_f64()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn hitch_is_clamped() {
        let mut t = Time::default();
        assert_eq!(t.advance(ms(16)), ms(16));
        assert_eq!(t.advance(Duration::from_secs(3)), MAX_DELTA);
        assert_eq!(t.frame(), 2);
        assert_eq!(t.elapsed_secs(), 0.266);
    }

    #[test]
    fn fixed_steps_match_elapsed_time() {
        let mut f = FixedTime::from_hz(50.0); // 20 ms steps
        let mut steps = 0;
        // 7 ms frames for 1 s.
        for _ in 0..1000 / 7 {
            f.accumulate(ms(7));
            while f.expend() {
                steps += 1;
            }
            assert!((0.0..1.0).contains(&f.overstep_fraction()));
        }
        // 142 frames * 7 ms = 994 ms → 49 whole steps.
        assert_eq!(steps, 49);
        assert_eq!(f.steps(), 49);
    }

    #[test]
    fn slow_frames_run_several_steps() {
        let mut f = FixedTime::from_hz(60.0);
        f.accumulate(ms(51)); // 3 steps of 16.67 ms (1/60 s rounds up to 16_666_667 ns)
        let mut n = 0;
        while f.expend() {
            n += 1;
        }
        assert_eq!(n, 3);
    }

    #[test]
    fn spiral_of_death_is_capped() {
        let mut f = FixedTime::from_hz(60.0);
        let dropped = f.accumulate(Duration::from_millis(200));
        assert!(dropped > Duration::ZERO);
        let mut n = 0;
        while f.expend() {
            n += 1;
        }
        assert_eq!(n, 5);
    }
}
