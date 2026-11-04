// UI pass: tessellated 2D triangles in physical pixels, premultiplied sRGB colors (egui's
// convention). Mirrors `ScreenUniform` / `UiVertex` in crates/ge_render/src/ui.rs.

struct Screen {
    size: vec2<f32>,
    // 1 when the target is sRGB (the hardware encodes, so we output linear).
    srgb_target: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> screen: Screen;
@group(1) @binding(0) var ui_texture: texture_2d<f32>;
@group(1) @binding(1) var ui_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let cutoff = c < vec3(0.04045);
    let lower = c / 12.92;
    let higher = pow((c + 0.055) / 1.055, vec3(2.4));
    return select(higher, lower, cutoff);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let cutoff = c < vec3(0.0031308);
    let lower = c * 12.92;
    let higher = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return select(higher, lower, cutoff);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.clip_position = vec4(
        in.pos.x / screen.size.x * 2.0 - 1.0,
        1.0 - in.pos.y / screen.size.y * 2.0,
        0.0,
        1.0,
    );
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // The texture is Rgba8UnormSrgb, so sampling already decodes to linear; decode the vertex
    // color the same way and multiply (as egui's own wgpu backend does for sRGB targets).
    let tex = textureSample(ui_texture, ui_sampler, in.uv);
    let color = vec4(srgb_to_linear(in.color.rgb), in.color.a);
    let linear = color * tex;
    if screen.srgb_target > 0.5 {
        return linear;
    }
    return vec4(linear_to_srgb(linear.rgb), linear.a);
}
