//! wgpu v30 backend. wgpu enumerates Vulkan before D3D12 on Windows, so the discrete GPU
//! usually comes up on Vulkan; `WGPU_BACKEND=dx12` overrides.

use std::sync::Arc;

use tracing::{error, info, info_span, warn};

use crate::pass::{RenderPass, RenderPassDesc, load_color, load_depth};
use crate::timestamps::{FrameQueries, GpuProfiler};
use crate::{
    AdapterSummary, GpuKind, GpuTimings, R_ALLOW_INTEGRATED_GPU, R_GPU_TIMESTAMPS, R_VSYNC,
    RhiError, SurfaceSource, Texture, TextureFormat,
};

/// Device + queue. One per process.
pub struct Rhi {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    summary: AdapterSummary,
    profiler: Option<GpuProfiler>,
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
    format: TextureFormat,
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

/// One frame being recorded: a color target (backbuffer or offscreen texture) plus a command
/// encoder. Hand it back to [`Rhi::end_frame`] / [`Rhi::submit_offscreen`].
pub struct Frame {
    surface_texture: Option<wgpu::SurfaceTexture>,
    view: wgpu::TextureView,
    size: (u32, u32),
    encoder: wgpu::CommandEncoder,
    queries: Option<FrameQueries>,
    reconfigure_after_present: bool,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("size", &self.size)
            .finish_non_exhaustive()
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
        let instance = create_instance();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| RhiError::Surface(e.to_string()))?;
        let adapter = pick_adapter(&instance, Some(&surface))?;
        let rhi = Self::with_adapter(instance, adapter)?;
        let swapchain = rhi.make_swapchain(surface, window, size)?;
        Ok((rhi, swapchain))
    }

    /// A device with no window, for tests and tools (golden images, cooker previews).
    pub fn new_headless() -> Result<Self, RhiError> {
        let _span = info_span!("Rhi::new_headless").entered();
        let instance = create_instance();
        let adapter = pick_adapter(&instance, None)?;
        Self::with_adapter(instance, adapter)
    }

    fn with_adapter(instance: wgpu::Instance, adapter: wgpu::Adapter) -> Result<Self, RhiError> {
        let summary = summarize(&adapter.get_info());
        info!(
            gpu = %summary.name,
            kind = ?summary.kind,
            backend = %summary.backend,
            driver = %summary.driver,
            "selected GPU adapter"
        );

        let required_features = adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ge_device"),
            required_features,
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

        let profiler = GpuProfiler::new(&device, &queue);
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            summary,
            profiler,
        })
    }

    pub fn adapter(&self) -> &AdapterSummary {
        &self.summary
    }

    /// Most recent completed frame's per-pass GPU times (a few frames old), if measured.
    pub fn gpu_timings(&self) -> Option<&GpuTimings> {
        self.profiler.as_ref().and_then(GpuProfiler::latest)
    }

    fn make_swapchain(
        &self,
        surface: wgpu::Surface<'static>,
        window: Arc<dyn SurfaceSource>,
        (width, height): (u32, u32),
    ) -> Result<Swapchain, RhiError> {
        let caps = surface.get_capabilities(&self.adapter);
        // sRGB backbuffer: shaders write linear, hardware encodes on store.
        let pick = |srgb: bool| {
            caps.formats.iter().find_map(|&f| {
                TextureFormat::from_wgpu(f)
                    .filter(|ours| ours.is_srgb() == srgb)
                    .map(|ours| (f, ours))
            })
        };
        let (wgpu_format, format) = pick(true).or_else(|| pick(false)).ok_or_else(|| {
            RhiError::Surface(format!("no supported format in {:?}", caps.formats))
        })?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: wgpu_format,
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
            format,
            vsync_version: R_VSYNC.version(),
            usable: false,
        };
        swapchain.resize(self, width, height);
        info!(?format, present_mode = ?swapchain.config.present_mode, "swapchain created");
        Ok(swapchain)
    }

    /// Harvests finished GPU work (timestamp readbacks) without blocking.
    fn poll(&mut self) {
        if let Err(e) = self.device.poll(wgpu::PollType::Poll) {
            warn!(error = %e, "device poll failed");
        }
        if let Some(p) = &mut self.profiler {
            p.collect();
        }
    }

    fn new_frame(&mut self, view: wgpu::TextureView, size: (u32, u32)) -> Frame {
        self.poll();
        let queries = match &mut self.profiler {
            Some(p) if R_GPU_TIMESTAMPS.get() => p.begin_frame(),
            _ => None,
        };
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Frame"),
            });
        Frame {
            surface_texture: None,
            view,
            size,
            encoder,
            queries,
            reconfigure_after_present: false,
        }
    }

    /// Acquires the next backbuffer. `Ok(None)` means "skip this frame" (minimized, occluded,
    /// timed out, or the surface was just reconfigured).
    pub fn begin_frame(&mut self, swapchain: &mut Swapchain) -> Result<Option<Frame>, RhiError> {
        if !swapchain.usable {
            self.poll();
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
        let mut frame = self.new_frame(view, swapchain.size());
        frame.surface_texture = Some(texture);
        frame.reconfigure_after_present = suboptimal;
        Ok(Some(frame))
    }

    /// Submits the frame's commands and presents the backbuffer.
    pub fn end_frame(&mut self, swapchain: &mut Swapchain, frame: Frame) {
        let _span = info_span!("Rhi::Present").entered();
        let reconfigure = frame.reconfigure_after_present;
        if let Some(texture) = self.submit(frame) {
            self.queue.present(texture);
        }
        if reconfigure {
            swapchain.surface.configure(&self.device, &swapchain.config);
        }
    }

    /// Starts a frame that renders into `target` instead of a window.
    pub fn begin_offscreen_frame(&mut self, target: &Texture) -> Frame {
        self.new_frame(target.view.clone(), target.size())
    }

    pub fn submit_offscreen(&mut self, frame: Frame) {
        let _ = self.submit(frame);
    }

    fn submit(&mut self, frame: Frame) -> Option<wgpu::SurfaceTexture> {
        let Frame {
            surface_texture,
            view,
            mut encoder,
            queries,
            ..
        } = frame;
        drop(view);
        let readback_slot = match (&mut self.profiler, queries) {
            (Some(p), Some(q)) => p.end_frame(&mut encoder, q),
            _ => None,
        };
        self.queue.submit([encoder.finish()]);
        if let (Some(p), Some(slot)) = (&mut self.profiler, readback_slot) {
            p.after_submit(slot);
        }
        surface_texture
    }

    /// Copies an RGBA8/BGRA8 texture back to the CPU as tightly packed rows. **Blocks** until
    /// the GPU is done: for tests and tools only, never in the frame loop.
    pub fn read_texture_rgba8(&self, texture: &Texture) -> Result<Vec<u8>, RhiError> {
        let (width, height) = texture.size();
        if texture.format().bytes_per_pixel() != 4 {
            return Err(RhiError::Readback(format!(
                "{:?} is not a 4-byte format",
                texture.format()
            )));
        }
        let row = width * 4;
        let padded_row =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TextureReadback"),
            size: u64::from(padded_row * height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.inner,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RhiError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| RhiError::Readback(e.to_string()))?
            .map_err(|e| RhiError::Readback(e.to_string()))?;

        let mut pixels = Vec::with_capacity((row * height) as usize);
        {
            let view = buffer
                .get_mapped_range(..)
                .map_err(|e| RhiError::Readback(e.to_string()))?;
            for y in 0..height as usize {
                let start = y * padded_row as usize;
                pixels.extend_from_slice(&view[start..start + row as usize]);
            }
        }
        buffer.unmap();
        Ok(pixels)
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

    /// Backbuffer format (sRGB when the surface offers one).
    pub fn format(&self) -> TextureFormat {
        self.format
    }

    /// False while the window has zero area (minimized).
    pub fn is_presentable(&self) -> bool {
        self.usable
    }
}

impl Frame {
    /// Size of the frame's color target in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Begins a labeled, timestamped pass. The color attachment is the frame's target.
    pub fn render_pass(&mut self, desc: &RenderPassDesc<'_>) -> RenderPass<'_> {
        let span = info_span!("Render::Pass", pass = desc.label).entered();
        let Frame {
            view,
            encoder,
            queries,
            ..
        } = self;
        let mut queries = queries.as_mut();
        let timestamp_writes = queries.as_mut().and_then(|q| q.pass_writes(desc.label));
        let color = desc.color.map(|load| wgpu::RenderPassColorAttachment {
            view: &*view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: load_color(load),
                store: wgpu::StoreOp::Store,
            },
        });
        let inner = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(desc.label),
            color_attachments: &[color],
            depth_stencil_attachment: desc.depth.map(|d| wgpu::RenderPassDepthStencilAttachment {
                view: &d.texture.view,
                depth_ops: Some(wgpu::Operations {
                    load: load_depth(d.load),
                    store: if d.store {
                        wgpu::StoreOp::Store
                    } else {
                        wgpu::StoreOp::Discard
                    },
                }),
                stencil_ops: None,
            }),
            timestamp_writes,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        RenderPass {
            inner,
            queries,
            _span: span,
        }
    }
}

fn create_instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
    wgpu::Instance::new(desc)
}

/// Prefers a discrete GPU (that can present to `surface`, if given); falls back to wgpu's
/// HighPerformance pick. Logs every adapter so hybrid-GPU problems are visible.
fn pick_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<wgpu::Adapter, RhiError> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    let presentable = |a: &wgpu::Adapter| surface.is_none_or(|s| a.is_surface_supported(s));
    for a in &adapters {
        let info = a.get_info();
        info!(
            gpu = %info.name,
            kind = ?info.device_type,
            backend = ?info.backend,
            presentable = presentable(a),
            "found adapter"
        );
    }

    let allow_integrated = R_ALLOW_INTEGRATED_GPU.get();
    if !allow_integrated {
        // Among discrete adapters, take the first one from wgpu's preferred backend order.
        if let Some(a) = adapters
            .iter()
            .find(|a| a.get_info().device_type == wgpu::DeviceType::DiscreteGpu && presentable(a))
        {
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
        compatible_surface: surface,
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
