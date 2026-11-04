//! Headless GPU checks for the passes added in plan 0004: the UI pass (scissor + textures),
//! lighting v0 and the grid overlay. These assert on specific pixels rather than comparing a
//! golden image, so they hold across GPUs and drivers. Skip when no GPU adapter is available.

use ge_render::lighting::{ColorManagement, LightItem, ViewportShading};
use ge_render::ui::{UiDrawList, UiMesh, UiTextureId, UiTextureUpdate, UiVertex};
use ge_render::{
    Camera, DrawItem, FrameInputs, Light, LightKind, MeshKind, Overlays, Primitive, PrimitiveMesh,
    RenderView, Renderer,
};
use ge_rhi::{Rhi, Texture, TextureDesc, TextureFormat, TextureUsage};
use ge_shader::ShaderLibrary;
use glam::{Mat4, Vec3, Vec4};

const SIZE: u32 = 64;

struct Gpu {
    rhi: Rhi,
    target: Texture,
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
        label: "PassTarget",
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

fn black_view() -> RenderView {
    RenderView {
        camera: Camera::default(),
        time: 0.0,
        clear_color: [0.0; 3],
    }
}

fn quad(color: [u8; 4], uv: bool) -> (Vec<UiVertex>, Vec<u32>) {
    let s = SIZE as f32;
    let v = |x: f32, y: f32, u: f32, w: f32| UiVertex {
        pos: [x, y],
        uv: if uv { [u, w] } else { [0.0, 0.0] },
        color,
    };
    (
        vec![
            v(0.0, 0.0, 0.0, 0.0),
            v(s, 0.0, 1.0, 0.0),
            v(s, s, 1.0, 1.0),
            v(0.0, s, 0.0, 1.0),
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

#[test]
fn ui_pass_clips_to_scissor_and_samples_textures() {
    let Some(mut gpu) = gpu() else { return };
    let half = SIZE / 2;

    // A full-screen red quad clipped to the left half.
    let (vertices, indices) = quad([255, 0, 0, 255], false);
    let mut ui = UiDrawList {
        meshes: vec![UiMesh {
            clip: [0, 0, half, SIZE],
            texture: None,
            vertices,
            indices,
        }],
        textures: Vec::new(),
    };
    let view = black_view();
    let mut inputs = FrameInputs::basic(&view, &[]);
    inputs.ui = Some(&mut ui);
    let pixels = draw(&mut gpu, inputs);
    assert_eq!(pixel(&pixels, 4, half), [255, 0, 0, 255], "inside the clip");
    assert_eq!(
        pixel(&pixels, SIZE - 4, half),
        [0, 0, 0, 255],
        "outside the clip"
    );

    // A 2×1 texture (green | blue), nearest-filtered, white vertex color; clip covers it all.
    let id = UiTextureId(7);
    let (vertices, indices) = quad([255; 4], true);
    let mut ui = UiDrawList {
        meshes: vec![UiMesh {
            clip: [0, 0, 10_000, 10_000], // clamped to the target
            texture: Some(id),
            vertices,
            indices,
        }],
        textures: vec![UiTextureUpdate::Set {
            id,
            origin: None,
            size: [2, 1],
            pixels: vec![0, 255, 0, 255, 0, 0, 255, 255],
            linear_filter: false,
        }],
    };
    let mut inputs = FrameInputs::basic(&view, &[]);
    inputs.ui = Some(&mut ui);
    let pixels = draw(&mut gpu, inputs);
    assert!(
        ui.textures.is_empty(),
        "texture updates are drained once applied"
    );
    assert_eq!(pixel(&pixels, 4, half), [0, 255, 0, 255]);
    assert_eq!(pixel(&pixels, SIZE - 4, half), [0, 0, 255, 255]);

    // Half-transparent premultiplied white over black blends to mid-grey (sRGB ≈ 188 for
    // linear 0.5 after the decode → blend → encode round trip, egui's sRGB-target behavior).
    let (vertices, indices) = quad([128, 128, 128, 128], false);
    let mut ui = UiDrawList {
        meshes: vec![UiMesh {
            clip: [0, 0, SIZE, SIZE],
            texture: None,
            vertices,
            indices,
        }],
        textures: Vec::new(),
    };
    let mut inputs = FrameInputs::basic(&view, &[]);
    inputs.ui = Some(&mut ui);
    let p = pixel(&draw(&mut gpu, inputs), half, half);
    assert!(p[0] > 40 && p[0] < 200 && p[0] == p[1], "{p:?}");
}

/// A smooth sphere in front of the camera, lit in Rendered mode.
fn lit_sphere(gpu: &mut Gpu, lights: &[LightItem], ambient: Vec3) -> [u8; 4] {
    let view = RenderView {
        camera: Camera {
            position: Vec3::new(0.0, 0.0, 4.0),
            target: Vec3::ZERO,
            ..Default::default()
        },
        time: 0.0,
        clear_color: [0.0; 3],
    };
    let mut item = DrawItem::new(
        MeshKind::Primitive(PrimitiveMesh::new(Primitive::UvSphere {
            segments: 32,
            rings: 16,
            radius: 1.0,
        })),
        Mat4::IDENTITY,
        Vec4::splat(0.8),
    );
    item.roughness = 0.6;
    let items = [item];
    let mut inputs = FrameInputs::basic(&view, &items);
    inputs.lights = lights;
    inputs.ambient = ambient;
    inputs.shading = ViewportShading::Rendered;
    inputs.color = ColorManagement::default();
    pixel(&draw(gpu, inputs), SIZE / 2, SIZE / 2)
}

fn light_at(kind: LightKind, position: Vec3, color: Vec3) -> LightItem {
    let mut light = Light::new(kind);
    light.color = color;
    let transform = ge_ecs::Transform::from_translation(position).looking_at(Vec3::ZERO, Vec3::Y);
    let global = ge_ecs::GlobalTransform(transform.to_affine());
    LightItem::from_light(&light, &global)
}

#[test]
fn rendered_mode_uses_scene_lights_and_world() {
    let Some(mut gpu) = gpu() else { return };

    let dark = lit_sphere(&mut gpu, &[], Vec3::ZERO);
    assert_eq!(&dark[..3], &[0, 0, 0], "no lights, no world: black");

    let ambient = lit_sphere(&mut gpu, &[], Vec3::splat(0.2));
    assert!(
        ambient[0] > 30 && ambient[0] == ambient[1],
        "world ambient: {ambient:?}"
    );

    // A red 1000 W point light 3 m in front of the sphere's facing point.
    let red = light_at(LightKind::Point, Vec3::new(0.0, 0.0, 4.0), Vec3::X);
    let lit = lit_sphere(&mut gpu, &[red], Vec3::ZERO);
    assert!(
        lit[0] > 150 && lit[1] < 10 && lit[2] < 10,
        "red point light: {lit:?}"
    );

    // A white sun from the camera side plus the red point: red channel saturates, others rise.
    let sun = light_at(LightKind::Sun, Vec3::new(0.0, 0.0, 10.0), Vec3::ONE);
    let both = lit_sphere(&mut gpu, &[red, sun], Vec3::ZERO);
    assert!(both[1] > 40 && both[0] >= lit[0], "sun + point: {both:?}");

    // A spot aimed away from the sphere contributes nothing.
    let mut away = light_at(LightKind::Spot, Vec3::new(0.0, 0.0, 4.0), Vec3::ONE);
    away.direction = Vec3::Z;
    let spot = lit_sphere(&mut gpu, &[away], Vec3::ZERO);
    assert_eq!(&spot[..3], &[0, 0, 0], "spot cone masks: {spot:?}");
}

#[test]
fn grid_overlay_draws_on_the_ground_plane() {
    let Some(mut gpu) = gpu() else { return };
    let view = RenderView {
        camera: Camera {
            position: Vec3::new(0.3, 5.0, 0.3),
            target: Vec3::new(0.3, 0.0, 0.0),
            up: Vec3::Z,
            ..Default::default()
        },
        time: 0.0,
        clear_color: [0.0; 3],
    };
    let mut inputs = FrameInputs::basic(&view, &[]);
    let without = draw(&mut gpu, FrameInputs::basic(&view, &[]));
    inputs.overlays = Overlays { grid: true };
    let with = draw(&mut gpu, inputs);
    assert!(without.chunks_exact(4).all(|p| p[..3] == [0, 0, 0]));
    let lit = with.chunks_exact(4).filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(lit > 50, "grid lines cover some pixels ({lit})");
    assert!(
        lit < (SIZE * SIZE) as usize / 2,
        "but not the whole floor ({lit})"
    );
}

#[test]
fn material_asset_textures_are_sampled() {
    use ge_asset::{AssetId, MaterialAsset, PixelFormat, SubRef, Texture};
    use ge_render::assets::AssetUpload;
    use std::sync::Arc;

    let Some(mut gpu) = gpu() else { return };
    // 64×1 texture: left half red, right half green (sRGB), one level. Wide enough that
    // bilinear filtering only blends right at the middle.
    let tex_id = AssetId::<Texture>::from_raw(0, 0);
    let row: Vec<u8> = (0..64)
        .flat_map(|x| {
            if x < 32 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            }
        })
        .collect();
    let texture = Texture {
        format: PixelFormat::Rgba8Srgb,
        width: 64,
        height: 1,
        mips: vec![row],
    };
    let mat_id = AssetId::<MaterialAsset>::from_raw(0, 0);
    let mut material = MaterialAsset {
        metallic: 0.0,
        roughness: 1.0,
        ..Default::default()
    };
    material.base_color_texture = Some(SubRef {
        label: "Texture0".into(),
        id: Some(tex_id),
    });
    let mut uploads = vec![
        AssetUpload::Texture(tex_id, Arc::new(texture)),
        AssetUpload::Material(mat_id, Arc::new(material)),
    ];

    // A plane facing the camera, filling the view; lit only by a white world.
    let view = RenderView {
        camera: Camera {
            position: Vec3::new(0.0, 3.0, 0.0),
            target: Vec3::ZERO,
            up: Vec3::NEG_Z,
            ..Default::default()
        },
        time: 0.0,
        clear_color: [0.0; 3],
    };
    let mut item = DrawItem::new(
        MeshKind::Plane,
        Mat4::from_scale(Vec3::splat(4.0)),
        Vec4::ONE,
    );
    item.roughness = 1.0;
    item.material_asset = Some(mat_id);
    let items = [item];
    let mut inputs = FrameInputs::basic(&view, &items);
    inputs.shading = ViewportShading::Rendered;
    inputs.ambient = Vec3::ONE;
    inputs.asset_uploads = Some(&mut uploads);
    let pixels = draw(&mut gpu, inputs);
    assert!(uploads.is_empty(), "uploads drained");
    assert_eq!(gpu.renderer.gpu_asset_counts(), (0, 1, 1));
    // The plane's U runs along world Z, which is screen-vertical here (camera up = −Z):
    // U = 0 (red) is at the top of the image, U = 1 (green) at the bottom.
    let top = pixel(&pixels, SIZE / 2, 8);
    let bottom = pixel(&pixels, SIZE / 2, SIZE - 8);
    assert!(top[0] > 150 && top[1] < 60, "top half samples red: {top:?}");
    assert!(
        bottom[1] > 150 && bottom[0] < 60,
        "bottom half samples green: {bottom:?}"
    );
}
