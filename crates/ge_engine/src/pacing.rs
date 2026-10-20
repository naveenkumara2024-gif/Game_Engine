//! Frame pacing. With vsync on, presentation paces the loop; `t.MaxFps` adds an optional CPU
//! cap (battery, thermals, or testing frame-rate independence). Rust's `sleep` uses
//! high-resolution waitable timers on Windows, so a short spin after it is enough for accuracy.

use std::time::{Duration, Instant};

ge_core::declare_cvar!(pub static T_MAX_FPS: f32 = (
    "t.MaxFps",
    0.0,
    "Frame-rate cap (0 = uncapped; vsync still applies)"
));

/// Sleep until this close to the deadline, then spin: sleep can overshoot by ~0.5 ms.
const SPIN_MARGIN: Duration = Duration::from_micros(1500);

/// Target frame duration for a cap, or `None` when uncapped.
pub(crate) fn frame_budget(max_fps: f32) -> Option<Duration> {
    (max_fps.is_finite() && max_fps > 0.0)
        .then(|| Duration::from_secs_f64(1.0 / f64::from(max_fps)))
}

/// Blocks until `frame_start + budget` when `t.MaxFps` is set.
pub(crate) fn pace(frame_start: Instant) {
    let Some(budget) = frame_budget(T_MAX_FPS.get()) else {
        return;
    };
    let _span = tracing::info_span!("FramePacing").entered();
    let deadline = frame_start + budget;
    let now = Instant::now();
    if deadline <= now {
        return;
    }
    let remaining = deadline - now;
    if remaining > SPIN_MARGIN {
        std::thread::sleep(remaining - SPIN_MARGIN);
    }
    while Instant::now() < deadline {
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_from_cap() {
        assert_eq!(frame_budget(0.0), None);
        assert_eq!(frame_budget(-5.0), None);
        assert_eq!(frame_budget(f32::NAN), None);
        let b = frame_budget(30.0).unwrap();
        assert!((b.as_secs_f64() - 1.0 / 30.0).abs() < 1e-6);
    }

    #[test]
    fn capped_frames_take_the_budget() {
        T_MAX_FPS.set(100.0);
        let start = Instant::now();
        pace(start);
        let took = start.elapsed();
        T_MAX_FPS.set(0.0);
        assert!(took >= Duration::from_millis(10), "{took:?}");
        assert!(took < Duration::from_millis(30), "{took:?}");
    }
}
