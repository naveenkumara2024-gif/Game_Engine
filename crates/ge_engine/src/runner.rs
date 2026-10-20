//! Platform glue: drives an [`App`] from the OS event loop, owns the window and the GPU, and
//! hands each frame's `RenderSnapshot` to the renderer (lore §6.3).

use std::sync::Arc;
use std::time::{Duration, Instant};

use ge_platform::{PlatformApp, PlatformContext, PlatformEvent, Window};
use ge_render::{RenderSnapshot, RenderView, Renderer};
use ge_rhi::{Rhi, Swapchain};
use ge_shader::ShaderLibrary;
use tracing::{debug, error, info, info_span, warn};

use crate::input::PlatformEvents;
use crate::{App, R_CLEAR_COLOR, pacing};

/// Tick period while the window can't present (≈60 Hz keeps simulation alive cheaply).
const MINIMIZED_TICK: Duration = Duration::from_millis(16);

struct Gpu {
    rhi: Rhi,
    swapchain: Swapchain,
    renderer: Renderer,
}

pub(crate) struct Engine<'a> {
    app: &'a mut App,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    frame: u64,
    last_tick: Instant,
    stats: FrameStats,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(app: &'a mut App) -> Self {
        Self {
            app,
            window: None,
            gpu: None,
            frame: 0,
            last_tick: Instant::now(),
            stats: FrameStats::default(),
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
                let empty = RenderSnapshot::default();
                let snapshot = self
                    .app
                    .world()
                    .get_resource::<RenderSnapshot>()
                    .unwrap_or(&empty);
                let view = RenderView {
                    camera: snapshot.camera.unwrap_or_default(),
                    time: snapshot.time,
                    clear_color: R_CLEAR_COLOR.get(),
                };
                renderer.render(rhi, &mut frame, &view, &snapshot.items);
                rhi.end_frame(swapchain, frame);
            }
            Ok(None) => {}
            Err(e) => {
                error!(error = %e, "fatal render error");
                self.app.request_exit("render error");
            }
        }
    }

    fn update_title(&mut self, dt: Duration) {
        let (Some(window), Some(avg)) = (&self.window, self.stats.push(dt)) else {
            return;
        };
        // GPU time comes from timestamps, not fps: on this laptop presentation is capped by the
        // cross-adapter copy (CLAUDE.md §9).
        let timings = self.gpu.as_ref().and_then(|g| g.rhi.gpu_timings());
        let gpu_ms = timings.map_or_else(|| "n/a".to_owned(), |t| format!("{:.2} ms", t.total_ms));
        debug!(
            cpu_ms = avg.as_secs_f64() * 1000.0,
            gpu = ?timings.map(|t| &t.passes),
            "frame stats"
        );
        window.set_title(&format!(
            "{} — CPU {:.2} ms | GPU {gpu_ms} | {:.0} fps",
            self.app.window.title,
            avg.as_secs_f64() * 1000.0,
            1.0 / avg.as_secs_f64()
        ));
    }
}

impl PlatformApp for Engine<'_> {
    fn init(&mut self, ctx: &PlatformContext<'_>) -> Result<(), Box<dyn std::error::Error>> {
        let _span = info_span!("Engine::init").entered();
        let window = ctx.create_window(&self.app.window)?;
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
        self.app.startup();
        self.last_tick = Instant::now();
        Ok(())
    }

    fn event(&mut self, event: PlatformEvent) {
        match &event {
            PlatformEvent::CloseRequested => self.app.request_exit("window closed"),
            PlatformEvent::Resized { width, height } => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.swapchain.resize(&gpu.rhi, *width, *height);
                }
            }
            _ => {}
        }
        self.app
            .world_mut()
            .resource_mut::<PlatformEvents>()
            .0
            .push(event);
    }

    fn tick(&mut self) -> bool {
        let _span = info_span!("Frame", frame = self.frame).entered();
        let frame_start = Instant::now();
        let dt = frame_start - self.last_tick;
        self.last_tick = frame_start;
        self.update_title(dt);

        {
            let _span = info_span!("GameThread").entered();
            self.app.update(dt);
        }
        if self.app.exit_requested().is_some() {
            return false;
        }
        self.render();
        self.frame += 1;
        pacing::pace(frame_start);
        true
    }

    fn shutdown(&mut self) {
        info!(
            frames = self.frame,
            reason = self.app.exit_requested().unwrap_or("os"),
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
}
