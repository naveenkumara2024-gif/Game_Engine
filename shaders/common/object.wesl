// Per-object constants (group 1, dynamic offset). Mirrors `ObjectUniform` in
// crates/ge_render/src/renderer.rs.

struct Object {
    world_from_local: mat4x4<f32>,
    // Inverse-transpose of world_from_local (upper 3x3 used).
    normal_from_local: mat4x4<f32>,
    // rgb = base color (tint × material), a = alpha.
    tint: vec4<f32>,
    // x = metallic, y = roughness, z = normal map strength, w = alpha cutoff (masked).
    material: vec4<f32>,
    // rgb = emission color × strength, w = dielectric F0.
    emission: vec4<f32>,
}
