# 0008 — M5: Render architecture

**Status:** approved 2026-11-05 (as proposed) · in progress: step 1 (render graph) done 2026-11-05; steps 2–3 next · **Milestone:** M5 (CLAUDE.md §9, lore §8)

## Progress and deviations

**Step 1 (render graph) is done.** `ge_render::graph` provides validation, culling, lifetimes, aliasing and a cross-frame `TransientPool`. Every per-frame pass is now a graph node:

- Shadows.Cascade ×N, Shadows.Atlas
- DepthPrepass, LightCulling, Opaque
- Bloom.Down ×6, Bloom.Up ×5
- Tonemap, Ui

SceneColor, SceneDepth and the bloom levels are pooled transients. The M4 goldens (`tests/pbr.rs`) pass unchanged, and there are 3 new headless graph tests (`tests/graph.rs`) plus 10 unit tests. The arena runs at 144 Hz with `r.Graph.Stats=1`: 20 passes, 8 transients, a steady pool of 8.

Deviations:
- **The real frame has nothing to alias yet.** I claimed in the plan that the bloom chain would alias; that was wrong. The bloom levels all have different sizes and stay live until the upsample chain ends, and SceneColor and SceneDepth overlap. So today `r.Graph.Stats` reports physical = virtual. Aliasing itself is covered by the unit tests. It starts paying off once M12 adds same-size post targets (TAA history, SSAO, half-res chains), and `tests/graph.rs` says so.
- **The grid stays inside the Opaque pass** rather than becoming its own pass. A separate pass would cost an extra load/store of color and depth at 1080p just to make culling visible. It is still skipped when the overlays are off.
- **The IBL environment bake runs before the graph.** It is a persistent, change-driven bake that mutates `Environment`, not a per-frame pass. It can move into the graph as a side-effect pass when it gets a GPU timestamp budget of its own.

## Goal

M4 made the image right. M5 makes the *shape* right, so that M6–M12 build on a render world, a render graph and a render thread instead of one 260-line `render_frame`.

**M5 exit criteria** (lore §8):
- render world + **extract**;
- a **pipelined render thread** with at most one frame of latency;
- a **render graph** with transient aliasing and pass culling;
- cached draw packets;
- debug labels.

M5 is done when:
- `apps/game` and `apps/editor` render on a `RenderThread`, and the main thread is never more than one frame ahead (measured, shown in the title stats);
- every pass in the frame is a graph node with declared reads and writes; unused passes are culled, and transient textures are pooled and aliased;
- a static scene re-uploads **zero** object data and rebuilds **zero** draw packets per frame;
- the M4 golden tests (`crates/ge_render/tests/pbr.rs`) still pass unchanged, and the arena still holds 60 fps at 1080p (GPU timestamps, Turbo plan).

## Lore read

§2.5 (the thread roster: Game/Render/RHI threads, the render heartbeat), §3.6 (the frame pipeline; the game thread blocks when render is >1 frame behind), §3.7 (game↔render mirrors, `MarkRenderStateDirty`, cached mesh draw commands, RDG), §6.3 (our frame lifecycle and backpressure), §6.7 (proxies, GPU scene, the graph API sketch, draw submission), §7 (Rust pitfalls: no borrows across frames, `Send` boundaries), §8 (the M5/M12 boundary).

## Where we are today

- `extract_render_snapshot` rebuilds `RenderSnapshot.items` (a `Vec<DrawItem>`) **every frame** from every visible mesh.
- The runner calls `Renderer::render_frame` inline on the main thread, right after `App::update`.
- `render_frame` records about 12 passes by hand in a fixed order:
  - Shadows (cascades + atlas)
  - DepthPrepass
  - LightCulling
  - Opaque (+ sky, transparent and grid)
  - the Bloom chain
  - Tonemap
  - Ui
- `Post` owns SceneColor, SceneDepth and the bloom chain as long-lived textures.
- Each frame, every object's uniforms are re-packed and re-uploaded. Draws are walked from the item list on every pass.

## Design

Three steps, done in this order. Each step ends green: checks pass, the goldens pass and the game runs.

### Step 1: render graph (`ge_render::graph`)

