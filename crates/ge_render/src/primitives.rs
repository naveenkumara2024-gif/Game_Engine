//! Procedural primitives for the editor's Add ▸ Mesh menu. Shapes, parameter names and default
//! sizes follow Blender's Add Mesh menu (plan 0004, finding B7) so tutorials translate 1:1; the
//! generators themselves are our own.
//!
//! Every generator builds a *smooth* indexed mesh with analytic normals and hard edges split
//! where the surface has a crease (cylinder caps, cone base). `flat` shading then "unwelds" it
//! so each triangle carries its face normal. Glossy/diffuse shading of smooth vs flat is one of
//! the first things the Light and Material tabs teach.
//!
//! Bridge until assets (M3): primitives are addressed by value through [`MeshKind::Primitive`]
//! and cached by the renderer; M3 replaces this with `Handle<Mesh>`.
//!
//! [`MeshKind::Primitive`]: crate::MeshKind::Primitive

use std::f32::consts::{PI, TAU};
use std::hash::{Hash, Hasher};

use glam::Vec3;

use crate::mesh::{MeshData, Vertex};

/// A primitive shape and its generator settings (Blender's "Adjust Last Operation" panel).
/// Lengths are in metres, counts are clamped to sensible minimums by [`Primitive::sanitized`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Primitive {
    Plane {
        size: f32,
    },
    Cube {
        size: f32,
    },
    /// A filled disc (Blender's Circle with "N-Gon" fill: an empty circle would be invisible).
    Circle {
        vertices: u32,
        radius: f32,
    },
    UvSphere {
        segments: u32,
        rings: u32,
        radius: f32,
    },
    IcoSphere {
        subdivisions: u32,
        radius: f32,
    },
    Cylinder {
        vertices: u32,
        radius: f32,
        depth: f32,
    },
    Cone {
        vertices: u32,
        /// Base radius.
        radius1: f32,
        /// Tip radius; 0 makes a point.
        radius2: f32,
        depth: f32,
    },
    Torus {
        major_segments: u32,
        minor_segments: u32,
        major_radius: f32,
        minor_radius: f32,
    },
}

