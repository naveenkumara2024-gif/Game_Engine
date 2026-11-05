// Lighting (plan 0006): GGX specular + Lambert diffuse, clustered punctual and area lights,
// soft lights via the representative-point method (Karis 2013), image-based ambient.
// Units follow Blender (see crates/ge_render/src/lighting.rs).

const PI = 3.14159265;
// Lights a cluster can reference; mirrors `MAX_LIGHTS_PER_CLUSTER` in clusters.rs.
const MAX_LIGHTS_PER_CLUSTER = 128u;

// Mirrors `LightUniform` in crates/ge_render/src/lighting.rs.
struct Light {
    // xyz = world position, w = range where the light fades to zero (0 = unlimited).
    position_range: vec4<f32>,
    // xyz = direction the light travels, w = kind (0 point, 1 sun, 2 spot, 3 area).
    direction_kind: vec4<f32>,
    // rgb = color × intensity (W/sr, or W/m² for a sun), w = diffuse factor.
    radiance_diffuse: vec4<f32>,
    // x = cos(half cone angle), y = blend width in cosine, z = specular factor, w = radius.
    spot_specular: vec4<f32>,
    // xyz = area light "up" axis, w = shadow slot (-1 = no shadow).
    up_shadow: vec4<f32>,
    // x, y = area half width/height, z = sun angular radius, w = 1 for soft falloff.
    shape: vec4<f32>,
}

