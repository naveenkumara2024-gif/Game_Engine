//! The pipelined render thread (plan 0008 step 3; law 3; lore Â§2.5, Â§3.6, Â§6.3).
//!
//! ```text
//! Main   [ tick N Â· extract Â· submit(N) ][ tick N+1 Â· extract Â· submit(N+1) waits for N ] â€¦
//! Render           [ frame N-1 â€¦â€¦â€¦ ][ frame N: apply changes Â· graph Â· record Â· present ] â€¦
//! ```
//!
//! - The main thread owns the window and the simulation; `RenderThread` owns the GPU (`Rhi`,
//!   swapchain, renderer). They share no mutable state: commands go one way and events come
//!   back, both over channels.
//! - Frames go through a **rendezvous** channel: `submit(N)` returns only once the render
//!   thread has taken N, which it does only after finishing N-1. So the main thread is never
//!   more than one frame ahead (law 3), and [`RenderLink::lag`] measures that.
//! - Snapshots are recycled: the render thread sends each one back after drawing, and the
//!   main thread reuses it for the next extract. Three exist in steady state (being
//!   extracted, being drawn, spare), so frames don't allocate them. Anything the render side
//!   didn't drain (a skipped frame's changes) comes back with it and merges into the next
//!   frame, so nothing is lost.
//! - `r.RenderThread 0` runs the same worker inline on the main thread, for bisecting
//!   "threading bug vs rendering bug" (like UE's `r.rhithread.enable`).

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use ge_render::{FrameInputs, RenderSnapshot, RenderView, Renderer};
use ge_rhi::{GpuTimings, Rhi, Swapchain};
use tracing::{error, info, info_span};

ge_core::declare_cvar!(pub static R_RENDER_THREAD: bool = (
    "r.RenderThread",
    true,
    "Render on a dedicated RenderThread, one frame behind the game thread (0 = render inline on \
     the main thread; a debugging aid). Read at startup",
    ge_core::CVarFlags::READ_ONLY,
));

/// Tick period while the window can't present (â‰ˆ60 Hz keeps simulation alive cheaply).
const MINIMIZED_TICK: Duration = Duration::from_millis(16);

/// GPU timings are cloned back to the main thread every this many frames (the title and
/// `EngineStats` refresh about once a second).
const TIMINGS_EVERY: u64 = 16;

/// One frame for the render side.
pub(crate) struct RenderFrame {
    pub index: u64,
    pub snapshot: Box<RenderSnapshot>,
    /// Background radiance (linear RGB) when the snapshot has no world.
    pub background: [f32; 3],
}

pub(crate) enum RenderCommand {
    Frame(RenderFrame),
    Resize(u32, u32),
    Shutdown,
}

pub(crate) enum RenderEvent {
    /// Frame `index` is finished (drawn, or absorbed when it couldn't present). Its snapshot
    /// comes back for reuse.
    Done {
        index: u64,
        snapshot: Box<RenderSnapshot>,
        render_ms: f32,
        gpu: Option<GpuTimings>,
    },
    Fatal(String),
}

/// What executes frames on the render side: the GPU, or a test double.
pub(crate) trait RenderWorker: Send + 'static {
    fn frame(&mut self, frame: &mut RenderFrame) -> Result<(), String>;
    fn resize(&mut self, width: u32, height: u32);
    fn gpu_timings(&self) -> Option<GpuTimings> {
        None
    }
}

/// Runs one command. Returns false on shutdown.
fn handle(worker: &mut dyn RenderWorker, cmd: RenderCommand, events: &Sender<RenderEvent>) -> bool {
    match cmd {
        RenderCommand::Frame(mut frame) => {
            let start = Instant::now();
            let result = {
                let _span = info_span!("RenderThread", frame = frame.index).entered();
                worker.frame(&mut frame)
            };
            if let Err(e) = result {
                let _ = events.send(RenderEvent::Fatal(e));
            }
            let gpu = frame
                .index
                .is_multiple_of(TIMINGS_EVERY)
                .then(|| worker.gpu_timings())
                .flatten();
            // The main thread may already be gone during shutdown; nothing to do then.
            let _ = events.send(RenderEvent::Done {
                index: frame.index,
                snapshot: frame.snapshot,
                render_ms: start.elapsed().as_secs_f32() * 1000.0,
                gpu,
            });
            true
        }
        RenderCommand::Resize(w, h) => {
            worker.resize(w, h);
            true
        }
        RenderCommand::Shutdown => false,
    }
}

