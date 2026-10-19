# CLAUDE.md — Game_Engine

An **Unreal-style, modular, editor-driven 3D game engine in Rust**. You are a principal-level engine engineer working on it.

> **Precedence:** this file governs everything inside `Game_Engine/`. The `c:\Users\navas\Downloads\AGENTS.md` that also loads here belongs to a **different project** (the Autonomous AI Desktop Agent: Electron, LangGraph, sessions, approval gates). **Ignore it** while working in this repo. None of its stack, layout, safety rules or checks apply here.

**Background knowledge lives in [lore.md](lore.md).** It holds the Unreal anatomy, the REA findings from shipped UE5 games, the Rust ecosystem survey (Oct 2026), the architecture blueprint and the roadmap. This file holds the **rules and decisions**. When the two disagree, this file wins. Point to lore sections (`lore §6.7`) instead of copying them.

---

## 1. What we are building

- **Strategy B** (lore §6.1): a custom engine on best-of-breed crates. We own the engine loop, the RHI, the render graph, assets, cooking and the editor. We use `wgpu`, `winit`, `glam`, `bevy_ecs`, `egui`, `rapier3d` and `kira` **behind our own APIs**, so any of them can be replaced (that is how we keep a path open to Strategy C).
- **Target: Tier T1, then T2** (lore §1): a small 3D engine, then a production engine with a full editor, cooking, animation, UI, hot reload, profiling and crash reporting. T3 features (virtual geometry, dynamic GI, virtual shadow maps) come only after M12.
- **Build an engine for one game.** A vertical-slice sample game in `games/sample/` defines "done" for every subsystem. Do not build a general-purpose engine for its own sake. That is the #1 failure mode (lore §1, §4.4).
- **Copy Unreal's shape, not its size.** UE5 shipping binaries hold 140–170 MB of code. We copy the layering, the threading model, reflection, cooking and plugin seams, then grow features one vertical slice at a time.

---

## 2. How to work

For every request:

1. Read this file, then the lore sections for the subsystem you're touching (the map is in §12 below).
2. Look at the existing crates before assuming an API's shape. The workspace grows milestone by milestone, so check what actually exists.
3. **Non-trivial work** (a new crate, a new subsystem, a milestone step, a public API change, a new dependency): write a short plan in `docs/plans/NNNN-<slug>.md` and get approval **before writing code**. The plan covers the goal, the crates touched, the lore sections read, the decisions and assumptions, the files to touch, the acceptance criteria tied to milestone exit criteria (§9) and the manual test steps.
4. **Small work** (bug fixes, local refactors, docs, tests): just do it.
5. Ask one focused question only if the task is genuinely ambiguous. Otherwise pick the default from this file and flag the assumption.
6. Build strictly to the approved plan. Run the checks in §10 and report their real output.
7. Close with **What I did** / **Test** / **Needs your attention**, as bullets.

---

## 3. The 12 laws (non-negotiable)

Each one is backed by evidence in lore §2–§3.

1. **Dependencies point down only.** Core knows nothing about rendering. The editor depends on the runtime, never the reverse.
2. **Game state and render state are separate data.** They sync at exactly one **extract** point per frame.
3. **Frames are pipelined across threads, and latency is bounded.** The main thread waits if the render thread falls more than one frame behind.
4. **Reflect everything** that the editor, serializer, network or scripts must see: `#[derive(Reflect)]`.
5. **Use handles, not pointers.** Generational IDs into pools, arenas or the ECS. No `Rc<RefCell<…>>` object webs.
6. **The render graph owns GPU resource lifetimes,** plus barriers, transient aliasing and pass culling.
7. **Cook offline.** The runtime never parses editor or source formats. Dependency graphs are precomputed.
8. **Everything is tunable at runtime** through typed cvars and scalability tiers. Debug features are stripped in shipping.
9. **Features are plugins.** Games are plugins too.
10. **Instrument from day one:** `tracing` spans, Tracy, GPU timestamps, memory tags.
11. **Keep a crash-safe path:** a panic hook, an emergency arena, minidumps and an out-of-process reporter.
12. **Only `ge_rhi` touches the graphics API.** Renderer code never calls `wgpu` (or D3D12/Vulkan) directly.

---

## 4. Locked stack (defaults)

Change any of these only through an approved plan.

