//! glTF 2.0 importer (`.glb` and `.gltf`). Produces labelled sub-assets:
//!
//! - `Mesh{m}/{p}`: one [`Mesh`] per primitive `p` of glTF mesh `m` (triangles only);
//! - `Material{i}`: one [`MaterialAsset`] per glTF material;
//! - `Texture{img}` (color, sRGB) and `Texture{img}Linear` (data) per image use;
//! - `Model`: the default scene's node tree.
//!
//! Buffers and images are resolved here (GLB blob, `data:` URIs or files next to the
//! `.gltf`), so every external file is a declared dependency and part of the DDC key.

use std::collections::{BTreeMap, BTreeSet};

use glam::{Mat4, Quat, Vec3};
use gltf::buffer::Source as BufferSource;
use gltf::image::Source as ImageSource;
use tracing::warn;

use crate::AssetError;
use crate::meta::ImportSettings;
use crate::types::{
    AlphaMode, ImportOutput, MaterialAsset, Mesh, ModelAsset, ModelNode, ModelPrimitive, SubAsset,
    SubRef,
};

pub const IMPORTER_VERSION: u32 = 1;

fn err(m: impl std::fmt::Display) -> AssetError {
    AssetError::Import(format!("glTF: {m}"))
}

/// External files (relative URIs) the document references, in a stable order.
pub fn dependencies(bytes: &[u8]) -> Result<Vec<String>, AssetError> {
    let gltf = gltf::Gltf::from_slice_without_validation(bytes).map_err(err)?;
    let mut uris = BTreeSet::new();
    for b in gltf.document.buffers() {
        if let BufferSource::Uri(uri) = b.source()
            && !uri.starts_with("data:")
        {
            uris.insert(percent_decode(uri));
        }
    }
    for i in gltf.document.images() {
        if let ImageSource::Uri { uri, .. } = i.source()
            && !uri.starts_with("data:")
        {
            uris.insert(percent_decode(uri));
        }
    }
    Ok(uris.into_iter().collect())
}

