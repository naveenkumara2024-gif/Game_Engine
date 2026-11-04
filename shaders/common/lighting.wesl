// Lighting v0 (plan 0004): punctual lights, GGX specular + Lambert diffuse, flat world ambient.
// Units follow Blender (see crates/ge_render/src/lighting.rs). M4 adds shadows, IBL, clusters.

const PI = 3.14159265;
const MAX_LIGHTS = 16u;

// Mirrors `LightUniform` in crates/ge_render/src/lighting.rs.
struct Light {
    // xyz = world position, w = custom-distance cutoff (0 = unlimited).
    position_cutoff: vec4<f32>,
    // xyz = direction the light travels, w = kind (0 point, 1 sun, 2 spot, 3 area).
    direction_kind: vec4<f32>,
    // rgb = color × intensity (W/sr, or W/m² for a sun), w = diffuse factor.
    radiance_diffuse: vec4<f32>,
    // x = cos(half cone angle), y = blend width in cosine, z = specular factor.
    spot_specular: vec4<f32>,
}

struct Lights {
    items: array<Light, 16>,
}

struct Surface {
    position: vec3<f32>,
    normal: vec3<f32>,
    view_dir: vec3<f32>,
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,
}

fn d_ggx(n_dot_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

// Height-correlated Smith visibility (includes the 1 / (4 n·l n·v) term).
fn v_smith(n_dot_v: f32, n_dot_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn f_schlick(f0: vec3<f32>, v_dot_h: f32) -> vec3<f32> {
    return f0 + (vec3(1.0) - f0) * pow(1.0 - v_dot_h, 5.0);
}

// Outgoing radiance from one light (before the direct-light multiplier).
fn shade_light(light: Light, s: Surface) -> vec3<f32> {
    let kind = u32(light.direction_kind.w + 0.5);
    let dir = light.direction_kind.xyz;
    var l: vec3<f32>;
    var irradiance = light.radiance_diffuse.rgb;
    if kind == 1u {
        l = -dir;
    } else {
        let to_light = light.position_cutoff.xyz - s.position;
        let dist2 = max(dot(to_light, to_light), 1e-4);
        l = to_light * inverseSqrt(dist2);
        irradiance = irradiance / dist2;
        let cutoff = light.position_cutoff.w;
        if cutoff > 0.0 {
            // Smooth window so the light fades to exactly zero at the custom distance.
            let r = sqrt(dist2) / cutoff;
            let w = clamp(1.0 - r * r * r * r, 0.0, 1.0);
            irradiance = irradiance * w * w;
        }
        let cos_axis = dot(-l, dir);
        if kind == 2u {
            let spot_cos = light.spot_specular.x;
            let blend = max(light.spot_specular.y, 1e-4);
            irradiance = irradiance * smoothstep(0.0, 1.0, (cos_axis - spot_cos) / blend);
        } else if kind == 3u {
            // Area v0: one-sided Lambertian emitter.
            irradiance = irradiance * max(cos_axis, 0.0);
        }
    }

    let n_dot_l = dot(s.normal, l);
    if n_dot_l <= 0.0 {
        return vec3(0.0);
    }
    let h = normalize(l + s.view_dir);
    let n_dot_v = max(dot(s.normal, s.view_dir), 1e-4);
    let n_dot_h = max(dot(s.normal, h), 0.0);
    let v_dot_h = max(dot(s.view_dir, h), 0.0);

    // Blender maps roughness r to GGX alpha = r².
    let a = max(s.roughness * s.roughness, 0.002);
    let f0 = mix(vec3(0.04), s.albedo, s.metallic);
    let f = f_schlick(f0, v_dot_h);
    let specular = f * d_ggx(n_dot_h, a) * v_smith(n_dot_v, n_dot_l, a);
    let kd = (vec3(1.0) - f) * (1.0 - s.metallic);
    let diffuse = kd * s.albedo / PI;

    let diffuse_factor = light.radiance_diffuse.w;
    let specular_factor = light.spot_specular.z;
    return (diffuse * diffuse_factor + specular * specular_factor) * irradiance * n_dot_l;
}

// Flat ambient from a uniform world color: diffuse sees ρ·L, specular a rough Fresnel guess.
fn shade_ambient(ambient: vec3<f32>, s: Surface) -> vec3<f32> {
    let f0 = mix(vec3(0.04), s.albedo, s.metallic);
    let diffuse = s.albedo * (1.0 - s.metallic);
    let specular = f0 * (1.0 - 0.5 * s.roughness);
    return (diffuse + specular) * ambient;
}
