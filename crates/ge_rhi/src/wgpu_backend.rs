//! wgpu v30 backend. wgpu enumerates Vulkan before D3D12 on Windows, so the discrete GPU
//! usually comes up on Vulkan; `WGPU_BACKEND=dx12` overrides.

use std::sync::Arc;

use tracing::{error, info, info_span, warn};

use crate::{
    AdapterSummary, ClearColor, GpuKind, R_ALLOW_INTEGRATED_GPU, R_VSYNC, RhiError, SurfaceSource,
};

/// Device + queue. One per process.
pub struct Rhi {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    summary: AdapterSummary,
}

impl std::fmt::Debug for Rhi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rhi")
            .field("adapter", &self.summary)
            .finish_non_exhaustive()
    }
}

/// A presentable surface bound to one window.
pub struct Swapchain {
    surface: wgpu::Surface<'static>,
    window: Arc<dyn SurfaceSource>,
    config: wgpu::SurfaceConfiguration,
    vsync_version: u32,
    /// False while the window has zero area (minimized): no configure, no frames.
    usable: bool,
}

impl std::fmt::Debug for Swapchain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Swapchain")
            .field("size", &(self.config.width, self.config.height))
            .field("format", &self.config.format)
            .field("present_mode", &self.config.present_mode)
            .finish_non_exhaustive()
    }
}

/// One frame in flight: the acquired backbuffer plus a command encoder.
/// Hand it back to [`Rhi::end_frame`] to submit and present.
pub struct Frame {
    texture: wgpu::SurfaceTexture,
    view: wgpu::TextureView,
    encoder: wgpu::CommandEncoder,
    reconfigure_after_present: bool,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame").finish_non_exhaustive()
    }
}

impl Rhi {
    /// Creates the device and a swapchain for `window`. Picks the discrete GPU on hybrid
    /// laptops unless `r.AllowIntegratedGpu` is set (lore §6.4, §9).
    pub fn new(
        window: Arc<dyn SurfaceSource>,
        size: (u32, u32),
    ) -> Result<(Self, Swapchain), RhiError> {
        let _span = info_span!("Rhi::new").entered();

        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
        let instance = wgpu::Instance::new(desc);

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RhiError::Surface(e.to_string()))?;

        let adapter = pick_adapter(&instance, &surface)?;
        let summary = summarize(&adapter.get_info());
        info!(
            gpu = %summary.name,
            kind = ?summary.kind,
            backend = %summary.backend,
            driver = %summary.driver,
            "selected GPU adapter"
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ge_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| RhiError::Device(e.to_string()))?;

        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            error!(error = %e, "uncaptured GPU error");
        }));
        device.set_device_lost_callback(|reason, msg| {
            error!(?reason, %msg, "GPU device lost");
        });

        let rhi = Self {
            instance,
            adapter,
            device,
            queue,
            summary,
        };
        let swapchain = rhi.make_swapchain(surface, window, size);
        Ok((rhi, swapchain))
    }

    pub fn adapter(&self) -> &AdapterSummary {
        &self.summary
    }

    fn make_swapchain(
        &self,
        surface: wgpu::Surface<'static>,
        window: Arc<dyn SurfaceSource>,
        (width, height): (u32, u32),
    ) -> Swapchain {
        let caps = surface.get_capabilities(&self.adapter);
        // sRGB backbuffer: shaders write linear, hardware encodes on store.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            desired_maximum_frame_latency: 2,
            present_mode: choose_present_mode(&caps.present_modes, R_VSYNC.get()),
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        let mut swapchain = Swapchain {
            surface,
            window,
            config,
            vsync_version: R_VSYNC.version(),
            usable: false,
        };
        swapchain.resize(self, width, height);
        info!(?format, present_mode = ?swapchain.config.present_mode, "swapchain created");
        swapchain
    }

    /// Acquires the next backbuffer. `Ok(None)` means "skip this frame" (minimized, occluded,
    /// timed out, or the surface was just reconfigured).
    pub fn begin_frame(&self, swapchain: &mut Swapchain) -> Result<Option<Frame>, RhiError> {
        if !swapchain.usable {
            return Ok(None);
        }
        if swapchain.vsync_version != R_VSYNC.version() {
            swapchain.vsync_version = R_VSYNC.version();
            let caps = swapchain.surface.get_capabilities(&self.adapter);
            swapchain.config.present_mode = choose_present_mode(&caps.present_modes, R_VSYNC.get());
            swapchain.surface.configure(&self.device, &swapchain.config);
            info!(present_mode = ?swapchain.config.present_mode, "present mode changed");
        }

        let (texture, suboptimal) = match swapchain.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => (t, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => (t, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                swapchain.surface.configure(&self.device, &swapchain.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                warn!("surface lost; recreating");
                self.recreate_surface(swapchain)?;
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                error!("validation error while acquiring backbuffer");
                return Ok(None);
            }
        };

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame"),
            });
        Ok(Some(Frame {
            texture,
            view,
            encoder,
            reconfigure_after_present: suboptimal,
        }))
    }

    /// Submits the frame's commands and presents the backbuffer.
    pub fn end_frame(&self, swapchain: &mut Swapchain, frame: Frame) {
        let _span = info_span!("Rhi::Present").entered();
        let Frame {
            texture,
            view,
            encoder,
            reconfigure_after_present,
        } = frame;
        drop(view);
        self.queue.submit([encoder.finish()]);
        self.queue.present(texture);
        if reconfigure_after_present {
            swapchain.surface.configure(&self.device, &swapchain.config);
        }
    }

    fn recreate_surface(&self, swapchain: &mut Swapchain) -> Result<(), RhiError> {
        swapchain.surface = self
            .instance
            .create_surface(swapchain.window.clone())
            .map_err(|e| RhiError::SurfaceLost(e.to_string()))?;
        if swapchain.usable {
            swapchain.surface.configure(&self.device, &swapchain.config);
        }
        Ok(())
    }
}

