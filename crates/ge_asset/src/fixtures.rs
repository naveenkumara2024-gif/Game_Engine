//! Asset fixtures written from code (plan 0005, decision 13): a textured crate `.glb` and a BC1
//! `.ktx2`. Used by tests and by `cargo xtask gen-assets`, so the sample's assets carry no
//! third-party license.

use crate::import::ktx2;
use crate::types::PixelFormat;

/// `(positions, normals, uvs, indices)` of a mesh.
type MeshArrays = (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<u16>);

/// Box with 24 vertices (flat faces) and UVs per face: `(positions, normals, uvs, indices)`.
fn box_mesh(half: [f32; 3]) -> MeshArrays {
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let (mut p, mut n, mut t, mut i) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (normal, u, v) in faces {
        let base = p.len() as u16;
        for (su, sv, tu, tv) in [
            (-1.0, -1.0, 0.0, 1.0),
            (1.0, -1.0, 1.0, 1.0),
            (1.0, 1.0, 1.0, 0.0),
            (-1.0, 1.0, 0.0, 0.0),
        ] {
            let pos: [f32; 3] =
                std::array::from_fn(|k| (normal[k] + u[k] * su + v[k] * sv) * half[k]);
            p.push(pos);
            n.push(normal);
            t.push([tu, tv]);
        }
        i.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (p, n, t, i)
}

/// A 64×64 plank texture as PNG bytes.
fn plank_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(64, 64, |x, y| {
        let plank = y / 16;
        let seam = y % 16 == 0 || (x + plank * 23) % 64 == 0;
        let grain = ((x as f32 * 0.35 + plank as f32 * 1.7).sin() * 12.0) as i32;
        let base = [150 + grain, 100 + grain / 2, 55];
        if seam {
            image::Rgba([70, 45, 25, 255])
        } else {
            image::Rgba([
                base[0].clamp(0, 255) as u8,
                base[1].clamp(0, 255) as u8,
                base[2] as u8,
                255,
            ])
        }
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode png");
    png
}

/// The planks' tangent-space normal map (PNG, linear): grooves at the seams and a faint
/// grain, so the crate shows normal mapping (plan 0006).
fn plank_normal_png() -> Vec<u8> {
    // Height field: 0 in the 2-texel groove around each seam, 1 on the plank, plus grain.
    let height = |x: i32, y: i32| -> f32 {
        let x = x.rem_euclid(64) as u32;
        let y = y.rem_euclid(64) as u32;
        let plank = y / 16;
        let dy = (y % 16).min(16 - y % 16);
        let dx = ((x + plank * 23) % 64).min(64 - (x + plank * 23) % 64);
        let groove = (dy.min(dx) as f32 / 2.0).min(1.0);
        groove + (x as f32 * 0.35 + plank as f32 * 1.7).sin() * 0.03
    };
    let img = image::RgbaImage::from_fn(64, 64, |x, y| {
        let (x, y) = (x as i32, y as i32);
        // Central differences; +V points down the image (glTF UVs), so flip the y slope.
        let du = (height(x + 1, y) - height(x - 1, y)) * 0.5 * 1.5;
        let dv = (height(x, y + 1) - height(x, y - 1)) * 0.5 * 1.5;
        let n = [-du, dv, 1.0];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let enc = |c: f32| ((c / len * 0.5 + 0.5) * 255.0).round() as u8;
        image::Rgba([enc(n[0]), enc(n[1]), enc(n[2]), 255])
    });
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode png");
    png
}

fn pad4(v: &mut Vec<u8>, byte: u8) {
    while !v.len().is_multiple_of(4) {
        v.push(byte);
    }
}

/// `crate.glb`: a wooden crate (plank texture + normal map, roughness 0.7) with a metal band
/// child node (untextured, metallic). Two materials, two textures, two nodes.
pub fn crate_glb() -> Vec<u8> {
    let (pos, nrm, uv, idx) = box_mesh([0.5, 0.5, 0.5]);
    let png = plank_png();
    let normal_png = plank_normal_png();
    let mut bin = Vec::new();
    let mut views = Vec::new(); // (offset, length, target)
    let mut push = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| {
        pad4(bin, 0);
        views.push((bin.len(), bytes.len(), target));
        bin.extend_from_slice(bytes);
    };
    let f32s = |v: &[[f32; 3]]| {
        v.iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>()
    };
    push(&mut bin, &f32s(&pos), Some(34962));
    push(&mut bin, &f32s(&nrm), Some(34962));
    push(
        &mut bin,
        &uv.iter()
            .flatten()
            .flat_map(|f| f.to_le_bytes())
            .collect::<Vec<u8>>(),
        Some(34962),
    );
    push(
        &mut bin,
        &idx.iter()
            .flat_map(|i| i.to_le_bytes())
            .collect::<Vec<u8>>(),
        Some(34963),
    );
    push(&mut bin, &png, None);
    push(&mut bin, &normal_png, None);
    pad4(&mut bin, 0);

    let view_json: Vec<String> = views
        .iter()
        .map(|(o, l, t)| match t {
            Some(t) => format!(r#"{{"buffer":0,"byteOffset":{o},"byteLength":{l},"target":{t}}}"#),
            None => format!(r#"{{"buffer":0,"byteOffset":{o},"byteLength":{l}}}"#),
        })
        .collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0","generator":"ge_asset fixtures"}},
"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[
 {{"name":"Crate","mesh":0,"translation":[0,0.5,0],"children":[1]}},
 {{"name":"Band","mesh":1,"translation":[0,0.15,0],"scale":[1.04,0.12,1.04]}}],
"meshes":[
 {{"name":"Box","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"material":0}}]}},
 {{"name":"BandBox","primitives":[{{"attributes":{{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2}},"indices":3,"material":1}}]}}],
"materials":[
 {{"name":"Wood","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}},"metallicFactor":0.0,"roughnessFactor":0.7}},"normalTexture":{{"index":1}}}},
 {{"name":"Metal","pbrMetallicRoughness":{{"baseColorFactor":[0.55,0.56,0.6,1.0],"metallicFactor":1.0,"roughnessFactor":0.3}}}}],
"textures":[{{"source":0}},{{"source":1}}],
"images":[{{"bufferView":4,"mimeType":"image/png"}},{{"bufferView":5,"mimeType":"image/png"}}],
"accessors":[
 {{"bufferView":0,"componentType":5126,"count":24,"type":"VEC3","min":[-0.5,-0.5,-0.5],"max":[0.5,0.5,0.5]}},
 {{"bufferView":1,"componentType":5126,"count":24,"type":"VEC3"}},
 {{"bufferView":2,"componentType":5126,"count":24,"type":"VEC2"}},
 {{"bufferView":3,"componentType":5123,"count":36,"type":"SCALAR"}}],
"bufferViews":[{}],
"buffers":[{{"byteLength":{}}}]}}"#,
        view_json.join(","),
        bin.len()
    );
    let mut json = json.into_bytes();
    pad4(&mut json, b' ');

    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&bin);
    glb
}

/// `checker_bc1.ktx2`: a 64×64 BC1 sRGB checker with its full mip chain.
pub fn checker_bc1_ktx2() -> Vec<u8> {
    let block = |rgb565: u16| {
        let mut b = rgb565.to_le_bytes().to_vec();
        b.extend_from_slice(&rgb565.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b
    };
    let (light, dark) = (0xC618u16, 0x4208u16); // greys in RGB565
    let mut mips = Vec::new();
    let mut size = 64u32;
    loop {
        let blocks = size.div_ceil(4);
        let mut level = Vec::new();
        for by in 0..blocks {
            for bx in 0..blocks {
                // 8×8-texel checker squares = 2×2 blocks at level 0, then they shrink.
                let cell = (8 >> mips.len()).max(4) / 4;
                let on = ((bx / cell) + (by / cell)) % 2 == 0;
                level.extend(block(if on { light } else { dark }));
            }
        }
        mips.push(level);
        if size == 1 {
            break;
        }
        size /= 2;
    }
    ktx2::write(PixelFormat::Bc1Srgb, 64, 64, &mips)
}
