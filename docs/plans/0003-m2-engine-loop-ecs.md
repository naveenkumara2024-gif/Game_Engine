# 0003 — M2: Engine loop + ECS

**Status:** proposed 2026-10-19 · **Milestone:** M2 (CLAUDE.md §9, lore §8)

## Goal

Replace M1's hard-coded loop with a real engine shape. Gameplay lives in an ECS world, games are **plugins**, and each frame runs the CLAUDE.md §6 tick groups in order, with a fixed physics step plus variable update. Input arrives as **named actions**, transforms propagate through a parent/child hierarchy, the loop paces frames, and exit stays cooperative.

M2 is done when `apps/game` is just `App::new().add_plugins(DefaultPlugins).add_plugins(SamplePlugin).run()`, and the sample game's player can be driven with WASD (camera with the mouse) while child entities follow their parents.

## Lore read

§2.2, §2.10 (entry point, exit), §3.3 (startup phases, tick groups, exit), §3.5 (gameplay framework, Mass), §3.6 (threads), §4.1 (Bevy ECS), §4.4 (LogLog lessons: fat components, no `RefCell`, iteration speed), §6.3 (frame lifecycle), §6.5 (World/ECS, tick groups, actor facade), §6.6 (reflection), §7 (pitfalls), §8 (M2 exit criteria).

## Current state

M1 is done: `ge_rhi`, `ge_shader` and `ge_render` draw a list of `DrawItem`s. `ge_engine` owns the winit loop, the exit flag, an orbit camera and the hard-coded demo scene. **M1 isn't committed yet.**

## Decisions & assumptions (flagged)

1. **New crates:**
   - `ge_ecs` (rank 5, → `ge_core`): wraps `bevy_ecs 0.20`. It holds the `Transform` / `GlobalTransform` components, the hierarchy, transform propagation, and the `Time` / `FixedTime` resources.
   - `games/sample` (`sample`, rank 10): the vertical-slice game, written as a plugin.
   - Both are added to xtask's rank table, and `games/*` joins the workspace members.
2. **⚠ Facade over bevy_ecs: needs your OK, because it deviates from CLAUDE.md §4.** The rule says to "never re-export raw bevy types". Fully wrapping `Query` / `Commands` / `Res` / system params means re-implementing Bevy's system-param machinery, which is weeks of work and fights the borrow checker. My proposal:
   - **Our own types** for everything engine-shaped: `App`, `Plugin`, tick-group labels, `Transform`, `GlobalTransform`, `Time`, input actions and the exit request.
   - **A curated `ge_ecs::prelude`** that re-exports the ECS vocabulary: `Component`, `Resource`, `Entity`, `Query`, `Commands`, `Res`/`ResMut`, `With`/`Without`/`Changed`, `ChildOf`/`Children`, `IntoScheduleConfigs`.
   - **Game and engine crates depend on `ge_ecs` only, never on `bevy_ecs`.** Bevy's derives emit a bare `bevy_ecs::` path when the crate isn't in `Cargo.toml`, so the prelude carries a hidden `pub use bevy_ecs`. I verified this in `bevy_macro_utils` 0.20.
   - **Result:** a Bevy upgrade touches `ge_ecs` plus whatever vocabulary Bevy itself changed. If you agree, I'll reword the CLAUDE.md §4 row to match.
