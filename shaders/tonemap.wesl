// Tonemap (plan 0006, decision 1): HDR scene color + bloom → exposure, white balance, view
// transform, look, gamma → the display target. The only place color management happens.

import package::common::view::View;
import package::common::color::color_manage;

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var scene_color: texture_2d<f32>;
@group(0) @binding(2) var bloom_texture: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VertexOut;
    out.clip_position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var c = textureLoad(scene_color, vec2<i32>(in.clip_position.xy), 0).rgb;
    let bloom_intensity = view.params.w;
    if bloom_intensity > 0.0 {
        c += textureSampleLevel(bloom_texture, linear_sampler, in.uv, 0.0).rgb * bloom_intensity;
    }
    return vec4(color_manage(view, c), 1.0);
}
