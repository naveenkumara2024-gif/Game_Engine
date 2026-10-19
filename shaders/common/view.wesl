// Per-view constants, bound at @group(0) @binding(0) by every scene pass.
// Mirrors `ViewUniform` in crates/ge_render/src/renderer.rs; keep the two in sync.

struct View {
    // Reversed-Z, infinite far plane: depth 1 at the near plane, 0 at infinity.
    clip_from_world: mat4x4<f32>,
    // xyz = camera position in world space, w = time in seconds.
    position_time: vec4<f32>,
}
