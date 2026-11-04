// Viewport overlay: an infinite ground grid on y = 0 with colored X / Z axis lines (Blender's
// floor grid; Blender is Z-up so its "Y axis" is our Z). Drawn in the opaque pass after meshes,
// depth-tested against them, blended, not depth-writing.

import package::common::view::View;

@group(0) @binding(0) var<uniform> view: View;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

// Full-screen triangle.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    let uv = vec2(f32((i << 1u) & 2u), f32(i & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VertexOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

fn unproject(ndc: vec2<f32>, depth: f32) -> vec3<f32> {
    let p = view.world_from_clip * vec4(ndc, depth, 1.0);
    return p.xyz / p.w;
}

// Anti-aliased line coverage for a grid of the given cell size.
fn grid_lines(p: vec2<f32>, cell: f32, width: f32) -> f32 {
    let coord = p / cell;
    let deriv = fwidth(coord);
    let g = abs(fract(coord - 0.5) - 0.5) / deriv;
    let line = min(g.x, g.y);
    return 1.0 - min(line / width, 1.0);
}

struct FragOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs_main(in: VertexOut) -> FragOut {
    // Reversed-Z: depth 1 is the near plane; 0.5 is some finite point further along the ray.
    let near = unproject(in.ndc, 1.0);
    let far = unproject(in.ndc, 0.5);
    let dir = far - near;
    var out: FragOut;
    if abs(dir.y) < 1e-6 {
        discard;
    }
    let t = -near.y / dir.y;
    if t <= 0.0 {
        discard;
    }
    let p = near + dir * t;
    let clip = view.clip_from_world * vec4(p, 1.0);
    out.depth = clip.z / clip.w;

    let small = grid_lines(p.xz, 1.0, 1.0);
    let large = grid_lines(p.xz, 10.0, 1.5);
    var color = vec3(0.32);
    var alpha = max(small * 0.35, large * 0.6);

    // Axis lines: X in red (z = 0), Z in blue (x = 0).
    let axis_w = fwidth(p.xz) * 1.5;
    if abs(p.z) < axis_w.y {
        color = vec3(0.86, 0.22, 0.27);
        alpha = 0.9;
    } else if abs(p.x) < axis_w.x {
        color = vec3(0.19, 0.45, 0.86);
        alpha = 0.9;
    }

    // Fade with distance from the camera so the horizon doesn't moiré.
    let dist = length(p - view.position_time.xyz);
    alpha *= clamp(1.0 - dist / 150.0, 0.0, 1.0);
    if alpha <= 0.001 {
        discard;
    }
    out.color = vec4(color, alpha);
    return out;
}
