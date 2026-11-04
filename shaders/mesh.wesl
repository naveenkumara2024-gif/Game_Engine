// Mesh shader: Solid (studio light) or Rendered (scene lights + world), then color management.
// Hot-reloadable: edit and save while the game runs.

import package::common::view::View;
import package::common::color::color_manage;
import package::common::lighting::{Lights, Surface, shade_light, shade_ambient};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<uniform> lights: Lights;

// Mirrors `ObjectUniform` in crates/ge_render/src/renderer.rs.
struct Object {
    world_from_local: mat4x4<f32>,
    // Inverse-transpose of world_from_local (upper 3x3 used).
    normal_from_local: mat4x4<f32>,
    // rgb = base color (tint × material), a = unused.
    tint: vec4<f32>,
    // x = metallic, y = roughness.
    material: vec4<f32>,
    // rgb = emission color × strength.
    emission: vec4<f32>,
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
    @location(2) world_position: vec3<f32>,
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let world = object.world_from_local * vec4(in.position, 1.0);
    var out: VertexOut;
    out.clip_position = view.clip_from_world * world;
    out.world_normal = (object.normal_from_local * vec4(in.normal, 0.0)).xyz;
    out.color = in.color * object.tint.rgb;
    out.world_position = world.xyz;
    return out;
}

// Solid mode: the fixed studio light the engine has used since M1.
const SUN_DIRECTION = vec3(0.35, 1.0, 0.45); // towards the sun
const SUN_COLOR = vec3(1.0, 0.95, 0.85);
const AMBIENT = vec3(0.10, 0.12, 0.16);

fn shade_studio(n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let lambert = max(dot(n, normalize(SUN_DIRECTION)), 0.0);
    return albedo * (AMBIENT + SUN_COLOR * lambert);
}

fn shade_scene(in: VertexOut, n: vec3<f32>) -> vec3<f32> {
    var s: Surface;
    s.position = in.world_position;
    s.normal = n;
    s.view_dir = normalize(view.position_time.xyz - in.world_position);
    s.albedo = in.color;
    s.metallic = clamp(object.material.x, 0.0, 1.0);
    s.roughness = clamp(object.material.y, 0.0, 1.0);

    var direct = vec3(0.0);
    let count = min(u32(view.ambient.w + 0.5), 16u);
    for (var i = 0u; i < count; i++) {
        direct += shade_light(lights.items[i], s);
    }
    return direct * view.white_balance.w + shade_ambient(view.ambient.rgb, s)
        + object.emission.rgb;
}

@if(debug_normals)
fn shade(in: VertexOut, n: vec3<f32>) -> vec3<f32> {
    return n * 0.5 + 0.5;
}

@if(!debug_normals)
fn shade(in: VertexOut, n: vec3<f32>) -> vec3<f32> {
    if view.color_management.w > 0.5 {
        return color_manage(view, shade_scene(in, n));
    }
    return color_manage(view, shade_studio(n, in.color));
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Double-sided materials light their back faces with the flipped normal.
    var n = normalize(in.world_normal);
    if !front {
        n = -n;
    }
    // Linear output; the sRGB render target encodes on store.
    return vec4(shade(in, n), 1.0);
}
