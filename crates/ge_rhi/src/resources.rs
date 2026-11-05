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
    /// Packed HDR color (no alpha), half the bandwidth of `Rgba16Float` (bloom chain).
    Rg11b10Float,
    Rgba32Float,
    R32Float,
    /// Reversed-Z depth target (lore §6.7): clear to 0, compare `Greater`.
    Depth32Float,
    /// Block-compressed (4×4 texel blocks). Need [`Rhi::supports_bc`].
    Bc1RgbaUnorm,
    Bc1RgbaUnormSrgb,
    Bc3RgbaUnorm,
    Bc3RgbaUnormSrgb,
    Bc4RUnorm,
    Bc5RgUnorm,
    Bc7RgbaUnorm,
    Bc7RgbaUnormSrgb,
}

impl TextureFormat {
    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            Self::Rgba8UnormSrgb
                | Self::Bgra8UnormSrgb
                | Self::Bc1RgbaUnormSrgb
                | Self::Bc3RgbaUnormSrgb
                | Self::Bc7RgbaUnormSrgb
        )
    }

    /// Bytes per 4×4 block for block-compressed formats, `None` otherwise.
    pub fn block_bytes(self) -> Option<u32> {
        match self {
            Self::Bc1RgbaUnorm | Self::Bc1RgbaUnormSrgb | Self::Bc4RUnorm => Some(8),
            Self::Bc3RgbaUnorm
            | Self::Bc3RgbaUnormSrgb
            | Self::Bc5RgUnorm
            | Self::Bc7RgbaUnorm
            | Self::Bc7RgbaUnormSrgb => Some(16),
            _ => None,
        }
    }

    pub fn is_compressed(self) -> bool {
        self.block_bytes().is_some()
    }

    /// Bytes of one tightly packed mip level of `width` × `height` texels, plus the row pitch
    /// and number of rows (block rows for compressed formats).
    pub fn level_layout(self, width: u32, height: u32) -> (u32, u32, u32) {
        match self.block_bytes() {
            Some(block) => {
                let bw = width.div_ceil(4).max(1);
                let bh = height.div_ceil(4).max(1);
                (bw * bh * block, bw * block, bh)
            }
            None => {
                let row = width * self.bytes_per_pixel();
                (row * height, row, height)
            }
        }
    }

    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8Unorm | Self::Rgba8UnormSrgb | Self::Bgra8Unorm | Self::Bgra8UnormSrgb => 4,
            Self::Depth32Float | Self::Rg11b10Float | Self::R32Float => 4,
            Self::Rgba16Float => 8,
            Self::Rgba32Float => 16,
            // Per-texel size is meaningless for block formats; see `level_layout`.
            _ => 0,
        }
    }

    pub(crate) fn to_wgpu(self) -> wgpu::TextureFormat {
        match self {
            Self::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            Self::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            Self::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            Self::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            Self::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
            Self::Rg11b10Float => wgpu::TextureFormat::Rg11b10Ufloat,
            Self::Rgba32Float => wgpu::TextureFormat::Rgba32Float,
            Self::R32Float => wgpu::TextureFormat::R32Float,
            Self::Depth32Float => wgpu::TextureFormat::Depth32Float,
            Self::Bc1RgbaUnorm => wgpu::TextureFormat::Bc1RgbaUnorm,
            Self::Bc1RgbaUnormSrgb => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
            Self::Bc3RgbaUnorm => wgpu::TextureFormat::Bc3RgbaUnorm,
            Self::Bc3RgbaUnormSrgb => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
            Self::Bc4RUnorm => wgpu::TextureFormat::Bc4RUnorm,
            Self::Bc5RgUnorm => wgpu::TextureFormat::Bc5RgUnorm,
            Self::Bc7RgbaUnorm => wgpu::TextureFormat::Bc7RgbaUnorm,
            Self::Bc7RgbaUnormSrgb => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
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
    pub const STORAGE: Self = Self(wgpu::BufferUsages::STORAGE);
    pub const INDIRECT: Self = Self(wgpu::BufferUsages::INDIRECT);
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
    pub const STORAGE_BINDING: Self = Self(wgpu::TextureUsages::STORAGE_BINDING);
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
    Compute,
    /// Fragment + compute (light lists read by shading and written by culling).
    FragmentCompute,
    All,
}

impl ShaderStages {
    fn to_wgpu(self) -> wgpu::ShaderStages {
        match self {
            Self::Vertex => wgpu::ShaderStages::VERTEX,
            Self::Fragment => wgpu::ShaderStages::FRAGMENT,
            Self::VertexFragment => wgpu::ShaderStages::VERTEX_FRAGMENT,
            Self::Compute => wgpu::ShaderStages::COMPUTE,
            Self::FragmentCompute => wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
            Self::All => wgpu::ShaderStages::all(),
        }
    }
}

/// How a texture is viewed by shaders and render targets.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum TextureDim {
    D2,
    D2Array,
    Cube,
}

