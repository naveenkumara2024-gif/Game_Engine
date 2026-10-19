//! Indexed triangle meshes. M1 builds them procedurally; glTF import arrives with assets (M3).

use bytemuck::{Pod, Zeroable};
use ge_rhi::{Buffer, BufferUsage, Rhi, VertexAttribute, VertexFormat, VertexLayout};
use glam::Vec3;

/// Interleaved vertex. Matches `VertexIn` in `shaders/mesh.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

const VERTEX_ATTRIBUTES: [VertexAttribute; 3] = [
    VertexAttribute {
        location: 0,
        offset: 0,
        format: VertexFormat::Float32x3,
    },
    VertexAttribute {
        location: 1,
        offset: 12,
        format: VertexFormat::Float32x3,
    },
    VertexAttribute {
        location: 2,
        offset: 24,
        format: VertexFormat::Float32x3,
    },
];

impl Vertex {
    pub const LAYOUT: VertexLayout<'static> = VertexLayout {
        stride: size_of::<Vertex>() as u64,
        attributes: &VERTEX_ATTRIBUTES,
    };
}

/// CPU-side mesh: counter-clockwise front faces, 32-bit indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl MeshData {
    /// Axis-aligned cube centered on the origin, flat-shaded (4 vertices per face).
    pub fn cube(half_extent: f32, color: [f32; 3]) -> Self {
        // (normal, u, v) with u × v = normal, so the quad below winds CCW seen from outside.
        let faces = [
            (Vec3::X, Vec3::Y, Vec3::Z),
            (Vec3::NEG_X, Vec3::Z, Vec3::Y),
            (Vec3::Y, Vec3::Z, Vec3::X),
            (Vec3::NEG_Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        ];
        let mut mesh = Self::default();
        for (n, u, v) in faces {
            mesh.push_quad(n * half_extent, u * half_extent, v * half_extent, n, color);
        }
        mesh
    }

    /// Square in the XZ plane facing +Y.
    pub fn plane(half_size: f32, color: [f32; 3]) -> Self {
        let mut mesh = Self::default();
        mesh.push_quad(
            Vec3::ZERO,
            Vec3::Z * half_size,
            Vec3::X * half_size,
            Vec3::Y,
            color,
        );
        mesh
    }

    fn push_quad(&mut self, center: Vec3, u: Vec3, v: Vec3, normal: Vec3, color: [f32; 3]) {
        let base = self.vertices.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            self.vertices.push(Vertex {
                position: (center + u * su + v * sv).to_array(),
                normal: normal.to_array(),
                color,
            });
        }
        self.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// Every index points at a vertex and the index count is a whole number of triangles.
    pub fn is_valid(&self) -> bool {
        self.indices.len().is_multiple_of(3)
            && self
                .indices
                .iter()
                .all(|&i| (i as usize) < self.vertices.len())
    }
}

/// A mesh uploaded to GPU buffers.
#[derive(Debug)]
pub struct GpuMesh {
    pub vertex_buffer: Buffer,
    pub index_buffer: Buffer,
    pub index_count: u32,
}

impl GpuMesh {
    pub fn upload(rhi: &Rhi, label: &str, mesh: &MeshData) -> Self {
        debug_assert!(mesh.is_valid(), "mesh '{label}' has out-of-range indices");
        Self {
            vertex_buffer: rhi.create_buffer_init(
                &format!("{label}::Vertices"),
                BufferUsage::VERTEX,
                bytemuck::cast_slice(&mesh.vertices),
            ),
            index_buffer: rhi.create_buffer_init(
                &format!("{label}::Indices"),
                BufferUsage::INDEX,
                bytemuck::cast_slice(&mesh.indices),
            ),
            index_count: mesh.indices.len() as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_outward_ccw(mesh: &MeshData) {
        for tri in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(mesh.vertices[tri[k] as usize].position));
            let n = Vec3::from(mesh.vertices[tri[0] as usize].normal);
            let face = (b - a).cross(c - a);
            assert!(
                face.dot(n) > 0.0,
                "triangle {tri:?} winds against its normal"
            );
        }
    }

    #[test]
    fn cube_is_valid_and_ccw() {
        let cube = MeshData::cube(0.5, [1.0; 3]);
        assert!(cube.is_valid());
        assert_eq!(cube.vertices.len(), 24);
        assert_eq!(cube.indices.len(), 36);
        assert_outward_ccw(&cube);
        // Every face sits on the surface of the cube.
        for v in &cube.vertices {
            let p = Vec3::from(v.position);
            assert!((p.abs().max_element() - 0.5).abs() < 1e-6);
            assert!((p.dot(Vec3::from(v.normal)) - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn plane_faces_up() {
        let plane = MeshData::plane(2.0, [1.0; 3]);
        assert!(plane.is_valid());
        assert_outward_ccw(&plane);
    }

    #[test]
    fn invalid_index_detected() {
        let mut m = MeshData::plane(1.0, [1.0; 3]);
        m.indices[0] = 99;
        assert!(!m.is_valid());
    }

    #[test]
    fn vertex_layout_matches_struct() {
        assert_eq!(Vertex::LAYOUT.stride, 36);
    }
}
