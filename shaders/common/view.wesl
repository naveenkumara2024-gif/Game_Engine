// Per-view constants, bound at @group(0) @binding(0) by every scene pass and the light-culling
// compute pass. Mirrors `ViewUniform` in crates/ge_render/src/frame.rs; keep the two in sync.

struct View {
    // Reversed-Z, infinite far plane: depth 1 at the near plane, 0 at infinity.
    clip_from_world: mat4x4<f32>,
    world_from_clip: mat4x4<f32>,
    view_from_world: mat4x4<f32>,
    view_from_clip: mat4x4<f32>,
    // xyz = camera position in world space, w = time in seconds.
    position_time: vec4<f32>,
    // x = exposure multiplier (2^EV), y = gamma, z = view transform (0 Standard, 1 AgX-style,
    // 2 Khronos PBR Neutral), w = shading (0 Solid studio light, 1 Rendered scene lights).
    color_management: vec4<f32>,
    // rgb = white-balance multiplier (1 = off), w = direct light intensity multiplier.
    white_balance: vec4<f32>,
    // rgb = flat world ambient (used when there is no environment map), w = environment strength.
    ambient: vec4<f32>,
    // x, y = target size in pixels, z = near plane, w = far end of the cluster grid.
    screen: vec4<f32>,
    // xyz = cluster grid dimensions, w = number of directional lights (first in the light list).
    cluster_dims: vec4<u32>,
    // x = light count, y = environment mip count, z = 1 when IBL is on, w = flags (bit 0: shadows on, bit 1:
    // r.Clusters.BruteForce).
    counts: vec4<u32>,
    // x = cluster slice scale, y = slice bias, z = look (0 none, 1-4 contrast), w = bloom intensity.
    params: vec4<f32>,
    // rgb = background (sky/world color) radiance when there is no environment map, w = unused.
    background: vec4<f32>,
}
