// M1 mesh shader: per-vertex color, Lambert sun + ambient.
// Hot-reloadable: edit and save while the game runs.

import package::common::view::View;

@group(0) @binding(0) var<uniform> view: View;

// Mirrors `ObjectUniform` in crates/ge_render/src/renderer.rs.
struct Object {
    world_from_local: mat4x4<f32>,
    // Inverse-transpose of world_from_local (upper 3x3 used).
    normal_from_local: mat4x4<f32>,
    tint: vec4<f32>,
}

@group(1) @binding(0) var<uniform> object: Object;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) color: vec3<f32>,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let world = object.world_from_local * vec4(in.position, 1.0);
    var out: VertexOut;
    out.clip_position = view.clip_from_world * world;
    out.world_normal = (object.normal_from_local * vec4(in.normal, 0.0)).xyz;
    out.color = in.color * object.tint.rgb;
    return out;
}

const SUN_DIRECTION = vec3(0.35, 1.0, 0.45); // towards the sun
const SUN_COLOR = vec3(1.0, 0.95, 0.85);
const AMBIENT = vec3(0.10, 0.12, 0.16);

@if(debug_normals)
fn shade(n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    return n * 0.5 + 0.5;
}

@if(!debug_normals)
fn shade(n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let lambert = max(dot(n, normalize(SUN_DIRECTION)), 0.0);
    return albedo * (AMBIENT + SUN_COLOR * lambert);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Linear output; the sRGB render target encodes on store.
    return vec4(shade(normalize(in.world_normal), in.color), 1.0);
}