3. **Single-threaded schedule executor for M2.** `bevy_ecs` uses `default-features = false, features = ["std"]`. That leaves out the `multi_threaded` feature, which would bring in Bevy's own thread pool next to our planned `rayon` pool, and leaves out `bevy_reflect`. Parallel system execution comes back with `ge_tasks` when there's real load (M5+). Bevy's `trace` feature (a span per system) turns on with our `tracy` feature.
4. **Reflection (`ge_reflect`) is deferred to M6,** where the inspector is its first consumer. Law 4 says to reflect everything. Adding the derive to the handful of M2 components later is cheap, and pulling `bevy_reflect` in now costs compile time for no current user. **Flagged as a law-4 deferral.**
5. **`App` and `Plugin` (in `ge_engine`):**
   - `trait Plugin { fn name(&self) -> &'static str; fn dependencies(&self) -> &[&'static str] { &[] } fn build(&self, app: &mut App); }`
   - `App::add_plugins` collects plugins. `run()` builds them in **dependency order** (a topological sort). A missing dependency or a cycle is a startup error that names the plugins involved.
   - Startup phases from CLAUDE.md §6 keep their order: cvars/command line → logging → plugins → window + RHI + global shaders → `Startup` schedule → loop.
6. **Tick groups.** Each one is a bevy `Schedule`, run in this fixed order per frame:
   - `First`: input actions are updated.
   - `PrePhysics`.
   - `FixedUpdate` (= Physics), run 0..N times by the accumulator.
   - `PostPhysics`, then `Animation`, which stays empty until M8.
   - `PostUpdate`: transform propagation.
   - `Extract`: game state is copied into the `RenderSnapshot` — the **one** sync point (law 2).
   
   `Startup` runs once. Plugins add systems with `app.add_systems(TickGroup::PrePhysics, system)`.
7. **Time and fixed step:**
   - `Time`: delta, elapsed, frame index. A delta above 250 ms is clamped as a hitch, so the game never "jumps".
   - `FixedTime`: step, `overstep_fraction` for interpolation. The rate comes from cvar `p.FixedHz` (default 60).
   - At most 5 fixed steps per frame (spiral-of-death clamp). Leftover time is dropped and logged at debug level.
8. **Input actions (`ge_engine::input`):**
   - `InputMap` resource: action name → bindings (`Key("KeyW")`, `MouseButton(Right)`, and a `Key2D` composite for WASD → `Vec2`). Mouse motion and wheel are axes too.
   - `ActionState` resource: `pressed`, `just_pressed`, `just_released`, `axis2d`, `mouse_delta`, `wheel`. It's rebuilt in `First` from that frame's `PlatformEvent`s.
   - The game declares bindings in code. RON/config bindings come with config layering (M3).
   - **Gamepads (`gilrs`) are deferred to M7** (character controller).
   - Focus loss clears held keys, so keys can't get stuck down.
9. **Hierarchy and transforms:**
   - We use bevy's built-in `ChildOf`/`Children` relationship.
   - `Transform` (translation, rotation, scale) is local; `GlobalTransform` (affine) is world.
   - Propagation runs in `PostUpdate` and only recomputes changed subtrees (`Changed<Transform>` / `Changed<ChildOf>`). `Transform` requires `GlobalTransform` through bevy's `#[require]`, so spawning stays one line.
10. **Rendering through extract:**
    - `ge_render` (rank 7 may depend on `ge_ecs`) gets `MeshRenderer { mesh, tint }` and `Camera3d` components, plus an `extract_render_snapshot` system that fills `RenderSnapshot { view, items }` from `GlobalTransform` + components.
    - The engine hands the snapshot to `Renderer::render`. Gameplay never touches the renderer.
    - Still single-threaded; the render thread arrives in M5.
