//! Asset types v1 (plan 0005, decision 4). Plain data, GPU-agnostic, serializable (the DDC
//! stores them with postcard). The renderer turns them into GPU resources.

use serde::{Deserialize, Serialize};

use crate::handle::AssetId;

/// An indexed triangle mesh (one glTF *primitive*). Y-up, right-handed, metres (glTF's own
/// convention, which is ours).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// xyz = tangent, w = bitangent sign (glTF / MikkTSpace convention). From the file when
    /// it has them, else generated from the UVs at import.
    pub tangents: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

impl Mesh {
    /// Per-vertex tangents from the UVs: accumulated per triangle, orthogonalized against the
    /// normal, with the bitangent sign in w.
    pub fn generate_tangents(&mut self) {
        let n = self.positions.len();
        let mut tan = vec![glam::Vec3::ZERO; n];
        let mut bit = vec![glam::Vec3::ZERO; n];
        for tri in self.indices.chunks_exact(3) {
            let [a, b, c] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            if a.max(b).max(c) >= n {
                continue;
            }
            let p = |i: usize| glam::Vec3::from(self.positions[i]);
            let (e1, e2) = (p(b) - p(a), p(c) - p(a));
            let (ua, ub, uc) = (self.uvs[a], self.uvs[b], self.uvs[c]);
            let (du1, dv1) = (ub[0] - ua[0], ub[1] - ua[1]);
            let (du2, dv2) = (uc[0] - ua[0], uc[1] - ua[1]);
            let det = du1 * dv2 - du2 * dv1;
            if det.abs() < 1e-12 {
                continue;
            }
            let t = (e1 * dv2 - e2 * dv1) / det;
            let bt = (e2 * du1 - e1 * du2) / det;
            for i in [a, b, c] {
                tan[i] += t;
                bit[i] += bt;
            }
        }
        self.tangents = (0..n)
            .map(|i| {
                let nrm = glam::Vec3::from(self.normals[i]);
                let t = (tan[i] - nrm * nrm.dot(tan[i])).normalize_or(nrm.any_orthonormal_vector());
                let w = if nrm.cross(t).dot(bit[i]) < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                t.extend(w).to_array()
            })
            .collect();
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Every index is in range, attribute arrays agree, and there is a whole number of
    /// triangles.
    pub fn is_valid(&self) -> bool {
        let n = self.positions.len();
        self.normals.len() == n
            && self.uvs.len() == n
            && self.tangents.len() == n
            && self.indices.len().is_multiple_of(3)
            && self.indices.iter().all(|&i| (i as usize) < n)
    }

    pub fn compute_bounds(&mut self) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        if self.positions.is_empty() {
            min = [0.0; 3];
            max = [0.0; 3];
        }
        self.bounds_min = min;
        self.bounds_max = max;
    }
}

/// Pixel formats textures can arrive in. sRGB variants are decoded to linear by the sampler.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelFormat {
    Rgba8,
    Rgba8Srgb,
    Bc1,
    Bc1Srgb,
    Bc3,
    Bc3Srgb,
    Bc4,
    Bc5,
    Bc7,
    Bc7Srgb,
    /// Half-float RGBA (HDR environment maps).
    Rgba16Float,
}

impl PixelFormat {
    pub fn is_compressed(self) -> bool {
        !matches!(self, Self::Rgba8 | Self::Rgba8Srgb | Self::Rgba16Float)
    }

    pub fn is_srgb(self) -> bool {
        matches!(
            self,
            Self::Rgba8Srgb | Self::Bc1Srgb | Self::Bc3Srgb | Self::Bc7Srgb
        )
    }

    /// Bytes of one tightly packed level (4×4 blocks for BC formats).
    pub fn level_size(self, width: u32, height: u32) -> usize {
        let blocks = || (width.div_ceil(4).max(1) * height.div_ceil(4).max(1)) as usize;
        match self {
            Self::Rgba8 | Self::Rgba8Srgb => (width * height * 4) as usize,
            Self::Rgba16Float => (width * height * 8) as usize,
            Self::Bc1 | Self::Bc1Srgb | Self::Bc4 => blocks() * 8,
            Self::Bc3 | Self::Bc3Srgb | Self::Bc5 | Self::Bc7 | Self::Bc7Srgb => blocks() * 16,
        }
    }
}

/// A 2D texture with its full mip chain (level 0 first).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Texture {
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

