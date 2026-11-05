// Environment processing for IBL (plan 0006, decision 4). Run on the GPU once per
// environment change, never per frame:
//   source cube  ← world color | procedural sky | equirectangular HDRI
//   source mips  ← 2×2 downsample chain (for filtered importance sampling)
//   specular cube← GGX-prefiltered mips (roughness = mip / (mips − 1))
//   SH9          ← cosine-convolved irradiance / π
//   BRDF LUT     ← split-sum scale/bias (once at startup)
// Cube faces are written through 2D-array storage views: layer = face (+X −X +Y −Y +Z −Z).

const PI = 3.14159265;

struct EnvParams {
    // x = output face size, y = mode (0 color, 1 sky, 2 equirect), z = rotation (radians),
    // w = roughness (prefilter) or source mip count.
    a: vec4<f32>,
    // rgb = world color (mode 0) or sun direction toward the sun (mode 1); w = strength.
    b: vec4<f32>,
    // rgb = sky tint, w = sun elevation influence.
    c: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: EnvParams;
@group(0) @binding(1) var source_2d: texture_2d<f32>;
@group(0) @binding(2) var source_cube: texture_cube<f32>;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var out_cube: texture_storage_2d_array<rgba16float, write>;

// Direction through texel `xy` of cube face `face` (size `n`), matching WebGPU cube layout.
fn face_dir(face: u32, xy: vec2<u32>, n: f32) -> vec3<f32> {
    let uv = (vec2<f32>(xy) + 0.5) / n * 2.0 - 1.0;
    var d: vec3<f32>;
    switch face {
        case 0u: { d = vec3(1.0, -uv.y, -uv.x); }
        case 1u: { d = vec3(-1.0, -uv.y, uv.x); }
        case 2u: { d = vec3(uv.x, 1.0, uv.y); }
        case 3u: { d = vec3(uv.x, -1.0, -uv.y); }
        case 4u: { d = vec3(uv.x, -uv.y, 1.0); }
        default: { d = vec3(-uv.x, -uv.y, -1.0); }
    }
    return normalize(d);
}

// Our own analytic daylight sky (not a physical scattering model): a zenith-to-horizon
// gradient that brightens and warms toward the sun, a soft glow around it, dim ground below.
// The sun disk itself is left out: the Sun light already adds it.
fn sky(dir: vec3<f32>) -> vec3<f32> {
    let sun = normalize(params.b.xyz);
    let elevation = clamp(sun.y, -0.2, 1.0);
    let day = smoothstep(-0.15, 0.25, elevation);
    let zenith = mix(vec3(0.02, 0.03, 0.06), vec3(0.18, 0.36, 0.85), day) * params.c.rgb;
    let horizon = mix(vec3(0.25, 0.12, 0.06), vec3(0.75, 0.85, 1.0), day);
    let up = max(dir.y, 0.0);
    var col = mix(horizon, zenith, pow(up, 0.45));
    let cos_sun = max(dot(dir, sun), 0.0);
    let warm = mix(vec3(1.0, 0.45, 0.2), vec3(1.0, 0.9, 0.75), day);
    col += warm * (pow(cos_sun, 8.0) * 0.6 + pow(cos_sun, 64.0) * 1.5) * (0.3 + 0.7 * day);
    if dir.y < 0.0 {
        let ground = vec3(0.18, 0.17, 0.16) * (0.05 + 0.6 * day);
        col = mix(col, ground, smoothstep(0.0, 0.08, -dir.y));
    }
    return col * params.b.w;
}

@compute @workgroup_size(8, 8, 1)
fn make_source(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = u32(params.a.x);
    if gid.x >= n || gid.y >= n {
        return;
    }
    let dir = face_dir(gid.z, gid.xy, f32(n));
    let mode = u32(params.a.y + 0.5);
    var c: vec3<f32>;
    if mode == 0u {
        c = params.b.rgb;
    } else if mode == 1u {
        c = sky(dir);
    } else {
        let rot = params.a.z;
        let x = dir.x * cos(rot) - dir.z * sin(rot);
        let z = dir.x * sin(rot) + dir.z * cos(rot);
        let u = 0.5 + atan2(z, x) / (2.0 * PI);
        let v = 0.5 - asin(clamp(dir.y, -1.0, 1.0)) / PI;
        c = textureSampleLevel(source_2d, linear_sampler, vec2(u, v), 0.0).rgb * params.b.w;
    }
    textureStore(out_cube, gid.xy, gid.z, vec4(c, 1.0));
}

// One mip of the source cube from the previous one (sampled with the linear sampler at the
// previous level's lod, which averages 2×2).
@compute @workgroup_size(8, 8, 1)
fn downsample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n = u32(params.a.x);
    if gid.x >= n || gid.y >= n {
        return;
    }
    let dir = face_dir(gid.z, gid.xy, f32(n));
    let c = textureSampleLevel(source_cube, linear_sampler, dir, params.a.w).rgb;
    textureStore(out_cube, gid.xy, gid.z, vec4(c, 1.0));
}

