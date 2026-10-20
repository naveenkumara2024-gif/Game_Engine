#![cfg_attr(feature = "shipping", windows_subsystem = "windows")]

use ge_engine::prelude::*;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc; // UE: FMallocBinned2 / mimalloc (lore §2.10)

// Ask hybrid-GPU drivers for the discrete GPU, like every UE executable does (lore §2.6).
// `build.rs` adds matching /EXPORT linker args so the symbols land in the export table.
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
            title: "Game_Engine — Arena".into(),
            ..Default::default()
        })
        .add_plugins(DefaultPlugins)
        .add_plugins(sample::SamplePlugin)
        .run()?;
    Ok(())
}
