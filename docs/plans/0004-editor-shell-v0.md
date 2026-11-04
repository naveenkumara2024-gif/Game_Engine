# 0004 — Editor shell v0 (Blender-style properties, floating canvas layout)

**Status:** approved 2026-11-04, implemented 2026-11-04 (all automated checks green; parts of the manual walkthrough verified by scripted screenshots, the mouse-driven parts are for you) · **Milestone:** pulls a slice of **M6 (Editor shell)** ahead of M3 (CLAUDE.md §9).

**Deviations from this plan:**
- **Play / Stop snapshots the scene generically.** Instead of typed records of a known component list, Play clones every scene tree with bevy's entity cloner (`ge_ecs::clone_tree`) and disables the copies (`ge_ecs::Disabled`, which default queries skip). Stop despawns the played scene and re-enables the copies. Game components such as the sample's `Player` and `FixedMotion` survive as long as they implement `Clone`. Entity ids change, so selection and undo history are cleared on Play and on Stop.
- **Delete doesn't despawn.** Delete disables the subtree, and undo re-enables it. The entity is only despawned once its edit drops out of the history.
- **Color management uses uniforms, not `@if` permutations.** Exposure, gamma, white balance, view transform and shading mode are read at run time, so changing them never rebuilds a pipeline. `r.DebugNormals` is still a permutation.
- **The renderer API is additive.** `RenderView` and `Renderer::render` are unchanged (golden image identical); the new path is `Renderer::render_frame(FrameInputs)`.
- **Golden image (b), the lit sphere, became pixel assertions** in `crates/ge_render/tests/passes.rs`, alongside the UI-pass scissor/texture test and a grid test. They hold across GPUs better than an image diff.
- **`Name` is a `Cow<str>` component,** not an interned `ge_core::Name`, because renaming on every keystroke would leak interned strings.
- **Single selection only** (no box select yet); Q is click-select. Modal G/R/S with axis lock (stretch goal) and drag-to-reparent in the outliner are not done.
- **Display Device became Planned (M12),** since we always output sRGB.
- **Editor defaults:** Rendered shading and the AgX view transform, which is Blender's default. The standalone game keeps Solid shading and Standard, so it looks exactly as before. The sample gained a Sun light (2 W/m²), which is used only in Rendered shading.
- **UI cost is shown live** as "UI x ms" in the stats pill, measured around the `Editor::Ui` system. It also has a `tracing` span for Tracy.
- **Dependency note:** `transform-gizmo` 0.11 brings a second `glam` (0.32). The editor converts through `mint` values at the gizmo boundary only. egui is pinned to 0.36.2, the newest version both transform-gizmo-egui 0.11 and egui-phosphor 0.14 support.
- **Engine seam added:** `EXIT_WINDOW_CLOSED`, so Play mode can turn a game's quit into Stop while closing the window still exits.

## Goal

Ship a first editor, `apps/editor`, that a beginner can open without feeling overwhelmed and use to **learn real-time rendering the way EEVEE teaches it**:

- The scene fills the whole window. A few floating panels sit on top of it, laid out like the reference screenshot: a centred tool bar on top, a properties panel on the left, small clusters in the corners.
- **Add ▸ Mesh / Light / Camera / Empty** works like Blender's Shift+A.
- The left panel is a **Blender-style properties editor** with Render, World, Object, Data (Mesh / Light / Camera) and Material tabs. The Render tab lists **every EEVEE setting from Blender 5.2**, in Blender's panel order.
- Each setting is either **Live** (it changes our renderer now) or marked **Planned (Mx)**. A Planned setting is greyed out but still has a tooltip explaining what it does in EEVEE and which milestone makes it real. So the panel doubles as a study guide and as a visible roadmap.
- You can select, move, rotate and scale objects with undo/redo, see the scene in an outliner, and press **Play / Pause / Stop** to run the sample arena in the editor. Stop restores the edited scene.

v0 is done when `cargo run -p editor` opens the arena, a user can add a UV sphere and a spot light, change the light's colour, power and cone and see it shade the sphere, change **Exposure** and **View Transform** in Render ▸ Color Management and see the frame change, undo all of it, and Play/Stop the arena game.

