//! GPU resources and pipeline state. Each type is an opaque wrapper; descriptors are our own
//! plain enums/structs so callers never name a `wgpu` type (law 12).

use std::ops::BitOr;

use wgpu::util::DeviceExt;

use crate::{Rhi, RhiError};

// ---------------------------------------------------------------------------------------------
// Formats and flags
// ---------------------------------------------------------------------------------------------

/// The texture formats the engine uses so far. Grows on demand.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TextureFormat {
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    Rgba16Float,
    /// Reversed-Z depth target (lore §6.7): clear to 0, compare `Greater`.
    Depth32Float,
}

impl TextureFormat {
    pub fn is_srgb(self) -> bool {
        matches!(self, Self::Rgba8UnormSrgb | Self::Bgra8UnormSrgb)
    }

    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Bgra8Unorm | Self::Bgra8UnormSrgb => 4,
            Self::Depth32Float => 4,
            Self::Rgba16Float => 8,
        }
    }

    pub(crate) fn to_wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            Self::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            Self::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            Self::Depth32Float => wgpu::TextureFormat::Depth32Float,
        }
    }

    pub(crate) fn from_wgpu(format: wgpu::TextureFormat) -> Option<Self> {
        Some(match format {
            wgpu::TextureFormat::Rgba8Unorm => Self::Rgba8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb => Self::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm => Self::Bgra8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb => Self::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba16Float => Self::Rgba16Float,
            wgpu::TextureFormat::Depth32Float => Self::Depth32Float,
            _ => return None,
        })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct BufferUsage(wgpu::BufferUsages);

impl BufferUsage {
    pub const VERTEX: Self = Self(wgpu::BufferUsages::VERTEX);
    pub const INDEX: Self = Self(wgpu::BufferUsages::INDEX);
    pub const UNIFORM: Self = Self(wgpu::BufferUsages::UNIFORM);
    pub const COPY_SRC: Self = Self(wgpu::BufferUsages::COPY_SRC);
    pub const COPY_DST: Self = Self(wgpu::BufferUsages::COPY_DST);
}

impl BitOr for BufferUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct TextureUsage(wgpu::TextureUsages);

impl TextureUsage {
    pub const RENDER_ATTACHMENT: Self = Self(wgpu::TextureUsages::RENDER_ATTACHMENT);
    pub const TEXTURE_BINDING: Self = Self(wgpu::TextureUsages::TEXTURE_BINDING);
    pub const COPY_SRC: Self = Self(wgpu::TextureUsages::COPY_SRC);
    pub const COPY_DST: Self = Self(wgpu::TextureUsages::COPY_DST);
}

impl BitOr for TextureUsage {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ShaderStages {
    Vertex,
    Fragment,
    VertexFragment,
}

impl ShaderStages {
    fn to_wgpu(self) -> wgpu::ShaderStages {
        match self {
            Self::Vertex => wgpu::ShaderStages::VERTEX,
            Self::Fragment => wgpu::ShaderStages::FRAGMENT,
            Self::VertexFragment => wgpu::ShaderStages::VERTEX_FRAGMENT,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum VertexFormat {
    Float32x2,
    Float32x3,
    Float32x4,
}

impl VertexFormat {
    fn to_wgpu(self) -> wgpu::VertexFormat {
        match self {
            Self::Float32x2 => wgpu::VertexFormat::Float32x2,
            Self::Float32x3 => wgpu::VertexFormat::Float32x3,
            Self::Float32x4 => wgpu::VertexFormat::Float32x4,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CompareFunction {
    Never,
    Less,
    LessEqual,
    Equal,
    GreaterEqual,
    Greater,
    NotEqual,
    Always,
}

impl CompareFunction {
    fn to_wgpu(self) -> wgpu::CompareFunction {
        match self {
            Self::Never => wgpu::CompareFunction::Never,
            Self::Less => wgpu::CompareFunction::Less,
            Self::LessEqual => wgpu::CompareFunction::LessEqual,
            Self::Equal => wgpu::CompareFunction::Equal,
            Self::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
            Self::Greater => wgpu::CompareFunction::Greater,
            Self::NotEqual => wgpu::CompareFunction::NotEqual,
            Self::Always => wgpu::CompareFunction::Always,
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum CullMode {
    None,
    Front,
    Back,
}

// ---------------------------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub struct BufferDesc<'a> {
    pub label: &'a str,
    pub size: u64,
    pub usage: BufferUsage,
}

#[derive(Copy, Clone, Debug)]
pub struct TextureDesc<'a> {
    pub label: &'a str,
    pub size: (u32, u32),
    pub format: TextureFormat,
    pub usage: TextureUsage,
}

#[derive(Copy, Clone, Debug)]
pub enum BindingType {
    UniformBuffer {
        has_dynamic_offset: bool,
        /// Size of one bound element; checked against the shader at pipeline creation.
        min_binding_size: u64,
    },
}

#[derive(Copy, Clone, Debug)]
pub struct BindGroupLayoutEntry {
    pub binding: u32,
    pub visibility: ShaderStages,
    pub ty: BindingType,
}

#[derive(Copy, Clone, Debug)]
pub enum BindingResource<'a> {
    Buffer {
        buffer: &'a Buffer,
        offset: u64,
        /// `None` binds the rest of the buffer.
        size: Option<u64>,
    },
}

#[derive(Copy, Clone, Debug)]
pub struct BindGroupEntry<'a> {
    pub binding: u32,
    pub resource: BindingResource<'a>,
}

#[derive(Copy, Clone, Debug)]
pub struct VertexAttribute {
    pub location: u32,
    pub offset: u64,
    pub format: VertexFormat,
}

/// One per-vertex buffer layout. Instance-rate buffers arrive with the GPU scene (M5/M12).
#[derive(Copy, Clone, Debug)]
pub struct VertexLayout<'a> {
    pub stride: u64,
    pub attributes: &'a [VertexAttribute],
}

#[derive(Copy, Clone, Debug)]
pub struct DepthState {
    pub format: TextureFormat,
    pub write_enabled: bool,
    pub compare: CompareFunction,
}

#[derive(Copy, Clone, Debug)]
pub struct RenderPipelineDesc<'a> {
    pub label: &'a str,
    pub shader: &'a ShaderModule,
    pub vertex_entry: &'a str,
    pub fragment_entry: &'a str,
    pub vertex_layouts: &'a [VertexLayout<'a>],
    pub bind_group_layouts: &'a [&'a BindGroupLayout],
    pub color_formats: &'a [TextureFormat],
    pub depth: Option<DepthState>,
    pub cull_mode: CullMode,
}

// ---------------------------------------------------------------------------------------------
// Resources (cheap to clone: wgpu objects are reference counted internally)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Buffer {
    pub(crate) inner: wgpu::Buffer,
}

impl Buffer {
    pub fn size(&self) -> u64 {
        self.inner.size()
    }
}

#[derive(Clone, Debug)]
pub struct Texture {
    pub(crate) inner: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    size: (u32, u32),
    format: TextureFormat,
}

impl Texture {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn format(&self) -> TextureFormat {
        self.format
    }
}

#[derive(Clone, Debug)]
pub struct ShaderModule {
    pub(crate) inner: wgpu::ShaderModule,
}

#[derive(Clone, Debug)]
pub struct BindGroupLayout {
    pub(crate) inner: wgpu::BindGroupLayout,
}

#[derive(Clone, Debug)]
pub struct BindGroup {
    pub(crate) inner: wgpu::BindGroup,
}

#[derive(Clone, Debug)]
pub struct RenderPipeline {
    pub(crate) inner: wgpu::RenderPipeline,
}

// ---------------------------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------------------------

impl Rhi {
    /// Required alignment of dynamic uniform-buffer offsets on this device.
    pub fn uniform_offset_alignment(&self) -> u64 {
        u64::from(self.device.limits().min_uniform_buffer_offset_alignment)
    }

    pub fn create_buffer(&self, desc: &BufferDesc<'_>) -> Buffer {
        Buffer {
            inner: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(desc.label),
                size: desc.size,
                usage: desc.usage.0,
                mapped_at_creation: false,
            }),
        }
    }

    pub fn create_buffer_init(&self, label: &str, usage: BufferUsage, contents: &[u8]) -> Buffer {
        Buffer {
            inner: self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: usage.0,
                }),
        }
    }

    /// Queues a CPU → GPU upload; it lands before the next submitted frame.
    pub fn write_buffer(&self, buffer: &Buffer, offset: u64, data: &[u8]) {
        self.queue.write_buffer(&buffer.inner, offset, data);
    }

    pub fn create_texture(&self, desc: &TextureDesc<'_>) -> Texture {
        let (width, height) = desc.size;
        let inner = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(desc.label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: desc.format.to_wgpu(),
            usage: desc.usage.0,
            view_formats: &[],
        });
        let view = inner.create_view(&wgpu::TextureViewDescriptor::default());
        Texture {
            inner,
            view,
            size: (width.max(1), height.max(1)),
            format: desc.format,
        }
    }

    /// Compiles WGSL. Errors are caught with a validation scope and returned instead of
    /// reaching the uncaptured-error handler, so shader hot reload can keep the old version.
    pub fn create_shader_module(&self, label: &str, wgsl: &str) -> Result<ShaderModule, RhiError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let inner = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(wgsl.into()),
            });
        match pollster::block_on(scope.pop()) {
            None => Ok(ShaderModule { inner }),
            Some(e) => Err(RhiError::Shader {
                label: label.to_owned(),
                message: e.to_string(),
            }),
        }
    }

    pub fn create_bind_group_layout(
        &self,
        label: &str,
        entries: &[BindGroupLayoutEntry],
    ) -> BindGroupLayout {
        let entries: Vec<_> = entries
            .iter()
            .map(|e| wgpu::BindGroupLayoutEntry {
                binding: e.binding,
                visibility: e.visibility.to_wgpu(),
                ty: match e.ty {
                    BindingType::UniformBuffer {
                        has_dynamic_offset,
                        min_binding_size,
                    } => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset,
                        min_binding_size: wgpu::BufferSize::new(min_binding_size),
                    },
                },
                count: None,
            })
            .collect();
        BindGroupLayout {
            inner: self
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(label),
                    entries: &entries,
                }),
        }
    }

    pub fn create_bind_group(
        &self,
        label: &str,
        layout: &BindGroupLayout,
        entries: &[BindGroupEntry<'_>],
    ) -> BindGroup {
        let entries: Vec<_> = entries
            .iter()
            .map(|e| wgpu::BindGroupEntry {
                binding: e.binding,
                resource: match e.resource {
                    BindingResource::Buffer {
                        buffer,
                        offset,
                        size,
                    } => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer.inner,
                        offset,
                        size: size.and_then(wgpu::BufferSize::new),
                    }),
                },
            })
            .collect();
        BindGroup {
            inner: self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout.inner,
                entries: &entries,
            }),
        }
    }

    /// Builds a pipeline; validation errors come back as [`RhiError::Pipeline`].
    pub fn create_render_pipeline(
        &self,
        desc: &RenderPipelineDesc<'_>,
    ) -> Result<RenderPipeline, RhiError> {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);

        let groups: Vec<Option<&wgpu::BindGroupLayout>> = desc
            .bind_group_layouts
            .iter()
            .map(|l| Some(&l.inner))
            .collect();
        let layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(desc.label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });

        let attributes: Vec<Vec<wgpu::VertexAttribute>> = desc
            .vertex_layouts
            .iter()
            .map(|l| {
                l.attributes
                    .iter()
                    .map(|a| wgpu::VertexAttribute {
                        format: a.format.to_wgpu(),
                        offset: a.offset,
                        shader_location: a.location,
                    })
                    .collect()
            })
            .collect();
        let buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = desc
            .vertex_layouts
            .iter()
            .zip(&attributes)
            .map(|(l, attrs)| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: l.stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: attrs,
                })
            })
            .collect();
        let targets: Vec<Option<wgpu::ColorTargetState>> = desc
            .color_formats
            .iter()
            .map(|f| {
                Some(wgpu::ColorTargetState {
                    format: f.to_wgpu(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();

        let inner = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(desc.label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &desc.shader.inner,
                    entry_point: Some(desc.vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: match desc.cull_mode {
                        CullMode::None => None,
                        CullMode::Front => Some(wgpu::Face::Front),
                        CullMode::Back => Some(wgpu::Face::Back),
                    },
                    ..Default::default()
                },
                depth_stencil: desc.depth.map(|d| wgpu::DepthStencilState {
                    format: d.format.to_wgpu(),
                    depth_write_enabled: Some(d.write_enabled),
                    depth_compare: Some(d.compare.to_wgpu()),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &desc.shader.inner,
                    entry_point: Some(desc.fragment_entry),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                multiview_mask: None,
                cache: None,
            });

        match pollster::block_on(scope.pop()) {
            None => Ok(RenderPipeline { inner }),
            Some(e) => Err(RhiError::Pipeline {
                label: desc.label.to_owned(),
                message: e.to_string(),
            }),
        }
    }
}
