// Shadow map depth pass (plan 0006, decision 3): position only, through one shadow view's
// matrix (dynamic offset). Masked materials alpha-test so leaves and fences cast holes.

import package::common::object::Object;

struct ShadowView {
    clip_from_world: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> shadow_view: ShadowView;
@group(1) @binding(0) var<uniform> object: Object;
@group(2) @binding(0) var base_color_texture: texture_2d<f32>;
@group(2) @binding(3) var material_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(3) uv: vec2<f32>,
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_shadow(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = shadow_view.clip_from_world * object.world_from_local * vec4(in.position, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_shadow_masked(in: VertexOut) {
    let alpha = object.tint.a * textureSample(base_color_texture, material_sampler, in.uv).a;
    if alpha < object.material.w {
        discard;
    }
}
