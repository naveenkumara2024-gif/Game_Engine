# 0001 — M0: Workspace & window

**Status:** done 2026-10-19 · **Milestone:** M0 (CLAUDE.md §9, lore §8)

**Deviations from this plan:**
- The RHI is built from opaque types (`Rhi`, `Swapchain`, `Frame`) that never expose a `wgpu` type, not a `dyn` trait. The seam is the same; a trait gets added when a second backend exists.
- Git items (`.gitignore`, CI workflow) were dropped at the user's request.
- The minimized-window tick sleeps 16 ms; it was busy-spinning at about 400k ticks/s.
- The discrete GPU comes up on Vulkan, not D3D12, because wgpu enumerates Vulkan first.

## Goal

Stand up the cargo workspace, the tooling and the first vertical thread through the layers. M0 is done when a window opens and **`ge_rhi` clears it to a color on the RTX 3050**, with the chosen adapter logged, `tracing` wired up (Tracy optional), a cvar registry stub, and one command that runs every check.

## Lore read

§0 (laws, stack), §2.2 (engine owns the entry point), §2.6 (GPU policy exports), §6.2 (layout and layering), §6.3 (winit loop sketch), §6.4 (handles, cvars, allocator, discrete GPU), §6.11 (profiles, Windows linker), §7 (pitfalls), §9 (machine).

## Current state

The folder holds only `lore.md`, `CLAUDE.md` and this plan. It is **not a git repository**. Installed: Rust 1.97.1 (MSVC), clippy, rustfmt, Miri. Not installed: Tracy GUI, cargo-nextest, cargo-deny, Vulkan SDK.

## Decisions & assumptions (flagged)

1. **Create only the crates M0 needs, not all 18 stubs.** Empty crates add compile time and noise. Each later milestone adds its own crates. M0 crates:
   - `ge_core`: `Handle<T>`/`Pool<T>`, string interner (stub), typed `CVar<T>` + registry (via `inventory`), `init_logging()` (`tracing-subscriber`, optional `tracing-tracy` behind a `tracy` feature), engine error type.
   - `ge_platform`: winit 0.30 wrapper: window creation, window/input event forwarding, `raw-window-handle` access. Nothing else.
   - `ge_rhi`: a **thin RHI trait** (`Device`, `Surface`, a frame begin/end flow and a `clear` pass), plus a wgpu v30 backend. Requests `PowerPreference::HighPerformance`, logs the adapter name, backend, driver and device type, and fails loudly if a software or integrated adapter is picked while a discrete one exists. The swapchain handles resize and minimize (zero-size surfaces are skipped).
   - `ge_engine`: **minimal** `Engine` + `run()` that owns the winit `ApplicationHandler`, the loop and the cooperative `request_exit(reason)` flag (law: the engine owns the entry point). `App`/`Plugin`/schedules wait for M2.
   - `apps/game`: thin binary that calls `ge_engine::run(config)`. Its `build.rs` force-exports `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` (`#[unsafe(no_mangle)]` statics + `/EXPORT:` linker args). The `#![windows_subsystem = "windows"]` line is gated to the future `shipping` feature.
   - `xtask`: `cargo xtask ci` runs fmt-check → clippy `-D warnings` → test → `check-layers`. `check-layers` reads `cargo metadata` and fails if a dependency edge breaks the allowed direction in CLAUDE.md §5.
