//! Headless GPU checks for the M4 renderer (plan 0006): HDR + view transforms, clustered
//! lighting, shadows, IBL, bloom, normal maps and alpha. Like `passes.rs` they assert on
//! pixels and ratios rather than a golden image, so they hold across GPUs. Skip without a GPU.

use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use ge_asset::{AssetId, MaterialAsset, PixelFormat, SubRef, Texture};
use ge_render::assets::AssetUpload;
use ge_render::lighting::{ColorManagement, LightItem, ViewTransform, ViewportShading};
use ge_render::{
    BloomSettings, Camera, DrawAlpha, DrawItem, FrameInputs, Light, LightKind, MeshKind, Primitive,
    PrimitiveMesh, RenderView, RenderWorld, Renderer, WorldSurface,
};
use ge_rhi::{Rhi, Texture as GpuTexture, TextureDesc, TextureFormat, TextureUsage};
use ge_shader::ShaderLibrary;
use glam::{Mat4, Vec3, Vec4};

const SIZE: u32 = 64;

struct Gpu {
    rhi: Rhi,
    target: GpuTexture,
    renderer: Renderer,
}

fn gpu() -> Option<Gpu> {
    ge_core::log::init_logging();
    let rhi = match Rhi::new_headless() {
        Ok(rhi) => rhi,
        Err(e) => {
            eprintln!("skipping: no GPU ({e})");
            return None;
        }
    };
    let format = TextureFormat::Rgba8UnormSrgb;
    let target = rhi.create_texture(&TextureDesc {
        label: "PbrTarget",
        size: (SIZE, SIZE),
        format,
        usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
    });
    let renderer = Renderer::new(
        &rhi,
        format,
        ShaderLibrary::new(ShaderLibrary::default_root()),
    );
    assert_eq!(
        renderer.missing_pipelines(),
        Vec::<&str>::new(),
        "shaders failed to build; see log"
    );
    Some(Gpu {
        rhi,
        target,
        renderer,
    })
}

