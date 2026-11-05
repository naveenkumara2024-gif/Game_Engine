// Mesh shader (plan 0006): depth prepass entry + forward+ shading into the HDR scene color.
// Solid shading = the fixed studio light; Rendered = clustered scene lights, shadows and the
// environment. Color management happens later, in tonemap.wesl.
// Hot-reloadable: edit and save while the game runs.

import package::common::view::View;
import package::common::lighting::{
    Light, Surface, MAX_LIGHTS_PER_CLUSTER, shade_light, shade_flat_ambient, cluster_index,
};
import package::common::shadows::{shadow_visibility};
import package::common::ibl::{shade_environment};
import package::common::object::Object;

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read> cluster_counts: array<u32>;
@group(0) @binding(3) var<storage, read> cluster_lights: array<u32>;

@group(1) @binding(0) var<uniform> object: Object;

// Material textures (white / flat when the object has no material asset).
@group(2) @binding(0) var base_color_texture: texture_2d<f32>;
// glTF packing: G = roughness, B = metallic.
@group(2) @binding(1) var metallic_roughness_texture: texture_2d<f32>;
@group(2) @binding(2) var emissive_texture: texture_2d<f32>;
@group(2) @binding(3) var material_sampler: sampler;
@group(2) @binding(4) var normal_texture: texture_2d<f32>;
@group(2) @binding(5) var occlusion_texture: texture_2d<f32>;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    // xyz = tangent, w = bitangent sign (MikkTSpace).
    @location(4) tangent: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) world_tangent: vec4<f32>,
}

fn transform_vertex(in: VertexIn) -> VertexOut {
    let world = object.world_from_local * vec4(in.position, 1.0);
    var out: VertexOut;
    out.clip_position = view.clip_from_world * world;
    out.world_normal = (object.normal_from_local * vec4(in.normal, 0.0)).xyz;
    out.color = in.color * object.tint.rgb;
    out.world_position = world.xyz;
    out.uv = in.uv;
    out.world_tangent = vec4((object.world_from_local * vec4(in.tangent.xyz, 0.0)).xyz, in.tangent.w);
    return out;
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    return transform_vertex(in);
}

// Depth prepass: the same position math as vs_main, so depths match exactly.
@vertex
fn vs_depth(in: VertexIn) -> VertexOut {
    return transform_vertex(in);
}

// Masked materials: alpha test in the prepass (and shadows).
@fragment
fn fs_alpha_test(in: VertexOut) {
    let alpha = object.tint.a * textureSample(base_color_texture, material_sampler, in.uv).a;
    if alpha < object.material.w {
        discard;
    }
}

// Solid mode: the fixed studio light the engine has used since M1.
const SUN_DIRECTION = vec3(0.35, 1.0, 0.45); // towards the sun
const SUN_COLOR = vec3(1.0, 0.95, 0.85);
const AMBIENT = vec3(0.10, 0.12, 0.16);

fn shade_studio(n: vec3<f32>, albedo: vec3<f32>) -> vec3<f32> {
    let lambert = max(dot(n, normalize(SUN_DIRECTION)), 0.0);
    return albedo * (AMBIENT + SUN_COLOR * lambert);
}

// Vertex color × tint × base color texture (sRGB texture → sampled as linear).
fn albedo(in: VertexOut) -> vec4<f32> {
    let t = textureSample(base_color_texture, material_sampler, in.uv);
    return vec4(in.color * t.rgb, object.tint.a * t.a);
}

// Shading normal: the geometric normal perturbed by the normal map (tangent space).
fn shading_normal(in: VertexOut, n: vec3<f32>) -> vec3<f32> {
    let t_len = length(in.world_tangent.xyz);
    let strength = object.material.z;
    let sample = textureSample(normal_texture, material_sampler, in.uv).xyz * 2.0 - 1.0;
    if t_len < 1e-5 || strength <= 0.0 {
        return n;
    }
    let t = normalize(in.world_tangent.xyz - n * dot(n, in.world_tangent.xyz / t_len) * t_len);
    let b = cross(n, t) * in.world_tangent.w;
    let tn = vec3(sample.xy * strength, sample.z);
    return normalize(t * tn.x + b * tn.y + n * tn.z);
}