| Concern | Choice |
|---|---|
| Language | Rust **edition 2024**, toolchain 1.97.1 stable, MSVC (`x86_64-pc-windows-msvc`) |
| Window / input | `winit` 0.30.x (`ApplicationHandler`), `gilrs` |
| GPU | `wgpu` v30 **behind the `ge_rhi` trait**. Request `PowerPreference::HighPerformance`. |
| Shaders | WGSL + **WESL** (`@if` permutations), compiled with `naga`/`wesl`; files in `shaders/*.wesl` |
| ECS | `bevy_ecs` standalone, wrapped by `ge_ecs` (never re-export raw bevy types from public APIs) |
| Math | `glam` |
| Reflection | `bevy_reflect` + our derive macros in `ge_reflect_derive` |
| Serialization | `serde` + `ron` (authoring), `rkyv` (cooked), `postcard` (compact binary) |
| Parallelism | `rayon`, `crossbeam` channels, `parking_lot`; a small async executor for IO only |
| Physics | `rapier3d` behind the `PhysicsBackend` trait |
| Audio | `kira` on a dedicated audio thread |
| Editor UI | `egui` + `egui_dock` + `transform-gizmo-egui`, in-process |
| Game UI | our own retained UI + `taffy` layout + `cosmic-text`/`parley` |
| Assets in | `gltf`, `image`, `ktx2`, `meshopt`; `notify` for hot reload; `blake3` for the DDC key |
| Profiling | `tracing` + `tracing-tracy` |
| Allocator | `mimalloc` as `#[global_allocator]`; `bumpalo` frame arenas |
| Crash | panic hook + Embark `crash-handler`/`minidumper`; symbolicate with `rust-minidump` |
| Hot reload | `subsecond` (dev only) + asset and shader hot reload |
| Handles | `slotmap` / our own `Pool<T>` + `Handle<T>` |

Before adding any dependency outside this table, state why in the plan. Prefer crates that are maintained, that don't pull a second copy of `wgpu`/`winit`, and that are MIT/Apache licensed.

---

## 5. Workspace layout and layering

The crate prefix is `ge_` (a placeholder until the engine is named; see §11). The full layout with per-crate responsibilities is in lore §6.2.

```
Game_Engine/
├── Cargo.toml          # [workspace], [workspace.dependencies], profiles
├── crates/ge_core, ge_alloc, ge_platform, ge_tasks, ge_reflect(+_derive), ge_ecs,
│          ge_asset, ge_rhi, ge_shader, ge_render, ge_physics, ge_audio, ge_anim,
│          ge_ui, ge_net, ge_script, ge_engine, ge_editor
├── apps/game, editor, cooker, crash_reporter     # thin binaries
├── games/sample/       # vertical-slice game, as a plugin crate
├── assets/  shaders/  xtask/  docs/
└── lore.md  CLAUDE.md
```

**Allowed dependency direction** (CI enforces it):

```
ge_core ← ge_alloc ← ge_platform ← ge_tasks ← ge_reflect ← ge_ecs ← ge_asset
ge_rhi (← ge_core, ge_platform) ← ge_shader ← ge_render (← ge_ecs, ge_asset)
ge_physics / ge_audio / ge_anim / ge_ui / ge_net / ge_script  (← ge_ecs, ge_asset, …)
ge_engine (← all runtime crates) ← ge_editor (← ge_engine) ;  apps/* at the top
```

- **Never:** runtime crate → `ge_editor` or any `*_editor` crate; `ge_render` → `ge_editor`; `ge_core` → any other `ge_*` crate.
- `ge_core` + `ge_reflect` + `ge_ui` must build and run **without** `ge_render`/`ge_engine`. Tools (cooker, crash reporter) are small programs on the lower layers (lore §2.13).
- Crates only add dependencies through `[workspace.dependencies]`, so every crate uses the same version.
- `apps/*` binaries stay thin: parse args, build the `App`, add plugins, run.

---

## 6. Runtime architecture rules

**Engine loop** (lore §3.3, §6.3):

- `ge_engine` owns startup → init → loop → shutdown. Games never write their own `main` loop.
- Startup runs in ordered phases: config/cvars → logging/tracing → tasks/threads → plugins (by declared dependencies) → RHI + global shaders → worlds.
- Exit is cooperative: `request_exit(reason)` sets a flag that the loop polls. A forced terminate is a separate path.

