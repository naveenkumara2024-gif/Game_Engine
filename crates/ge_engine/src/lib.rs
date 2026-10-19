//! Engine loop (lore §3.3, §6.3). Owns startup → loop → shutdown; games and tools are
//! libraries that call [`run`]. `App`/`Plugin`/tick-group schedules arrive in M2.

mod orbit_camera;

use std::sync::Arc;
use std::time::{Duration, Instant};

/// Tick period while the window can't present (≈60 Hz keeps simulation alive cheaply).
const MINIMIZED_TICK: Duration = Duration::from_millis(16);

use ge_platform::{PlatformApp, PlatformContext, PlatformEvent, Window, WindowDesc};
use ge_render::{RenderView, Renderer, demo};
use ge_rhi::{Rhi, Swapchain};
use ge_shader::ShaderLibrary;
use tracing::{debug, error, info, info_span, warn};

use crate::orbit_camera::OrbitCamera;

pub use ge_platform::WindowDesc as WindowConfig;

ge_core::declare_cvar!(pub static R_CLEAR_COLOR: [f32; 3] =
    ("r.ClearColor", [0.10, 0.20, 0.40], "Background color behind the scene, linear RGB"));

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
    renderer: Renderer,
}

struct Engine {
    config: EngineConfig,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// Set by [`Engine::request_exit`]; polled once per tick (UE's `RequestEngineExit`).
    exit_reason: Option<String>,
    frame: u64,
    start: Instant,
    last_tick: Instant,
    stats: FrameStats,
    camera: OrbitCamera,
}

impl Engine {
    fn new(config: EngineConfig) -> Self {
        Self {
            config,
            window: None,
            gpu: None,
            exit_reason: None,
            frame: 0,
            start: Instant::now(),
            last_tick: Instant::now(),
            stats: FrameStats::default(),
            camera: OrbitCamera::new(demo::demo_camera()),
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
        let Some(Gpu {
            rhi,
            swapchain,
            renderer,
        }) = &mut self.gpu
        else {
            return;
        };
        // Shader hot reload runs even while minimized, so edits are picked up on restore.
        renderer.update(rhi);
        if !swapchain.is_presentable() {
            // Minimized: nothing to draw, so don't spin a core at millions of ticks/s.
            std::thread::sleep(MINIMIZED_TICK);
            return;
        }
        match rhi.begin_frame(swapchain) {
            Ok(Some(mut frame)) => {
                // M1 has no world yet: the demo scene stands in for extracted render proxies.
                let time = self.start.elapsed().as_secs_f32();
                let view = RenderView {
                    camera: self.camera.camera(),
                    time,
                    clear_color: R_CLEAR_COLOR.get(),
                };
                renderer.render(rhi, &mut frame, &view, &demo::demo_scene(time));
                rhi.end_frame(swapchain, frame);
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
        let mut shaders = ShaderLibrary::new(ShaderLibrary::default_root());
        if let Err(e) = shaders.watch() {
            warn!(error = %e, "shader hot reload unavailable");
        }
        let renderer = Renderer::new(&rhi, swapchain.format(), shaders);
        self.window = Some(window);
        self.gpu = Some(Gpu {
            rhi,
            swapchain,
            renderer,
        });
        self.start = Instant::now();
        self.last_tick = Instant::now();
        Ok(())
    }

    fn event(&mut self, event: PlatformEvent) {
        self.camera.event(&event);
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
            // GPU time comes from timestamps, not fps: on this laptop presentation is capped by
            // the cross-adapter copy (CLAUDE.md §9).
            let timings = self.gpu.as_ref().and_then(|g| g.rhi.gpu_timings());
            let gpu_ms =
                timings.map_or_else(|| "n/a".to_owned(), |t| format!("{:.2} ms", t.total_ms));
            debug!(
                cpu_ms = avg.as_secs_f64() * 1000.0,
                gpu = ?timings.map(|t| &t.passes),
                "frame stats"
            );
            window.set_title(&format!(
                "{} — CPU {:.2} ms | GPU {gpu_ms} | {:.0} fps",
                self.config.window.title,
                avg.as_secs_f64() * 1000.0,
                1.0 / avg.as_secs_f64()
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