impl Primitive {
    /// Every shape with Blender's default settings, in Add-menu order.
    pub const ALL_DEFAULTS: [Primitive; 8] = [
        Primitive::Plane { size: 2.0 },
        Primitive::Cube { size: 2.0 },
        Primitive::Circle {
            vertices: 32,
            radius: 1.0,
        },
        Primitive::UvSphere {
            segments: 32,
            rings: 16,
            radius: 1.0,
        },
        Primitive::IcoSphere {
            subdivisions: 2,
            radius: 1.0,
        },
        Primitive::Cylinder {
            vertices: 32,
            radius: 1.0,
            depth: 2.0,
        },
        Primitive::Cone {
            vertices: 32,
            radius1: 1.0,
            radius2: 0.0,
            depth: 2.0,
        },
        Primitive::Torus {
            major_segments: 48,
            minor_segments: 12,
            major_radius: 1.0,
            minor_radius: 0.25,
        },
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Primitive::Plane { .. } => "Plane",
            Primitive::Cube { .. } => "Cube",
            Primitive::Circle { .. } => "Circle",
            Primitive::UvSphere { .. } => "UV Sphere",
            Primitive::IcoSphere { .. } => "Ico Sphere",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Cone { .. } => "Cone",
            Primitive::Torus { .. } => "Torus",
        }
    }

    /// Whether this shape looks best smooth-shaded by default (curved surfaces).
    pub fn default_smooth(&self) -> bool {
        matches!(
            self,
            Primitive::UvSphere { .. } | Primitive::IcoSphere { .. } | Primitive::Torus { .. }
        )
    }

    /// Clamps counts and sizes so any UI input produces a valid, bounded mesh. Ico sphere
    /// subdivisions are capped at 6 (≈82k triangles): a slider can't hang the editor.
    pub fn sanitized(self) -> Self {
        let len = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
        let pos = |v: f32| if v.is_finite() { v.max(1e-4) } else { 1.0 };
        match self {
            Primitive::Plane { size } => Primitive::Plane { size: pos(size) },
            Primitive::Cube { size } => Primitive::Cube { size: pos(size) },
            Primitive::Circle { vertices, radius } => Primitive::Circle {
                vertices: vertices.clamp(3, 512),
                radius: pos(radius),
            },
            Primitive::UvSphere {
                segments,
                rings,
                radius,
            } => Primitive::UvSphere {
                segments: segments.clamp(3, 512),
                rings: rings.clamp(3, 256),
                radius: pos(radius),
            },
            Primitive::IcoSphere {
                subdivisions,
                radius,
            } => Primitive::IcoSphere {
                subdivisions: subdivisions.clamp(1, 6),
                radius: pos(radius),
            },
            Primitive::Cylinder {
                vertices,
                radius,
                depth,
            } => Primitive::Cylinder {
                vertices: vertices.clamp(3, 512),
                radius: pos(radius),
                depth: pos(depth),
            },
            Primitive::Cone {
                vertices,
                radius1,
                radius2,
                depth,
            } => Primitive::Cone {
                vertices: vertices.clamp(3, 512),
                radius1: len(radius1),
                radius2: len(radius2),
                depth: pos(depth),
            },
            Primitive::Torus {
                major_segments,
                minor_segments,
                major_radius,
                minor_radius,
            } => Primitive::Torus {
                major_segments: major_segments.clamp(3, 512),
                minor_segments: minor_segments.clamp(3, 256),
                major_radius: pos(major_radius),
                minor_radius: pos(minor_radius),
            },
        }
    }

    /// Half-extents of the shape's local bounding box (centred on the origin), for picking
    /// and selection outlines.
    pub fn half_extents(&self) -> Vec3 {
        match self.sanitized() {
            Primitive::Plane { size } => Vec3::new(size * 0.5, 0.0, size * 0.5),
            Primitive::Cube { size } => Vec3::splat(size * 0.5),
            Primitive::Circle { radius, .. } => Vec3::new(radius, 0.0, radius),
            Primitive::UvSphere { radius, .. } | Primitive::IcoSphere { radius, .. } => {
                Vec3::splat(radius)
            }
            Primitive::Cylinder { radius, depth, .. } => Vec3::new(radius, depth * 0.5, radius),
            Primitive::Cone {
                radius1,
                radius2,
                depth,
                ..
            } => {
                let r = radius1.max(radius2);
                Vec3::new(r, depth * 0.5, r)
            }
            Primitive::Torus {
                major_radius,
                minor_radius,
                ..
            } => Vec3::new(
                major_radius + minor_radius,
                minor_radius,
                major_radius + minor_radius,
            ),
        }
    }

    /// Builds the mesh. Y is up (Blender's Z): planes and circles face +Y, cylinders, cones
    /// and tori stand on the Y axis.
    pub fn build(&self, smooth: bool) -> MeshData {
        let mut mesh = match self.sanitized() {
            Primitive::Plane { size } => MeshData::plane(size * 0.5, WHITE),
            Primitive::Cube { size } => MeshData::cube(size * 0.5, WHITE),
            Primitive::Circle { vertices, radius } => disc(vertices, radius, 0.0, Vec3::Y),
            Primitive::UvSphere {
                segments,
                rings,
                radius,
            } => uv_sphere(segments, rings, radius),
            Primitive::IcoSphere {
                subdivisions,
                radius,
            } => ico_sphere(subdivisions, radius),
            Primitive::Cylinder {
                vertices,
                radius,
                depth,
            } => frustum(vertices, radius, radius, depth),
            Primitive::Cone {
                vertices,
                radius1,
                radius2,
                depth,
            } => frustum(vertices, radius1, radius2, depth),
            Primitive::Torus {
                major_segments,
                minor_segments,
                major_radius,
                minor_radius,
            } => torus(major_segments, minor_segments, major_radius, minor_radius),
        };
        if !smooth {
            mesh = unweld_flat(&mesh);
        }
        mesh.generate_tangents();
        mesh
    }
}