impl TextureDim {
    fn to_wgpu(self) -> wgpu::TextureViewDimension {
        match self {
            Self::D2 => wgpu::TextureViewDimension::D2,
            Self::D2Array => wgpu::TextureViewDimension::D2Array,
            Self::Cube => wgpu::TextureViewDimension::Cube,
        }
    }
}

/// What a sampled texture binding returns.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SampleKind {
    Float {
        filterable: bool,
    },
    /// Depth textures (`texture_depth_2d[_array]`), for comparison sampling.
    Depth,
}

/// Depth bias in pipeline state (shadow maps). With reversed-Z, push *away* from the light
/// with negative values.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct DepthBias {
    pub constant: i32,
    pub slope_scale: f32,
    pub clamp: f32,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum VertexFormat {
    Float32x2,
    Float32x3,
    Float32x4,
    /// Four bytes read as 0..1 floats (packed vertex colors).
    Unorm8x4,
}

impl VertexFormat {
    fn to_wgpu(self) -> wgpu::VertexFormat {
        match self {
            Self::Float32x2 => wgpu::VertexFormat::Float32x2,
            Self::Float32x3 => wgpu::VertexFormat::Float32x3,
            Self::Float32x4 => wgpu::VertexFormat::Float32x4,
            Self::Unorm8x4 => wgpu::VertexFormat::Unorm8x4,
        }
    }
}

/// Color blending for a pipeline's targets. `None` in a descriptor means opaque (overwrite).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BlendMode {
    /// `src.rgb * src.a + dst.rgb * (1 - src.a)`.
    Alpha,
    /// Color already multiplied by alpha: `src.rgb + dst.rgb * (1 - src.a)` (egui, UI).
    PremultipliedAlpha,
    /// `src + dst` (bloom accumulation, light accumulation).
    Additive,
}

