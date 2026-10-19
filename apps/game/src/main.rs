#![cfg_attr(feature = "shipping", windows_subsystem = "windows")]

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
    ge_engine::run(ge_engine::EngineConfig {
        window: ge_engine::WindowConfig {
            title: "Game_Engine".into(),
            ..Default::default()
        },
        args: std::env::args().collect(),
    })?;
    Ok(())
}