/// Bit-exact key so the renderer can cache generated meshes by value.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PrimitiveMesh {
    pub shape: Primitive,
    pub smooth: bool,
}

impl PrimitiveMesh {
    pub fn new(shape: Primitive) -> Self {
        Self {
            shape,
            smooth: shape.default_smooth(),
        }
    }

    fn key(&self) -> (u8, [u32; 4], bool) {
        let b = f32::to_bits;
        let (tag, k) = match self.shape {
            Primitive::Plane { size } => (0, [b(size), 0, 0, 0]),
            Primitive::Cube { size } => (1, [b(size), 0, 0, 0]),
            Primitive::Circle { vertices, radius } => (2, [vertices, b(radius), 0, 0]),
            Primitive::UvSphere {
                segments,
                rings,
                radius,
            } => (3, [segments, rings, b(radius), 0]),
            Primitive::IcoSphere {
                subdivisions,
                radius,
            } => (4, [subdivisions, b(radius), 0, 0]),
            Primitive::Cylinder {
                vertices,
                radius,
                depth,
            } => (5, [vertices, b(radius), b(depth), 0]),
            Primitive::Cone {
                vertices,
                radius1,
                radius2,
                depth,
            } => (6, [vertices, b(radius1), b(radius2), b(depth)]),
            Primitive::Torus {
                major_segments,
                minor_segments,
                major_radius,
                minor_radius,
            } => (
                7,
                [
                    major_segments,
                    minor_segments,
                    b(major_radius),
                    b(minor_radius),
                ],
            ),
        };
        (tag, k, self.smooth)
    }
}

impl Eq for PrimitiveMesh {}

impl Hash for PrimitiveMesh {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

const WHITE: [f32; 3] = [1.0; 3];

fn vertex(p: Vec3, n: Vec3, uv: [f32; 2]) -> Vertex {
    Vertex {
        position: p.to_array(),
        normal: n.normalize_or_zero().to_array(),
        color: WHITE,
        uv,
        tangent: [1.0, 0.0, 0.0, 1.0],
    }
}

/// Filled disc at height `y` facing `normal` (±Y), as a triangle fan around a centre vertex.
fn disc(segments: u32, radius: f32, y: f32, normal: Vec3) -> MeshData {
    let mut mesh = MeshData::default();
    append_disc(&mut mesh, segments, radius, y, normal);
    mesh
}

fn append_disc(mesh: &mut MeshData, segments: u32, radius: f32, y: f32, normal: Vec3) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices
        .push(vertex(Vec3::new(0.0, y, 0.0), normal, [0.5, 0.5]));
    for i in 0..segments {
        let a = TAU * i as f32 / segments as f32;
        let p = Vec3::new(a.cos() * radius, y, -a.sin() * radius);
        // Planar mapping seen from above.
        let uv = [0.5 + a.cos() * 0.5, 0.5 + a.sin() * 0.5];
        mesh.vertices.push(vertex(p, normal, uv));
    }
    for i in 0..segments {
        let a = base + 1 + i;
        let b = base + 1 + (i + 1) % segments;
        // Angles grow counter-clockwise seen from +Y (x = cos, z = -sin).
        if normal.y > 0.0 {
            mesh.indices.extend([base, a, b]);
        } else {
            mesh.indices.extend([base, b, a]);
        }
    }
}

fn uv_sphere(segments: u32, rings: u32, radius: f32) -> MeshData {
    let mut mesh = MeshData::default();
    // (rings + 1) latitude rows × (segments + 1) columns; the seam column is duplicated so a
    // future UV set stays continuous.
    for r in 0..=rings {
        let theta = PI * r as f32 / rings as f32; // 0 at the north pole
        for s in 0..=segments {
            let phi = TAU * s as f32 / segments as f32;
            let n = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                -theta.sin() * phi.sin(),
            );
            let uv = [s as f32 / segments as f32, r as f32 / rings as f32];
            mesh.vertices.push(vertex(n * radius, n, uv));
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let b = a + row;
            // Skip the degenerate triangles at the poles.
            if r != 0 {
                mesh.indices.extend([a, b, a + 1]);
            }
            if r != rings - 1 {
                mesh.indices.extend([a + 1, b, b + 1]);
            }
        }
    }
    mesh
}