fn radical_inverse(i: u32) -> f32 {
    var b = (i << 16u) | (i >> 16u);
    b = ((b & 0x55555555u) << 1u) | ((b & 0xAAAAAAAAu) >> 1u);
    b = ((b & 0x33333333u) << 2u) | ((b & 0xCCCCCCCCu) >> 2u);
    b = ((b & 0x0F0F0F0Fu) << 4u) | ((b & 0xF0F0F0F0u) >> 4u);
    b = ((b & 0x00FF00FFu) << 8u) | ((b & 0xFF00FF00u) >> 8u);
    return f32(b) * 2.3283064365386963e-10;
}

fn importance_ggx(xi: vec2<f32>, n: vec3<f32>, a: f32) -> vec3<f32> {
    let phi = 2.0 * PI * xi.x;
    let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y));
    let sin_t = sqrt(1.0 - cos_t * cos_t);
    let h = vec3(sin_t * cos(phi), sin_t * sin(phi), cos_t);
    let up = select(vec3(1.0, 0.0, 0.0), vec3(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    return normalize(tx * h.x + ty * h.y + n * h.z);
}

fn d_ggx(n_dot_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

const PREFILTER_SAMPLES = 64u;

// GGX prefilter with filtered importance sampling (sample lower source mips where the pdf is
// low), which removes the fireflies plain importance sampling leaves.
@compute @workgroup_size(8, 8, 1)
fn prefilter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let n_size = u32(params.a.x);
    if gid.x >= n_size || gid.y >= n_size {
        return;
    }
    let n = face_dir(gid.z, gid.xy, f32(n_size));
    let roughness = params.a.w;
    let a = max(roughness * roughness, 0.002);
    let src_size = params.b.x;
    let src_mips = params.b.y;
    let texel_solid_angle = 4.0 * PI / (6.0 * src_size * src_size);
    var sum = vec3(0.0);
    var weight = 0.0;
    for (var i = 0u; i < PREFILTER_SAMPLES; i++) {
        let xi = vec2(f32(i) / f32(PREFILTER_SAMPLES), radical_inverse(i));
        let h = importance_ggx(xi, n, a);
        let l = normalize(2.0 * dot(n, h) * h - n);
        let n_dot_l = dot(n, l);
        if n_dot_l > 0.0 {
            let n_dot_h = max(dot(n, h), 0.0);
            let pdf = d_ggx(n_dot_h, a) * 0.25 + 1e-4;
            let sample_solid_angle = 1.0 / (f32(PREFILTER_SAMPLES) * pdf);
            let lod = clamp(0.5 * log2(sample_solid_angle / texel_solid_angle) + 1.0, 0.0, src_mips - 1.0);
            sum += textureSampleLevel(source_cube, linear_sampler, l, lod).rgb * n_dot_l;
            weight += n_dot_l;
        }
    }
    textureStore(out_cube, gid.xy, gid.z, vec4(sum / max(weight, 1e-4), 1.0));
}

// ---- SH9 projection (one workgroup over a small source mip) --------------------------------

@group(1) @binding(0) var<storage, read_write> sh_out: array<vec4<f32>, 9>;

var<workgroup> partial: array<array<vec3<f32>, 9>, 64>;

fn sh_basis(d: vec3<f32>) -> array<f32, 9> {
    return array<f32, 9>(
        0.282095,
        0.488603 * d.y,
        0.488603 * d.z,
        0.488603 * d.x,
        1.092548 * d.x * d.y,
        1.092548 * d.y * d.z,
        0.315392 * (3.0 * d.z * d.z - 1.0),
        1.092548 * d.x * d.z,
        0.546274 * (d.x * d.x - d.y * d.y),
    );
}

@compute @workgroup_size(64, 1, 1)
fn project_sh(@builtin(local_invocation_index) li: u32) {
    let n = u32(params.a.x);
    let lod = params.a.w;
    var acc: array<vec3<f32>, 9>;
    var total_weight = 0.0;
    let texels = n * n * 6u;
    for (var t = li; t < texels; t += 64u) {
        let face = t / (n * n);
        let xy = vec2(t % n, (t / n) % n);
        let dir = face_dir(face, xy, f32(n));
        // Solid angle of a cube texel ∝ 1 / (1 + u² + v²)^(3/2).
        let uv = (vec2<f32>(xy) + 0.5) / f32(n) * 2.0 - 1.0;
        let w = 1.0 / pow(1.0 + dot(uv, uv), 1.5);
        let c = textureSampleLevel(source_cube, linear_sampler, dir, lod).rgb;
        let y = sh_basis(dir);
        for (var k = 0u; k < 9u; k++) {
            acc[k] += c * y[k] * w;
        }
        total_weight += w;
    }
    partial[li] = acc;
    workgroupBarrier();
    if li == 0u {
        var sum: array<vec3<f32>, 9>;
        for (var i = 0u; i < 64u; i++) {
            for (var k = 0u; k < 9u; k++) {
                sum[k] += partial[i][k];
            }
        }
        // Normalize to the sphere (4π), convolve with the cosine lobe (Â0 = π, Â1 = 2π/3,
        // Â2 = π/4) and divide by π so evaluation yields the albedo multiplier.
        let norm = 4.0 * PI / (total_weight_all(n));
        let band = array<f32, 9>(1.0, 2.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 0.25, 0.25, 0.25, 0.25, 0.25);
        for (var k = 0u; k < 9u; k++) {
            sh_out[k] = vec4(sum[k] * norm * band[k], 0.0);
        }
    }
}

// Sum of the texel solid-angle weights over the whole cube (same formula as above).
fn total_weight_all(n: u32) -> f32 {
    var s = 0.0;
    for (var y = 0u; y < n; y++) {
        for (var x = 0u; x < n; x++) {
            let uv = (vec2(f32(x), f32(y)) + 0.5) / f32(n) * 2.0 - 1.0;
            s += 1.0 / pow(1.0 + dot(uv, uv), 1.5);
        }
    }
    return s * 6.0;
}

// ---- Split-sum BRDF lookup (x = n·v, y = roughness) -----------------------------------------

@group(0) @binding(5) var lut_target: texture_storage_2d<rgba16float, write>;

fn v_smith_ibl(n_dot_v: f32, n_dot_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

@compute @workgroup_size(8, 8, 1)
fn brdf_lut(@builtin(global_invocation_id) gid: vec3<u32>) {
    let size = u32(params.a.x);
    if gid.x >= size || gid.y >= size {
        return;
    }
    let n_dot_v = (f32(gid.x) + 0.5) / f32(size);
    let roughness = (f32(gid.y) + 0.5) / f32(size);
    let a = max(roughness * roughness, 0.002);
    let v = vec3(sqrt(1.0 - n_dot_v * n_dot_v), 0.0, n_dot_v);
    let n = vec3(0.0, 0.0, 1.0);
    var scale = 0.0;
    var bias = 0.0;
    let samples = 256u;
    for (var i = 0u; i < samples; i++) {
        let xi = vec2(f32(i) / f32(samples), radical_inverse(i));
        let h = importance_ggx(xi, n, a);
        let l = normalize(2.0 * dot(v, h) * h - v);
        let n_dot_l = max(l.z, 0.0);
        let n_dot_h = max(h.z, 0.0);
        let v_dot_h = max(dot(v, h), 0.0);
        if n_dot_l > 0.0 {
            // pdf = D·n·h / (4 v·h); with V = vis term including 1/(4 n·l n·v):
            let g = v_smith_ibl(n_dot_v, n_dot_l, a) * 4.0 * n_dot_l * v_dot_h / max(n_dot_h, 1e-5);
            let fc = pow(1.0 - v_dot_h, 5.0);
            scale += (1.0 - fc) * g;
            bias += fc * g;
        }
    }
    textureStore(lut_target, gid.xy, vec4(scale / f32(samples), bias / f32(samples), 0.0, 1.0));
}
