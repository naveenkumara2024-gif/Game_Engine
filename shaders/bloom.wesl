// Bloom (plan 0006, decision 5): the dual-filter chain popularized by Jimenez (SIGGRAPH 2014).
// Downsample with a 13-tap filter (the first pass applies a soft threshold and a Karis average
// against fireflies), then upsample with a 3×3 tent, adding each level into the one above.

struct Params {
    // Texel size of the *source* texture.
    src_texel: vec2<f32>,
    threshold: f32,
    knee: f32,
    // Upsample tent radius in source texels.
    radius: f32,
    // 1 for the first downsample (threshold + Karis average).
    first: f32,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

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

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_sampler, uv, 0.0).rgb;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + luma(c));
}

// Soft knee threshold: 0 below threshold - knee, quadratic ramp, linear above.
fn threshold(c: vec3<f32>) -> vec3<f32> {
    let b = max(c.r, max(c.g, c.b));
    let knee = max(params.knee, 1e-4);
    var soft = clamp(b - params.threshold + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee + 1e-5);
    let w = max(soft, b - params.threshold) / max(b, 1e-5);
    return c * max(w, 0.0);
}

@fragment
fn fs_down(in: VertexOut) -> @location(0) vec4<f32> {
    let t = params.src_texel;
    let uv = in.uv;
    let a = tap(uv + t * vec2(-2.0, -2.0));
    let b = tap(uv + t * vec2(0.0, -2.0));
    let c = tap(uv + t * vec2(2.0, -2.0));
    let d = tap(uv + t * vec2(-2.0, 0.0));
    let e = tap(uv);
    let f = tap(uv + t * vec2(2.0, 0.0));
    let g = tap(uv + t * vec2(-2.0, 2.0));
    let h = tap(uv + t * vec2(0.0, 2.0));
    let i = tap(uv + t * vec2(2.0, 2.0));
    let j = tap(uv + t * vec2(-1.0, -1.0));
    let k = tap(uv + t * vec2(1.0, -1.0));
    let l = tap(uv + t * vec2(-1.0, 1.0));
    let m = tap(uv + t * vec2(1.0, 1.0));
    var result: vec3<f32>;
    if params.first > 0.5 {
        // Karis average per 2×2 group stops single bright pixels from flickering.
        let g0 = (a + b + d + e) * 0.25;
        let g1 = (b + c + e + f) * 0.25;
        let g2 = (d + e + g + h) * 0.25;
        let g3 = (e + f + h + i) * 0.25;
        let g4 = (j + k + l + m) * 0.25;
        let w0 = karis(g0) * 0.125;
        let w1 = karis(g1) * 0.125;
        let w2 = karis(g2) * 0.125;
        let w3 = karis(g3) * 0.125;
        let w4 = karis(g4) * 0.5;
        result = (g0 * w0 + g1 * w1 + g2 * w2 + g3 * w3 + g4 * w4) / (w0 + w1 + w2 + w3 + w4);
        result = threshold(result);
    } else {
        result = e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
    }
    return vec4(max(result, vec3(0.0)), 1.0);
}

@fragment
fn fs_up(in: VertexOut) -> @location(0) vec4<f32> {
    let t = params.src_texel * params.radius;
    let uv = in.uv;
    var s = tap(uv) * 4.0;
    s += (tap(uv + vec2(-t.x, 0.0)) + tap(uv + vec2(t.x, 0.0)) + tap(uv + vec2(0.0, -t.y)) + tap(uv + vec2(0.0, t.y))) * 2.0;
    s += tap(uv + vec2(-t.x, -t.y)) + tap(uv + vec2(t.x, -t.y)) + tap(uv + vec2(-t.x, t.y)) + tap(uv + vec2(t.x, t.y));
    return vec4(s / 16.0, 1.0);
}
