use ge_engine::prelude::*;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Ask hybrid-GPU drivers for the discrete GPU (lore §2.6); `build.rs` exports these.
// SAFETY: plain immutable statics with the exact names and types the NVIDIA/AMD drivers read.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub static NvOptimusEnablement: u32 = 1;
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub static AmdPowerXpressRequestHighPerformance: i32 = 1;

fn main() -> anyhow::Result<()> {
    App::new()
        .set_window(WindowDesc {
            title: "Game_Engine Editor".into(),
            width: 1600,
            height: 900,
            ..Default::default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(ge_editor::EditorPlugin)
        .add_plugins(sample::SamplePlugin)
        .run()?;
    Ok(())
}