**Threads** (lore §2.5, §3.6):

- The **main thread** owns winit events and simulation.
- A **render thread** consumes an extracted `RenderSnapshot`.
- An **audio thread** runs separately. **IO** runs on an async executor. Everything else is a task on the `rayon`-backed pool.
- Threads exchange **commands and snapshots** over channels. They never share mutable state.

**Frame order** (one tick):

```
PrePhysics → Physics (fixed step) → PostPhysics → Animation → PostUpdate (transform propagation) → Extract
```

**ECS:**

- Systems declare read/write sets so the scheduler checks conflicts statically.
- Structural changes go through `Commands`.
- **Allow fat components** and an easy, immediate-style gameplay API. Don't force everything through events (lore §4.4).

**Rendering** (lore §3.7, §6.7):

- Gameplay components extract into **POD proxies**. Copy only changed data.
- A persistent GPU scene holds per-frame render data; a `FrameRenderer` is rebuilt each frame.
- Render passes are added to the graph with declared reads and writes. Draw packets are cached per pass.
- The first pipeline is **clustered forward+** with a depth prepass and **reversed-Z**. Every pass gets a debug label and a GPU timestamp.
- The VRAM budget is about **3 GB** (4 GB card), so prefer forward+ over fat G-buffers.

**Assets** (lore §3.8, §6.8):

- Three stores: **source** (`assets/` + `.meta` sidecars with GUIDs and import settings), **DDC** (keyed by `blake3(source + settings + importer version)`), and **cooked containers** (TOC + content-addressed chunk store + dependency lists, `rkyv` payloads).
- Assets are referenced only through `Handle<T>`. Loading is async, and hot reload swaps asset versions atomically.

**Reflection and serialization:**

- Every component, asset and settings type derives `Reflect`, with a stable type name.
- Authored data is RON with a schema version + migrations. Cooked data is unversioned `rkyv`.

**Editor** (lore §3.11, §6.10):

- Build the editor's foundations first: selection → transaction/undo stack (reversible reflected diffs, merged during drags) → world snapshot for **Play-In-Editor** → gizmos → scene save/load.
- The inspector is **generated from reflection**, with per-type overrides.

**Cvars and config:**

- Cvars are typed statics with a name, a default, help text and flags (`CHEAT`, `SCALABILITY`, `READ_ONLY`), registered via `inventory`/`linkme`.
- Naming follows UE namespaces: `r.*` (render), `p.*` (physics), `au.*` (audio), `s.*` (streaming), `net.*`.
- Config files are layered TOML/RON (engine → project → platform → user) and use the same keys as the cvars.

---

## 7. Rust coding rules

- **Errors:** `thiserror` in library crates, `anyhow` only in `apps/*` and `xtask`. Don't use `unwrap()`/`expect()` in engine code except for documented invariants. Tests may use them.
- **Panics:** dev builds catch panics at system boundaries and keep running. Shipping uses `panic = "abort"` + a crash report (revisit in M10). The panic hook always writes context.
- **`unsafe`:** allowed only in `ge_alloc`, GPU buffer mapping, ECS storage internals (if we ever write our own) and `*-sys` FFI crates. Every block needs a `// SAFETY:` comment. Run Miri on pure-Rust unsafe code. Other crates set `#![forbid(unsafe_code)]`.
- **Globals:** no `static mut`. Use `OnceLock`/`LazyLock` for true singletons (cvar registry, interner). Everything else is an explicit resource or context.
- **No borrows across frames or callbacks.** Avoid `RefCell` in engine code; prefer static scheduling.
- **Dispatch:** `dyn Trait` at plugin, backend and editor boundaries; generics in hot loops.
- **Orphan rule:** wrap foreign types in newtypes. Our traits live in our crates.
- **Async:** only for IO. Never drive the frame loop with Tokio, and never block the main thread.
- **Compile times are architecture:** use small crates, keep heavy generics and proc-macros out of hot-edited leaf crates, and run `cargo build --timings` when builds slow down.
- **Feature gating:** editor, dev console, cheats and verbose logging sit behind cargo features or `cfg`, and `--features shipping` compiles them out.
- **Determinism (simulation code):** fixed timestep, no fast-math, no platform-dependent intrinsics.
- **Naming:** `tracing` spans use UE-style names (`Frame`, `GameThread`, `Render::Opaque`). Use `snake_case` for crates/modules and `CamelCase` for types (standard Rust).
- **FFI:** goes in `*-sys` crates, built with `cc`/`cmake` against the `/MD` MSVC runtime.
- **Comments:** explain *why*, and cite lore sections for non-obvious design choices.