11. **Exit:** an `ExitRequest` resource. Systems call `exit.request("reason")`, and the loop polls it once per frame after `Extract` (UE's `RequestEngineExit`). Window close and the sample's `Quit` action go through it.
12. **Frame pacing:**
    - Cvar `t.MaxFps` (default 0 = uncapped; vsync still applies). When set, a sleep-then-short-spin limiter runs at the end of the tick.
    - Rust's `sleep` on Windows uses high-resolution waitable timers, so no FFI is needed.
    - The title bar keeps CPU/GPU ms.
13. **Sample game = third-person arena:** the provisional default from CLAUDE.md §11, which must be settled at M2, **so please confirm.**
    - `games/sample` spawns the arena floor, a few pillars and a **player** cube. The player has two child cubes ("shoulders") plus a grandchild that spins, which proves the hierarchy.
    - The camera has right-drag orbit and wheel zoom around the player.
    - A `PlayerController` system in `FixedUpdate` moves the player with WASD relative to the camera's yaw. `Escape` maps to `Quit`.
    - The M1 cube grid becomes a sample-game decoration. The engine's `orbit_camera.rs` and the demo-scene wiring are removed.
    - `ge_render::demo` stays, because the golden-image test uses it.

## Dependencies added

| Crate | Where | Why |
|---|---|---|
| `bevy_ecs 0.20` (`default-features = false`, `std`; `trace` behind `tracy`) | `ge_ecs` only | In the locked stack (§4). MSRV 1.95 is fine on our 1.97.1. |
| `glam` | `ge_ecs` | Already in the workspace. |

Nothing new outside the §4 table.

## Files to touch

```
Cargo.toml                         # members += games/*, bevy_ecs dep, ge_ecs, sample
crates/ge_ecs/{Cargo.toml, src/lib.rs, src/transform.rs, src/time.rs}            # new
crates/ge_engine/src/{lib.rs, app.rs, plugin.rs, schedule.rs, input.rs, pacing.rs} # app.rs etc. new
crates/ge_engine/src/orbit_camera.rs                 # removed (moves to the sample as gameplay)
crates/ge_render/src/{lib.rs, components.rs}         # MeshRenderer, Camera3d, extract system
games/sample/{Cargo.toml, src/lib.rs, src/player.rs, src/camera.rs, src/arena.rs} # new
apps/game/src/main.rs                                # App::new()...run()
xtask/src/main.rs                                    # rank: ge_ecs already 5; add `sample` path check
CLAUDE.md §4 row (if decision 2 approved), §9 status, §11 (vertical slice settled)
```

## Out of scope

Render thread, pipelined extract and the render graph (M5); assets (M3); reflection/inspector (M6); physics and the character controller (M7); gamepad; the in-game console; `subsecond` (M11).

## Acceptance criteria (M2 exit criteria)

- [ ] `cargo xtask ci` passes, including the layering check with `ge_ecs` and `sample`. No crate except `ge_ecs` depends on `bevy_ecs`.
- [ ] `apps/game/src/main.rs` contains no game logic: it builds an `App`, adds plugins and runs.
- [ ] Unit tests:
  - plugin dependency ordering, plus missing-dependency and cycle errors;
  - tick groups run in the CLAUDE.md §6 order;
  - the fixed-step accumulator (step count, max-steps clamp, overstep fraction);
  - hitch clamp;
  - input action edges (pressed/just_pressed/just_released, focus loss clears, WASD composite normalizes diagonals);
  - transform propagation for a three-level hierarchy, including reparenting;
  - `ExitRequest` keeps the first reason.
- [ ] The golden-image test still passes (renderer output unchanged).
- [ ] Manual: WASD moves the player relative to the camera, the children follow and the grandchild spins, right-drag orbits, the wheel zooms, Escape quits cleanly through `ExitRequest`.
- [ ] Movement is frame-rate independent: the same distance per second at `-cvar t.MaxFps=30` as uncapped. A test drives the fixed step directly, and the manual check covers the rest.

## Manual test steps

1. Switch to the **Turbo** power plan, then run `cargo run -p game`. Expect the arena, the player cube with its child cubes, and CPU/GPU ms in the title.
2. Hold W for about 3 s and note how far the player moves. Then run `cargo run -p game -- -cvar t.MaxFps=30` and repeat. The distance should be about the same.
3. Right-drag to orbit and scroll to zoom. WASD should stay relative to the camera.
4. Alt-tab away while holding W. The player should stop, not keep walking.
5. Press Escape. The log should show `exit requested reason="quit action"` and a clean shutdown.
