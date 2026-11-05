// Clustered light culling (plan 0006, decision 2). One invocation per froxel cluster: build
// its view-space bounding box from the screen tile and an exponential depth slice, then list
// every local light whose range sphere touches it. Directional lights (the first
// `cluster_dims.w` entries) affect every pixel and are not culled.

import package::common::view::View;
import package::common::lighting::{Light, MAX_LIGHTS_PER_CLUSTER};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> lights: array<Light>;
@group(0) @binding(2) var<storage, read_write> cluster_counts: array<u32>;
@group(0) @binding(3) var<storage, read_write> cluster_lights: array<u32>;

// A point on the near plane in view space (reversed-Z: the near plane is NDC z = 1).
fn near_point(ndc: vec2<f32>) -> vec3<f32> {
    let p = view.view_from_clip * vec4(ndc, 1.0, 1.0);
    return p.xyz / p.w;
}

@compute @workgroup_size(64)
fn cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = view.cluster_dims.xyz;
    let total = dims.x * dims.y * dims.z;
    let c = gid.x;
    if c >= total {
        return;
    }
    let x = c % dims.x;
    let y = (c / dims.x) % dims.y;
    let z = c / (dims.x * dims.y);

    // Tile bounds in NDC (tile row 0 is the top of the screen).
    let x0 = f32(x) / f32(dims.x) * 2.0 - 1.0;
    let x1 = f32(x + 1u) / f32(dims.x) * 2.0 - 1.0;
    let y0 = 1.0 - f32(y + 1u) / f32(dims.y) * 2.0;
    let y1 = 1.0 - f32(y) / f32(dims.y) * 2.0;

    let near = view.screen.z;
    let far = view.screen.w;
    let d0 = near * pow(far / near, f32(z) / f32(dims.z));
    var d1 = near * pow(far / near, f32(z + 1u) / f32(dims.z));
    if z + 1u == dims.z {
        d1 = 1e7; // the last slice reaches the horizon
    }

    var lo = vec3(1e30);
    var hi = vec3(-1e30);
    for (var k = 0u; k < 4u; k++) {
        let ndc = vec2(select(x0, x1, (k & 1u) != 0u), select(y0, y1, (k & 2u) != 0u));
        let p = near_point(ndc);
        // Points along the ray at view depth d0 and d1 (view space looks down -Z).
        let a = p * (d0 / near);
        let b = p * (d1 / near);
        lo = min(lo, min(a, b));
        hi = max(hi, max(a, b));
    }

    let base = c * MAX_LIGHTS_PER_CLUSTER;
    var count = 0u;
    let light_count = view.counts.x;
    for (var i = view.cluster_dims.w; i < light_count; i++) {
        let light = lights[i];
        let range = light.position_range.w;
        var hit = range <= 0.0;
        if !hit {
            let center = (view.view_from_world * vec4(light.position_range.xyz, 1.0)).xyz;
            let closest = clamp(center, lo, hi);
            let d = closest - center;
            hit = dot(d, d) <= range * range;
        }
        if hit && count < MAX_LIGHTS_PER_CLUSTER {
            cluster_lights[base + count] = i;
            count++;
        }
    }
    cluster_counts[c] = count;
}