fn ico_sphere(subdivisions: u32, radius: f32) -> MeshData {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut points: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    // Blender's subdivision 1 is the bare icosahedron.
    for _ in 1..subdivisions {
        let mut midpoints = std::collections::HashMap::new();
        let mut mid = |a: u32, b: u32, points: &mut Vec<Vec3>| -> u32 {
            *midpoints.entry((a.min(b), a.max(b))).or_insert_with(|| {
                let p = (points[a as usize] + points[b as usize]).normalize();
                points.push(p);
                points.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = mid(a, b, &mut points);
            let bc = mid(b, c, &mut points);
            let ca = mid(c, a, &mut points);
            next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    MeshData {
        vertices: points
            .iter()
            .map(|&n| {
                // Spherical mapping (one seam; ico spheres have no natural UV layout).
                let uv = [
                    0.5 + n.z.atan2(n.x) / TAU,
                    0.5 - n.y.clamp(-1.0, 1.0).asin() / PI,
                ];
                vertex(n * radius, n, uv)
            })
            .collect(),
        indices: faces.into_iter().flatten().collect(),
    }
}

/// Cylinder (r1 == r2) or cone/frustum standing on the Y axis, centred at the origin, with
/// caps on any end whose radius is non-zero. Side normals account for the slope.
fn frustum(segments: u32, r1: f32, r2: f32, depth: f32) -> MeshData {
    let mut mesh = MeshData::default();
    let (y0, y1) = (-depth * 0.5, depth * 0.5);
    let slope = (r1 - r2) / depth; // outward tilt of the side normal
    let base = mesh.vertices.len() as u32;
    for i in 0..=segments {
        let a = TAU * i as f32 / segments as f32;
        let dir = Vec3::new(a.cos(), 0.0, -a.sin());
        let n = dir + Vec3::Y * slope;
        let u = i as f32 / segments as f32;
        mesh.vertices
            .push(vertex(dir * r1 + Vec3::Y * y0, n, [u, 1.0]));
        mesh.vertices
            .push(vertex(dir * r2 + Vec3::Y * y1, n, [u, 0.0]));
    }
    for i in 0..segments {
        let b0 = base + i * 2; // bottom, this column
        let t0 = b0 + 1;
        let b1 = b0 + 2;
        let t1 = b0 + 3;
        if r1 > 0.0 {
            mesh.indices.extend([b0, b1, t0]);
        }
        if r2 > 0.0 {
            mesh.indices.extend([t0, b1, t1]);
        }
    }
    if r1 > 0.0 {
        append_disc(&mut mesh, segments, r1, y0, Vec3::NEG_Y);
    }
    if r2 > 0.0 {
        append_disc(&mut mesh, segments, r2, y1, Vec3::Y);
    }
    mesh
}

fn torus(major: u32, minor: u32, major_radius: f32, minor_radius: f32) -> MeshData {
    let mut mesh = MeshData::default();
    for i in 0..=major {
        let u = TAU * i as f32 / major as f32;
        let ring = Vec3::new(u.cos(), 0.0, -u.sin());
        for j in 0..=minor {
            let v = TAU * j as f32 / minor as f32;
            let n = ring * v.cos() + Vec3::Y * v.sin();
            mesh.vertices.push(vertex(
                ring * major_radius + n * minor_radius,
                n,
                [i as f32 / major as f32, j as f32 / minor as f32],
            ));
        }
    }
    let row = minor + 1;
    for i in 0..major {
        for j in 0..minor {
            let a = i * row + j;
            let b = a + row;
            mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

/// Flat shading: every triangle gets its own three vertices carrying the face normal.
fn unweld_flat(mesh: &MeshData) -> MeshData {
    let mut out = MeshData {
        vertices: Vec::with_capacity(mesh.indices.len()),
        indices: (0..mesh.indices.len() as u32).collect(),
    };
    for tri in mesh.indices.chunks_exact(3) {
        let v = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
        let [a, b, c] = v.map(|v| Vec3::from(v.position));
        let n = (b - a).cross(c - a).normalize_or_zero();
        for mut vert in v {
            vert.normal = n.to_array();
            out.vertices.push(vert);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every triangle's geometric normal agrees with its vertex normals (outward, CCW), no
    /// triangle is degenerate, and normals are unit length.
    fn assert_well_formed(name: &str, mesh: &MeshData) {
        assert!(mesh.is_valid(), "{name}: bad indices");
        assert!(!mesh.indices.is_empty(), "{name}: empty");
        for v in &mesh.vertices {
            let n = Vec3::from(v.normal);
            assert!((n.length() - 1.0).abs() < 1e-4, "{name}: normal {n:?}");
        }
        for tri in mesh.indices.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|k| Vec3::from(mesh.vertices[tri[k] as usize].position));
            let face = (b - a).cross(c - a);
            assert!(face.length() > 1e-9, "{name}: degenerate triangle {tri:?}");
            let avg: Vec3 = tri
                .iter()
                .map(|&i| Vec3::from(mesh.vertices[i as usize].normal))
                .sum();
            assert!(face.dot(avg) > 0.0, "{name}: triangle {tri:?} winds inward");
        }
    }

    #[test]
    fn every_primitive_is_well_formed_smooth_and_flat() {
        for p in Primitive::ALL_DEFAULTS {
            for smooth in [true, false] {
                let mesh = p.build(smooth);
                assert_well_formed(&format!("{} smooth={smooth}", p.label()), &mesh);
            }
        }
    }

    #[test]
    fn vertices_lie_inside_the_bounds() {
        for p in Primitive::ALL_DEFAULTS {
            let h = p.half_extents() + Vec3::splat(1e-4);
            for v in p.build(true).vertices {
                let q = Vec3::from(v.position).abs();
                assert!(q.cmple(h).all(), "{}: {q:?} outside {h:?}", p.label());
            }
        }
    }

    #[test]
    fn expected_triangle_counts() {
        let tris = |p: Primitive| p.build(true).indices.len() / 3;
        // UV sphere: two pole fans plus quads between: 2·s + 2·s·(r − 2).
        assert_eq!(tris(Primitive::ALL_DEFAULTS[3]), 2 * 32 + 2 * 32 * 14);
        // Ico sphere subdivision 2 = 80 faces (Blender's default).
        assert_eq!(tris(Primitive::ALL_DEFAULTS[4]), 80);
        // Cylinder: 2 per side quad + 2 caps of 32.
        assert_eq!(tris(Primitive::ALL_DEFAULTS[5]), 64 + 64);
        // Cone with a point: 1 per side + base cap.
        assert_eq!(tris(Primitive::ALL_DEFAULTS[6]), 32 + 32);
        // Torus: 2 per quad.
        assert_eq!(tris(Primitive::ALL_DEFAULTS[7]), 48 * 12 * 2);
    }

    #[test]
    fn sanitize_bounds_runaway_inputs() {
        let p = Primitive::IcoSphere {
            subdivisions: 99,
            radius: f32::NAN,
        }
        .sanitized();
        assert_eq!(
            p,
            Primitive::IcoSphere {
                subdivisions: 6,
                radius: 1.0
            }
        );
        let c = Primitive::Circle {
            vertices: 0,
            radius: -1.0,
        }
        .sanitized();
        assert!(matches!(c, Primitive::Circle { vertices: 3, .. }));
    }

    #[test]
    fn cache_key_distinguishes_settings_and_shading() {
        use std::collections::HashSet;
        let a = PrimitiveMesh::new(Primitive::ALL_DEFAULTS[3]);
        let mut b = a;
        b.smooth = !b.smooth;
        let mut c = a;
        c.shape = Primitive::UvSphere {
            segments: 8,
            rings: 16,
            radius: 1.0,
        };
        let set: HashSet<_> = [a, b, c, a].into_iter().collect();
        assert_eq!(set.len(), 3);
    }
}