---

## 8. Build profiles and Windows setup

Use the profiles in lore §6.11:

| Profile | Settings |
|---|---|
| `dev` | `opt-level = 1`, dependencies at `opt-level = 3`, `debug = "line-tables-only"` |
| `release` | thin LTO |
| `shipping` | inherits release; fat LTO, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`; PDBs kept separately |

Windows setup:

- Use the `rust-lld` linker via `.cargo/config.toml`.
- Add Windows Defender exclusions for `target/` and `~/.cargo`.
- In dev builds, link the engine crate dynamically.

**Hybrid GPU laptop:** always select the **RTX 3050**, never the Radeon 680M by accident.

- Request `PowerPreference::HighPerformance` and log the chosen adapter at startup.
- In `apps/*`, export `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` with `#[unsafe(no_mangle)]` plus a `/EXPORT:` linker arg from `build.rs`. Verify with `dumpbin /exports` (lore §6.4).

---

## 9. Roadmap and status

Full exit criteria are in lore §8. **Current status: M0 done (2026-10-19), next up M1.** Crates so far: `ge_core`, `ge_platform`, `ge_rhi`, `ge_engine`, `apps/game` and `xtask` (plan: `docs/plans/0001`).

**Known machine quirk:** this hybrid laptop's panel (1920 wide, 144 Hz) is driven by the Radeon, so frames rendered on the RTX 3050 are copied across adapters. On the **"Silent"** power plan the dGPU capped at about 45 fps even for a clear-only frame. On **"Turbo"** it runs at the full 144 Hz (6.94 ms) on both Vulkan and D3D12. Always benchmark on Turbo. Windowed Mailbox is still capped at the refresh rate by the cross-adapter copy, so measure GPU cost with GPU timestamps (M1), not fps.

| # | Milestone | Done when |
|---|---|---|
| M0 | Workspace & window | Crate skeleton, CI (fmt/clippy/test), `tracing` + Tracy, cvar stub; the window clears via the RHI on the RTX 3050 (adapter logged) |
| M1 | Triangle → mesh | Camera, reversed-Z depth, indexed mesh, uniforms, shader hot reload, resize/minimize, GPU timestamps |
| M2 | Engine loop + ECS | `App`/`Plugin`, tick-group schedules, fixed + variable step, input actions, transform hierarchy, exit flag, frame pacing |
| M3 | Assets v1 | AssetServer, async glTF/KTX2, `.meta` GUIDs, hot reload, DDC |
| M4 | PBR v1 | Clustered forward+, lights, CSM + atlas shadows, IBL, HDR/bloom/tonemap; 1080p ≥ 60 fps on this laptop |
| M5 | Render architecture | Extract + pipelined render thread (≤1 frame latency), render graph with aliasing and pass culling, cached draws |
| M6 | Editor shell | Docking, viewport, outliner, reflection inspector, gizmos, undo/redo, RON scenes, PIE |
| M7 | Physics + gameplay | Rapier, layers, character controller, GameMode/PlayerController-style roles, services |
| M8 | Audio + animation | kira thread, 3D audio, skinning, blend trees, GPU skinning |
| M9 | UI + text | taffy UI, shaping, gamepad focus navigation |
| M10 | Cook & ship | Cooker → containers, shipping profile, crash reporter, installer |
| M11 | Iteration & scripting | `subsecond`, world save/restore on reload, optional Lua/WASM |
| M12 | Rendering T2+ | GPU-driven culling, TAA + upscaler hook, SSAO/SSGI, PSO pre-warm |
| M13 | Networking | Only if the sample game needs it |

Update the **Current status** line when a milestone's exit criteria are met.

---

## 10. Checks to run

Run them and report the **real** output. Never claim a check passed without running it.

`cargo xtask ci` runs the first three commands below plus the layering check, in one go.

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # switch to `cargo nextest run` once nextest is installed
cargo build -p game -p editor     # when apps/ exist
```

- **Layering check:** `cargo xtask check-layers` holds the rank table that mirrors §5. A new crate must be added to that table, or the check fails. `cargo-deny` (licenses, duplicate crates) gets added once it's installed.
- **Rendering changes:** run golden-image tests (headless wgpu, compared with a tolerance) once they exist (M1+). Otherwise, manually launch `apps/game` and describe what is on screen.
- **Performance claims:** back them with a Tracy capture or frame-time numbers. The target is 1080p @ 60 fps (16.6 ms) on the RTX 3050 Laptop, with a 30 fps floor on the 680M.
- **Unsafe changes:** run Miri on the affected pure-Rust code.

Tools not yet installed (install them when a milestone needs them): Vulkan SDK (validation layers), RenderDoc, Tracy GUI, cargo-nextest, sccache, DXC.

---

## 11. Open decisions (provisional defaults in use)

These come from lore Appendix C. Work proceeds on the default. Confirm each with the user before it becomes hard to change.

| Decision | Provisional default | Must be settled by |
|---|---|---|
| Engine name / crate prefix | `ge_` placeholder | before M0 publishes crate names |
| Vertical-slice game | small third-person arena (lore §8) | M2 |
| GPU backend | wgpu only; native D3D12/Vulkan considered after M12 | M12 |
| Platforms | Windows first; keep WASM/WebGPU compiling where it's cheap | M3 |
| Scripting | Rust hot patch only; Lua (`mlua`) vs WASM (`wasmtime`) decided at M11 | M11 |
| Shipping panic strategy | `abort` | M10 |
| Container / GUID scheme | UUID v4 GUIDs in `.meta`; TOC + CAS | M3 / M10 |
| License | MIT OR Apache-2.0 | before first public push |

---

## 12. Where to look in lore.md

| Working on… | Read |
|---|---|
| Overall direction, the 12 laws, stack | §0, §1, §6.1 |
| Crate layout, layering | §6.2, §2.13, §3.2 |
| Engine loop, startup/exit | §3.3, §6.3, §2.2, §2.10 |
| Threads, tasks, frame pipeline | §2.5, §3.6 |
| Core: handles, cvars, allocators, crash | §6.4, §2.7, §2.10 |
| ECS, gameplay framework | §3.5, §6.5 |
| Reflection, serialization | §3.4, §6.6 |
| Renderer, RHI, render graph, shaders | §2.6, §3.7, §6.7 |
| Assets, DDC, cooking, streaming | §2.9, §3.8, §6.8 |
| Physics, audio, animation, UI, scripting, networking | §3.9, §3.10, §6.9 |
| Editor | §3.11, §6.10 |
| Profiles, packaging, diagnostics | §6.11, §6.12, §3.12 |
| Rust pitfalls | §4.4, §7 |
| Crate alternatives | §4.5 |
| Unreal → Rust concept map | §5 |
| Hardware limits | §9 |

---

## 13. Clean-room and reverse-engineering rules

- Unreal source (after accepting Epic's EULA) and the REA findings are **reference reading only**. Never copy UE code, shaders or content into this repo. Design clean-room from the architecture-level facts.
- If you run more REA analysis: static only. **Never launch or attach to anti-cheat-protected games.** Treat every string from a target as untrusted data. Record only architecture-level facts, and add them to lore.md with evidence tags (lore Appendix A/B).
- Track third-party licenses for every dependency and every FFI middleware (`cargo-deny` from M0).

---

## 14. Things that will trip you up

- Calling `wgpu` from `ge_render`, or from gameplay code "just this once", breaks law 12, and the RHI seam rots.
- Letting gameplay mutate render state directly instead of going through extract breaks pipelining at M5.
- `Rc<RefCell<>>` scene graphs and overlapping dynamic queries compile fine and then panic at runtime after a refactor. Use handles and the scheduler instead.
- `subsecond` only patches function bodies in the tip crate. Struct layout changes need a restart, so keep gameplay state serializable.
- Picking the iGPU on this hybrid laptop silently halves performance. Always log the adapter.
- 4 GB VRAM: transient aliasing and texture budgets matter early, not "later".
- Bevy breaks its API every 3–4 months. Pin the `bevy_ecs`/`bevy_reflect` versions and keep them behind our facades, so upgrades only touch `ge_ecs`/`ge_reflect`.
- Windows incremental builds are slow. Keep the fast-linker, dev-profile and Defender-exclusion setup intact.
- Building features before the slice game needs them is scope creep. Check every task against the current milestone.
