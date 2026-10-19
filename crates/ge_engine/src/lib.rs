//! Engine loop (lore §3.3, §6.3). Owns startup → loop → shutdown; games and tools are
//! libraries that call [`run`]. `App`/`Plugin`/tick-group schedules arrive in M2.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tick period while the window can't present (≈60 Hz keeps simulation alive cheaply).
const MINIMIZED_TICK: Duration = Duration::from_millis(16);

use ge_platform::{PlatformApp, PlatformContext, PlatformEvent, Window, WindowDesc};
use ge_rhi::{ClearColor, Rhi, Swapchain};
use tracing::{error, info, info_span, warn};

pub use ge_platform::WindowDesc as WindowConfig;

ge_core::declare_cvar!(pub static R_CLEAR_COLOR: [f32; 3] =
    ("r.ClearColor", [0.10, 0.20, 0.40], "Backbuffer clear color, linear RGB (M0 test hook)"));

#[derive(Clone, Debug, Default)]
pub struct EngineConfig {
    pub window: WindowDesc,
    /// Process arguments; `-cvar name=value` pairs are applied before init.
    pub args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Platform(#[from] ge_platform::PlatformError),
}

/// Startup phases, in order: logging → command-line cvars → platform loop → window → RHI.
pub fn run(config: EngineConfig) -> Result<(), EngineError> {
    ge_core::log::init_logging();
    info!(version = env!("CARGO_PKG_VERSION"), "engine starting");

    for e in ge_core::cvar::apply_command_line(&config.args) {
        warn!(error = %e, "ignoring command-line cvar");
    }

    let mut engine = Engine::new(config);
    ge_platform::run(&mut engine)?;
    info!("engine stopped");
    Ok(())
}

struct Gpu {
    rhi: Rhi,
    swapchain: Swapchain,
}

struct Engine {
    config: EngineConfig,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// Set by [`Engine::request_exit`]; polled once per tick (UE's `RequestEngineExit`).
    exit_reason: Option<String>,
    frame: u64,
    last_tick: Instant,
    stats: FrameStats,
}

impl Engine {
    fn new(config: EngineConfig) -> Self {
        Self {
            config,
            window: None,
            gpu: None,
            exit_reason: None,
            frame: 0,
            last_tick: Instant::now(),
            stats: FrameStats::default(),
        }
    }

    /// Cooperative exit: the loop finishes the current tick, then shuts down cleanly.
    fn request_exit(&mut self, reason: &str) {
        if self.exit_reason.is_none() {
            info!(reason, "exit requested");
            self.exit_reason = Some(reason.to_owned());
        }
    }

    fn render(&mut self) {
        let Some(gpu) = &mut self.gpu else { return };
        if !gpu.swapchain.is_presentable() {
            // Minimized: nothing to draw, so don't spin a core at millions of ticks/s.
            std::thread::sleep(MINIMIZED_TICK);
            return;
        }
        match gpu.rhi.begin_frame(&mut gpu.swapchain) {
            Ok(Some(mut frame)) => {
                let [r, g, b] = R_CLEAR_COLOR.get();
                frame.clear(ClearColor([r, g, b, 1.0]));
                gpu.rhi.end_frame(&mut gpu.swapchain, frame);
            }
            Ok(None) => {}
            Err(e) => {
                error!(error = %e, "fatal render error");
                self.request_exit("render error");
            }
        }
    }
}

impl PlatformApp for Engine {
    fn init(&mut self, ctx: &PlatformContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
        let _span = info_span!("Engine::init").entered();
        let window = ctx.create_window(&self.config.window)?;
        let (rhi, swapchain) = Rhi::new(window.clone(), window.size())?;
        self.window = Some(window);
        self.gpu = Some(Gpu { rhi, swapchain });
        self.last_tick = Instant::now();
        Ok(())
    }

    fn event(&mut self, event: PlatformEvent) {
        match event {
            PlatformEvent::CloseRequested => self.request_exit("window closed"),
            PlatformEvent::Resized { width, height } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.swapchain.resize(&gpu.rhi, width, height);
                }
            }
            PlatformEvent::Key {
                key,
                pressed: true,
                repeat: false,
            } if key == "Escape" => {
                self.request_exit("escape pressed");
            }
            _ => {}
        }
    }

    fn tick(&mut self) -> bool {
        let _span = info_span!("Frame", frame = self.frame).entered();
        let now = Instant::now();
        let dt = now - self.last_tick;
        self.last_tick = now;

        if let Some(window) = &self.window
            && let Some(avg) = self.stats.push(dt)
        {
            let ms = avg.as_secs_f64() * 1000.0;
            window.set_title(&format!(
                "{} — {ms:.2} ms ({:.0} fps)",
                self.config.window.title,
                1000.0 / ms
            ));
        }

        self.render();
        self.frame += 1;
        self.exit_reason.is_none()
    }

    fn shutdown(&mut self) {
        info!(
            frames = self.frame,
            reason = self.exit_reason.as_deref().unwrap_or("os"),
            "shutting down"
        );
        // GPU objects before the window they present to.
        self.gpu = None;
        self.window = None;
    }
}

/// Averages frame time over ~1 s windows for the title-bar readout.
#[derive(Default)]
struct FrameStats {
    accum: Duration,
    frames: u32,
}

impl FrameStats {
    fn push(&mut self, dt: Duration) -> Option<Duration> {
        self.accum += dt;
        self.frames += 1;
        if self.accum < Duration::from_secs(1) {
            return None;
        }
        let avg = self.accum / self.frames;
        *self = Self::default();
        Some(avg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_stats_average_once_per_second() {
        let mut s = FrameStats::default();
        for _ in 0..9 {
            assert_eq!(s.push(Duration::from_millis(100)), None);
        }
        assert_eq!(
            s.push(Duration::from_millis(100)),
            Some(Duration::from_millis(100))
        );
        assert_eq!(s.frames, 0);
    }

    #[test]
    fn clear_color_cvar_registered() {
        assert!(ge_core::cvar::find("r.ClearColor").is_some());
    }
}