fn draw(gpu: &mut Gpu, inputs: FrameInputs<'_>) -> Vec<u8> {
    let mut frame = gpu.rhi.begin_offscreen_frame(&gpu.target);
    gpu.renderer.render_frame(&gpu.rhi, &mut frame, inputs);
    gpu.rhi.submit_offscreen(frame);
    gpu.rhi.read_texture_rgba8(&gpu.target).expect("readback")
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

fn center(pixels: &[u8]) -> [u8; 4] {
    pixel(pixels, SIZE / 2, SIZE / 2)
}

/// sRGB 8-bit → linear.
fn linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn view_from(position: Vec3, target: Vec3, up: Vec3) -> RenderView {
    RenderView {
        camera: Camera {
            position,
            target,
            up,
            ..Default::default()
        },
        time: 0.0,
        clear_color: [0.0; 3],
    }
}

/// Looking straight down at the origin from `height`; screen-up is world −Z.
fn top_down(height: f32) -> RenderView {
    view_from(Vec3::new(0.0, height, 0.0), Vec3::ZERO, Vec3::NEG_Z)
}

fn rendered<'a>(view: &'a RenderView, items: &'a [DrawItem]) -> FrameInputs<'a> {
    let mut inputs = FrameInputs::basic(view, items);
    inputs.shading = ViewportShading::Rendered;
    inputs.color = ColorManagement::default();
    inputs
}

fn floor(half_size: f32) -> DrawItem {
    let mut item = DrawItem::new(
        MeshKind::Plane,
        Mat4::from_scale(Vec3::splat(half_size)),
        Vec4::splat(0.8),
    );
    item.roughness = 1.0;
    item
}

fn sphere(tint: Vec4, metallic: f32, roughness: f32) -> DrawItem {
    let mut item = DrawItem::new(
        MeshKind::Primitive(PrimitiveMesh::new(Primitive::UvSphere {
            segments: 48,
            rings: 24,
            radius: 1.0,
        })),
        Mat4::IDENTITY,
        tint,
    );
    item.metallic = metallic;
    item.roughness = roughness;
    item
}

fn light_at(kind: LightKind, position: Vec3, aim: Vec3) -> LightItem {
    let light = Light::new(kind);
    let transform = ge_ecs::Transform::from_translation(position).looking_at(aim, Vec3::Y);
    let global = ge_ecs::GlobalTransform(transform.to_affine());
    LightItem::from_light(&light, &global)
}

fn color_world(color: Vec3) -> RenderWorld {
    RenderWorld {
        color,
        strength: 1.0,
        surface: WorldSurface::Color,
        hdri: None,
        hdri_rotation: 0.0,
        probe_resolution: 128,
    }
}

#[test]
fn hdr_emission_clips_under_standard_and_rolls_off_under_agx() {
    let Some(mut gpu) = gpu() else { return };
    let view = top_down(3.0);
    let mut plane = floor(10.0);
    plane.tint = Vec4::new(0.0, 0.0, 0.0, 1.0);
    plane.emission = Vec3::splat(8.0);
    let items = [plane];

    let standard = center(&draw(&mut gpu, rendered(&view, &items)));
    assert_eq!(
        &standard[..3],
        &[255, 255, 255],
        "Standard clips: {standard:?}"
    );

    let agx_at = |gpu: &mut Gpu, emission: f32| {
        let mut plane = items[0];
        plane.emission = Vec3::splat(emission);
        let items = [plane];
        let mut inputs = rendered(&view, &items);
        inputs.color.view = ViewTransform::AgX;
        center(&draw(gpu, inputs))
    };
    let agx8 = agx_at(&mut gpu, 8.0);
    let agx2 = agx_at(&mut gpu, 2.0);
    assert!(agx8[0] < 255, "AgX keeps emission 8 below white: {agx8:?}");
    assert!(
        agx2[0] + 10 < agx8[0],
        "AgX still separates 2 from 8: {agx2:?} vs {agx8:?}"
    );

    // Exposure is applied in HDR before the view transform: −4 EV brings 8 down to 0.5.
    let mut inputs = rendered(&view, &items);
    inputs.color.exposure_ev = -4.0;
    let dimmed = center(&draw(&mut gpu, inputs));
    assert!(
        (linear(dimmed[0]) - 0.5).abs() < 0.03,
        "8 × 2^-4 = 0.5 linear: {dimmed:?}"
    );
}

fn light_grid(count: usize, spacing: f32, range: f32, radiance: f32) -> Vec<LightItem> {
    let cols = 20;
    let rows = count.div_ceil(cols);
    (0..count)
        .map(|i| {
            let x = (i % cols) as f32 * spacing - (cols - 1) as f32 * spacing * 0.5;
            let z = (i / cols) as f32 * spacing - (rows - 1) as f32 * spacing * 0.5;
            let p = Vec3::new(x, 0.3 + (i % 3) as f32 * 0.25, z);
            let mut l = light_at(LightKind::Point, p, p + Vec3::NEG_Y);
            let hue = [Vec3::X, Vec3::Y, Vec3::Z][i % 3];
            l.radiance = (hue * 0.8 + Vec3::splat(0.2)) * radiance;
            l.range = range;
            l
        })
        .collect()
}

/// Clustered shading must match the brute-force reference (`r.Clusters.BruteForce`: every
/// light for every pixel) — a light missing from any cluster its sphere overlaps, a cluster
/// list overflowing, or the last light in the buffer being dropped all show up as a mismatch.
/// Run at 300 lights (dozens per cluster) and at the 1024-light cap. Both stay under the
/// 128-lights-per-cluster list limit; lights past it are dropped (documented in plan 0006).
#[test]
fn clustered_lighting_matches_brute_force() {
    use ge_core::cvar::{SetBy, set};
    let Some(mut gpu) = gpu() else { return };
    let view = top_down(9.0);
    let items = [floor(12.0)];
    for (count, spacing, range, radiance) in [(300, 0.45, 1.2, 0.04), (1024, 0.25, 0.6, 0.03)] {
        let lights = light_grid(count, spacing, range, radiance);
        let mut render = |brute: bool| {
            set(
                "r.Clusters.BruteForce",
                if brute { "1" } else { "0" },
                SetBy::Code,
            )
            .unwrap();
            let mut inputs = rendered(&view, &items);
            inputs.lights = &lights;
            let px = draw(&mut gpu, inputs);
            set("r.Clusters.BruteForce", "0", SetBy::Code).unwrap();
            px
        };
        let reference = render(true);
        let clustered = render(false);
        let lit = reference
            .chunks_exact(4)
            .filter(|p| p[..3].iter().any(|&v| v > 20))
            .count();
        assert!(
            lit > (SIZE * SIZE) as usize / 2,
            "{count}: floor lit ({lit})"
        );
        let worst = reference
            .iter()
            .zip(&clustered)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= 2,
            "{count} lights: clustered differs from brute force by {worst}"
        );
    }
}