## Lore read

§3.11 and §6.10 (editor), §3.4 and §6.6 (reflection), §3.7 and §6.7 (rendering), §6.3 (frame lifecycle), §2.13 (layering), §4.4 (field lessons), §7 (pitfalls), §8 (M4/M6/M12 exit criteria). New field notes on Blender are below; they get added to lore as §2.16.

## Blender 5.2 field notes (static only, CLAUDE.md §13)

**Target:** the portable `blender-5.2.3-candidate` Windows build in `Downloads`, analysed statically. REA/Ghidra timed out on the 113 MB `blender.exe` (two attempts, 330 s deadline). Evidence therefore comes from a raw string scan of the binary **[SCAN]** and from the shipped UI scripts `5.2/scripts/startup/bl_ui/*.py` **[DOC]**.

**Clean room:** Blender is GPL-3.0-or-later. We take **only architecture facts and UI vocabulary** (panel names, panel order, setting names). No code, no icons, and no tooltip text get copied. We write every tooltip ourselves.

| # | Finding | Evidence | What we do with it |
|---|---|---|---|
| B1 | The data layer and the reflection layer are separate modules. `makesdna` holds plain structs (DNA). `makesrna` puts properties on top of them (RNA: name, range, unit, description), and the UI, Python, animation and undo all bind to RNA. | [SCAN] 81 embedded `source/blender/makesrna/intern` paths, `RNA_def_property` ×9 | This confirms law 4. Until `ge_reflect` lands (M6), v0 uses a small **property schema table**, a mini-RNA, and every panel row is generated from it. |
| B2 | Panels are **declarative**. Each one has an id, a label, an optional parent (for sub-panels), an optional checkbox in its header (`use_motion_blur` etc.), a `poll()` that decides whether it shows, and a list of compatible render engines. The properties editor shows only the tabs that fit the active object. | [DOC] `properties_render.py`, `properties_data_light.py` (`bl_parent_id`, `COMPAT_ENGINES`, header `prop(..., text="")`) | `PanelRegistry` with `PanelDesc { id, label, tab, parent, header_toggle, poll, draw, default_open }`. Plugins register panels (law 9). Tabs and panels hide when they don't apply, which is the main tool for keeping the UI from overwhelming people. |
| B3 | Every button, menu entry and shortcut runs an **operator** (poll / invoke / exec / modal, plus an undo flag). Keymaps map keys to operators, and an undo system records each step. | [SCAN] `windowmanager/intern` ×76, `wm_operator` ×11, `editors/undo` ×11, `BKE_undosys` ×6, `ED_undo_push`, `WM_keymap` | `Operator` registry + `CommandStack`. The tool bar, the Add menu, shortcuts and the **F3 command search** all call the same operators, so every action is undoable and searchable. |
| B4 | Edited data and evaluated data are different copies (the depsgraph's copy-on-evaluation). | [SCAN] `depsgraph/intern` ×9 | Same idea as our extract (law 2). The editor edits components, and the renderer only sees the `RenderSnapshot`. |
| B5 | The viewport combines separate **draw engines**: the render engine (EEVEE), Workbench, and an Overlay engine for grid, selection and gizmos. | [SCAN] `draw/engines` ×19, `draw/intern` ×9 | The grid and selection overlays are their own pass after the forward pass, so the lighting path stays clean. |
| B6 | The GPU layer sits behind a backend interface, with Vulkan and OpenGL backends. | [SCAN] `gpu/vulkan` ×67, `gpu/opengl` ×10, `gpu/intern` ×20 | Confirms law 12 and the `ge_rhi` seam. |
| B7 | **Add menu** (Shift+A) ▸ Mesh: Plane, Cube, Circle, UV Sphere, Ico Sphere, Cylinder, Cone, Torus. Light: Point, Sun, Spot, Area. Plus Camera, Empty and more. | [DOC] `space_view3d.py` `VIEW3D_MT_mesh_add` | Our Add menu (below) shows only the categories that work. |
| B8 | The EEVEE panel tree in 5.2 (full list in "Render tab" below). Bloom, SSR and the "Soft Shadows" toggle from EEVEE Legacy are gone; 5.x uses Raytracing, Fast GI and virtual shadow maps instead. | [DOC] `properties_render.py` (`RENDER_PT_eevee_*`) | We mirror the 5.2 tree, not the Legacy one. |

## Layout (from the reference screenshot)

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ [☰]          [ ⬚Q ✥G ⟳R ⤢S │ ＋Add▾ │ 🌐Global▾ 🧲Snap │ ◐Shading▾ │ ⋮ ]   [⏵ Play] [▤]   │
│                 "G move · R rotate · S scale · Shift+A add · F3 search"                │
│ ┌──┬──────────────────┐                                                ┌───────────┐ │
│ │🎥│ Render           │                                                │ Outliner  │ │
│ │🌍│ ▸ Sampling       │                 full-window 3D viewport        │ ▾ Arena   │ │
│ │▣ │ ▸ Raytracing  ☐  │              (grid, axes, selection box,       │   Floor 👁│ │
│ │△ │ ▾ Color Mgmt     │               light/camera icons, gizmo)       │ ▸ Player 👁│ │
│ │● │   View  AgX ▾    │                                                │   Sun   👁│ │
│ │  │   Exposure ━━○━  │                                         (X Y Z)│           │ │
│ └──┴──────────────────┘                                          nav   └───────────┘ │
│ [− 50° +] [↶ ↷]                     RTX 3050 · Vulkan · 2.1/1.4 ms · 37 obj   [ⓘ][?]  │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

| Screenshot region | Becomes |
|---|---|
| ☰ top-left | **Main menu:** File (New Scene, Open/Save: *Planned M6*), Edit (Undo, Redo, Undo History, Preferences), View (toggle each panel, Reset Layout), Help (Shortcuts, About). |
| Centred tool bar | **Tools:** Select Box (Q), Move (G), Rotate (R), Scale (S). **Add ▾** (Shift+A). Transform orientation Global/Local, Snap toggle with increment. **Viewport shading ▾:** Solid / Normals / Lit (Blender's Solid / Material / Rendered). **⋮** opens Overlays (Grid, Axes, Light & camera icons, Selection bounds, Stats). |
| Hint line under the bar | Context hints like Excalidraw's "Click and drag…". They change with the tool and the modal state, e.g. "Move · X/Y/Z lock axis · Ctrl snap · Esc cancel". |
| "Share" (accent colour, top right) | **⏵ Play** in the accent colour. While playing it turns into a **❚❚ Pause · ■ Stop** pill. |
| Library/sidebar toggle | Opens the **Outliner** as a right sidebar. |
| Left style panel | **Properties editor:** a vertical icon tab strip (Render 🎥, World 🌍, Object ▣, Data △/💡/📷 depending on the object type, Material ●) plus a scrollable, resizable panel. N toggles it. Tabs appear only when they apply (B2). |
| Zoom − 100% + (bottom left) | **View cluster:** field of view − 50° +, then Undo/Redo (↶ ↷). |
| Bottom right | **Stats** (adapter, backend, CPU/GPU ms, fps, object count), **ⓘ** to toggle the stats overlay, **?** for a shortcuts sheet. |
| (new) | **Navigation gizmo** bottom-right of the viewport: clickable X/Y/Z axis dots, Blender style. |

**Theme:** dark, with tokens matching the reference: canvas `#121212`, panel `#232329`, panel border `#2e2d39`, text `#e3e3e8`, muted text `#8f8f9d`, accent `#a8a5ff`, accent background `#403e6a`, selection `#ffa53a`. The tokens live in one `theme.rs`. Panels have 8 px rounded corners and a soft shadow, as in the screenshot. A UI scale slider is under Edit ▸ Preferences.

**Not overwhelming (design rules):**
1. Only tabs and panels that apply are shown (B2). Sub-panels start closed except the first.
2. Each Render section keeps Blender's sub-panel tree, but the advanced ones (Clamping, Denoising, Memory, Simplify) sit behind a **"Show advanced"** toggle at the top of the tab.
3. Every row has a tooltip. Hovering a Planned row shows *"In EEVEE: … · Here: Planned (M4 shadows)."*
4. **F3 command search** finds any operator or setting by name.
5. Defaults match Blender's, so a Blender tutorial's numbers mean the same thing here.

## Properties tabs: what each setting does in v0

**Live** changes the frame or the scene now. **Mx** means planned for that milestone (greyed out with a tooltip). **n/a** means we have no such feature planned (shown greyed only under "Show advanced").

### Render tab (EEVEE 5.2 tree, B8)

- **Render Engine:** "Forward+ (EEVEE-style)", the only entry.
- **Sampling** (M12 TAA)
  - Viewport: Samples, Temporal Reprojection, Jittered Shadows
  - Render: Samples
  - Advanced: Light Threshold — **M4**
  - Shadows (header toggle): Rays, Steps, Volume Shadows + Steps, Resolution — **M4**
- **Clamping**
  - Surface: Direct Light, Indirect Light — M4 / M12
  - Volume: Direct Light, Indirect Light — T3
- **Light Paths ▸ Intensity:** Direct Light — **Live** (global direct-light multiplier); Indirect Light — M12
- **Raytracing** (header toggle) — **M12** (SSR/SSGI)
  - Method (Light Probe / Screen-Trace), Resolution, Max Roughness
  - Screen Tracing: Precision, Thickness, Backface Hit + radiance scale
  - Denoising: Spatial Reuse, Temporal Accumulation, Bilateral Filter
  - Fast GI Approximation (header): Threshold, Method (AO / Global Illumination), Resolution, Rays, Steps, Precision, Distance, Thickness Near/Far, Bias
- **Volumes** — T3: Resolution, Steps, Distribution, Max Depth, Custom Range (Start/End)
- **Curves** — n/a: Shape, Additional Subdivision
- **Depth of Field** — M12: Max Size, Sprite Threshold, Neighbor Rejection, Jitter Camera + Overblur
- **Motion Blur** (header toggle) — M12: Position, Shutter, Shutter Curve
- **Film:** Filter Size — M12; Transparent — M5; Overscan + Size — M12
- **Performance:**
  - High Quality Normals, Anisotropic Filter — M3/M4 (samplers)
  - Memory: Shadow Pool — M4; Light Probes Volume Pool — M12
  - Viewport: Pixel Size — M5 (render scale)
  - Compositor — n/a
- **Simplify** — n/a: Max Subdivision, Max Child Particles, Volume Resolution, Normals (LODs come at M12)
- **Color Management:**
  - **Live:** Display Device (sRGB); View Transform (Standard, AgX, Khronos PBR Neutral; Filmic and False Color are M4); Look (None for now); **Exposure**; **Gamma**; White Balance (header toggle) with Temperature and Tint
  - M4: Curves, Working Space

### World tab

- **Surface:** Color, Strength — **Live** (background colour plus a flat ambient term; replaces `r.ClearColor` in the editor)
- **Volume** — T3
- **Settings:**
  - Light Probe Resolution — M4 (IBL)
  - Sun: Threshold, Angle — M4
  - Sun Shadow: toggle, Jitter, Overblur, Filter, Resolution Limit — M4
- **Mist Pass** — n/a
- **Viewport Display:** Color — **Live**

### Object tab (any selected object)

- **Live:**
  - Name
  - Transform: Location, Rotation (XYZ Euler in degrees, or Quaternion), Scale
  - Relations: Parent, read-only for now (reparent by dragging in the Outliner)
  - Visibility: Show in Viewport
  - Viewport Display: Show Name, Show Axes, Show Bounds
- **Delta Transform** — M6
- **Shading** (cast shadow, etc.) — M4

### Data tab, mesh (△)

- **Primitive** — **Live:** the generator settings, regenerated on change, like Blender's "Adjust Last Operation" panel
  - Size; Radius / Depth
  - Segments / Rings; Ico subdivisions
  - Torus major/minor radius and segments
  - Cylinder/Cone fill type
- **Shade Smooth / Flat** — **Live**
- **Stats** (vertices, triangles) — **Live**
- Imported glTF meshes — M3

### Data tab, light (💡)

- **Live:**
  - Type: Point / Sun / Spot / Area
  - Temperature (on/off + Kelvin, blackbody colour)
  - Color / Tint, **Power (W)** (Strength for Sun), Exposure, Normalize
  - Custom Distance (cutoff)
  - Influence: Diffuse, Glossy
  - Beam Shape: Spot Size, Spot Blend, Show Cone
- **M4:**
  - Radius / Soft Falloff (point and spot), Angle (sun)
  - Area Shape / Size. Until then an area light is shaded as a spot with a 180° cone; the panel says so.
  - Shadow: toggle, Jitter + Overblur, Filter, Resolution Limit
- **T3:** Influence: Transmission, Volume Scatter

### Data tab, camera (📷)

- **Live:**
  - Lens: Type (Perspective), Focal Length ⇄ Field of View, Clip Start
  - "Set Active Camera" (the camera used in Play)
- Orthographic and Shift X/Y — M6
- Depth of Field — M12
- Clip End is n/a: reversed-Z gives an infinite far plane, and the tooltip explains why

### Material tab (●) — the Principled BSDF subset

- **Surface:**
  - **Live:** Base Color, Metallic, Roughness, Emission Color + Strength
  - M4: Alpha
  - M12: IOR, Subsurface, Specular (IOR Level, Tint), Transmission, Coat, Sheen, Thin Film
- **Settings:**
  - Backface Culling (Camera) — **Live**
  - Backface Culling (Shadow) — M4
  - Render Method (Dithered / Blended), Transparent Shadows — M4
  - Displacement, Thickness, Raytraced Transmission — M12
  - Pass Index — n/a
- **Viewport Display:** Color, Metallic, Roughness — **Live**

**Physics, Modifiers, Constraints and Particles tabs are not shown** until their milestones (M7 and later). Hiding them is part of keeping the UI calm.

## Decisions & assumptions (flagged)

1. **Roadmap order.** This takes the M6 editor *shell* (layout, selection, undo, gizmos, outliner, Play/Stop) ahead of M3. M6 itself still owns the reflection inspector, RON scene save/load and full Play-In-Editor. CLAUDE.md §9 gets a line saying "0004 editor shell v0 done ahead of M3" when this is finished. **Needs your OK.**
2. **Lighting v0 (pulls the first step of M4 forward).** This is a forward pass with up to 16 point/spot/sun/area lights, GGX specular + Lambert diffuse, a flat ambient term, and no shadows, IBL or clustering yet. Without it the Light and Material tabs would do nothing. M4 builds clustering, shadows and IBL on top of it rather than replacing it. Lights come from new `Light` components through the extract (law 2). **Needs your OK.**
3. **Tone mapping happens in the forward shader for now.** There's no HDR target until M4, so Exposure, View Transform (Standard / AgX / Khronos PBR Neutral), Gamma and White Balance are applied at the end of the mesh fragment shader in linear space, as WESL `@if` permutations. M4 moves this into a proper post pass, and the settings keep their names.
4. **No reflection yet: a property schema table instead (B1).** `ge_render::settings` defines `RenderSettings`, `WorldSettings` and so on as plain structs, plus a `const` schema: path, label, EEVEE name, tooltip, range, unit, default, status (`Live` / `Planned(Milestone)` / `NotApplicable`). The UI generates every row from the schema. Unit tests check that every `Live` row has a consumer and every row has a tooltip. At M6 the schema becomes `#[derive(Reflect)]` attributes, so plan this API to be thrown away.
5. **Live render settings are cvars too (law 8).** Each live scalar gets an `r.*` cvar, for example `r.Exposure`, `r.ViewTransform`, `r.DirectLightIntensity`. The panel writes the cvar, so the console, the command line and the UI share one value. Planned rows get no cvar until they're real.
6. **egui behind our own wiring, no `egui-wgpu` or `egui-winit` (law 12).** `egui-wgpu` would make a second crate call `wgpu`. Instead:
   - `ge_editor` turns egui's tessellated output into a POD `ge_render::UiDrawList` (vertices, indices, batches with a clip rect and texture id, texture deltas). The list travels in the `RenderSnapshot` (law 2).
   - `ge_render` draws it in a **UI pass** through `ge_rhi`. That's about 250 lines, and the same pass serves the M9 game UI.
   - `ge_editor` turns `PlatformEvents` into `egui::RawInput` itself.
   - New facade rule in xtask: only `ge_editor` may depend on `egui*` / `transform-gizmo-egui`.
7. **Floating layout, not docking.** The reference is a full-canvas layout with floating panels, so v0 uses egui `Area`s and `Window`s with fixed anchors. `egui_dock` (in the stack) is held back until M6, where users ask for rearrangeable panels. The scene renders straight to the swapchain and the UI draws on top. Render-to-texture viewports also come at M6.
8. **Gizmos:** `transform-gizmo-egui` (in the stack). **Picking:** a CPU ray against each object's oriented bounding box. A pixel-accurate ID pass comes at M6. **Selection highlight:** an orange projected bounding box drawn by the egui painter. An outline pass comes at M6.
9. **Transform tools behave like gizmo modes in v0:** Q / G / R / S switch the gizmo. Blender's *modal* G-move-with-mouse plus X/Y/Z axis lock is a stretch goal inside this plan, built on the same operator (step 7). Default: gizmo modes first.
10. **Play / Pause / Stop (Play-In-Editor v0):**
    - A `SimulationState { Editing, Playing, Paused }` resource, defaulting to `Playing`, so `apps/game` doesn't change.
    - While `Editing` or `Paused`, `App::update` skips PrePhysics, FixedUpdate, PostPhysics and Animation. First, Ui, PostUpdate and Extract still run.
    - **Play** snapshots every non-editor entity's known components (Name, Transform, ChildOf, MeshRenderer, Material, Light, Camera3d, Visibility) into typed records. **Stop** despawns them and respawns from the snapshot.
    - Components outside that list are **not** restored. The full world clone comes with reflection at M6, and the limit is documented in code.
    - While playing, the game's `ExitRequest` (Escape → Quit) means **Stop**, not "close the editor". The editor calls a new `ExitRequest::cancel()`. Shift+Esc also stops (Unreal's PIE key); Alt+P plays.
11. **Editor camera:** a separate `EditorViewCamera` entity, excluded from the outliner and the snapshot.
    - Controls: MMB orbit, Shift+MMB pan, wheel zoom, F or Numpad-. frames the selection, Numpad 1/3/7 for front/right/top views.
    - The extract uses `ActiveCamera`: the editor camera while editing, the scene's active `Camera3d` while playing.
12. **Primitive meshes:** `MeshKind` grows into `MeshKind::Primitive(PrimitiveDesc)`, with CPU generators for Plane, Cube, Circle, UV Sphere, Ico Sphere, Cylinder, Cone and Torus, using Blender's default sizes. The renderer caches GPU meshes by descriptor. This is a bridge to `Handle<Mesh>` at M3.
13. **Icons:** `egui-phosphor` (MIT, icon font, no other dependencies). **It's a new dependency outside the §4 table:** egui has no icon set, and Blender's icons are GPL, so we can't use those. **Needs your OK.**
14. **Names:** a new `Name` component in `ge_ecs` (interned through `ge_core::name`). The sample's entities get names so the outliner reads well.
15. **No persistence in v0.** File ▸ Save/Open are greyed with "Planned (M6, RON scenes)". Preferences live only for the session.

## Engine seams this needs (small, generic, not editor-specific)

| Crate | Change |
|---|---|
| `ge_platform` | New `PlatformEvent` variants: `CursorMoved { x, y }` (window pixels), `Text(String)` (IME/committed text), `Modifiers { shift, ctrl, alt }`, and `MouseWheel` pixel deltas. egui needs all of these, and today's events don't have them. |
| `ge_rhi` | Alpha blending in `RenderPipelineDesc`, `set_scissor_rect` on `RenderPass`, sampled 2D textures with upload (`write_texture`), and samplers. Still no `wgpu` types in the public API. |
| `ge_render` | `UiDrawList` + UI pass; grid/axes overlay pass (infinite grid shader); `Light`, `Material` components plus lighting v0 + tone mapping permutations; `settings` schema module; primitive generators; `ActiveCamera`. |
| `ge_ecs` | `Name` component; `Visibility` component. |
| `ge_engine` | `SimulationState` gate in `App::update`; new `TickGroup::Ui` between Animation and PostUpdate (UI edits propagate the same frame); an `EngineStats` resource (adapter, backend, CPU/GPU ms, fps), filled by the runner instead of only the title bar; `ExitRequest::cancel()`; `WindowInfo` resource (size, scale factor). |
| `xtask` | Facade rule `("egui", "ge_editor")`; `ge_editor` (rank 10) and `editor` (rank 11) are already in the table. |

## New crate: `ge_editor` (rank 10, depends on `ge_engine` only among `ge_*` crates)

```
crates/ge_editor/src/
  lib.rs           EditorPlugin (deps: ge.input, ge.transform, ge.render)
  egui_glue.rs     PlatformEvents → RawInput; egui output → ge_render::UiDrawList
  theme.rs         colour tokens, spacing, rounding, fonts, UI scale
  layout.rs        the floating regions (tool bar, hint, left panel, outliner, corners)
  operators.rs     Operator trait (id, label, shortcut, poll, exec) + registry + keymap
  commands.rs      CommandStack: undo/redo, merge-while-dragging, history list
  selection.rs     Selection resource, picking (ray vs OBB)
  viewport.rs      editor camera, navigation gizmo, overlays (bounds, light/camera icons)
  gizmo.rs         transform-gizmo-egui wiring → commands
  add_menu.rs      Shift+A menu → AddObject operators
  outliner.rs      tree, visibility eye, rename, delete, drag-reparent
  properties/      mod.rs (tab strip, PanelRegistry), render.rs, world.rs, object.rs,
                   mesh.rs, light.rs, camera.rs, material.rs  — rows generated from the schema
  search.rs        F3 command search
  play.rs          SimulationState control, snapshot/restore
apps/editor/       main.rs: App::new().add_plugins(DefaultPlugins).add_plugins(EditorPlugin)
                            .add_plugins(sample::SamplePlugin).run()   (+ dGPU exports, build.rs)
```

## Dependencies to add

| Crate | Where | Why |
|---|---|---|
| `egui` (core only, default features off except what we need) | `ge_editor` | Editor UI, in the §4 stack. |
| `transform-gizmo-egui` | `ge_editor` | Gizmos, in the §4 stack. Check before adding that it doesn't pull in `wgpu` or a second `glam`, and that its egui version matches. |
| `egui-phosphor` | `ge_editor` | Icon font. **Outside the §4 table (decision 13).** |

Versions are pinned to the newest releases that agree with each other at implementation time, then recorded in this plan's deviations. No new dependency for `ge_rhi` or `ge_render`.

## Build order (each step ends with `cargo xtask ci` green and is demoable)

1. **Engine seams:** platform events, `SimulationState`, `TickGroup::Ui`, `EngineStats`, `WindowInfo`, `ExitRequest::cancel`, `Name`/`Visibility`. Tests first.
2. **RHI + UI pass:** blending, scissor, texture upload, sampler; `UiDrawList` + UI pass + a golden test; grid overlay pass.
3. **`ge_editor` skeleton + `apps/editor`:** egui wiring, theme, every floating region in place with placeholder contents, stats corner live. *Demo: the arena with the editor chrome around it.*
4. **Viewport + selection:** editor camera, navigation gizmo, picking, bounds highlight, outliner, gizmo modes, CommandStack undo/redo.
5. **Add menu:** primitive generators, Light/Camera/Empty entities, viewport icons for lights and cameras, Data ▸ Mesh generator settings, Delete, Duplicate (Shift+D), Hide (H / Alt+H).
6. **Lighting v0 + Material + Color Management** made Live; Light and Material tabs wired up.
7. **Full properties:** every tab and panel generated from the schema, Planned badges and tooltips, Show advanced, F3 search. Stretch: modal G/R/S with axis lock.
8. **Play / Pause / Stop** with snapshot/restore; the game's Quit maps to Stop.
9. **lore §2.16** (Blender field notes above); CLAUDE.md §9 status line.

## Files to touch

```
Cargo.toml                                    # egui, transform-gizmo-egui, egui-phosphor, ge_editor
crates/ge_platform/src/lib.rs                 # new events
crates/ge_rhi/src/{resources.rs,pass.rs,wgpu_backend.rs}   # blend, scissor, textures, samplers
crates/ge_render/src/{lib.rs,components.rs,renderer.rs,mesh.rs}
crates/ge_render/src/{ui_pass.rs,grid.rs,lighting.rs,settings.rs,primitives.rs}   # new
shaders/{mesh.wesl, ui.wesl, grid.wesl, common/lighting.wesl, common/tonemap.wesl}
crates/ge_ecs/src/{lib.rs, name.rs}
crates/ge_engine/src/{app.rs,schedule.rs,runner.rs,plugins.rs,lib.rs}
crates/ge_editor/**                           # new
apps/editor/{Cargo.toml,build.rs,src/main.rs} # new
games/sample/src/*.rs                         # Name components; Light instead of baked colours
xtask/src/main.rs                             # egui facade rule
lore.md §2.16, CLAUDE.md §9
```

## Out of scope (stays in its milestone)

- Docking and rearrangeable panels, render-to-texture viewport, reflection inspector, RON scene save/load, full world-clone PIE (M6)
- glTF import and an asset browser (M3)
- Shadows, IBL, clustered lights, HDR post pass (M4)
- Render thread, render graph (M5)
- Physics tab (M7)
- Node-based material editor (not planned)

## Acceptance criteria

- [ ] `cargo xtask ci` passes. Only `ge_editor` names `egui*`, only `ge_rhi` names `wgpu`, and only `ge_ecs` names `bevy_*`.
- [ ] `apps/game` behaves exactly as before. The existing golden image and the headless sample test still pass, because `SimulationState` defaults to `Playing`.
- [ ] Unit tests:
  - CommandStack (undo, redo, drag merge, redo cleared by a new edit)
  - ray-OBB picking
  - each primitive generator (vertex/index counts, unit normals, outward winding)
  - schema integrity (every row has a tooltip and an EEVEE name; every `Live` row has a cvar and a consumer)
  - tone-map functions (Standard at exposure 0 is identity before gamma; AgX/PBR Neutral are monotonic)
  - Play→edit→Stop restores transforms, adds and deletes
  - `ExitRequest::cancel`
  - `SimulationState` skips the gameplay tick groups
- [ ] New golden images: (a) the UI pass draws a fixed batch with scissor clipping; (b) a sphere lit by one sun and one red point light.
- [ ] Manual: the walkthrough in "Goal" works end to end on the RTX 3050 (Turbo plan). The editor's UI cost is ≤ 1.0 ms CPU at 1080p, measured with the `Editor::Ui` span in Tracy or the stats corner.

## Manual test steps

1. Switch to the Turbo power plan and run `cargo run -p editor`. Expect the arena on a dark canvas with the floating tool bar, the left properties panel, the stats corner showing the RTX 3050, and the outliner listing Floor / Player / Pillars.
2. Shift+A ▸ Mesh ▸ UV Sphere. It appears at the origin, selected, with an orange box. Data ▸ Primitive: set Segments to 8 and it regenerates.
3. Shift+A ▸ Light ▸ Spot. Set Power to 1000 W and Color to red, and widen Spot Size. The sphere and floor take on a red cone of light.
4. Render ▸ Color Management: switch View Transform Standard → AgX (highlights roll off) and drag Exposure (brightness changes). Hover "Raytracing" and read the Planned (M12) tooltip.
5. Press G and drag the gizmo, then Ctrl+Z several times. Each step undoes in order and Ctrl+Shift+Z redoes.
6. Alt+P plays: WASD moves the player. Esc returns to Editing (the editor stays open) and the sphere and light are still there. Press Play again, then Stop: the player is back at its start position.
7. F3, type "exposure": the setting is found and focused.
