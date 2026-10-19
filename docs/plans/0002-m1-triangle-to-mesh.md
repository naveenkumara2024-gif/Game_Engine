# 0002 — M1: Triangle → mesh

**Status:** done 2026-10-19 · **Milestone:** M1 (CLAUDE.md §9, lore §8)

**Deviations from this plan:**
- **One pass, `Opaque`** (color + depth clear, then draws). There is no separate depth prepass yet; forward+ with a prepass arrives in M4.
- **wesl 0.6 workaround:** `Compiler::compile(fs_path)` resolves a single-segment path like `mesh.wesl` to the package root (`package.wesl`), so `ge_shader` builds the `ModulePath` itself and calls `compile_module`.
- **glam 0.34** removed `Mat4::look_at_rh` / `perspective_*`. The camera uses `glam::camera::rh::{view, proj::directx}`; WebGPU clip space matches glam's `directx` convention (z ∈ [0, 1], +Y up).
- **Added an `r.DebugNormals` cvar.** It selects the `@if(debug_normals)` shader permutation, which proves permutations work end to end. Without a console it can only be set on the command line.
- **`ge_shader` and `ge_render` are `default-features = false` in `[workspace.dependencies]`.** Cargo can't turn a default feature off per crate, so `hot-reload` is opted into explicitly: `ge_engine` → `ge_render` → `ge_shader`.
- `ge_engine` logs `frame stats` (CPU ms + per-pass GPU ms) at `debug` level once per second.
- `MAX_OBJECTS` is 1024 per frame (one dynamic-offset uniform buffer).

## Goal

Get real geometry on screen through the RHI seam. M1 is done when an **indexed mesh** is drawn with a **camera**, **reversed-Z depth**, **uniform buffers**, **WESL shaders that hot-reload**, survives **resize/minimize**, and every pass reports **GPU timestamps** (title bar + Tracy).

## Lore read

§2.6 (GPU policy), §3.7 (rendering anatomy), §6.2 (layout), §6.4 (handles), §6.7 (rendering architecture: RHI scope, shaders, debuggability), §6.11, §7 (pitfalls), §8 (M1 exit criteria), §9 (machine).

## Current state

M0 crates: `ge_core` (Handle/Pool, cvars, logging), `ge_platform` (winit wrapper, keyboard only), `ge_rhi` (opaque `Rhi`/`Swapchain`/`Frame`, clear only), `ge_engine` (loop, exit flag, frame-time title), `apps/game`, `xtask`. `ge_shader` and `ge_render` already have slots in xtask's rank table (5 and 7).

## Decisions & assumptions (flagged)

1. **Two new crates, nothing else.**
   - `ge_shader` (→ `ge_core`): loads `shaders/*.wesl`, compiles WESL → WGSL with the `wesl` crate (imports + `@if` permutations), watches the folder with `notify`, and reports changed shaders over a channel. It produces **WGSL text**, not GPU objects, so it doesn't need `ge_rhi`.
   - `ge_render` (→ `ge_core`, `ge_rhi`, `ge_shader`, `glam`): `Camera`, procedural `Mesh` (cube + ground plane), per-view and per-object uniforms, the mesh pipeline, depth target, and the frame's pass list. It never names a `wgpu` type (law 12).
   - `ge_engine` stops calling `Frame::clear` and drives `ge_render::Renderer` instead. It also holds a **temporary orbit camera controller** (right-drag orbit, wheel zoom) until M2 input actions replace it.
2. **RHI grows by opaque types, same style as M0** (plan 0001 deviation): `Buffer`, `Texture`, `ShaderModule`, `RenderPipeline`, `BindGroupLayout`, `BindGroup`, plus our own descriptor enums (`TextureFormat` subset, `VertexFormat`, `CompareFunction`, `BufferUsage` …). Render passes are recorded through `Frame::render_pass(desc) -> RenderPass<'_>` with `set_pipeline / set_bind_group / set_vertex_buffer / set_index_buffer / draw_indexed`. Every pass requires a label and gets a timestamp pair automatically. No `dyn` trait yet (one backend). No render graph yet; that's M5.
3. **Reversed-Z:** `Depth32Float`, clear to `0.0`, compare `Greater`, `glam::Mat4::perspective_infinite_reverse_rh`. The depth texture is recreated on resize. Swapchain stays sRGB; shaders write linear.
4. **Scene for M1:** a 5×5 grid of spinning cubes at varied depths over a ground plane, Lambert + ambient with a fixed sun direction and per-vertex color. Enough to prove depth, indexing and per-object uniforms. Geometry is procedural; glTF waits for M3.
5. **Uniforms:** group 0 = `ViewUniform` (view-proj, camera position, time), group 1 = per-object `world_from_local` + normal matrix in **one uniform buffer with dynamic offsets**, rewritten each frame via `queue.write_buffer`. Instancing/GPU scene waits for M5/M12.
6. **Shader hot reload (dev only):**
   - Source of truth: `shaders/mesh.wesl` importing `shaders/common/view.wesl` (proves WESL imports work).
   - Shader root resolved from `GE_SHADER_DIR`, else `<workspace>/shaders` via `CARGO_MANIFEST_DIR` at build time.
   - On a file change (debounced ~100 ms), recompile on the main thread at the top of the next frame. Validation errors are caught with a wgpu **error scope inside `ge_rhi`** and returned as `RhiError::Shader`, so **a broken shader logs an error and the old pipeline keeps rendering.** No crash, no uncaptured-error spam.
   - Behind a `hot-reload` cargo feature, on by default. Reading `.wesl` source at runtime breaks law 7 in spirit, so this is **dev-only until M10** cooks shaders.