/// A cube between the light and the floor point under the camera darkens that point, for
/// each shadow path (sun cascades, spot atlas tile, point cube faces in the atlas).
#[test]
fn sun_spot_and_point_lights_cast_shadows() {
    let Some(mut gpu) = gpu() else { return };
    let view = top_down(6.0);
    let light_pos = Vec3::new(2.0, 3.0, 0.0);
    // The cube sits on the segment from the light to the origin.
    let cube = DrawItem::new(
        MeshKind::Cube,
        Mat4::from_scale_rotation_translation(
            Vec3::splat(0.6),
            glam::Quat::IDENTITY,
            Vec3::new(1.0, 1.5, 0.0),
        ),
        Vec4::splat(0.8),
    );
    for kind in [LightKind::Sun, LightKind::Spot, LightKind::Point] {
        let mut light = light_at(kind, light_pos, Vec3::ZERO);
        light.cast_shadow = true;
        if kind == LightKind::Sun {
            light.radiance = Vec3::splat(3.0);
        }
        let lights = [light];
        let mut shade = |cast: bool| {
            let mut c = cube;
            c.cast_shadow = cast;
            let items = [floor(5.0), c];
            let mut inputs = rendered(&view, &items);
            inputs.lights = &lights;
            linear(center(&draw(&mut gpu, inputs))[0])
        };
        let lit = shade(false);
        let shadowed = shade(true);
        assert!(lit > 0.1, "{kind:?}: the floor is lit ({lit})");
        assert!(
            shadowed < lit * 0.2,
            "{kind:?}: the cube shadows the floor ({shadowed} vs {lit})"
        );
    }
}

/// White furnace: a sphere inside a uniform white world reflects (diffuse + specular) close to
/// the world's radiance, for rough dielectrics and smooth metals alike. A red world tints a
/// metal sphere red.
#[test]
fn ibl_conserves_energy_and_reflects_the_world() {
    let Some(mut gpu) = gpu() else { return };
    let view = view_from(Vec3::new(0.0, 0.0, 4.0), Vec3::ZERO, Vec3::Y);
    for (metallic, roughness) in [(0.0, 1.0), (0.0, 0.3), (1.0, 0.1), (1.0, 0.7)] {
        let items = [sphere(Vec4::ONE, metallic, roughness)];
        let mut inputs = rendered(&view, &items);
        inputs.world = Some(color_world(Vec3::ONE));
        let p = center(&draw(&mut gpu, inputs));
        let l = linear(p[0]);
        assert!(
            (0.85..=1.05).contains(&l),
            "furnace metallic {metallic} roughness {roughness}: {l} ({p:?})"
        );
    }

    let items = [sphere(Vec4::ONE, 1.0, 0.2)];
    let mut inputs = rendered(&view, &items);
    inputs.world = Some(color_world(Vec3::new(1.0, 0.0, 0.0)));
    let red = center(&draw(&mut gpu, inputs));
    assert!(
        red[0] > 200 && red[1] < 30 && red[2] < 30,
        "metal reflects red: {red:?}"
    );
}

#[test]
fn bloom_spreads_bright_pixels_and_stays_local() {
    let Some(mut gpu) = gpu() else { return };
    let view = view_from(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, Vec3::Y);
    let mut cube = DrawItem::new(
        MeshKind::Cube,
        Mat4::from_scale(Vec3::splat(0.5)),
        Vec4::new(0.0, 0.0, 0.0, 1.0),
    );
    cube.emission = Vec3::splat(40.0);
    let items = [cube];
    let off = draw(&mut gpu, rendered(&view, &items));
    let mut inputs = rendered(&view, &items);
    inputs.bloom = BloomSettings {
        intensity: 0.3,
        ..BloomSettings::OFF
    };
    let on = draw(&mut gpu, inputs);

    let near = SIZE / 2 + 10;
    assert_eq!(
        &pixel(&off, near, SIZE / 2)[..3],
        &[0, 0, 0],
        "no bloom: black"
    );
    let glow = pixel(&on, near, SIZE / 2);
    assert!(glow[0] > 10, "bloom glows next to the cube: {glow:?}");
    let corner = pixel(&on, 1, 1);
    assert!(
        corner[0] < glow[0],
        "and fades with distance: {corner:?} vs {glow:?}"
    );
    // Bloom only adds light: the cube itself stays white, black stays non-negative.
    assert_eq!(&center(&on)[..3], &[255, 255, 255]);
}