impl Swapchain {
    /// Call on every window resize. A zero-sized window (minimized) pauses presentation.
    pub fn resize(&mut self, rhi: &Rhi, width: u32, height: u32) {
        self.usable = width > 0 && height > 0;
        if !self.usable {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&rhi.device, &self.config);
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// False while the window has zero area (minimized).
    pub fn is_presentable(&self) -> bool {
        self.usable
    }
}

impl Frame {
    /// Clears the backbuffer. Temporary M0 entry point; real passes come from the render
    /// graph (M5).
    pub fn clear(&mut self, color: ClearColor) {
        let _span = info_span!("Render::Clear").entered();
        let [r, g, b, a] = color.0.map(f64::from);
        let _pass = self.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
}

/// Prefers a discrete GPU that can present to `surface`; falls back to wgpu's
/// HighPerformance pick. Logs every adapter so hybrid-GPU problems are visible.
fn pick_adapter(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'_>,
) -> Result<wgpu::Adapter, RhiError> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    for a in &adapters {
        let info = a.get_info();
        info!(
            gpu = %info.name,
            kind = ?info.device_type,
            backend = ?info.backend,
            presentable = a.is_surface_supported(surface),
            "found adapter"
        );
    }

    let allow_integrated = R_ALLOW_INTEGRATED_GPU.get();
    if !allow_integrated {
        // Among discrete adapters, take the first one from wgpu's preferred backend order.
        if let Some(a) = adapters.iter().find(|a| {
            a.get_info().device_type == wgpu::DeviceType::DiscreteGpu
                && a.is_surface_supported(surface)
        }) {
            return Ok(a.clone());
        }
    }

    let power_preference = if allow_integrated {
        wgpu::PowerPreference::LowPower
    } else {
        wgpu::PowerPreference::HighPerformance
    };
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference,
        force_fallback_adapter: false,
        compatible_surface: Some(surface),
        ..Default::default()
    }))
    .map_err(|e| RhiError::NoAdapter(e.to_string()))?;

    if adapter.get_info().device_type != wgpu::DeviceType::DiscreteGpu && !allow_integrated {
        warn!("no presentable discrete GPU found; running on a non-discrete adapter");
    }
    Ok(adapter)
}

fn choose_present_mode(modes: &[wgpu::PresentMode], vsync: bool) -> wgpu::PresentMode {
    let preferred: &[wgpu::PresentMode] = if vsync {
        &[wgpu::PresentMode::Fifo]
    } else {
        &[
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Fifo,
        ]
    };
    preferred
        .iter()
        .copied()
        .find(|m| modes.contains(m))
        .unwrap_or(wgpu::PresentMode::Fifo)
}

fn summarize(info: &wgpu::AdapterInfo) -> AdapterSummary {
    AdapterSummary {
        name: info.name.clone(),
        kind: match info.device_type {
            wgpu::DeviceType::DiscreteGpu => GpuKind::Discrete,
            wgpu::DeviceType::IntegratedGpu => GpuKind::Integrated,
            wgpu::DeviceType::VirtualGpu => GpuKind::Virtual,
            wgpu::DeviceType::Cpu => GpuKind::Cpu,
            wgpu::DeviceType::Other => GpuKind::Other,
        },
        backend: format!("{:?}", info.backend),
        driver: format!("{} {}", info.driver, info.driver_info)
            .trim()
            .to_owned(),
        vendor_id: info.vendor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn present_mode_fallbacks() {
        use wgpu::PresentMode::*;
        assert_eq!(choose_present_mode(&[Fifo, Mailbox], true), Fifo);
        assert_eq!(choose_present_mode(&[Fifo, Mailbox], false), Mailbox);
        assert_eq!(choose_present_mode(&[Fifo, Immediate], false), Immediate);
        assert_eq!(choose_present_mode(&[Fifo], false), Fifo);
        assert_eq!(choose_present_mode(&[], true), Fifo);
    }
}