fn shade_scene(in: VertexOut, n: vec3<f32>, frag_xy: vec2<f32>, base: vec3<f32>) -> vec3<f32> {
    var s: Surface;
    s.position = in.world_position;
    s.normal = n;
    s.view_dir = normalize(view.position_time.xyz - in.world_position);
    s.albedo = base;
    let mr = textureSample(metallic_roughness_texture, material_sampler, in.uv);
    s.metallic = clamp(object.material.x * mr.b, 0.0, 1.0);
    s.roughness = clamp(object.material.y * mr.g, 0.0, 1.0);
    s.f0_dielectric = object.emission.w;
    s.occlusion = textureSample(occlusion_texture, material_sampler, in.uv).r;
    let emissive = textureSample(emissive_texture, material_sampler, in.uv).rgb;

    // Shadows look up with the geometric normal (normal offset), shading uses the mapped one.
    let geo_n = normalize(in.world_normal);
    let view_z = -(view.view_from_world * vec4(in.world_position, 1.0)).z;
    var direct = vec3(0.0);
    let shadows_on = view.counts.w & 1u;
    let suns = view.cluster_dims.w;
    for (var i = 0u; i < suns; i++) {
        let light = lights[i];
        direct += shade_light(light, s, shadow_visibility(light, s.position, geo_n, view_z, shadows_on));
    }
    if (view.counts.w & 2u) != 0u {
        // r.Clusters.BruteForce: every light for every pixel, the reference the clustered
        // path is tested against.
        for (var i = suns; i < view.counts.x; i++) {
            let light = lights[i];
            direct += shade_light(light, s, shadow_visibility(light, s.position, geo_n, view_z, shadows_on));
        }
    } else {
        let cluster = cluster_index(frag_xy, view_z, view.screen, view.cluster_dims.xyz, view.params.x, view.params.y);
        let count = cluster_counts[cluster];
        for (var k = 0u; k < count; k++) {
            let index = cluster_lights[cluster * MAX_LIGHTS_PER_CLUSTER + k];
            let light = lights[index];
            direct += shade_light(light, s, shadow_visibility(light, s.position, geo_n, view_z, shadows_on));
        }
    }

    var ambient: vec3<f32>;
    if view.counts.z == 1u {
        ambient = shade_environment(s, f32(view.counts.y)) * view.ambient.w;
    } else {
        ambient = shade_flat_ambient(view.ambient.rgb, s);
    }
    return direct * view.white_balance.w + ambient + object.emission.rgb * emissive;
}

@if(debug_normals)
fn shade(in: VertexOut, n: vec3<f32>, frag_xy: vec2<f32>, base: vec3<f32>) -> vec3<f32> {
    return n * 0.5 + 0.5;
}

@if(!debug_normals)
fn shade(in: VertexOut, n: vec3<f32>, frag_xy: vec2<f32>, base: vec3<f32>) -> vec3<f32> {
    if view.color_management.w > 0.5 {
        return shade_scene(in, n, frag_xy, base);
    }
    return shade_studio(n, base);
}

@fragment
fn fs_main(in: VertexOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // Double-sided materials light their back faces with the flipped normal.
    var n = normalize(in.world_normal);
    if !front {
        n = -n;
    }
    let base = albedo(in);
    let shading_n = shading_normal(in, n);
    // Linear HDR radiance; alpha is used by the blended (transparent) pipeline only.
    let color = shade(in, shading_n, in.clip_position.xy, base.rgb);
    // Masked materials: cut out here too (the prepass left those pixels at far depth).
    if object.material.w > 0.0 && base.a < object.material.w {
        discard;
    }
    return vec4(color, base.a);
}