```rust
let mut g = RenderGraph::new();
let depth  = g.create_texture("SceneDepth", TextureDesc::depth(size));
let color  = g.create_texture("SceneColor", TextureDesc::hdr(size));
let csm    = g.import_texture("ShadowCascades", &self.shadows.csm);     // persistent
let out    = g.import_backbuffer(frame.backbuffer());                    // the graph's output
g.add_raster_pass("DepthPrepass", |p| {
    p.write_depth(depth, LoadOp::Clear(DEPTH_CLEAR));
    move |ctx: &mut RasterContext| ctx.draw_bin(Bin::Prepass)
});
g.add_compute_pass("LightCulling", |p| { p.read(depth); /* … */ });
g.add_raster_pass("Opaque", |p| {
    p.read(csm);
    p.write_color(color, LoadOp::Clear(bg));
    p.write_depth(depth, LoadOp::Load);
    move |ctx| { ctx.draw_bin(Bin::Opaque); ctx.draw_sky(); ctx.draw_bin_sorted(Bin::Transparent) }
});
// … Bloom, Tonemap → writes `out`, Ui → writes `out`
g.compile(&mut self.pool)?.execute(rhi, frame, &mut self.ctx);
```

- **Handles, not references** (law 5). `GraphTexture`/`GraphBuffer` are indices into the graph. A pass's execute closure receives a context that resolves each handle to a `TextureView`.
- **Compile:**
  1. Passes run in the order they were added. Declaring a read before the matching write is a validation error, which the tests cover.
  2. **Culling** walks backwards from the outputs (the imported backbuffer, plus passes marked `side_effect`, such as readback). Any pass whose writes nobody reads is dropped. This removes the bloom chain when bloom is off and the grid when overlays are off. It also removes shadow passes when no light needs them; today `if` statements do that work.
  3. **Lifetimes:** each transient resource lives from its first-use pass index to its last-use pass index.
  4. **Allocation:** we greedily allocate from a cross-frame `TransientPool`. A physical texture is reused by any later virtual texture with an identical `TextureDesc` whose lifetime starts after the previous owner's ends. Pool entries unused for 3 frames are freed. Resize therefore needs no special code: old sizes simply age out.
- **Aliasing, honestly:** wgpu has no placed resources or heaps, so we can't overlap the memory of textures with *different* descriptors. We alias by **reusing whole textures with compatible descriptors** across disjoint lifetimes. That is what the API allows. Memory-level aliasing is a native-backend feature (after M12) behind the same graph API. On today's frame, the gain is that the bloom mips become separate transient textures, and the downsample and upsample levels share them. The graph reports the virtual vs physical texture count and the bytes saved through `r.Graph.Stats`, which is logged once per second at debug level.
- **Barriers:** wgpu inserts them, so the graph only records intent. The read/write declarations are where a native backend will derive them.
- **Debug labels + timestamps:** each graph pass opens its RHI pass with its `&'static str` name, so existing per-pass GPU timestamps and Tracy spans keep working. Pass names stay the same as today, so the timings readout does not change.
- **Bind groups for transient textures** are built per frame from the resolved views; this is a handful of cheap groups. They are cached per physical texture once a profile shows it matters.
- `ge_rhi` gains no scene concepts. It gains possibly one helper: a `TextureDesc` equality/hash and a view-of-texture accessor. Only `ge_rhi` names wgpu, so law 12 holds.
- `Post` stops owning SceneColor, SceneDepth and bloom. They become graph transients, and `Post` keeps only pipelines, samplers and parameters.

### Step 2: render world, change-detected extract and cached draw packets

```
Main thread (ECS)                                Render side (persistent)
──────────────────                               ────────────────────────
extract_render_changes:                          RenderScene
  Added/Changed<GlobalTransform|MeshRenderer|      proxies: SlotMap<ProxyId, PrimitiveProxy>
                Material|Visibility>  → Upsert     entity → ProxyId
  RemovedComponents<MeshRenderer>,  → Remove       GPU scene: object buffer, one slot per
  hidden                                            proxy; only dirty slots are written
  material asset modified → Upsert its users      draw bins (Prepass, Opaque, Shadow,
lights, camera, settings → small, sent whole        Transparent): packets sorted by
                                                    (pipeline, material, mesh), rebuilt
                                                    only when a proxy is added, removed or
                                                    changes bin
```