enum Backend {
    Threaded {
        commands: Sender<RenderCommand>,
        thread: Option<JoinHandle<()>>,
    },
    Inline(Box<dyn RenderWorker>),
    Stopped,
}

/// The main thread's end of the render pipeline.
pub(crate) struct RenderLink {
    backend: Backend,
    events_tx: Sender<RenderEvent>,
    events: Receiver<RenderEvent>,
    sent: u64,
    completed: u64,
    spare: Option<Box<RenderSnapshot>>,
    gpu: Option<GpuTimings>,
    render_ms: f32,
    fatal: Option<String>,
}

impl RenderLink {
    pub(crate) fn new(worker: Box<dyn RenderWorker>, threaded: bool) -> std::io::Result<Self> {
        let (events_tx, events) = unbounded();
        let backend = if threaded {
            let (commands, rx) = bounded::<RenderCommand>(0);
            let tx = events_tx.clone();
            let thread = std::thread::Builder::new()
                .name("RenderThread".into())
                .spawn(move || {
                    let mut worker = worker;
                    while let Ok(cmd) = rx.recv() {
                        if !handle(worker.as_mut(), cmd, &tx) {
                            break;
                        }
                    }
                    // GPU objects drop here, on the thread that used them, before the main
                    // thread drops the window (shutdown joins first).
                    drop(worker);
                })?;
            info!("render thread started");
            Backend::Threaded {
                commands,
                thread: Some(thread),
            }
        } else {
            info!("rendering inline on the main thread (r.RenderThread 0)");
            Backend::Inline(worker)
        };
        Ok(Self {
            backend,
            events_tx,
            events,
            sent: 0,
            completed: 0,
            spare: None,
            gpu: None,
            render_ms: 0.0,
            fatal: None,
        })
    }

    fn send(&mut self, cmd: RenderCommand) -> Result<(), String> {
        let _span = info_span!("WaitForRenderThread").entered();
        match &mut self.backend {
            Backend::Threaded { commands, .. } => commands
                .send(cmd)
                .map_err(|_| "render thread stopped".to_owned()),
            Backend::Inline(worker) => {
                handle(worker.as_mut(), cmd, &self.events_tx);
                Ok(())
            }
            Backend::Stopped => Err("render thread stopped".to_owned()),
        }
    }

    /// Hands `snapshot` (this frame's extract) to the render side and leaves a recycled one in
    /// its place for the next extract. Blocks while the render side is still on the frame
    /// before the previous one. `Err` when the render thread is gone (panicked).
    pub(crate) fn submit(
        &mut self,
        snapshot: &mut RenderSnapshot,
        background: [f32; 3],
    ) -> Result<(), String> {
        // Swap contents with the recycled box: no allocation once three snapshots exist.
        let mut boxed = self.spare.take().unwrap_or_default();
        std::mem::swap(snapshot, &mut *boxed);
        self.sent += 1;
        let result = self.send(RenderCommand::Frame(RenderFrame {
            index: self.sent,
            snapshot: boxed,
            background,
        }));
        self.drain_events();
        result
    }