pub fn import(
    bytes: &[u8],
    deps: &BTreeMap<String, Vec<u8>>,
    settings: &ImportSettings,
) -> Result<ImportOutput, AssetError> {
    let gltf = gltf::Gltf::from_slice(bytes).map_err(err)?;
    let doc = &gltf.document;

    let resolve_uri = |uri: &str| -> Result<Vec<u8>, AssetError> {
        if let Some(rest) = uri.strip_prefix("data:") {
            let (_, data) = rest
                .split_once(";base64,")
                .ok_or_else(|| err("only base64 data URIs are supported"))?;
            return base64_decode(data).ok_or_else(|| err("bad base64 data URI"));
        }
        deps.get(&percent_decode(uri))
            .cloned()
            .ok_or_else(|| err(format!("missing file '{uri}' next to the .gltf")))
    };

    let mut buffers = Vec::new();
    for b in doc.buffers() {
        let data = match b.source() {
            BufferSource::Bin => gltf
                .blob
                .clone()
                .ok_or_else(|| err("GLB has no BIN chunk"))?,
            BufferSource::Uri(uri) => resolve_uri(uri)?,
        };
        if data.len() < b.length() {
            return Err(err(format!(
                "buffer {} is shorter than declared",
                b.index()
            )));
        }
        buffers.push(data);
    }

    let mut out = ImportOutput::default();

    // Materials first: they decide which images are color (sRGB) and which are data.
    let mut wanted: BTreeSet<(usize, bool)> = BTreeSet::new();
    let tex_ref = |info: Option<gltf::texture::Texture<'_>>,
                   srgb: bool,
                   wanted: &mut BTreeSet<(usize, bool)>| {
        info.map(|t| {
            let img = t.source().index();
            wanted.insert((img, srgb));
            SubRef::new(texture_label(img, srgb))
        })
    };
    for m in doc.materials() {
        let Some(index) = m.index() else { continue };
        let pbr = m.pbr_metallic_roughness();
        let strength = m.emissive_strength().unwrap_or(1.0);
        let e = m.emissive_factor();
        let material = MaterialAsset {
            name: m.name().unwrap_or_default().to_owned(),
            base_color: pbr.base_color_factor(),
            base_color_texture: tex_ref(
                pbr.base_color_texture().map(|i| i.texture()),
                true,
                &mut wanted,
            ),
            metallic: pbr.metallic_factor(),
            roughness: pbr.roughness_factor(),
            metallic_roughness_texture: tex_ref(
                pbr.metallic_roughness_texture().map(|i| i.texture()),
                false,
                &mut wanted,
            ),
            emissive: [e[0] * strength, e[1] * strength, e[2] * strength],
            emissive_texture: tex_ref(m.emissive_texture().map(|i| i.texture()), true, &mut wanted),
            normal_texture: tex_ref(m.normal_texture().map(|n| n.texture()), false, &mut wanted),
            normal_scale: m.normal_texture().map_or(1.0, |n| n.scale()),
            occlusion_texture: tex_ref(
                m.occlusion_texture().map(|o| o.texture()),
                false,
                &mut wanted,
            ),
            alpha_mode: match m.alpha_mode() {
                gltf::material::AlphaMode::Opaque => AlphaMode::Opaque,
                gltf::material::AlphaMode::Mask => AlphaMode::Mask,
                gltf::material::AlphaMode::Blend => AlphaMode::Blend,
            },
            alpha_cutoff: m.alpha_cutoff().unwrap_or(0.5),
            double_sided: m.double_sided(),
        };
        out.assets
            .push((format!("Material{index}"), SubAsset::Material(material)));
    }

    // Images: decode each one once, convert per use.
    let images: Vec<_> = doc.images().collect();
    let mut decoded: BTreeMap<usize, Option<Vec<u8>>> = BTreeMap::new();
    for &(img, srgb) in &wanted {
        let Some(image) = images.get(img) else {
            continue;
        };
        let bytes = decoded.entry(img).or_insert_with(|| {
            let bytes = match image.source() {
                ImageSource::View { view, .. } => {
                    let buf = &buffers[view.buffer().index()];
                    buf.get(view.offset()..view.offset() + view.length())
                        .map(<[u8]>::to_vec)
                }
                ImageSource::Uri { uri, .. } => resolve_uri(uri).ok(),
            };
            if bytes.is_none() {
                warn!(image = img, "glTF image data missing; using white");
            }
            bytes
        });
        let Some(bytes) = bytes else { continue };
        match super::image::decode(bytes, srgb, settings.generate_mips) {
            Ok(t) => out
                .assets
                .push((texture_label(img, srgb), SubAsset::Texture(t))),
            Err(e) => warn!(image = img, error = %e, "glTF image skipped; using white"),
        }
    }

    // Meshes.
    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                warn!(mesh = mesh.index(), mode = ?prim.mode(), "only triangle primitives are imported");
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let Some(positions) = reader.read_positions() else {
                warn!(mesh = mesh.index(), "primitive without positions skipped");
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();
            let n = positions.len();
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..n as u32).collect(),
            };
            let mut uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|t| t.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0; 2]; n]);
            if settings.flip_uvs {
                for uv in &mut uvs {
                    uv[1] = 1.0 - uv[1];
                }
            }
            let normals = reader
                .read_normals()
                .map(|n| n.collect())
                .unwrap_or_else(|| smooth_normals(&positions, &indices));
            let mut m = Mesh {
                positions,
                normals,
                uvs,
                indices,
                ..Default::default()
            };
            if !m.is_valid() {
                warn!(
                    mesh = mesh.index(),
                    "primitive with inconsistent attributes skipped"
                );
                continue;
            }
            m.compute_bounds();
            out.assets
                .push((mesh_label(mesh.index(), prim.index()), SubAsset::Mesh(m)));
        }
    }
    let have: BTreeSet<String> = out.assets.iter().map(|(l, _)| l.clone()).collect();

    // Model: the default scene (or the first one).
    let scene = doc.default_scene().or_else(|| doc.scenes().next());
    let mut model = ModelAsset::default();
    for node in doc.nodes() {
        let (t, r, s) = node.transform().decomposed();
        let primitives = node
            .mesh()
            .map(|mesh| {
                mesh.primitives()
                    .filter(|p| have.contains(&mesh_label(mesh.index(), p.index())))
                    .map(|p| ModelPrimitive {
                        mesh: SubRef::new(mesh_label(mesh.index(), p.index())),
                        material: p
                            .material()
                            .index()
                            .map(|i| SubRef::new(format!("Material{i}"))),
                    })
                    .collect()
            })
            .unwrap_or_default();
        model.nodes.push(ModelNode {
            name: node
                .name()
                .map_or_else(|| format!("Node.{:03}", node.index()), str::to_owned),
            translation: t,
            rotation: r,
            scale: s,
            children: node.children().map(|c| c.index()).collect(),
            primitives,
        });
    }
    model.roots = scene.map_or_else(Vec::new, |s| s.nodes().map(|n| n.index()).collect());
    if settings.scale != 1.0 {
        for &root in &model.roots {
            let node = &mut model.nodes[root];
            let m = Mat4::from_scale(Vec3::splat(settings.scale))
                * Mat4::from_scale_rotation_translation(
                    Vec3::from(node.scale),
                    Quat::from_array(node.rotation),
                    Vec3::from(node.translation),
                );
            let (s, r, t) = m.to_scale_rotation_translation();
            node.scale = s.to_array();
            node.rotation = r.to_array();
            node.translation = t.to_array();
        }
    }
    if !model.is_valid() {
        return Err(err("node tree references missing nodes"));
    }
    out.assets.push(("Model".into(), SubAsset::Model(model)));
    Ok(out)
}

pub fn mesh_label(mesh: usize, primitive: usize) -> String {
    format!("Mesh{mesh}/{primitive}")
}

pub fn texture_label(image: usize, srgb: bool) -> String {
    if srgb {
        format!("Texture{image}")
    } else {
        format!("Texture{image}Linear")
    }
}

/// Area-weighted vertex normals for meshes exported without them.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks_exact(3) {
        let [a, b, c] = [0, 1, 2].map(|k| tri[k] as usize);
        if a.max(b).max(c) >= positions.len() {
            continue;
        }
        let (pa, pb, pc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        let n = (pb - pa).cross(pc - pa);
        for i in [a, b, c] {
            acc[i] += n;
        }
    }
    acc.into_iter()
        .map(|n| n.normalize_or(Vec3::Y).to_array())
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let clean: Vec<u8> = s
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_percent_decoding() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ").unwrap(), b"hello world");
        assert!(base64_decode("!!!").is_none());
        assert_eq!(percent_decode("my%20file.bin"), "my file.bin");
        assert_eq!(percent_decode("plain.png"), "plain.png");
    }

    #[test]
    fn smooth_normals_point_out_of_a_ccw_triangle() {
        let n = smooth_normals(&[[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], &[0, 1, 2]);
        assert_eq!(n[0], [0.0, 0.0, 1.0]);
    }
}