- **`PrimitiveProxy`** is today's `DrawItem` plus a stable `ProxyId` (a generational slotmap key, law 5). `DrawItem` stays the POD payload.
- **The extract sends deltas** (`SceneDelta { upserts, removes }`) instead of the full list. This is Unreal's `MarkRenderStateDirty`, done with ECS change detection. Lights, camera, world and settings stay whole-frame copies. They are tiny, and lights are capped at `MAX_LIGHTS`.
- **The GPU scene** is the existing dynamic-offset object buffer. Indexing it by proxy slot, not by list position, means only dirty slots are written. We write contiguous runs of dirty slots, so a moving player is one small write.
- **Draw packets:** `{ pipeline, material, mesh, object_offset }` per bin, cached and sorted. The bin for each pass is rebuilt only when membership changes. Transparent packets are re-sorted by distance every frame, since the camera moves. Shadow passes use the Shadow bin for every view.
- **Immediate path for tests and tools:** `FrameInputs { items, .. }` keeps working. `Renderer::render_frame` turns the slice into a full sync of a private scene, so the M4 goldens run unchanged. It is the "basic" path, like UE's `FCanvas`.
- `MAX_OBJECTS` (1024) remains the slot cap. When the cap is hit, we keep today's warning and skip the extra objects. Growing the buffer is cheap to add later, and GPU-driven instancing is M12.

### Step 3: pipelined render thread

```
Main   [ tick N · extract → send(Snapshot N) ][ tick N+1 · extract → send (blocks if render is still on N) ]
Render                 [ recv N · apply delta · graph · record · submit · present ][ recv N+1 … ]
```

- **Threads:**
  - `ge_engine::runner` spawns a long-lived thread named **`RenderThread`**, with a Tracy thread name and `tracing` spans `RenderThread` / `Render::Scene`.
  - It owns `Rhi`, `Swapchain` and `Renderer`. The window stays on the main thread because winit requires it. The thread holds an `Arc<Window>` only for the surface.
- **Channels** (crossbeam, already in the workspace):
  - **to render:** `RenderCommand::{ Frame(Box<RenderFrame>), Resize(u32,u32), Shutdown }`. Frames go over a **bounded(1)** channel.
  - **from render:** `RenderEvent::{ Stats(GpuTimings, render_ms), Fatal(String), Recycled(Box<RenderFrame>) }`.
  - Recycling hands the snapshot's `Vec` allocations back, so steady state allocates nothing.
- **≤1 frame latency (law 3).**
  - The main thread sends frame N+1 only when the slot is free. A bounded(1) channel plus the one frame the render thread is working on means the main thread is at most one frame ahead.
  - We measure the lag as `main_frame − last_rendered_frame` and publish it as `EngineStats.render_lag`. The title shows it, and a test asserts it stays ≤ 1.
- **Queues that must not be lost** still drain into the next frame that is sent, whether or not it presents: asset uploads, UI texture deltas and scene deltas.
  - While the window is minimized, the render thread still applies them and runs shader hot reload, as it does today.
  - The main thread keeps its 16 ms minimized tick.
- **Resize:** the main thread sends `Resize` ahead of the next frame. The render thread resizes the swapchain before acquiring.
- **Exit:** cooperative. On exit, the main thread sends `Shutdown` and joins the render thread, which drops GPU objects before the window. That keeps today's drop order.
  - A panic on the render thread arrives as `Fatal` or as a closed channel. The main thread then calls `request_exit("render thread")` and logs the panic, which feeds the crash path at M10.
- **`r.RenderThread`** (bool, default `true`, `READ_ONLY`, read at startup) is a debug switch, like UE's `r.rhithread.enable`. With `false`, the same `RenderFrame` is consumed inline on the main thread, so a bug can be bisected to "threading" vs "rendering". There is one code path: the inline mode calls the same `RenderWorker::frame()` function.
- **`Send` audit:** a compile-time `assert_send::<Rhi>()`, `assert_send::<Renderer>()` and `assert_send::<Swapchain>()`. wgpu's native types are `Send + Sync`. If our notify watcher in `ShaderLibrary` isn't `Send`, it is the one thing to fix.
- **Parallel command recording is deferred.** Its benefit is small at our draw counts (a few hundred), and wgpu needs one encoder per thread. The graph keeps the seam: passes don't share mutable state, so they could record in parallel later. This counts as "where the backend allows" (lore §6.7). It is planned for M12, together with GPU-driven draws.

## Decisions and assumptions (please confirm or override)

1. **We use a dedicated OS thread, not a rayon task,** for render. This matches lore §2.5's long-lived role threads, and render-thread affinity and priority can be added later.
2. **Aliasing** means desc-compatible texture reuse across disjoint lifetimes (wgpu limit, see above). Memory-overlap aliasing is a native-backend feature.
3. **Pass order is the add order.** There is no automatic reordering beyond culling. That makes it deterministic and easy to debug. The topological check only validates the order.
4. **Material asset hot reload** re-upserts every proxy that uses that material. This is rare, so we use a simple main-side `material → entities` scan.
5. **The editor changes nothing.** egui output already travels in the snapshot (`UiDrawList`), so the editor runs on the render thread for free. PIE snapshot and restore are unaffected because the render world is rebuilt from deltas. On Stop, despawned entities send `Remove`.
6. **No new third-party crates.** `slotmap` is in the locked stack (§4). If it isn't in the workspace yet, it is added to `[workspace.dependencies]` for `ProxyId`, or we use our own small generational `Pool<T>`.

