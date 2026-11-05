// Background (plan 0006): the world seen behind everything. Drawn as a full-screen triangle at
// depth 0 (infinitely far, reversed-Z) with a GreaterEqual test, so it only fills pixels no
// geometry covered. With an environment map it shows the environment, else the flat color.

import package::common::view::View;
import package::common::ibl::sample_sky;

@group(0) @binding(0) var<uniform> view: View;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VertexOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if view.counts.z == 1u && view.color_management.w > 0.5 {
        let near = view.world_from_clip * vec4(in.ndc, 1.0, 1.0);
        let far = view.world_from_clip * vec4(in.ndc, 0.5, 1.0);
        let dir = normalize(far.xyz / far.w - near.xyz / near.w);
        return vec4(sample_sky(dir) * view.ambient.w, 1.0);
    }
    return vec4(view.background.rgb, 1.0);
}