    pub(crate) fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.send(RenderCommand::Resize(width, height))
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                RenderEvent::Done {
                    index,
                    snapshot,
                    render_ms,
                    gpu,
                } => {
                    self.completed = self.completed.max(index);
                    self.spare = Some(snapshot);
                    self.render_ms = render_ms;
                    if gpu.is_some() {
                        self.gpu = gpu;
                    }
                }
                RenderEvent::Fatal(e) => self.fatal = Some(e),
            }
        }
    }

    /// Frames submitted that the render side hasn't finished (â‰¤ 1 by construction).
    pub(crate) fn lag(&self) -> u64 {
        self.sent - self.completed
    }

    pub(crate) fn gpu_timings(&self) -> Option<&GpuTimings> {
        self.gpu.as_ref()
    }

    /// Wall time of the render side's last finished frame (includes waiting on present).
    pub(crate) fn render_ms(&self) -> f32 {
        self.render_ms
    }

    pub(crate) fn take_fatal(&mut self) -> Option<String> {
        self.fatal.take()
    }

    /// Stops the render side and waits for it (GPU objects are dropped by then).
    pub(crate) fn shutdown(&mut self) {
        match std::mem::replace(&mut self.backend, Backend::Stopped) {
            Backend::Threaded { commands, thread } => {
                let _ = commands.send(RenderCommand::Shutdown);
                drop(commands);
                if let Some(thread) = thread
                    && thread.join().is_err()
                {
                    error!("render thread panicked");
                }
            }
            Backend::Inline(worker) => drop(worker),
            Backend::Stopped => {}
        }
    }
}

impl Drop for RenderLink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The real worker: owns the GPU and draws snapshots.
pub(crate) struct GpuWorker {
    pub rhi: Rhi,
    pub swapchain: Swapchain,
    pub renderer: Renderer,
}

impl GpuWorker {
    fn inputs<'a>(view: &'a RenderView, s: &'a mut RenderSnapshot) -> FrameInputs<'a> {
        FrameInputs {
            view,
            items: &[],
            scene: Some(&mut s.scene),
            lights: &s.lights,
            world: s.world,
            ambient: ge_ecs::prelude::Vec3::ZERO,
            color: s.color,
            look: s.look,
            shading: s.shading,
            direct_light: s.direct_light,
            bloom: s.bloom,
            overlays: s.overlays,
            ui: Some(&mut s.ui),
            asset_uploads: Some(&mut s.asset_uploads),
        }
    }
}

impl RenderWorker for GpuWorker {
    fn frame(&mut self, frame: &mut RenderFrame) -> Result<(), String> {
        let Self {
            rhi,
            swapchain,
            renderer,
        } = self;
        // Shader hot reload runs even while minimized, so edits are picked up on restore.
        renderer.update(rhi);
        let snapshot = &mut *frame.snapshot;
        // The world is the background (scene-linear; tonemapping color-manages it with the
        // rest of the frame, as Blender does).
        let background = snapshot
            .world
            .map_or(frame.background, |w| w.radiance().to_array());
        let view = RenderView {
            camera: snapshot.camera.unwrap_or_default(),
            time: snapshot.time,
            clear_color: background,
        };
        if !swapchain.is_presentable() {
            // Minimized: keep the changes, draw nothing, and don't spin a core.
            renderer.absorb(rhi, Self::inputs(&view, snapshot));
            std::thread::sleep(MINIMIZED_TICK);
            return Ok(());
        }
        match rhi.begin_frame(swapchain) {
            Ok(Some(mut gpu_frame)) => {
                renderer.render_frame(rhi, &mut gpu_frame, Self::inputs(&view, snapshot));
                rhi.end_frame(swapchain, gpu_frame);
                Ok(())
            }
            Ok(None) => {
                renderer.absorb(rhi, Self::inputs(&view, snapshot));
                Ok(())
            }
            Err(e) => {
                renderer.absorb(rhi, Self::inputs(&view, snapshot));
                Err(e.to_string())
            }
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.swapchain.resize(&self.rhi, width, height);
    }

    fn gpu_timings(&self) -> Option<GpuTimings> {
        self.rhi.gpu_timings().cloned()
    }
}

/// Compile-time check that the GPU side can move to the render thread.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<GpuWorker>();
    assert_send::<RenderSnapshot>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use ge_ecs::prelude::{Mat4, Vec4};
    use ge_render::{DrawItem, MeshKind, ProxyKey};
    use std::sync::{Arc, Mutex};

