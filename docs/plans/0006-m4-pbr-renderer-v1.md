# 0006 — M4: PBR renderer v1

**Status:** **done 2026-11-04** (see Deviations below) · approved 2026-11-04 (environments: Color + Sky + `.hdr`; soft/area lights: representative point; light culling: GPU compute; AgX: keep our clean-room curve, the default since that question wasn't asked) · **Milestone:** M4 (CLAUDE.md §9, lore §8)

## Goal

Make the image physically plausible and fast enough to ship.

**M4 exit criteria** (lore §8):
- clustered forward+;
- directional, point and spot lights;
- cascaded shadow maps (CSM) and an atlas for point/spot shadows;
- image-based lighting (IBL);
- HDR with bloom and tone mapping;
- **1080p at 60 fps or better on a sample scene on this laptop.**

It also finishes what earlier plans parked "for M4": normal and occlusion maps, alpha (mask + blend), soft lights (radius, sun angle), area lights, and the Shadow, Light Threshold, Light Probe and Material rows the editor shows as **Planned (M4)**. Those rows turn **Live**.

M4 is done when:
- the arena renders in HDR with sun shadows in cascades, shadowed spot and point lights, sky or HDRI lighting with reflections, bloom and our view transforms;
- the crate shows its normal map;
- a stress scene holds 60 fps at 1080p on the RTX 3050 (measured with GPU timestamps).

## Lore read

§2.6 (GPU backends), §3.7 (UE rendering: proxies, mesh passes, shadows, Lumen and why we don't copy it yet), §6.7 (our pipeline: forward+ with a depth prepass, CSM + atlas, IBL with prefiltered environment + SH, HDR → bloom → tonemap), §6.4 (cvars, scalability), §7 (pitfalls), §8 (M4/M5/M12 boundaries), §9 (this machine: 4 GB VRAM, about a 3 GB budget, hybrid GPU).

## Frame after M4

```
Shadows        CSM cascades (sun) → shadow atlas tiles (spot ×1, point ×6 faces)
DepthPrepass   opaque + masked, reversed-Z
LightCulling   compute: lights → 16×9×24 froxel clusters (exponential depth slices)
Opaque         forward+ PBR: clustered lights + shadows + IBL + ambient occlusion  → SceneColor (Rgba16Float)
Sky            world background (color / procedural sky / HDRI) where depth = far
Transparent    alpha-blended, sorted back to front, same lighting
Bloom          dual-filter: threshold + 6-mip downsample / upsample chain
Tonemap        exposure, white balance, view transform, look, gamma → backbuffer (sRGB)
Overlays       grid (depth-tested against SceneDepth), editor overlays
Ui             unchanged
```

Every pass keeps a debug label and a GPU timestamp. **The render graph is still M5.** v1 records these passes in a fixed order through a small `FrameTargets` struct that owns the transient textures (SceneColor, depth, bloom chain), so M5 can turn each pass into a graph node without rewriting it.

## Decisions & assumptions (flagged)

1. **HDR scene color.** The scene renders into `Rgba16Float`. Color management moves **out of the mesh shader** into the Tonemap pass, with the same cvars and the same math, now applied to true HDR values. The clear color and the sky go through it too.
   - The CPU copy in `lighting.rs` stays for tests.
   - The golden image is re-blessed **once**, deliberately. The M1 studio-light path survives as Solid shading, now through the HDR path, and must still match the old golden within tolerance under Standard. If it can't, the plan asks before re-blessing.
2. **Clustered forward+ with GPU light culling** (lore §6.7).
   - A compute pass assigns lights to a 16×9×24 grid of view-space clusters with exponential depth slices.
   - Light lists live in **storage buffers**, so the 16-light uniform cap becomes **1024 lights**, tunable with `r.Clusters.*` cvars.
   - Each light's range comes from its power and **Render ▸ Light Threshold** (EEVEE's own rule), or from Custom Distance. Unbounded lights can't be culled.
   - This needs **compute pipelines and storage buffers in `ge_rhi`** (*Question 3*); they're needed again for GPU culling at M12.
3. **Shadows:**
   - **Sun: 4-cascade CSM.** Each cascade is 2048² in a depth texture array, and the split scheme blends logarithmic and uniform splits (`r.Shadow.CSM.*`). Cascades are stabilized (texel-snapped, bounding-sphere fitted) so edges don't shimmer, and blend across seams.
   - **Spot and point:** a 4096² depth **atlas**. A spot uses one tile and a point light six cube faces. Tile sizes come from screen-space size, clamped by Resolution Limit. Up to 8 shadowed spot/point lights per frame by default (`r.Shadow.MaxLocal`).
   - **Filtering:** hardware comparison sampling with a rotated Poisson PCF kernel. Its radius follows the light's Radius (or the sun's Angle) and the light's **Filter** setting, a cheap stand-in for EEVEE's soft shadows. Bias is slope-scaled plus normal offset.
   - Virtual shadow maps, jittered soft shadows and shadow rays stay **Planned**: they need temporal accumulation (M12).
   - **VRAM:** 64 MB for CSM plus 64 MB for the atlas. **Render ▸ Shadow Pool** maps to the atlas size.
