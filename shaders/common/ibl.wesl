// Image-based lighting (plan 0006, decision 4): diffuse from SH9 irradiance, specular from a
// GGX-prefiltered environment cube with the split-sum BRDF lookup (Karis 2013).
// Mirrors crates/ge_render/src/ibl.rs.

import package::common::lighting::{Surface, surface_f0};

@group(0) @binding(8) var env_cube: texture_cube<f32>;
@group(0) @binding(9) var env_sampler: sampler;
@group(0) @binding(10) var brdf_lut: texture_2d<f32>;
// SH9, pre-convolved with the cosine lobe and divided by π: evaluating it gives the diffuse
// multiplier for albedo directly.
@group(0) @binding(11) var<storage, read> env_sh: array<vec4<f32>, 9>;

fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    let c = array<f32, 9>(
        0.282095,
        0.488603 * n.y,
        0.488603 * n.z,
        0.488603 * n.x,
        1.092548 * n.x * n.y,
        1.092548 * n.y * n.z,
        0.315392 * (3.0 * n.z * n.z - 1.0),
        1.092548 * n.x * n.z,
        0.546274 * (n.x * n.x - n.y * n.y),
    );
    var e = vec3(0.0);
    for (var i = 0u; i < 9u; i++) {
        e += env_sh[i].rgb * c[i];
    }
    return max(e, vec3(0.0));
}

// Ambient light from the environment, scaled by occlusion. `mip_count` = levels of env_cube.
fn shade_environment(s: Surface, mip_count: f32) -> vec3<f32> {
    let n_dot_v = clamp(dot(s.normal, s.view_dir), 1e-4, 1.0);
    let f0 = surface_f0(s);
    let ab = textureSampleLevel(brdf_lut, env_sampler, vec2(n_dot_v, s.roughness), 0.0).rg;
    // Single-scatter split sum, plus multiple-scattering energy compensation (Fdez-Agüera
    // 2019): single-scatter GGX loses up to ~30% of the energy on rough metals, which then
    // look too dark next to EEVEE (which compensates too).
    let fss_ess = f0 * ab.x + ab.y;
    let ems = 1.0 - (ab.x + ab.y);
    let f_avg = f0 + (vec3(1.0) - f0) / 21.0;
    let fms_ems = fss_ess * f_avg * ems / (vec3(1.0) - ems * f_avg);
    let r = reflect(-s.view_dir, s.normal);
    let lod = s.roughness * max(mip_count - 1.0, 0.0);
    let prefiltered = textureSampleLevel(env_cube, env_sampler, r, lod).rgb;
    let irradiance = sh_irradiance(s.normal);
    let kd = max(vec3(1.0) - fss_ess - fms_ems, vec3(0.0)) * (1.0 - s.metallic);
    let diffuse = kd * s.albedo * irradiance;
    let specular = prefiltered * fss_ess + fms_ems * irradiance;
    return (diffuse + specular) * s.occlusion;
}

fn sample_sky(dir: vec3<f32>) -> vec3<f32> {
    return textureSampleLevel(env_cube, env_sampler, dir, 0.0).rgb;
}