2. **Clear color comes from a cvar** (`r.ClearColor`, a string `"r,g,b"`, read each frame). This proves the cvar registry works end to end. A runtime console comes later, so for now cvars can be set on the command line: `game.exe -cvar r.ClearColor=0.1,0.2,0.4`.
3. **`mimalloc` as `#[global_allocator]` lives in `apps/game`**, not a library crate. `ge_alloc` waits until frame arenas are needed.
4. **Tracy:** `tracing-tracy` sits behind a `tracy` feature, off by default. A `Frame` span and a `Render::Clear` span are emitted every frame. I can't verify a Tracy connection because the GUI isn't installed, so the test steps cover that manually.
5. **GPU timestamps** are part of M1, not M0, matching the roadmap.
6. **Crate prefix stays `ge_`** until you name the engine. Renaming later is a mechanical find-and-replace.
7. **Lints:** a `[workspace.lints]` block with `unsafe_code = "deny"`. Only `apps/game` (GPU exports) and `ge_rhi` (if wgpu surface creation needs it) opt out locally, each with a `// SAFETY:` comment. Clippy runs at the `-D warnings` default level; pedantic stays off for now.
8. **Profiles and linker:** the profiles from lore §6.11 (`dev`, `release`, `shipping`) go in the root `Cargo.toml`. `.cargo/config.toml` sets `linker = "rust-lld"` for `x86_64-pc-windows-msvc`. If rust-lld breaks the `/EXPORT` args, I fall back to `link.exe` and record why.
9. **CI and git:** I add `.github/workflows/ci.yml` (windows-latest, runs `cargo xtask ci`) and a `.gitignore`. **I will not run `git init` unless you say so.** I'll leave that to you.
10. **Edition 2024, `rust-version = "1.97"`, license `MIT OR Apache-2.0`** in `[workspace.package]` (provisional, CLAUDE.md §11).

## Files to create

```
Cargo.toml                       # workspace, [workspace.package|dependencies|lints], profiles
.cargo/config.toml               # rust-lld, xtask alias
rustfmt.toml  .gitignore  .github/workflows/ci.yml
crates/ge_core/{Cargo.toml, src/lib.rs, src/handle.rs, src/cvar.rs, src/log.rs, src/name.rs}
crates/ge_platform/{Cargo.toml, src/lib.rs}
crates/ge_rhi/{Cargo.toml, src/lib.rs, src/wgpu_backend.rs}
crates/ge_engine/{Cargo.toml, src/lib.rs}
apps/game/{Cargo.toml, build.rs, src/main.rs}
xtask/{Cargo.toml, src/main.rs}
```

At the end, update CLAUDE.md §9 status to "M0 done". Fix §10 only if the commands change.

## Dependencies added (all in the CLAUDE.md §4 table, or tooling)

`wgpu 30`, `winit 0.30`, `raw-window-handle 0.6`, `glam`, `tracing`, `tracing-subscriber`, `tracing-tracy` (optional), `inventory`, `thiserror`, `anyhow` (apps/xtask only), `mimalloc`, `pollster` (blocking on wgpu adapter/device requests at init only), `cargo_metadata` (xtask).

## Risks and safety

- Without the Vulkan SDK there are no validation layers. wgpu's own validation stays on in dev, and D3D12 is the default backend on Windows.
- Hybrid GPU: if the adapter check finds the 680M despite HighPerformance + exports, startup logs an error. An `r.AllowIntegratedGpu=1` override lets the 680M be tested on purpose.
- No `unsafe` outside the two flagged spots.

## Acceptance criteria

- [ ] `cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, check-layers.
- [ ] `cargo run -p game` opens a resizable window cleared to the `r.ClearColor` color. The log shows the **NVIDIA GeForce RTX 3050 Laptop GPU** adapter and its backend.
- [ ] Resize and minimize/restore don't crash or spam errors. Closing the window exits through `request_exit`.
- [ ] `dumpbin /exports target\debug\game.exe` lists `NvOptimusEnablement` and `AmdPowerXpressRequestHighPerformance` (if `dumpbin` is on PATH through VS Build Tools).
- [ ] Unit tests: `Pool`/`Handle` (stale handles return `None`), cvar parse/set/get, check-layers rejects a deliberately inverted edge (test fixture).
- [ ] `cargo build -p game --features tracy` compiles.

## Manual test steps

1. `cargo run -p game`. Expect a colored window and an adapter log line naming the RTX 3050.
2. `cargo run -p game -- -cvar r.ClearColor=1,0,0`. Expect a red window.
3. Drag-resize, minimize, restore, close. Expect no panics, and a clean "exit requested: window closed" log line.
4. Optional: install the Tracy GUI, `cargo run -p game --features tracy`, connect, and look for the `Frame` and `Render::Clear` zones.
