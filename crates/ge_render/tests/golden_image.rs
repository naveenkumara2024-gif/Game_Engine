//! Golden-image test (CLAUDE.md §10): render the demo scene headless at a fixed time and compare
//! it to a stored PNG with a tolerance. Different GPUs and drivers rasterize slightly
//! differently, so a few edge pixels may differ; the scene must not.
//!
//! Re-bless after an intentional visual change: `GE_BLESS=1 cargo test -p ge_render --test golden_image`.
//! Skips (passes with a message) when no GPU adapter is available.

use std::path::PathBuf;

use ge_render::demo::{demo_camera, demo_scene};
use ge_render::{RenderView, Renderer};
use ge_rhi::{Rhi, TextureDesc, TextureFormat, TextureUsage};
use ge_shader::ShaderLibrary;

const SIZE: u32 = 256;
/// Max per-channel difference (out of 255) before a pixel counts as different.
const CHANNEL_TOLERANCE: u8 = 3;
/// Fraction of pixels allowed to exceed the channel tolerance (edges, driver rounding).
const MAX_DIFFERENT_FRACTION: f64 = 0.005;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/mesh.png")
}

fn render_demo() -> Option<Vec<u8>> {
    // Surfaces GPU validation errors (logged by ge_rhi) in the test output.
    ge_core::log::init_logging();
    let mut rhi = match Rhi::new_headless() {
        Ok(rhi) => rhi,
        Err(e) => {
            eprintln!("skipping golden image test: no GPU ({e})");
            return None;
        }
    };
    eprintln!("golden image rendered on {:?}", rhi.adapter());
    let format = TextureFormat::Rgba8UnormSrgb;
    let target = rhi.create_texture(&TextureDesc {
        label: "GoldenTarget",
        size: (SIZE, SIZE),
        format,
        usage: TextureUsage::RENDER_ATTACHMENT | TextureUsage::COPY_SRC,
    });
    let mut renderer = Renderer::new(
        &rhi,
        format,
        ShaderLibrary::new(ShaderLibrary::default_root()),
    );
    assert!(
        renderer.has_pipeline(),
        "mesh shader failed to build; see log"
    );

    let view = RenderView {
        camera: demo_camera(),
        time: 1.0,
        clear_color: [0.10, 0.20, 0.40],
    };
    let mut frame = rhi.begin_offscreen_frame(&target);
    renderer.render(&rhi, &mut frame, &view, &demo_scene(view.time));
    rhi.submit_offscreen(frame);
    Some(rhi.read_texture_rgba8(&target).expect("readback"))
}

#[test]
fn demo_scene_matches_golden() {
    let Some(pixels) = render_demo() else { return };
    let path = golden_path();

    if std::env::var_os("GE_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &pixels, SIZE, SIZE, image::ExtendedColorType::Rgba8).unwrap();
        eprintln!("blessed {}", path.display());
        return;
    }

    let golden = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "missing golden {} ({e}); run with GE_BLESS=1 to create it",
                path.display()
            )
        })
        .to_rgba8();
    assert_eq!(
        golden.dimensions(),
        (SIZE, SIZE),
        "golden has the wrong size"
    );

    let different = pixels
        .chunks_exact(4)
        .zip(golden.as_raw().chunks_exact(4))
        .filter(|(a, b)| {
            a.iter()
                .zip(*b)
                .any(|(x, y)| x.abs_diff(*y) > CHANNEL_TOLERANCE)
        })
        .count();
    let fraction = different as f64 / f64::from(SIZE * SIZE);
    if fraction > MAX_DIFFERENT_FRACTION {
        let actual = path.with_file_name("mesh.actual.png");
        image::save_buffer(
            &actual,
            &pixels,
            SIZE,
            SIZE,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        panic!(
            "{different} pixels ({:.2}%) differ from the golden; wrote {}",
            fraction * 100.0,
            actual.display()
        );
    }
}
