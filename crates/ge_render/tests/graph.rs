//! Headless checks that the renderer's frame really goes through the render graph (plan 0008
//! step 1): which passes run and which are culled, and what the transients cost. The pixel
//! output is covered by `pbr.rs`, which runs unchanged on the graph. Skip without a GPU.

use ge_render::lighting::{ColorManagement, LightItem, ViewportShading};
use ge_render::{
    BloomSettings, Camera, DrawItem, FrameInputs, Light, LightKind, MeshKind, RenderView, Renderer,
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
        label: "GraphTarget",
        size: (SIZE, SIZE),
        format,
        usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
    });
    let renderer = Renderer::new(
        &rhi,
        format,
        ShaderLibrary::new(ShaderLibrary::default_root()),
    );
    assert!(renderer.missing_pipelines().is_empty());
    Some(Gpu {
        rhi,
        target,
        renderer,
    })
}

fn draw(gpu: &mut Gpu, inputs: FrameInputs<'_>) {
    let mut frame = gpu.rhi.begin_offscreen_frame(&gpu.target);
    gpu.renderer.render_frame(&gpu.rhi, &mut frame, inputs);
    gpu.rhi.submit_offscreen(frame);
}

fn view() -> RenderView {
    RenderView {
        camera: Camera {
            position: Vec3::new(0.0, 4.0, 6.0),
            target: Vec3::ZERO,
            ..Default::default()
        },
        time: 0.0,
        clear_color: [0.0; 3],
    }
}

fn scene() -> [DrawItem; 2] {
    [
        DrawItem::new(
            MeshKind::Plane,
            Mat4::from_scale(Vec3::splat(5.0)),
            Vec4::ONE,
        ),
        DrawItem::new(MeshKind::Cube, Mat4::from_translation(Vec3::Y), Vec4::ONE),
    ]
}

fn sun() -> LightItem {
    let mut light = Light::new(LightKind::Sun);
    light.cast_shadow = true;
    let t = ge_ecs::Transform::from_translation(Vec3::new(2.0, 5.0, 1.0))
        .looking_at(Vec3::ZERO, Vec3::Y);
    LightItem::from_light(&light, &ge_ecs::GlobalTransform(t.to_affine()))
}

fn count(passes: &[&str], name: &str) -> usize {
    passes.iter().filter(|p| **p == name).count()
}

#[test]
fn solid_frame_without_bloom_culls_the_bloom_chain() {
    let Some(mut gpu) = gpu() else { return };
    let (view, items) = (view(), scene());
    draw(&mut gpu, FrameInputs::basic(&view, &items));
    let r = gpu.renderer.last_graph_report();
    assert_eq!(
        r.executed,
        ["DepthPrepass", "LightCulling", "Opaque", "Tonemap"]
    );
    // Declared every frame, culled because the tonemap doesn't read bloom at intensity 0.
    assert!(count(&r.culled, "Bloom.Down") > 0 && count(&r.culled, "Bloom.Up") > 0);
    assert!(
        r.culled.iter().all(|p| p.starts_with("Bloom.")),
        "{:?}",
        r.culled
    );
    // Only SceneDepth + SceneColor are allocated.
    assert_eq!((r.virtual_textures, r.physical_textures), (2, 2));
}