/// A plane with a tilted normal map lit straight from above is darker than the same plane
/// with the flat default normal.
#[test]
fn normal_maps_change_shading() {
    let Some(mut gpu) = gpu() else { return };
    let tex_id = AssetId::<Texture>::from_raw(0, 0);
    // Tangent-space normal tilted 60° along +T: (sin 60°, 0, cos 60°) → RGB.
    let n = [0.866f32, 0.0, 0.5].map(|c| ((c * 0.5 + 0.5) * 255.0).round() as u8);
    let texture = Texture {
        format: PixelFormat::Rgba8,
        width: 4,
        height: 4,
        mips: vec![[n[0], n[1], n[2], 255].repeat(16)],
    };
    let mat_id = AssetId::<MaterialAsset>::from_raw(0, 0);
    let mut material = MaterialAsset {
        roughness: 1.0,
        metallic: 0.0,
        ..Default::default()
    };
    material.normal_texture = Some(SubRef {
        label: "Texture0".into(),
        id: Some(tex_id),
    });
    let mut uploads = vec![
        AssetUpload::Texture(tex_id, Arc::new(texture)),
        AssetUpload::Material(mat_id, Arc::new(material)),
    ];

    let view = top_down(3.0);
    let sun = light_at(LightKind::Sun, Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO);
    let lights = [sun];
    let mut shade = |strength: f32, uploads: Option<&mut Vec<AssetUpload>>| {
        let mut plane = floor(4.0);
        plane.material_asset = Some(mat_id);
        plane.normal_strength = strength;
        let items = [plane];
        let mut inputs = rendered(&view, &items);
        inputs.lights = &lights;
        inputs.asset_uploads = uploads;
        linear(center(&draw(&mut gpu, inputs))[0])
    };
    let mapped = shade(1.0, Some(&mut uploads));
    let flat = shade(0.0, None);
    assert!(flat > 0.1, "lit: {flat}");
    // N·L drops from 1 to cos 60° = 0.5.
    let ratio = mapped / flat;
    assert!(
        (0.4..0.62).contains(&ratio),
        "normal map tilts shading: {ratio}"
    );
}

#[test]
fn alpha_mask_cuts_holes_in_color_and_shadow() {
    let Some(mut gpu) = gpu() else { return };
    let mut view = view_from(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
    view.clear_color = [0.0, 1.0, 0.0];
    let card = |alpha: f32| {
        let mut item = DrawItem::new(
            MeshKind::Plane,
            Mat4::from_rotation_x(FRAC_PI_2) * Mat4::from_scale(Vec3::splat(5.0)),
            Vec4::new(1.0, 0.0, 0.0, alpha),
        );
        item.alpha = DrawAlpha::Mask(0.5);
        item.double_sided = true;
        item
    };
    let mut shade = |alpha: f32| {
        let items = [card(alpha)];
        let mut inputs = rendered(&view, &items);
        inputs.ambient = Vec3::ONE;
        center(&draw(&mut gpu, inputs))
    };
    let kept = shade(0.8);
    let cut = shade(0.2);
    assert!(
        kept[0] > 150 && kept[1] < 80,
        "alpha 0.8 ≥ 0.5 is drawn: {kept:?}"
    );
    assert!(
        cut[1] > 200 && cut[0] < 30,
        "alpha 0.2 shows the background: {cut:?}"
    );

    // A cut-out caster casts no shadow. The sun is tilted so the caster's shadow lands on
    // the floor at the screen center while the caster itself is drawn off-center.
    let view = top_down(6.0);
    let lights = [{
        let mut l = light_at(LightKind::Sun, Vec3::new(2.0, 3.0, 0.0), Vec3::ZERO);
        l.cast_shadow = true;
        l.radiance = Vec3::splat(3.0);
        l
    }];
    let mut shade = |alpha: f32| {
        let mut caster = DrawItem::new(
            MeshKind::Plane,
            Mat4::from_translation(Vec3::new(1.0, 1.5, 0.0)) * Mat4::from_scale(Vec3::splat(0.4)),
            Vec4::new(1.0, 1.0, 1.0, alpha),
        );
        caster.alpha = DrawAlpha::Mask(0.5);
        caster.double_sided = true;
        caster.shadow_double_sided = true;
        let items = [floor(5.0), caster];
        let mut inputs = rendered(&view, &items);
        inputs.lights = &lights;
        linear(center(&draw(&mut gpu, inputs))[0])
    };
    let solid = shade(1.0);
    let holed = shade(0.1);
    assert!(holed > 0.1, "cut caster: the floor stays lit ({holed})");
    assert!(
        solid < holed * 0.2,
        "solid caster shadows it ({solid} vs {holed})"
    );
}

/// Two half-transparent cards, given front-first: the renderer must sort them back to front
/// so the front (red) card ends on top.
#[test]
fn blended_objects_sort_back_to_front() {
    let Some(mut gpu) = gpu() else { return };
    let view = view_from(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y);
    let card = |z: f32, color: Vec3| {
        let mut item = DrawItem::new(
            MeshKind::Plane,
            Mat4::from_translation(Vec3::new(0.0, 0.0, z))
                * Mat4::from_rotation_x(FRAC_PI_2)
                * Mat4::from_scale(Vec3::splat(3.0)),
            color.extend(0.5),
        );
        item.alpha = DrawAlpha::Blend;
        item.double_sided = true;
        item.cast_shadow = false;
        item.roughness = 1.0;
        item
    };
    let items = [card(1.0, Vec3::X), card(0.0, Vec3::Z)];
    let mut inputs = rendered(&view, &items);
    inputs.ambient = Vec3::ONE;
    let p = center(&draw(&mut gpu, inputs));
    assert!(p[0] > p[2] + 20, "front red card composited last: {p:?}");
}