7. **GPU timestamps:**
   - Request `Features::TIMESTAMP_QUERY` when the adapter has it; otherwise timings are simply absent (logged once).
   - One query set per frame-in-flight, resolved at `end_frame`, read back through a **3-deep ring** of mapped buffers polled with `device.poll(Poll)` (never blocks the main thread).
   - Output: a `GpuTimings` list (pass name → ms) exposed from `Rhi`. The title bar shows `CPU x.xx ms | GPU y.yy ms` (CLAUDE.md §9: measure GPU cost this way, not with fps).
   - Tracy: GPU zones through `tracy-client`'s `GpuContext` behind the existing `tracy` feature, calibrated once at startup with a blocking timestamp readback.
   - Cvar `r.GpuTimestamps` (default on).
8. **Headless golden-image test** (CLAUDE.md §10 says these start at M1): `Rhi::new_headless()` renders the scene at a fixed time to a 256×256 offscreen target, reads it back, and compares with `crates/ge_render/tests/golden/mesh.png` (per-channel tolerance 3/255, ≤ 0.5 % of pixels may exceed it). `GE_BLESS=1` rewrites the golden. If no GPU adapter is available, the test **skips with a message** rather than failing.
9. **Platform:** `PlatformEvent` gains `MouseButton`, `MouseMotion` (raw delta via `DeviceEvent`) and `MouseWheel`. Still no input mapping (M2).
10. **Toolchain pin:** `rust-version` goes from `"1.97"` to `"1.97.1"` (the locked toolchain). Resolver 3 respects MSRV, and `wesl 0.6` declares 1.97.1, so with `"1.97"` cargo falls back to the old 0.4 line.

## Dependencies added

| Crate | Where | Why |
|---|---|---|
| `wesl 0.6` | `ge_shader` | In the locked stack (§4). Pure Rust, no second `naga`. |
| `notify 8.2` | `ge_shader` (feature `hot-reload`) | In the locked stack (§4). License is CC0-1.0, which is permissive; noted for `cargo-deny`. Not 9.x, which is still a release candidate. |
| `bytemuck` (+ glam's `bytemuck` feature) | `ge_render`, `ge_rhi` | **Not in the table.** It casts POD vertex/uniform structs to bytes without `unsafe` in our crates. It's already in the tree through wgpu, so no new code is compiled. |
| `tracy-client 0.19` (optional) | `ge_rhi` (feature `tracy`) | Already in the tree through `tracing-tracy`. Needed for GPU zones, which `tracing` can't express. |
| `image` (png only, dev-dependency) | `ge_render` | In the locked stack (§4). Golden-image test only. |

## Files to touch

```
Cargo.toml                                   # members/deps, rust-version 1.97.1
shaders/mesh.wesl, shaders/common/view.wesl  # new
crates/ge_shader/{Cargo.toml, src/lib.rs, src/watch.rs}                       # new
crates/ge_render/{Cargo.toml, src/lib.rs, src/camera.rs, src/mesh.rs,
                  src/renderer.rs, tests/golden_image.rs, tests/golden/mesh.png} # new
crates/ge_rhi/src/{lib.rs, wgpu_backend.rs, resources.rs, pass.rs, timestamps.rs}
crates/ge_platform/src/lib.rs                # mouse events
crates/ge_engine/{Cargo.toml, src/lib.rs, src/orbit_camera.rs}
CLAUDE.md §9                                 # status → M1 done (at the end)
```

## Out of scope (later milestones)

ECS and input actions (M2), glTF and textures (M3), PBR, lights and shadows (M4), render graph, extract and render thread (M5), MSAA, materials, a PSO cache.

## Acceptance criteria (M1 exit criteria)

- [x] `cargo xtask ci` passes: fmt, clippy `-D warnings`, tests, check-layers (with `ge_shader` and `ge_render` in the graph).
- [x] `cargo run -p game` shows the lit cube grid over a ground plane on the RTX 3050. Near cubes correctly occlude far ones (reversed-Z depth).
- [ ] *(manual; covered by unit tests only)* Right-drag orbits and the wheel zooms.
- [x] Resizing keeps the aspect ratio correct. Minimize/restore causes no errors or validation spam.
- [x] Editing `shaders/mesh.wesl` while running updates the image within about 1 s. Saving a syntax error logs one error and keeps rendering the last good shader. Fixing it recovers.
- [x] The title bar shows a GPU ms figure from timestamps. *(Tracy part still manual:)* `cargo run -p game --features tracy` shows `Render::DepthPrepass`-style GPU zones in Tracy *(manual; needs the Tracy GUI)*.
- [x] `grep -r wgpu crates/ge_render crates/ge_engine` finds nothing (law 12).
- [x] Unit tests: camera matrices (a point at the near plane maps to depth 1 and at infinity to 0), cube mesh index validity, WESL import + `@if` compile, timestamp tick → ms conversion. The golden-image test passes (or skips without a GPU).

## Manual test steps

1. Switch to the **Turbo** power plan. Run `cargo run -p game`. Expect the cube grid, an RTX 3050 adapter log line, and `GPU x.xx ms` in the title.
2. Right-drag and scroll to orbit and zoom around the scene. Check that occlusion looks right from every side.
3. Resize, maximize, minimize and restore. Expect no panics or error logs.
4. With the game running, change the sun direction or a color in `shaders/mesh.wesl` and save. The image should update. Then delete a `;` and save: expect one error log while the old image stays. Restore the `;`.
5. Optional: run with `--features tracy` and look for CPU `Frame` zones plus GPU pass zones in Tracy.