struct Surface {
    position: vec3<f32>,
    normal: vec3<f32>,
    view_dir: vec3<f32>,
    albedo: vec3<f32>,
    metallic: f32,
    roughness: f32,
    // Reflectance at normal incidence for dielectrics (from IOR and Specular IOR Level).
    f0_dielectric: f32,
    occlusion: f32,
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

fn surface_f0(s: Surface) -> vec3<f32> {
    return mix(vec3(s.f0_dielectric), s.albedo, s.metallic);
}

// Specular lobe for light direction `l`, with GGX alpha `a` and an energy factor for widened
// lobes (soft lights).
fn specular_lobe(s: Surface, l: vec3<f32>, a: f32, energy: f32) -> vec3<f32> {
    let n_dot_l = dot(s.normal, l);
    if n_dot_l <= 0.0 {
        return vec3(0.0);
    }
    let h = normalize(l + s.view_dir);
    let n_dot_v = max(dot(s.normal, s.view_dir), 1e-4);
    let n_dot_h = max(dot(s.normal, h), 0.0);
    let v_dot_h = max(dot(s.view_dir, h), 0.0);
    let f = f_schlick(surface_f0(s), v_dot_h);
    return f * d_ggx(n_dot_h, a) * v_smith(n_dot_v, n_dot_l, a) * n_dot_l * energy;
}

fn diffuse_lobe(s: Surface, l: vec3<f32>) -> vec3<f32> {
    let n_dot_l = max(dot(s.normal, l), 0.0);
    let kd = (1.0 - s.metallic) * (1.0 - s.f0_dielectric);
    return kd * s.albedo / PI * n_dot_l;
}

// Distance window: fades to exactly zero at `range` so culled lights never pop.
fn range_window(dist: f32, range: f32) -> f32 {
    if range <= 0.0 {
        return 1.0;
    }
    let r = dist / range;
    let w = clamp(1.0 - r * r * r * r, 0.0, 1.0);
    return w * w;
}

// Outgoing radiance from one light, times its shadow `visibility` (1 = lit).
fn shade_light(light: Light, s: Surface, visibility: f32) -> vec3<f32> {
    if visibility <= 0.0 {
        return vec3(0.0);
    }
    let kind = u32(light.direction_kind.w + 0.5);
    let dir = light.direction_kind.xyz;
    let a = max(s.roughness * s.roughness, 0.002);
    let r = reflect(-s.view_dir, s.normal);
    let diffuse_factor = light.radiance_diffuse.w;
    let specular_factor = light.spot_specular.z;
    var radiance = light.radiance_diffuse.rgb * visibility;

    if kind == 1u {
        // Sun: a disk of angular radius `theta` at infinity. Specular aims at the disk point
        // closest to the reflection vector and widens the lobe to keep energy.
        let l = -dir;
        let theta = light.shape.z;
        var ls = l;
        var a_spec = a;
        if theta > 0.0 {
            let cos_t = cos(theta);
            let d = dot(l, r);
            if d < cos_t {
                let perp = normalize(r - l * d);
                ls = normalize(l * cos_t + perp * sin(theta));
            } else {
                ls = r;
            }
            a_spec = clamp(a + 0.5 * sin(theta), 0.0, 1.0);
        }
        let energy = (a / a_spec) * (a / a_spec);
        return (diffuse_lobe(s, l) * diffuse_factor
            + specular_lobe(s, ls, a_spec, energy) * specular_factor) * radiance;
    }

    let to_light = light.position_range.xyz - s.position;
    let dist2 = max(dot(to_light, to_light), 1e-4);
    let dist = sqrt(dist2);
    let l = to_light / dist;
    let radius = light.spot_specular.w;
    // Soft falloff (Blender): no infinite peak when the surface touches the light.
    var falloff = 1.0 / dist2;
    if light.shape.w > 0.5 {
        falloff = 1.0 / (dist2 + radius * radius);
    }
    radiance = radiance * falloff * range_window(dist, light.position_range.w);

    let cos_axis = dot(-l, dir);
    if kind == 2u {
        let blend = max(light.spot_specular.y, 1e-4);
        radiance = radiance * smoothstep(0.0, 1.0, (cos_axis - light.spot_specular.x) / blend);
    }

    var ls = l;
    var a_spec = a;
    if kind == 3u {
        // Area (rectangle): one-sided Lambertian emitter. Specular uses the point on the
        // rectangle closest to where the reflection ray hits its plane.
        radiance = radiance * max(cos_axis, 0.0);
        let up = light.up_shadow.xyz;
        let right = normalize(cross(dir, up));
        let half_size = light.shape.xy;
        let denom = dot(r, dir);
        if abs(denom) > 1e-4 {
            let t = dot(light.position_range.xyz - s.position, dir) / denom;
            let hit = s.position + r * max(t, 0.0) - light.position_range.xyz;
            let p = light.position_range.xyz
                + right * clamp(dot(hit, right), -half_size.x, half_size.x)
                + up * clamp(dot(hit, up), -half_size.y, half_size.y);
            ls = normalize(p - s.position);
        }
        let size = length(half_size);
        a_spec = clamp(a + size / (2.0 * dist), 0.0, 1.0);
    } else if radius > 0.0 {
        // Sphere light: closest point on the sphere to the reflection ray.
        let center_to_ray = dot(to_light, r) * r - to_light;
        let closest = to_light + center_to_ray * clamp(radius / max(length(center_to_ray), 1e-4), 0.0, 1.0);
        ls = normalize(closest);
        a_spec = clamp(a + radius / (2.0 * dist), 0.0, 1.0);
    }
    let energy = (a / a_spec) * (a / a_spec);
    return (diffuse_lobe(s, l) * diffuse_factor
        + specular_lobe(s, ls, a_spec, energy) * specular_factor) * radiance;
}

// Flat ambient from a uniform world color (when there's no environment map).
fn shade_flat_ambient(ambient: vec3<f32>, s: Surface) -> vec3<f32> {
    let f0 = surface_f0(s);
    let diffuse = s.albedo * (1.0 - s.metallic);
    let specular = f0 * (1.0 - 0.5 * s.roughness);
    return (diffuse + specular) * ambient * s.occlusion;
}

// Cluster index of a fragment: screen tile + exponential depth slice.
fn cluster_index(frag_xy: vec2<f32>, view_z: f32, screen: vec4<f32>, dims: vec3<u32>, scale: f32, bias: f32) -> u32 {
    let tile = vec2<u32>(clamp(frag_xy / screen.xy * vec2<f32>(dims.xy), vec2(0.0), vec2<f32>(dims.xy) - 1.0));
    let slice = u32(clamp(floor(log(max(view_z, 1e-4)) * scale + bias), 0.0, f32(dims.z) - 1.0));
    return (slice * dims.y + tile.y) * dims.x + tile.x;
}