    /// Records frame indices; optionally slow, failing or panicking.
    struct Fake {
        seen: Arc<Mutex<Vec<u64>>>,
        delay: Duration,
        fail_at: Option<u64>,
        panic_at: Option<u64>,
    }

    impl Fake {
        fn new(delay_ms: u64) -> (Self, Arc<Mutex<Vec<u64>>>) {
            let seen = Arc::new(Mutex::new(Vec::new()));
            let fake = Self {
                seen: seen.clone(),
                delay: Duration::from_millis(delay_ms),
                fail_at: None,
                panic_at: None,
            };
            (fake, seen)
        }
    }

    impl RenderWorker for Fake {
        fn frame(&mut self, frame: &mut RenderFrame) -> Result<(), String> {
            assert!(Some(frame.index) != self.panic_at, "fake render panic");
            std::thread::sleep(self.delay);
            self.seen.lock().unwrap().push(frame.index);
            if Some(frame.index) == self.fail_at {
                return Err("device lost".into());
            }
            Ok(())
        }
        fn resize(&mut self, _: u32, _: u32) {}
    }

    #[test]
    fn main_thread_stays_within_one_frame_and_order_is_kept() {
        for threaded in [true, false] {
            let (fake, seen) = Fake::new(10);
            let mut link = RenderLink::new(Box::new(fake), threaded).unwrap();
            let mut snapshot = RenderSnapshot::default();
            let start = Instant::now();
            for _ in 0..8 {
                link.submit(&mut snapshot, [0.0; 3]).unwrap();
                assert!(link.lag() <= 1, "lag {} (threaded {threaded})", link.lag());
            }
            if !threaded {
                assert_eq!(link.lag(), 0, "inline frames finish before submit returns");
            }
            // The main thread was paced by the render side, not free-running.
            assert!(
                start.elapsed() >= Duration::from_millis(70),
                "{:?}",
                start.elapsed()
            );
            link.shutdown();
            assert_eq!(*seen.lock().unwrap(), (1..=8).collect::<Vec<_>>());
        }
    }

    #[test]
    fn snapshots_are_recycled_and_undrained_changes_come_back() {
        let (fake, _) = Fake::new(0);
        let mut link = RenderLink::new(Box::new(fake), true).unwrap();
        let mut snapshot = RenderSnapshot::default();
        let item = DrawItem::new(MeshKind::Cube, Mat4::IDENTITY, Vec4::ONE);
        snapshot.scene.upsert(ProxyKey(7), item);
        // The fake doesn't drain the delta (like a frame it couldn't draw): after the
        // snapshot cycles back, the change is still pending for the next frame.
        let mut back = false;
        for _ in 0..4 {
            link.submit(&mut snapshot, [0.0; 3]).unwrap();
            back |= snapshot.scene.get(ProxyKey(7)).is_some();
        }
        assert!(back, "the recycled snapshot carries the undrained change");
    }

    #[test]
    fn render_errors_and_panics_reach_the_main_thread() {
        let (mut fake, _) = Fake::new(0);
        fake.fail_at = Some(2);
        let mut link = RenderLink::new(Box::new(fake), true).unwrap();
        let mut s = RenderSnapshot::default();
        for _ in 0..3 {
            link.submit(&mut s, [0.0; 3]).unwrap();
        }
        assert_eq!(link.take_fatal().as_deref(), Some("device lost"));

        let (mut fake, _) = Fake::new(0);
        fake.panic_at = Some(3);
        let mut link = RenderLink::new(Box::new(fake), true).unwrap();
        let results: Vec<bool> = (0..6)
            .map(|_| link.submit(&mut s, [0.0; 3]).is_ok())
            .collect();
        assert!(results[..3].iter().all(|ok| *ok), "{results:?}");
        assert!(!results[5], "a dead render thread is reported: {results:?}");
        link.shutdown(); // joins the panicked thread without propagating
    }
}