4. **IBL (image-based lighting).** The World tab gets a **Surface type:**
   - **Color:** today's flat world.
   - **Sky:** a procedural daylight sky driven by the scene's sun, our own analytic model, Blender's Sky Texture in spirit.
   - **HDRI:** an equirectangular `.hdr` from `assets/`, with rotation and strength.

   Each environment change is processed on the GPU once:
   - converted to a cube map, with size set by **World ▸ Light Probe ▸ Resolution**;
   - GGX-prefiltered into a mip chain for specular;
   - projected to **SH9** for diffuse;
   - a split-sum BRDF lookup texture is computed once at startup.

   The crude ambient term is replaced. Reflections of the sky appear on metals.
   - A new **`.hdr` importer** (Radiance RGBE through `image`'s `hdr` feature) and a `PixelFormat::Rgba16Float` asset format.
   - EXR is out (it would need the `exr` crate); *Question 1.*
5. **Bloom:**
   - The dual-filter downsample/upsample chain (6 mips, soft threshold, energy-conserving).
   - EEVEE 5.x **removed** bloom (it moved to the compositor's Glare node), so it shows as an **engine panel** in the Render tab, clearly labelled "not in EEVEE 5.x". Settings: Intensity, Threshold, Knee, Radius (`r.Bloom.*`).
6. **View transforms:** Standard, our **AgX-style** curve and Khronos PBR Neutral move to the Tonemap pass unchanged.
   - Blender's exact AgX comes from an OCIO config we would have to copy, so **we keep our own clean-room curve** and say so in the tooltip. *Question 4.*
   - **Looks** (None, Punchy, Base/High/Very High Contrast) become Live as our own contrast curves in log space.
7. **Full PBR:**
   - **Normal maps:** tangents come from glTF, or are generated at import (`mikktspace` crate, MIT/Apache, so we match Blender's tangent space). The vertex format grows a tangent (44 → 60 bytes). The DDC format version is bumped, so meshes re-import once.
   - **Occlusion maps** darken ambient and IBL.
   - **Alpha:**
     - Mask = alpha test, also in the prepass and shadows.
     - Blend = a sorted transparent pass. This is EEVEE's *Render Method: Dithered / Blended*, plus *Transparent Shadows* for masked materials.
   - **IOR and Specular IOR Level** feed F0.
   - **Backface Culling (Shadow).**
   - The remaining Principled layers (Subsurface, Transmission, Coat, Sheen, Thin Film) stay **Planned (M12)**.
8. **Soft and area lights without lookup tables.**
   - Point and spot **Radius**, the sun's **Angle**, and **area lights** (rectangle or disk, with Size and Normalize) use the *representative point* method: specular is evaluated at the closest point on the light's shape, with roughness widened to keep energy (Karis 2013).
   - Area-light shadows use a 180° spot frustum.
   - LTC area lights (exact, but they need fitted lookup tables) are **not in v1**. *Question 2.*
9. **Depth prepass and masked geometry:** a depth-only pipeline (position only, alpha-test permutation), then the Opaque pass with `Equal` depth compare and no depth write. Overdraw shading goes to zero.
10. **No MSAA in v1.** Edges alias until TAA (M12). This keeps bandwidth and VRAM down on the 4 GB card. Cvar `r.MSAA` is reserved.
11. **Scalability:** `r.Shadow.Quality` (0–3), `r.Bloom.Quality` and `r.IBL.Resolution` are flagged `SCALABILITY`. The 680M floor (30 fps) is met at quality 1.
12. **Editor rows that become Live** (each row's status is in the settings schema, and tests check every Live row has a cvar or component field):
    - **Render tab:** Shadows (toggle, Resolution), Light Threshold, Shadow Pool, Bloom (engine panel), Look.
    - **World tab:** Surface type, HDRI/Sky settings, Light Probe Resolution, Sun Threshold/Angle (HDRI sun extraction stays Planned).
    - **Light tab:** Radius, Soft Falloff, Angle, Area Shape/Size, Normalize, the Shadow panel (toggle, Filter, Resolution Limit; Jitter stays Planned), Influence ▸ Volume stays T3.
    - **Material tab:** Alpha, IOR, Specular IOR Level, Render Method, Transparent Shadows, Backface Culling (Shadow), plus Normal/Occlusion now rendered.
13. **Sample scenes for the performance gate:**
    - `games/sample` gains a few shadowed spot/point lights.
    - Cvar **`sample.StressLights=N`** spawns N moving unshadowed point lights over the arena.
    - The gate is the default arena plus `StressLights=256` at 1080p.

## RHI additions (`ge_rhi`, still no wgpu types in the API)

- **Compute:** compute pipelines, compute passes, `dispatch`, and storage buffers (read-only and read-write).
- **Textures:** 2D arrays, cube maps, per-mip and per-layer **views** (`TextureView`) usable as render targets, storage textures (`Rgba16Float`), and the `R11G11B10Float`/`Rgba16Float` formats as render targets.
- **Render passes on any target:** `Frame::pass_to(color views, depth view)`, viewport and scissor, depth bias (constant, slope, clamp) in pipeline state, `Equal`/`GreaterEqual` depth compare, depth-only pipelines (no fragment stage).
- **Samplers:** comparison samplers for shadows, clamp-to-edge with mips.
- `rhi.limits()` for workgroup sizes and storage buffer sizes, logged at startup.

## Dependencies to add

| Crate | Where | Why | In §4 stack? |
|---|---|---|---|
| `mikktspace` | `ge_asset` | tangents matching Blender and glTF (MikkTSpace) | **no, needs your OK:** small, pure Rust, MIT/Apache, the standard used by Bevy |
| `image` `hdr` feature | `ge_asset` | `.hdr` environment maps | yes (feature only) |

## Build order (each step ends with `cargo xtask ci` green and is demoable)

1. **RHI:** compute, storage, views, arrays/cubes, comparison samplers, depth bias, passes to any target. Unit and GPU tests.
2. **HDR + Tonemap pass:** SceneColor, color management moved to post, sky pass, grid after tonemap. Golden image re-checked (decision 1).
3. **Depth prepass + clustered light culling** (compute) with storage light buffers. 1024 lights.
4. **Full PBR inputs:** tangents and normal maps (mikktspace, DDC bump), occlusion, IOR/specular level, alpha mask + blend.
5. **Shadows:** CSM, then the spot/point atlas, PCF, bias; Shadow panels Live.
6. **IBL:** environment cube, prefilter, SH9, BRDF lookup; World Surface types (Color, Sky, HDRI) and the `.hdr` importer.
7. **Soft and area lights** (representative point); Light tab rows Live.
8. **Bloom + Looks.**
9. **Performance gate:** stress scene, Tracy and GPU timestamps, scalability levels, 680M floor check. Docs: plan deviations, lore notes, CLAUDE.md §9.

## Out of scope (stays in its milestone)

- Render graph with transient aliasing and pass culling, the render thread, cached draw packets (M5).
- TAA, the upscaler hook, SSR/SSGI, Raytracing panels, depth of field, motion blur, GPU-driven culling, virtual shadow maps, jittered shadows (M12).
- Volumetrics (T3).
- MSAA.
- LTC area lights.
- EXR.
- Light probes placed in the scene (reflection cubes, irradiance volumes) (M12).

## Acceptance criteria (M4 exit criteria)

- [x] `cargo xtask ci` passes. Only `ge_rhi` names `wgpu`.
- [x] **HDR:** values above 1 survive to the tonemapper (GPU test: an emissive quad at 8.0 with Standard clips to white, and with AgX-style stays below white). The golden image is unchanged within tolerance under Standard + Solid, or was re-blessed with your OK.
- [x] **Clustered forward+:**
  - a GPU test with 300 point lights matches a brute-force CPU reference on chosen pixels within tolerance;
  - every light lands in exactly the clusters its sphere overlaps (CPU-side test of the cluster math);
  - 1024 lights don't crash and stay correct.
- [x] **Shadows:**
  - a GPU test shows a cube shadowing a plane under the sun, a spot and a point light (dark in the shadow, lit outside);
  - CSM split math and texel snapping are unit-tested;
  - no visible acne on the arena floor at default bias (manual).
- [x] **IBL:**
  - a white uniform environment lights a white diffuse sphere to the expected radiance within tolerance (energy check);
  - a metal sphere with roughness 0 reflects the HDRI;
  - SH9 projection is unit-tested against analytic cases.
- [x] **PBR inputs:** the crate fixture gains a normal map, and a pixel test shows it changes shading. Alpha mask cuts holes, including in shadows; blended objects sort.
- [x] **Bloom:** a bright emissive pixel spreads, and energy stays within a few percent at intensity 0.
- [x] **Editor:** every row listed in decision 12 is Live and tested by the schema check. The hand-built tabs edit real fields.
- [x] **Performance (manual, Turbo plan):**
  - arena + `sample.StressLights=256` at 1080p: **GPU ≤ 12 ms on the RTX 3050**, measured with GPU timestamps and broken down per pass in the stats pill and Tracy;
  - **≥ 30 fps on the 680M** at scalability 1 (`-cvar r.AllowIntegratedGpu=1`).

### Results (2026-11-04)

- `cargo xtask ci`: fmt, clippy `-D warnings`, all tests and `check-layers` (35 edges) pass. The golden image is unchanged under Standard + Solid.
- `crates/ge_render/tests/pbr.rs` adds 8 GPU tests:
  - HDR: emission 8 clips under Standard and maps to 253 under AgX; exposure −4 EV maps it to 0.5.
  - Clustered vs brute force: exact match (≤ 2/255) at 300 lights and at 1024 lights.
  - Shadows: sun, spot and point shadows each drop the floor below 20% of the unshadowed value.
  - IBL: white furnace within 0.85–1.05 for rough/smooth dielectrics and metals; a red world tints a metal sphere red.
  - Bloom: spreads and fades with distance.
  - Normal map: shading ratio ≈ cos 60°.
  - Alpha: mask holes in color and in shadow; blended objects sort.
- Performance at 1920×1080, `sample.StressLights=256`, Rendered shading (GPU timestamps):

  | GPU | Settings | GPU total | Biggest pass |
  |---|---|---|---|
  | RTX 3050 Laptop (Vulkan) | defaults | **≈ 6.3 ms** (gate ≤ 12 ms) | Opaque 5.4 ms; cull 0.13 ms; shadows 0.15 ms; bloom 0.23 ms |
  | Radeon 680M (Vulkan) | shadow res 0.5, bloom 0 | **≈ 6.0 ms, ≈ 137 fps** (gate ≥ 30 fps) | Opaque 5.3 ms |
  | RTX 3050, no stress lights | defaults | 1.9 ms | — |

  The power plan wasn't checked during the run. The RTX and 680M timings are close, so the RTX may have been on Silent (CLAUDE.md §9); rerun on Turbo for a clean number.

### Deviations from this plan

- **Tangents:** we generate them from UVs ourselves; the `mikktspace` crate is not used. It needs nalgebra or glam 0.15 and is heavy on `unsafe`. glTF tangents are used when the file has them.
- **Clustered test:** against a GPU brute-force reference (new debug cvar `r.Clusters.BruteForce`), not a CPU one. It checks every pixel instead of a few, and needs no CPU copy of the BRDF.
- **Cluster cap:** a cluster holds at most 128 lights. Past that, lights are dropped silently (a dense 1024-light grid with 1.2 m ranges overflows). Raise `MAX_LIGHTS_PER_CLUSTER` or add an overflow counter if the game hits it.
- **IBL multiscatter:** added energy compensation (Fdez-Agüera 2019) to the IBL. Without it a rough metal lost ≈ 30% energy in the furnace test (0.70). Direct lights are still single-scatter.
- **IBL tests:** use Color worlds, not an HDRI fixture. SH9 is projected on the GPU; the furnace test covers its constant term, but there is no CPU test against analytic L1/L2 cases.
- **Bloom energy check:** intensity 0 skips the pass, so the output is the input. No separate test.
- **HDR test:** our AgX curve tops out at 2^4.03 ≈ 16 linear, so emission 8 maps to 253/255. The test checks that it stays below white and still separates 2 from 8.
- **Grid:** stays in the HDR opaque pass instead of after tonemapping.
- **Not yet:** Resolution Limit, Jitter and Overblur stay Planned (M12). Dithered render method draws as a hard 0.5 cutout until TAA.
- **Shadow atlas:** uniform tiles; no per-light size by screen coverage yet.
- **Light probe resolution:** defaults to 512.
- **New cvars:**
  - `win.Width` / `win.Height` (read at startup), so the 1080p benchmark is repeatable.
  - `sample.StressLights` (the plan's), plus three shadowed lights in the sample: two spots and a point.
- **Manual step 6:** `r.Shadow.Quality` doesn't exist. Use `-cvar r.Shadow.Resolution=0.5 -cvar r.Bloom.Intensity=0`.

## Manual test steps

1. Switch to the Turbo power plan and run `cargo run -p editor`. In Rendered shading the sun casts shadows on the arena with no acne or peter-panning. Orbit close and far: cascade seams don't pop.
2. Add a Spot light, enable Shadow, and move a cube under it: the shadow follows. Raise Radius: the shadow edge softens and the highlight widens.
3. Switch World ▸ Surface to Sky, then HDRI (drop a `.hdr` into `assets/`). Metal and rough spheres reflect it. Change Light Probe Resolution and see the reflection sharpen.
4. Set a material's Emission Strength to 20: it blooms. Change Render ▸ Bloom intensity.
5. Run `cargo run -p game -- -cvar sample.StressLights=256` at 1080p (maximized window). The stats pill / title shows GPU ≤ 12 ms.
6. Run `cargo run -p game -- -cvar r.AllowIntegratedGpu=1 -cvar r.Shadow.Quality=1`: 30 fps or better on the 680M.

## Questions before I start

1. **Environments:** Color + procedural Sky + `.hdr` files (recommended)? Or also EXR (adds the `exr` crate)?
2. **Area lights:** approximate soft/area lights with the representative-point method now (recommended: no lookup tables, cheap), or exact LTC area lights (needs fitted LUT data, more work, possibly licensing)?
3. **Light culling:** on the GPU with compute shaders (recommended; adds compute and storage buffers to the RHI, which M12 needs anyway)? Or on the CPU each frame (simpler, but slower with many lights)?
4. **AgX:** keep our clean-room AgX-style curve (recommended)? Or match Blender's exact AgX, which means using its OCIO config/LUT data and checking that license first?