## Files to touch

- **New:**
  - `crates/ge_render/src/graph/{mod,builder,compile,pool}.rs`: the graph, culling and the transient pool, with unit tests that need no GPU.
  - `crates/ge_render/src/scene_proxy.rs`: `RenderScene`, `PrimitiveProxy`, `SceneDelta`, the GPU scene slots and the draw bins.
  - `crates/ge_engine/src/render_thread.rs`: the worker, the commands, the inline mode and the latency stat.
- **Changed:**
  - `crates/ge_render/src/renderer.rs`: `render_frame` becomes graph setup, and the pass bodies move into closures and their modules.
  - `post.rs`, `shadows.rs`, `frame.rs`, `ui.rs`: their passes register as graph nodes.
  - `crates/ge_render/src/components.rs`: the extract emits `SceneDelta`, and `RenderSnapshot` becomes `RenderFrame`.
  - `crates/ge_engine/src/runner.rs`: it spawns, sends to and joins the render thread. `EngineStats` gets `render_lag` and `render_ms`.
- **Possibly:** `crates/ge_rhi/src/resources.rs` (`TextureDesc: Eq + Hash`, `Texture::view()`).
- **Bookkeeping:** `xtask` check-layers (no new crates, so likely no change) and CLAUDE.md §9 status when done.

## Acceptance criteria → tests

| Exit criterion | How it's checked |
|---|---|
| Render graph | Unit tests: a read-before-write error; ordering validation; a pass whose output is unused is culled; a side-effect pass survives; two same-desc transients with disjoint lifetimes share one physical texture, overlapping ones don't; a pool entry ages out after 3 frames; a resize reallocates. |
| Pass culling in the real frame | Headless: bloom off → no `Bloom.*` passes in the executed list; overlays off → no grid; no shadowed lights → no `Shadows.*`. |
| Transient aliasing in the real frame | Headless: `r.Graph.Stats` reports physical < virtual for the bloom chain. |
| Render world + change-detected extract | ECS tests: a static scene's second extract sends an empty delta; moving one entity → 1 upsert; despawn/hide → remove; a material asset modified → its users are upserted. |
| Cached draw packets | Renderer test: 2nd frame of a static scene → 0 object bytes written and 0 bin rebuilds (counters exposed for tests). |
| No visual regressions | `crates/ge_render/tests/pbr.rs` passes unchanged (golden pixel checks through the immediate path). |
| Pipelined render thread, ≤1 frame latency | Engine test with a headless worker that sleeps 30 ms per frame: the main thread blocks, `render_lag` never exceeds 1, and frames arrive in order with none dropped. A second run with `r.RenderThread=0` produces identical frame indices. |
| Exit and shutdown | Test: `request_exit` → the worker joins cleanly; a worker panic → the main thread exits with reason "render thread". |
| Performance | Manual: `cargo run --release -p game` on Turbo, arena at 1080p. Report CPU main ms, render ms and GPU ms from the title before and after. GPU time within noise of M4; main-thread ms should drop, since rendering moved off it. |
| Debug labels | Every graph pass name appears in GPU timings and Tracy (manual Tracy check, still pending since M1). |

Checks: `cargo xtask ci` (fmt, clippy `-D warnings`, test, layers) and `cargo build -p game -p editor`.

## Manual test steps

1. Run `cargo run -p game`. Orb Run plays as before. The title shows `lag ≤1` and render ms next to the GPU ms.
2. Resize, minimize and restore the window. There is no hang and no stretched frame. A shader edit while minimized hot-reloads on restore.
3. Run `cargo run -p editor`. Edit, Play, then Stop. The scene is unchanged after Stop. Toggle the grid, and toggle bloom in Properties; their passes disappear from the timings.
4. Run `cargo run -p game -- -cvar r.RenderThread=0`. The game is identical, with everything on the main thread.
5. Close the window mid-game. The process exits cleanly, with no wgpu "dropped while in use" warnings.

## Out of scope (stays where the roadmap puts it)

These belong to M12: GPU-driven culling and indirect draws, parallel command recording, async compute, a separate RHI submission thread and PSO pre-warm. The editor's own uses of the render graph, such as an outline or selection pass and a picking readback, belong to M6.
