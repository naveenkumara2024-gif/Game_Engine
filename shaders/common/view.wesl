// Per-view constants, bound at @group(0) @binding(0) by every scene pass.
// Mirrors `ViewUniform` in crates/ge_render/src/renderer.rs; keep the two in sync.

struct View {
    // Reversed-Z, infinite far plane: depth 1 at the near plane, 0 at infinity.
    clip_from_world: mat4x4<f32>,
    world_from_clip: mat4x4<f32>,
    // xyz = camera position in world space, w = time in seconds.
    position_time: vec4<f32>,
    // x = exposure multiplier (2^EV), y = gamma, z = view transform (0 Standard, 1 AgX-style,
    // 2 Khronos PBR Neutral), w = shading (0 Solid studio light, 1 Rendered scene lights).
    color_management: vec4<f32>,
    // rgb = white-balance multiplier (1 = off), w = direct light intensity multiplier.
    white_balance: vec4<f32>,
    // rgb = world ambient radiance (color × strength), w = number of active lights.
    ambient: vec4<f32>,
}
