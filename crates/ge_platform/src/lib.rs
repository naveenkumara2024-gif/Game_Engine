//! OS layer (lore §3.1 "ApplicationCore"). Owns the winit event loop and translates OS events
//! into engine [`PlatformEvent`]s, so nothing above this crate depends on winit directly.
//!
//! The engine implements [`PlatformApp`] and hands it to [`run`]; the platform calls back on
//! the main thread. Windows must be created and pumped on the main thread (lore §7.5).

use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};
use tracing::info;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};

#[derive(Clone, Debug)]
pub struct WindowDesc {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub resizable: bool,
}

impl Default for WindowDesc {
    fn default() -> Self {
        Self {
            title: "Game_Engine".into(),
            width: 1280,
            height: 720,
            resizable: true,
        }
    }
}

/// A native window. Cheap to clone via `Arc`; provides raw handles for the RHI.
#[derive(Debug)]
pub struct Window {
    inner: winit::window::Window,
}

impl Window {
    /// Inner (client-area) size in physical pixels. Zero while minimized.
    pub fn size(&self) -> (u32, u32) {
        let s = self.inner.inner_size();
        (s.width, s.height)
    }

    pub fn scale_factor(&self) -> f64 {
        self.inner.scale_factor()
    }

    pub fn set_title(&self, title: &str) {
        self.inner.set_title(title);
    }

    pub fn request_redraw(&self) {
        self.inner.request_redraw();
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.inner.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.inner.display_handle()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// Approximate pixels per wheel "line" for devices that report pixel deltas (touchpads).
const PIXELS_PER_LINE: f64 = 40.0;

/// OS events the engine cares about. Grows with input mapping in M2.
#[derive(Clone, Debug, PartialEq)]
pub enum PlatformEvent {
    Resized {
        width: u32,
        height: u32,
    },
    ScaleFactorChanged {
        scale_factor: f64,
    },
    CloseRequested,
    Focused(bool),
    /// A key went down or up. `key` is a stable, layout-independent name like `"Escape"`.
    Key {
        key: String,
        pressed: bool,
        repeat: bool,
    },
    MouseButton {
        button: MouseButton,
        pressed: bool,
    },
    /// Raw relative mouse motion in device units, unaffected by cursor acceleration or the
    /// window edge (what camera controls want).
    MouseMotion {
        dx: f64,
        dy: f64,
    },
    /// Wheel movement in lines; positive is away from the user.
    MouseWheel {
        lines: f32,
    },
}

/// Given to [`PlatformApp::init`] to create windows.
#[derive(Debug)]
pub struct PlatformContext<'a> {
    event_loop: &'a ActiveEventLoop,
}

impl PlatformContext<'_> {
    pub fn create_window(&self, desc: &WindowDesc) -> Result<Arc<Window>, PlatformError> {
        let attrs = winit::window::Window::default_attributes()
            .with_title(desc.title.clone())
            .with_inner_size(winit::dpi::PhysicalSize::new(desc.width, desc.height))
            .with_resizable(desc.resizable);
        let inner = self
            .event_loop
            .create_window(attrs)
            .map_err(|e| PlatformError::Window(e.to_string()))?;
        info!(title = %desc.title, size = ?inner.inner_size(), "window created");
        Ok(Arc::new(Window { inner }))
    }
}

/// Main-loop callbacks, all on the main thread.
pub trait PlatformApp {
    /// Called once when the OS is ready for windows. An error stops the loop.
    fn init(&mut self, ctx: &PlatformContext<'_>) -> Result<(), Box<dyn std::error::Error>>;
    fn event(&mut self, event: PlatformEvent);
    /// One engine tick. Return `false` to exit the loop.
    fn tick(&mut self) -> bool;
    /// The loop has ended; tear down while the OS objects still exist.
    fn shutdown(&mut self) {}
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("event loop: {0}")]
    EventLoop(String),
    #[error("window creation: {0}")]
    Window(String),
    #[error("app init failed: {0}")]
    Init(String),
}

/// Runs the OS event loop until the app asks to exit. Blocks the calling (main) thread.
pub fn run(app: &mut dyn PlatformApp) -> Result<(), PlatformError> {
    let event_loop = EventLoop::new().map_err(|e| PlatformError::EventLoop(e.to_string()))?;
    // Game loop: never sleep waiting for OS events.
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut runner = Runner {
        app,
        initialized: false,
        init_error: None,
    };
    event_loop
        .run_app(&mut runner)
        .map_err(|e| PlatformError::EventLoop(e.to_string()))?;
    match runner.init_error {
        Some(e) => Err(PlatformError::Init(e)),
        None => Ok(()),
    }
}

struct Runner<'a> {
    app: &'a mut dyn PlatformApp,
    initialized: bool,
    init_error: Option<String>,
}

impl ApplicationHandler for Runner<'_> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if let Err(e) = self.app.init(&PlatformContext { event_loop }) {
            self.init_error = Some(e.to_string());
            event_loop.exit();
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        if let Some(ev) = translate(&event) {
            self.app.event(ev);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.initialized && self.init_error.is_none() && !self.app.tick() {
            event_loop.exit();
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event
            && self.initialized
        {
            self.app.event(PlatformEvent::MouseMotion { dx, dy });
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if self.init_error.is_none() {
            self.app.shutdown();
        }
    }
}

fn translate(event: &WindowEvent) -> Option<PlatformEvent> {
    Some(match event {
        WindowEvent::Resized(s) => PlatformEvent::Resized {
            width: s.width,
            height: s.height,
        },
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => PlatformEvent::ScaleFactorChanged {
            scale_factor: *scale_factor,
        },
        WindowEvent::CloseRequested => PlatformEvent::CloseRequested,
        WindowEvent::Focused(f) => PlatformEvent::Focused(*f),
        WindowEvent::MouseInput { state, button, .. } => PlatformEvent::MouseButton {
            button: match button {
                winit::event::MouseButton::Left => MouseButton::Left,
                winit::event::MouseButton::Right => MouseButton::Right,
                winit::event::MouseButton::Middle => MouseButton::Middle,
                winit::event::MouseButton::Back => MouseButton::Other(3),
                winit::event::MouseButton::Forward => MouseButton::Other(4),
                winit::event::MouseButton::Other(n) => MouseButton::Other(*n),
            },
            pressed: state.is_pressed(),
        },
        WindowEvent::MouseWheel { delta, .. } => PlatformEvent::MouseWheel {
            lines: match delta {
                MouseScrollDelta::LineDelta(_, y) => *y,
                MouseScrollDelta::PixelDelta(p) => (p.y / PIXELS_PER_LINE) as f32,
            },
        },
        WindowEvent::KeyboardInput { event, .. } => {
            let winit::keyboard::PhysicalKey::Code(code) = event.physical_key else {
                return None;
            };
            PlatformEvent::Key {
                key: format!("{code:?}"),
                pressed: event.state.is_pressed(),
                repeat: event.repeat,
            }
        }
        _ => return None,
    })
}