impl BlendMode {
    fn to_wgpu(self) -> wgpu::BlendState {
        match self {
            Self::Alpha => wgpu::BlendState::ALPHA_BLENDING,
            Self::PremultipliedAlpha => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            Self::Additive => wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            },
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FilterMode {
    Nearest,
    Linear,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AddressMode {
    ClampToEdge,
    Repeat,
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
    pub(crate) fn to_wgpu(self) -> wgpu::CompareFunction {
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
pub struct SamplerDesc<'a> {
    pub label: &'a str,
    pub filter: FilterMode,
    pub address: AddressMode,
    /// Blend between mip levels (trilinear) instead of picking the nearest one.
    pub mipmaps: bool,
    /// Max anisotropy (1 = off). Only applies with linear filtering.
    pub anisotropy: u16,
    /// Makes a comparison sampler (shadow maps): sampling returns the comparison result.
    pub compare: Option<CompareFunction>,
}

#[derive(Copy, Clone, Debug)]
pub enum BindingType {
    UniformBuffer {
        has_dynamic_offset: bool,
        /// Size of one bound element; checked against the shader at pipeline creation.
        min_binding_size: u64,
    },
    /// A filterable float 2D texture (`texture_2d<f32>`).
    Texture2d,
    /// A filtering sampler.
    Sampler,
    /// Any sampled texture: float/depth, 2D/array/cube.
    Texture { sample: SampleKind, dim: TextureDim },
    /// A storage buffer (`var<storage, read>` or `read_write`).
    StorageBuffer { read_only: bool },
    /// A comparison sampler (`sampler_comparison`).
    ComparisonSampler,
    /// A write-only storage texture (`texture_storage_2d[_array]<format, write>`).
    StorageTexture {
        format: TextureFormat,
        dim: TextureDim,
    },
    /// A non-filtering sampler (for unfilterable float textures).
    NonFilteringSampler,
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
    Texture(&'a Texture),
    Sampler(&'a Sampler),
    /// A specific view (mip range, layer, cube).
    View(&'a TextureView),
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
    /// `None` = depth-only pipeline (shadow maps, prepass).
    pub fragment_entry: Option<&'a str>,
    pub vertex_layouts: &'a [VertexLayout<'a>],
    pub bind_group_layouts: &'a [&'a BindGroupLayout],
    pub color_formats: &'a [TextureFormat],
    pub depth: Option<DepthState>,
    pub cull_mode: CullMode,
    /// `None` = opaque.
    pub blend: Option<BlendMode>,
    pub depth_bias: Option<DepthBias>,
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
    mip_levels: u32,
    layers: u32,
}

/// A view of part of a texture: a mip range and a layer range, as 2D, array or cube.
#[derive(Clone, Debug)]
pub struct TextureView {
    pub(crate) inner: wgpu::TextureView,
    size: (u32, u32),
}

impl TextureView {
    pub(crate) fn from_raw(inner: wgpu::TextureView, size: (u32, u32)) -> Self {
        Self { inner, size }
    }

    /// Size of the view's first mip level in texels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// Which part of a texture a [`TextureView`] covers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ViewRange {
    pub dim: TextureDim,
    pub base_mip: u32,
    pub mips: u32,
    pub base_layer: u32,
    pub layers: u32,
}

impl ViewRange {
    /// One mip of one layer, as a 2D view (a render or storage target).
    pub fn level(mip: u32, layer: u32) -> Self {
        Self {
            dim: TextureDim::D2,
            base_mip: mip,
            mips: 1,
            base_layer: layer,
            layers: 1,
        }
    }
}

/// A texture with layers (array or cube) and mips.
#[derive(Copy, Clone, Debug)]
pub struct TextureDescEx<'a> {
    pub label: &'a str,
    pub size: (u32, u32),
    pub format: TextureFormat,
    pub usage: TextureUsage,
    pub mip_levels: u32,
    /// 6 for a cube map.
    pub layers: u32,
    /// The default view: `D2Array` for arrays, `Cube` for cube maps.
    pub dim: TextureDim,
}

impl Texture {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn mip_levels(&self) -> u32 {
        self.mip_levels
    }

    pub fn layers(&self) -> u32 {
        self.layers
    }

    /// The texture's default view (all mips and layers).
    pub fn full_view(&self) -> TextureView {
        TextureView {
            inner: self.view.clone(),
            size: self.size,
        }
    }

    /// A view of part of the texture.
    pub fn view(&self, range: ViewRange) -> TextureView {
        let aspect = if self.format == TextureFormat::Depth32Float {
            wgpu::TextureAspect::DepthOnly
        } else {
            wgpu::TextureAspect::All
        };
        TextureView {
            inner: self.inner.create_view(&wgpu::TextureViewDescriptor {
                label: None,
                format: None,
                dimension: Some(range.dim.to_wgpu()),
                usage: None,
                aspect,
                base_mip_level: range.base_mip,
                mip_level_count: Some(range.mips.max(1)),
                base_array_layer: range.base_layer,
                array_layer_count: Some(range.layers.max(1)),
            }),
            size: (
                (self.size.0 >> range.base_mip).max(1),
                (self.size.1 >> range.base_mip).max(1),
            ),
        }
    }

    pub fn format(&self) -> TextureFormat {
        self.format
    }
}

#[derive(Clone, Debug)]
pub struct Sampler {
    pub(crate) inner: wgpu::Sampler,
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
        self.create_texture_mips(desc, 1)
    }

    /// A 2D texture with `mip_levels` levels (fill each with [`Rhi::write_texture_mip`]).
    pub fn create_texture_mips(&self, desc: &TextureDesc<'_>, mip_levels: u32) -> Texture {
        self.create_texture_ex(&TextureDescEx {
            label: desc.label,
            size: desc.size,
            format: desc.format,
            usage: desc.usage,
            mip_levels,
            layers: 1,
            dim: TextureDim::D2,
        })
    }

    /// Arrays and cube maps (shadow cascades, environment cubes).
    pub fn create_texture_ex(&self, desc: &TextureDescEx<'_>) -> Texture {
        let mip_levels = desc.mip_levels;
        let layers = desc.layers.max(1);
        let (width, height) = desc.size;
        let inner = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(desc.label),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: layers,
            },
            mip_level_count: mip_levels.max(1),
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: desc.format.to_wgpu(),
            usage: desc.usage.0,
            view_formats: &[],
        });
        let view = inner.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(desc.dim.to_wgpu()),
            ..Default::default()
        });
        Texture {
            inner,
            view,
            size: (width.max(1), height.max(1)),
            format: desc.format,
            mip_levels: mip_levels.max(1),
            layers,
        }
    }

    /// Uploads one whole mip level of one layer (cube face / array slice).
    pub fn write_texture_layer(
        &self,
        texture: &Texture,
        mip: u32,
        layer: u32,
        data: &[u8],
    ) -> Result<(), RhiError> {
        let w = (texture.size.0 >> mip).max(1);
        let h = (texture.size.1 >> mip).max(1);
        let (bytes, row, rows) = texture.format.level_layout(w, h);
        if mip >= texture.mip_levels || layer >= texture.layers || data.len() != bytes as usize {
            return Err(RhiError::Upload(format!(
                "mip {mip} layer {layer} of a {:?} texture needs {bytes} bytes, got {}",
                texture.format,
                data.len()
            )));
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.inner,
                mip_level: mip,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Uploads one whole mip level (tightly packed rows, or blocks for compressed formats).
    pub fn write_texture_mip(
        &self,
        texture: &Texture,
        mip: u32,
        data: &[u8],
    ) -> Result<(), RhiError> {
        let w = (texture.size.0 >> mip).max(1);
        let h = (texture.size.1 >> mip).max(1);
        let (bytes, row, rows) = texture.format.level_layout(w, h);
        if mip >= texture.mip_levels || data.len() != bytes as usize {
            return Err(RhiError::Upload(format!(
                "mip {mip} of a {:?} {}x{} texture needs {bytes} bytes, got {}",
                texture.format,
                texture.size.0,
                texture.size.1,
                data.len()
            )));
        }
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.inner,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }

    /// Uploads `data` (tightly packed rows, the texture's format) into the `size` rectangle at
    /// `origin`. Lands before the next submitted frame.
    pub fn write_texture(
        &self,
        texture: &Texture,
        origin: (u32, u32),
        size: (u32, u32),
        data: &[u8],
    ) {
        let bpp = texture.format.bytes_per_pixel();
        debug_assert_eq!(data.len() as u64, u64::from(size.0 * size.1 * bpp));
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture.inner,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: origin.0,
                    y: origin.1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.0 * bpp),
                rows_per_image: Some(size.1),
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    pub fn create_sampler(&self, desc: &SamplerDesc<'_>) -> Sampler {
        let filter = match desc.filter {
            FilterMode::Nearest => wgpu::FilterMode::Nearest,
            FilterMode::Linear => wgpu::FilterMode::Linear,
        };
        let address = match desc.address {
            AddressMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
            AddressMode::Repeat => wgpu::AddressMode::Repeat,
        };
        Sampler {
            inner: self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(desc.label),
                address_mode_u: address,
                address_mode_v: address,
                address_mode_w: address,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: if desc.mipmaps {
                    wgpu::MipmapFilterMode::Linear
                } else {
                    wgpu::MipmapFilterMode::Nearest
                },
                anisotropy_clamp: if desc.filter == FilterMode::Linear && desc.mipmaps {
                    desc.anisotropy.clamp(1, 16)
                } else {
                    1
                },
                compare: desc.compare.map(CompareFunction::to_wgpu),
                ..Default::default()
            }),
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
                    BindingType::Texture2d => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    BindingType::Sampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                    }
                    BindingType::NonFilteringSampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering)
                    }
                    BindingType::ComparisonSampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
                    }
                    BindingType::Texture { sample, dim } => wgpu::BindingType::Texture {
                        sample_type: match sample {
                            SampleKind::Float { filterable } => {
                                wgpu::TextureSampleType::Float { filterable }
                            }
                            SampleKind::Depth => wgpu::TextureSampleType::Depth,
                        },
                        view_dimension: dim.to_wgpu(),
                        multisampled: false,
                    },
                    BindingType::StorageBuffer { read_only } => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    BindingType::StorageTexture { format, dim } => {
                        wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: format.to_wgpu(),
                            view_dimension: dim.to_wgpu(),
                        }
                    }
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
                    BindingResource::Texture(texture) => {
                        wgpu::BindingResource::TextureView(&texture.view)
                    }
                    BindingResource::Sampler(sampler) => {
                        wgpu::BindingResource::Sampler(&sampler.inner)
                    }
                    BindingResource::View(view) => wgpu::BindingResource::TextureView(&view.inner),
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
                    blend: desc.blend.map(BlendMode::to_wgpu),
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
                    bias: desc
                        .depth_bias
                        .map_or_else(Default::default, |b| wgpu::DepthBiasState {
                            constant: b.constant,
                            slope_scale: b.slope_scale,
                            clamp: b.clamp,
                        }),
                }),
                multisample: Default::default(),
                fragment: desc.fragment_entry.map(|entry| wgpu::FragmentState {
                    module: &desc.shader.inner,
                    entry_point: Some(entry),
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
