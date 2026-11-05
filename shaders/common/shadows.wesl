// Shadows (plan 0006, decision 3): cascaded shadow maps for the sun, a depth atlas for spot,
// point (6 cube faces) and area lights. Rotated-Poisson PCF on hardware comparison samples;
// slope-scaled bias is in the shadow pipeline, normal offset here. Reversed-Z everywhere:
// larger depth = closer to the light, so "lit" means `ref >= stored`.
// Mirrors crates/ge_render/src/shadows.rs.

import package::common::lighting::Light;

struct Cascades {
    // world → light clip space per cascade (orthographic, reversed-Z).
    clip_from_world: array<mat4x4<f32>, 4>,
    // View depth where each cascade ends.
    splits: vec4<f32>,
    // World size of one shadow texel per cascade (normal-offset bias).
    texel_world: vec4<f32>,
    // x = cascade count, y = 1 / resolution, z = blend fraction, w = filter radius (texels).
    params: vec4<f32>,
}

// One shadow view in the atlas (spot/area: one; point: six consecutive, +X -X +Y -Y +Z -Z).
struct LocalShadow {
    clip_from_world: mat4x4<f32>,
    // Atlas tile in UV: xy = origin, zw = size.
    rect: vec4<f32>,
    // x = 2·tan(fov/2) / tile resolution (texel size at distance 1), y = filter radius (texels),
    // z = 1 / atlas resolution, w = unused.
    params: vec4<f32>,
}

struct Shadows {
    cascades: Cascades,
    locals: array<LocalShadow>,
}

@group(0) @binding(4) var cascade_maps: texture_depth_2d_array;
@group(0) @binding(5) var shadow_atlas: texture_depth_2d;
@group(0) @binding(6) var shadow_sampler: sampler_comparison;
@group(0) @binding(7) var<storage, read> shadows: Shadows;

const POISSON = array<vec2<f32>, 12>(
    vec2(-0.326, -0.406), vec2(-0.840, -0.074), vec2(-0.696, 0.457), vec2(-0.203, 0.621),
    vec2(0.962, -0.195), vec2(0.473, -0.480), vec2(0.519, 0.767), vec2(0.185, -0.893),
    vec2(0.507, 0.064), vec2(0.896, 0.412), vec2(-0.322, -0.933), vec2(-0.792, -0.598),
);

// Per-pixel rotation so the 12 taps don't band (interleaved gradient noise, Jimenez 2014).
fn rotation(p: vec3<f32>) -> mat2x2<f32> {
    let a = fract(52.9829189 * fract(dot(p.xy * 0.37 + p.zz * 0.61, vec2(0.06711056, 0.00583715)) * 913.0)) * 6.2831853;
    let c = cos(a);
    let s = sin(a);
    return mat2x2(c, s, -s, c);
}

fn pcf_cascade(uv: vec2<f32>, depth: f32, layer: i32, radius_uv: f32, rot: mat2x2<f32>) -> f32 {
    var lit = 0.0;
    for (var i = 0u; i < 12u; i++) {
        let o = rot * POISSON[i] * radius_uv;
        lit += textureSampleCompareLevel(cascade_maps, shadow_sampler, uv + o, layer, depth);
    }
    return lit / 12.0;
}

fn cascade_lookup(p: vec3<f32>, n: vec3<f32>, i: u32, rot: mat2x2<f32>) -> f32 {
    let biased = p + n * shadows.cascades.texel_world[i] * 1.5;
    let c = shadows.cascades.clip_from_world[i] * vec4(biased, 1.0);
    let uv = vec2(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    if any(uv < vec2(0.0)) || any(uv > vec2(1.0)) || c.z > 1.0 {
        return 1.0;
    }
    let radius = shadows.cascades.params.w * shadows.cascades.params.y;
    return pcf_cascade(uv, c.z, i32(i), radius, rot);
}

fn sun_visibility(p: vec3<f32>, n: vec3<f32>, view_z: f32) -> f32 {
    let count = u32(shadows.cascades.params.x);
    let rot = rotation(p);
    for (var i = 0u; i < count; i++) {
        let split = shadows.cascades.splits[i];
        if view_z < split {
            let v = cascade_lookup(p, n, i, rot);
            // Blend into the next cascade over the last part of this one.
            let start = split * (1.0 - shadows.cascades.params.z);
            if i + 1u < count && view_z > start {
                let t = (view_z - start) / (split - start);
                return mix(v, cascade_lookup(p, n, i + 1u, rot), t);
            }
            // Fade out at the end of the last cascade.
            if i + 1u == count && view_z > start {
                return mix(v, 1.0, (view_z - start) / (split - start));
            }
            return v;
        }
    }
    return 1.0;
}

fn local_lookup(slot: u32, p: vec3<f32>, n: vec3<f32>, dist: f32) -> f32 {
    let view = shadows.locals[slot];
    let offset = view.params.x * dist * 1.5;
    let c = view.clip_from_world * vec4(p + n * offset, 1.0);
    if c.w <= 0.0 {
        return 1.0;
    }
    let ndc = c.xyz / c.w;
    let local_uv = vec2(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if any(local_uv < vec2(0.0)) || any(local_uv > vec2(1.0)) {
        return 1.0;
    }
    let texel = view.params.z;
    let lo = view.rect.xy + vec2(texel);
    let hi = view.rect.xy + view.rect.zw - vec2(texel);
    let uv = view.rect.xy + local_uv * view.rect.zw;
    let rot = rotation(p);
    let radius = view.params.y * texel;
    var lit = 0.0;
    for (var i = 0u; i < 12u; i++) {
        let o = clamp(uv + rot * POISSON[i] * radius, lo, hi);
        lit += textureSampleCompareLevel(shadow_atlas, shadow_sampler, o, ndc.z);
    }
    return lit / 12.0;
}

// 1 = fully lit. `enabled` is bit 0 of View.counts.w.
fn shadow_visibility(light: Light, p: vec3<f32>, n: vec3<f32>, view_z: f32, enabled: u32) -> f32 {
    let slot = i32(light.up_shadow.w);
    if enabled == 0u || slot < 0 {
        return 1.0;
    }
    let kind = u32(light.direction_kind.w + 0.5);
    if kind == 1u {
        return sun_visibility(p, n, view_z);
    }
    let to_p = p - light.position_range.xyz;
    let dist = length(to_p);
    var face = 0u;
    if kind == 0u {
        // Point light: pick the cube face by the dominant axis.
        let a = abs(to_p);
        if a.x >= a.y && a.x >= a.z {
            face = select(1u, 0u, to_p.x > 0.0);
        } else if a.y >= a.z {
            face = select(3u, 2u, to_p.y > 0.0);
        } else {
            face = select(5u, 4u, to_p.z > 0.0);
        }
    }
    return local_lookup(u32(slot) + face, p, n, dist);
}
