// Color management (Render ▸ Color Management), applied at the end of every scene shader until
// the HDR post pass lands (M4). Mirrors `ColorManagement::apply` in
// crates/ge_render/src/lighting.rs — keep them in sync (the clear color uses the Rust one).

import package::common::view::View;

fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0031308 {
        return x * 12.92;
    }
    return 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.04045 {
        return x / 12.92;
    }
    return pow((x + 0.055) / 1.055, 2.4);
}

fn agx_encode(v: f32) -> f32 {
    let min_ev = -12.47;
    let max_ev = 4.03;
    return clamp((log2(max(v, 1e-10)) - min_ev) / (max_ev - min_ev), 0.0, 1.0);
}

fn agx_sigmoid(t: f32) -> f32 {
    return 1.0 / (1.0 + exp(-3.2 * (t - 0.606) * 4.0));
}

fn agx_curve(x: f32) -> f32 {
    let s0 = agx_sigmoid(0.0);
    let s1 = agx_sigmoid(1.0);
    return (agx_sigmoid(x) - s0) / (s1 - s0);
}

// Our AgX-style curve (see `agx_style` in lighting.rs): log encode, S-curve, slight inset.
fn agx_style(c: vec3<f32>) -> vec3<f32> {
    let luma = dot(c, vec3(0.2126, 0.7152, 0.0722));
    let inset = max(mix(c, vec3(luma), 0.12), vec3(1e-10));
    let d = vec3(
        agx_curve(agx_encode(inset.x)),
        agx_curve(agx_encode(inset.y)),
        agx_curve(agx_encode(inset.z)),
    );
    return vec3(srgb_to_linear(d.x), srgb_to_linear(d.y), srgb_to_linear(d.z));
}

// Khronos PBR Neutral, from the published specification (Apache-2.0).
fn pbr_neutral(color: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.8 - 0.04;
    let desaturation = 0.15;
    let x = min(color.r, min(color.g, color.b));
    var offset = 0.04;
    if x < 0.08 {
        offset = x - 6.25 * x * x;
    }
    var c = color - offset;
    let peak = max(c.r, max(c.g, c.b));
    if peak < start_compression {
        return max(c, vec3(0.0));
    }
    let d = 1.0 - start_compression;
    let new_peak = 1.0 - d * d / (peak + d - start_compression);
    c = c * (new_peak / peak);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return max(mix(c, vec3(new_peak), g), vec3(0.0));
}

// Scene-linear → display-linear for the sRGB target.
fn color_manage(view: View, scene: vec3<f32>) -> vec3<f32> {
    var c = scene * view.white_balance.rgb * view.color_management.x;
    let transform = u32(view.color_management.z + 0.5);
    if transform == 1u {
        c = agx_style(c);
    } else if transform == 2u {
        c = pbr_neutral(c);
    } else {
        c = clamp(c, vec3(0.0), vec3(1.0));
    }
    // Look: an S-curve on display-encoded values (our own; Blender's looks are OCIO LUTs).
    let look = view.params.z;
    if look > 0.5 {
        let k = 1.0 + 0.22 * look;
        var e = vec3(linear_to_srgb(c.x), linear_to_srgb(c.y), linear_to_srgb(c.z));
        e = clamp(e, vec3(0.0), vec3(1.0));
        let up = pow(e, vec3(k));
        e = up / (up + pow(vec3(1.0) - e, vec3(k)));
        c = vec3(srgb_to_linear(e.x), srgb_to_linear(e.y), srgb_to_linear(e.z));
    }
    let gamma = view.color_management.y;
    if gamma != 1.0 {
        let g = 1.0 / gamma;
        c = vec3(
            srgb_to_linear(pow(linear_to_srgb(c.x), g)),
            srgb_to_linear(pow(linear_to_srgb(c.y), g)),
            srgb_to_linear(pow(linear_to_srgb(c.z), g)),
        );
    }
    return c;
}