#[test]
fn rendered_frame_with_sun_and_bloom_runs_everything() {
    let Some(mut gpu) = gpu() else { return };
    let (view, items) = (view(), scene());
    let lights = [sun()];
    let mut inputs = FrameInputs::basic(&view, &items);
    inputs.shading = ViewportShading::Rendered;
    inputs.color = ColorManagement::default();
    inputs.lights = &lights;
    inputs.bloom = BloomSettings {
        intensity: 0.1,
        ..BloomSettings::OFF
    };
    draw(&mut gpu, inputs);
    let r = gpu.renderer.last_graph_report().clone();
    assert!(r.culled.is_empty(), "nothing to cull: {:?}", r.culled);
    assert!(
        count(&r.executed, "Shadows.Cascade") >= 1,
        "{:?}",
        r.executed
    );
    let first_shadow = r.executed.iter().position(|p| p.starts_with("Shadows."));
    let opaque = r.executed.iter().position(|p| *p == "Opaque");
    assert!(
        first_shadow < opaque,
        "shadows render before they're sampled"
    );
    // A 64² frame: bloom has 6 levels (32² … 1²). Levels have different sizes and are all
    // live until the upsample chain ends, so today's frame has nothing to alias; the
    // mechanism is covered by the graph's unit tests.
    let up = count(&r.executed, "Bloom.Up");
    assert_eq!((count(&r.executed, "Bloom.Down"), up), (6, 5));
    assert_eq!(r.virtual_textures, 2 + 6);
    assert_eq!(r.physical_textures, r.virtual_textures);
    assert_eq!(r.executed.last(), Some(&"Tonemap"));
}

#[test]
fn steady_state_frames_reuse_the_same_transients() {
    let Some(mut gpu) = gpu() else { return };
    let (view, items) = (view(), scene());
    for _ in 0..3 {
        draw(&mut gpu, FrameInputs::basic(&view, &items));
    }
    let r = gpu.renderer.last_graph_report();
    assert_eq!(r.physical_bytes, r.virtual_bytes);
    // SceneDepth (Depth32) + SceneColor (Rgba16Float) at 64².
    assert_eq!(r.physical_bytes, u64::from(SIZE * SIZE) * (4 + 8));
    assert_eq!(gpu.renderer.transient_pool_len(), 2, "no per-frame growth");
}

/// Plan 0008 step 2: the render world keeps proxies on the GPU. A static scene uploads no
/// object data and rebuilds no draw bins; moving one object rewrites one slot.
#[test]
fn render_world_uploads_only_changes() {
    use ge_render::{ProxyKey, SceneDelta};
    let Some(mut gpu) = gpu() else { return };
    let view = view();
    let items = scene();
    let frame = |gpu: &mut Gpu, delta: &mut SceneDelta| {
        let mut inputs = FrameInputs::basic(&view, &[]);
        inputs.scene = Some(delta);
        draw(gpu, inputs);
        gpu.renderer.scene_stats()
    };

    let mut delta = SceneDelta::default();
    for (i, item) in items.iter().enumerate() {
        delta.upsert(ProxyKey(i as u64), *item);
    }
    let first = frame(&mut gpu, &mut delta);
    assert!(delta.is_empty(), "the renderer drains the delta");
    assert_eq!(first.proxies, 2);
    assert_eq!(first.object_writes, 1, "both new slots in one write");
    assert_eq!(first.bin_rebuilds, 1);

    let still = frame(&mut gpu, &mut delta);
    assert_eq!(
        still.object_bytes_written, 0,
        "static scene: nothing uploaded"
    );
    assert_eq!(still.bin_rebuilds, 1, "static scene: bins reused");

    let mut moved = items[1];
    moved.world_from_local = Mat4::from_translation(Vec3::new(2.0, 1.0, 0.0));
    delta.upsert(ProxyKey(1), moved);
    let after = frame(&mut gpu, &mut delta);
    assert_eq!(after.object_writes, 1);
    assert!(after.object_bytes_written > 0 && after.object_bytes_written <= 256);
    assert_eq!(after.bin_rebuilds, 1, "a move doesn't touch the bins");

    delta.remove(ProxyKey(0));
    let removed = frame(&mut gpu, &mut delta);
    assert_eq!((removed.proxies, removed.bin_rebuilds), (1, 2));

    // The immediate path (tests, tools) diffs the slice the same way.
    draw(&mut gpu, FrameInputs::basic(&view, &items));
    draw(&mut gpu, FrameInputs::basic(&view, &items));
    assert_eq!(gpu.renderer.scene_stats().object_bytes_written, 0);
}