impl Texture {
    /// Every level has the size its format and dimensions require.
    pub fn is_valid(&self) -> bool {
        !self.mips.is_empty()
            && self.mips.iter().enumerate().all(|(i, data)| {
                let w = (self.width >> i).max(1);
                let h = (self.height >> i).max(1);
                data.len() == self.format.level_size(w, h)
            })
    }

    pub fn byte_size(&self) -> usize {
        self.mips.iter().map(Vec::len).sum()
    }
}

/// A reference from one sub-asset to another in the same source file: the `label` is stored,
/// the `id` is filled in when the source finishes loading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubRef<T: 'static> {
    pub label: String,
    #[serde(skip, default = "unresolved")]
    pub id: Option<AssetId<T>>,
}

fn unresolved<T>() -> Option<AssetId<T>> {
    None
}

impl<T> SubRef<T> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            id: None,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// glTF's metallic-roughness material (plan 0005, decision 4). Normal, occlusion and alpha are
/// stored now and rendered from M4.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAsset {
    pub name: String,
    /// Linear RGBA factor.
    pub base_color: [f32; 4],
    pub base_color_texture: Option<SubRef<Texture>>,
    pub metallic: f32,
    pub roughness: f32,
    /// glTF packs roughness in G and metallic in B.
    pub metallic_roughness_texture: Option<SubRef<Texture>>,
    /// Linear RGB factor × strength.
    pub emissive: [f32; 3],
    pub emissive_texture: Option<SubRef<Texture>>,
    pub normal_texture: Option<SubRef<Texture>>,
    pub normal_scale: f32,
    pub occlusion_texture: Option<SubRef<Texture>>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
}

impl Default for MaterialAsset {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            emissive: [0.0; 3],
            emissive_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
        }
    }
}

impl MaterialAsset {
    pub fn textures_mut(&mut self) -> impl Iterator<Item = &mut SubRef<Texture>> {
        [
            &mut self.base_color_texture,
            &mut self.metallic_roughness_texture,
            &mut self.emissive_texture,
            &mut self.normal_texture,
            &mut self.occlusion_texture,
        ]
        .into_iter()
        .flatten()
    }
}

/// One mesh drawn by a node, with its material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelPrimitive {
    pub mesh: SubRef<Mesh>,
    pub material: Option<SubRef<MaterialAsset>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelNode {
    pub name: String,
    pub translation: [f32; 3],
    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub children: Vec<usize>,
    pub primitives: Vec<ModelPrimitive>,
}

/// A glTF scene: a node tree whose nodes reference meshes and materials of the same file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelAsset {
    pub nodes: Vec<ModelNode>,
    pub roots: Vec<usize>,
}

impl ModelAsset {
    /// Node indices in a valid tree (no out-of-range children).
    pub fn is_valid(&self) -> bool {
        let n = self.nodes.len();
        self.roots.iter().all(|&r| r < n)
            && self
                .nodes
                .iter()
                .all(|node| node.children.iter().all(|&c| c < n))
    }
}

/// One output of an importer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SubAsset {
    Mesh(Mesh),
    Texture(Texture),
    Material(MaterialAsset),
    Model(ModelAsset),
}

impl SubAsset {
    pub fn kind(&self) -> &'static str {
        match self {
            SubAsset::Mesh(_) => "Mesh",
            SubAsset::Texture(_) => "Texture",
            SubAsset::Material(_) => "Material",
            SubAsset::Model(_) => "Model",
        }
    }
}

/// Everything one source file imports to: labelled sub-assets. This is what the DDC stores.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportOutput {
    pub assets: Vec<(String, SubAsset)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_sizes() {
        assert_eq!(PixelFormat::Rgba8.level_size(4, 2), 32);
        assert_eq!(PixelFormat::Bc1.level_size(4, 4), 8);
        assert_eq!(PixelFormat::Bc1.level_size(1, 1), 8, "partial block");
        assert_eq!(PixelFormat::Bc7Srgb.level_size(8, 8), 64);
    }

    #[test]
    fn import_output_round_trips_through_postcard() {
        let m = MaterialAsset {
            base_color_texture: Some(SubRef::new("Texture0")),
            ..Default::default()
        };
        let out = ImportOutput {
            assets: vec![
                ("Material0".into(), SubAsset::Material(m)),
                (
                    "Texture0".into(),
                    SubAsset::Texture(Texture {
                        format: PixelFormat::Rgba8Srgb,
                        width: 1,
                        height: 1,
                        mips: vec![vec![1, 2, 3, 4]],
                    }),
                ),
            ],
        };
        let bytes = postcard::to_allocvec(&out).unwrap();
        let back: ImportOutput = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, out);
    }
}
