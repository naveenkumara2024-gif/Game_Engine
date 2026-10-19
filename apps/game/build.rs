// Force-export the hybrid-GPU preference symbols from the executable (lore §6.4).
// `#[no_mangle]` alone does not put a symbol in an .exe's export table on MSVC.
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/EXPORT:NvOptimusEnablement,DATA");
        println!("cargo:rustc-link-arg-bins=/EXPORT:AmdPowerXpressRequestHighPerformance,DATA");
    }
}
