# LORE.md — Building an Unreal-class Game Engine in Rust

> **Compiled:** 2026-10-18 · **Project:** `Game_Engine` (empty workspace, Rust 1.97.1 / MSVC, Windows 11)
>
> **Purpose:** everything needed to design and build a modular, editor-driven, Unreal-style 3D engine in Rust:
> 1. what Unreal Engine really is — from Epic's docs **and** from reverse-engineering the UE5 games installed on this machine with REA;
> 2. what the Rust ecosystem offers as of October 2026;
> 3. a concrete architecture, rules, pitfalls and a milestone roadmap.
>
> **Evidence tags used throughout:**
> **[REA]** observed with the REA reverse-engineering MCP (Evidence IDs in Appendix A) ·
> **[SCAN]** observed by a read-only PE/string scan of the same binaries ·
> **[DOC]** official documentation · **[WEB]** other web sources ·
> **[INF]** inference — reasoned, not directly observed · **[REC]** recommendation for this project.

---

## Contents

0. [TL;DR](#0-tldr)
1. [Scope & reality check](#1-scope--reality-check)
2. [Field notes: what REA found inside shipped UE5 games](#2-field-notes-what-rea-found-inside-shipped-ue5-games)
3. [Unreal Engine anatomy (the reference architecture)](#3-unreal-engine-anatomy-the-reference-architecture)
4. [The Rust game-engine landscape (Oct 2026)](#4-the-rust-game-engine-landscape-oct-2026)
5. [Unreal → Rust concept map](#5-unreal--rust-concept-map)
6. [Architecture blueprint for our engine](#6-architecture-blueprint-for-our-engine)
7. [Rust-specific pitfalls & patterns](#7-rust-specific-pitfalls--patterns)
8. [Roadmap (milestones with exit criteria)](#8-roadmap-milestones-with-exit-criteria)
9. [This machine: constraints & opportunities](#9-this-machine-constraints--opportunities)
10. [Glossary](#10-glossary)
11. [Sources & reading list](#11-sources--reading-list)
- [Appendix A — REA / scan evidence ledger](#appendix-a--rea--scan-evidence-ledger)
- [Appendix B — How to reproduce the analysis](#appendix-b--how-to-reproduce-the-analysis)
- [Appendix C — Decisions to lock in before writing CLAUDE.md](#appendix-c--decisions-to-lock-in-before-writing-claudemd)

---

## 0. TL;DR

**"An engine like Unreal" is not "Nanite + Lumen".** It is a *skeleton* that everything else hangs off:

1. **Layered modules** with one-way dependencies: Core → Object model → Engine → Renderer/RHI → Editor.
2. **A reflection-driven object model.** Types describe themselves, which powers the editor, serialization, networking and scripting.
3. **A multi-threaded frame pipeline.** The game thread simulates frame N, the render thread builds frame N-1, the RHI thread/GPU executes N-2.
4. **A renderer behind a thin GPU abstraction (RHI)** with a **render graph** that owns transient resources, barriers and async work.
5. **A cooked content pipeline.** Editor formats are converted offline into runtime containers with precomputed dependencies.
6. **An editor built from the same runtime,** plus Play-In-Editor.
7. **Plugins and feature modules** so gameplay code and features are optional and swappable.
8. **Instrumentation everywhere:** console variables, tracing/profiling, crash handling.

**Scale check [SCAN]:** the UE5 shipping executables on this machine contain **139–173 MB of machine code**, roughly **0.6–0.86 million functions** with unwind info, about **3,700 rendering console variables**, and **286–410 reflected modules**. Do not try to clone that. Copy its **shape**, then grow features one vertical slice at a time.

**Recommended starting stack [REC]** (details in §6):

| Concern | Choice | Why |
|---|---|---|
| Window / input | `winit` (0.30.x stable), `gilrs` | Standard, cross-platform. |
| GPU | `wgpu` v30 **behind our own RHI trait** | D3D12/Vulkan/Metal/GL/WebGPU, validation, naga. The trait keeps a native backend possible later. |
| Shaders | WGSL + **WESL** (imports, `@if` permutations), compiled by `naga` | Native to wgpu; Bevy moved to WESL in 0.20. |
| ECS / world | `bevy_ecs` used standalone, wrapped by our engine API (or our own ECS later) | Mature parallel scheduler, change detection, observers, relationships. |
| Math | `glam` | SIMD, ubiquitous. |
| Reflection | `bevy_reflect` + derive macros (our "UHT") | Editor, scenes, serialization. |
| Physics | `rapier3d` behind a trait (Jolt via `oxijolt` later if needed) | Pure Rust, mature. |
| Audio | `kira` (on `cpal`) | Game-oriented mixer, tweening, clocks. |
| Editor UI | `egui` + `egui_dock` + `transform-gizmo-egui` | Fastest route to a real editor. |
| Assets | `gltf`, `image`, `ktx2`, `meshopt`; cooked with `rkyv`/`postcard` | Standard interchange formats in; zero-copy cooked data out. |
| Profiling | `tracing` + Tracy (`tracing-tracy`), GPU timestamp queries | Unreal Insights equivalent. |
| Crash | panic hook + Embark `crash-handler`/`minidumper` | Out-of-process crash reports (like CrashReportClient). |
| Hot reload | `subsecond` hot-patching (dev only) + asset hot reload | Iteration speed. |

**The 12 laws** (each is backed by evidence in §2–§3):

1. **Dependencies point down only.** Core never knows about rendering, and the editor depends on the runtime, never the reverse.
2. **Game state and render state are different data.** They sync at one explicit **extract** point (UE scene proxies / Bevy Extract).
3. **Pipeline frames across threads,** and bound the latency (at most 1–2 frames).
4. **Reflect everything that the editor, serializer, network or scripts must see.**
5. **Handles, not pointers.** Use generational IDs into pools/arenas.
6. **A render graph owns GPU resource lifetimes, barriers, aliasing and pass culling.**
7. **Cook offline.** Runtime never parses editor formats, and dependency graphs are precomputed.
8. **Make everything tunable at runtime (cvars + scalability tiers),** and strip debug features in shipping builds.
9. **Features are plugins or modules.** Games are modules too.
10. **Instrument from day one:** CPU/GPU tracing, memory tags, stats.
11. **Have a crash-safe path:** a crash allocator, minidumps, and an out-of-process reporter.
12. **Never let renderer code touch the graphics API directly. Only the RHI does.**

**First three milestones:**

- **M0:** cargo workspace, CI, a window, and a clear color via the RHI.
- **M1:** a camera-driven mesh with depth, plus shader hot reload.
- **M2:** an engine loop with ECS, transforms, input and fixed-step simulation (§8).

---

## 1. Scope & reality check

- Unreal is the product of **25+ years** (UE1 shipped 1998) and **hundreds of engineers**. UE5's flagship systems (Nanite, Lumen, Virtual Shadow Maps, TSR, World Partition, MetaSounds, Chaos) each took **multi-year, multi-person** efforts.
- The deliverable that matters is **a working skeleton with clean seams**. Features can then be added without rewrites.
- **Capability tiers** for planning:

| Tier | What it contains | Realistic for 1–2 devs? |
|---|---|---|
| **T0 Framework** | Window, input, GPU abstraction, 2D/3D draw, audio playback, asset loading | Weeks |
| **T1 Small 3D engine** | ECS world, transform hierarchy, glTF import, PBR forward+, shadows, physics, scene save/load, basic editor | Months |
| **T2 Production engine** | Full editor (inspector, gizmos, undo, PIE, asset browser), cooking/packaging, animation, UI, scripting/hot reload, profiling, crash reporting, networking | 1–3 years |
| **T3 Frontier** | Virtualized geometry (Nanite-class), dynamic GI (Lumen-class), virtual shadow maps, path tracing, world-partition streaming, ML upscalers | Research-grade; adopt incrementally |

- **Build an engine *for one game*** (a vertical-slice target). It defines "done" for every subsystem and stops the "general-purpose engine" trap. That trap is the #1 failure mode called out in post-mortems such as LogLog Games' (§4.4).

---

## 2. Field notes: what REA found inside shipped UE5 games

### 2.1 Targets & method

| Artifact (installed on this PC) | Bytes | What it is | How analyzed |
|---|---|---|---|
| `Neighbors Suburban Warfare\JP\Binaries\Win64\JP-Win64-Shipping.exe` | 192,685,568 | Monolithic UE ≈5.6 game (Invisible Walls), built on Epic's **Lyra** sample framework | REA `inspect_pe_resources`; [SCAN] imports/exports/sections/strings |
| `MarvelRivals\MarvelGame\Marvel\Binaries\Win64\Marvel-Win64-Shipping.exe` | 302,572,544 | Monolithic UE ≈5.3 licensee fork (NetEase), protected | REA `inspect_pe_resources`; [SCAN] |
| `Neighbors Suburban Warfare\JP.exe` | 3,975,920 | Root launcher, **EasyAntiCheat** bootstrapper (PDB `start_protected_game.pdb`) | REA Ghidra `binary_overview` |
| `…\Engine\Binaries\Win64\CrashReportClient.exe` | 27,903,488 | UE "Program" target (Slate desktop app) | [SCAN]; REA Ghidra deep pass **timed out** (330 s startup deadline) |
| `…\Engine\Binaries\Win64\EpicWebHelper.exe` | 4,088,320 | UE "Program" target linking only `Runtime/Core` (CEF helper) | REA Ghidra: strings → xrefs → decompilation |

Fortnite and Electrician Simulator folders contain only Epic-launcher stubs/staging (not installed), so they could not be analyzed.

**Method rules applied:**

- Static inspection only. Nothing was executed or attached.
- Anti-cheat-protected titles were never launched.
- Target strings were treated as untrusted data.
- No game code or content was copied.
- Only architecture-level facts (module names, file formats, imports, configuration names) are recorded here.

> Unreal's source is available on GitHub after accepting Epic's EULA. Treat it as *reference reading*, not as code to copy into this engine. Do a clean-room design.

### 2.2 Finding: the executable shell is a shared engine module

- **[REA]** Both games — from different studios, ~2.5 years of engine versions apart — carry **byte-identical** resources:
  - dialogs 101–108;
  - accelerator 101;
  - `AFX_DIALOG_LAYOUT`;
  - the application manifest.

  Their payload SHA-256 values match between the two `inspect_pe_resources` results.
- **[INF]** These come from UE's `Launch` module, which provides `WinMain` → `GuardedMain` → `FEngineLoop` for *every* UE executable. The game is a set of modules linked into that shell.
- **[SCAN]** Neighbors' embedded paths include `Runtime/Launch/Private/LaunchEngineLoop.cpp`. CrashReportClient (§2.13) contains strings `FEngineLoop::PreInitPreStartupScreen`, `FEngineLoop::PreInitPostStartupScreen`, `LoadPreInitModules`, `Loading Plugins (PreInit)`, `RequestEngineExit must be given a valid reason`.
- **Rule for us:** one `engine_main` crate owns process startup, the loop and shutdown. Games and tools are libraries/plugins it loads.

### 2.3 Finding: scale of a shipping UE5 binary [SCAN]

| Metric | Neighbors (≈UE 5.6) | Marvel Rivals (≈UE 5.3 fork) |
|---|---|---|
| File size | 192.7 MB | 302.6 MB |
| `.text` (machine code) | 139.2 MB | 172.9 MB **+ 22.1 MB** in two garbage-named executable sections (`.2>r`, `.F\(`) — a code protector [INF] |
| `.rdata` (read-only data) | 40.3 MB | 83.1 MB |
| `.pdata` entries (≈ functions with unwind info) | ~598,000 | ~863,000 |
| MSVC linker | 14.38 (VS 2022 17.8) | 14.40 (VS 2022 17.10) |
| Distinct `r.*` rendering cvars | 3,694 | ~3,800 |
| `/Script/<Module>` reflection packages | 286 | 410 (174 shared) |
| Log categories kept in shipping | 74 | 1,260 |
| Embedded engine source paths (from `check()`/`ensure()` sites) | 819 | 2,943 |
| Version marker | "Unknown-CL-0" (no CL), Lyra modules | Branch `++depot_marvel+S10.5_release` |

**Engine-version inference [INF]:**

- **Neighbors ≈ UE 5.6.** It has MegaLights, including the directional and translucency cvars, Nanite skinning, Nanite "assembly" types and Local Fog Volumes. It has *no* 5.7 Nanite Foliage/Voxel or 5.8 Lumen Lite/Mesh Terrain markers, and no `StructUtils` module (merged into CoreUObject around 5.5).
- **Marvel ≈ UE 5.3.** The old "Strata" and new "Substrate" material names coexist, and there is no MegaLights and no Local Fog Volume.

### 2.4 Finding: the module map (recovered from embedded source paths) [SCAN]

UE ships monolithic in shipping builds, but the module structure survives in `__FILE__` strings. Union of both games: **3,213 distinct source files**.

| Layer | Modules observed (count of referenced source files, Neighbors+Marvel) |
|---|---|
| Core | `Core` (46/82) — `HAL`, `Async`, `Tasks`, `Containers`, `Serialization`, `IO`, `Misc`, `Internationalization`, `Logging`, `Modules`, `ProfilingDebugging`, `Compression`, `Virtualization`, `Math`, `Stats` |
| Object model | `CoreUObject` (31/57) — `UObject`, `Serialization` (incl. `AsyncLoading2.cpp` = Zen loader), `AssetRegistry` |
| Platform | `ApplicationCore`, `Windows`, `Launch`, `Projects` |
| Engine | `Engine` (114/346) — `Animation`, `Components`, `PhysicsEngine`, `Particles`, `WorldPartition`, `Rendering`, `Net`, `Streaming`, `Materials`, `GameFramework`, `VT`, `ShaderCompiler`, `DeviceProfiles`, `Collision`, `Subsystems`, `Kismet`, `Audio` |
| Rendering | `Renderer` (`Nanite`, `Lumen`, `VirtualShadowMaps`, `VT`, `PostProcess`, `RayTracing`, `InstanceCulling`, `VariableRateShading`, `MaterialCache`), `RenderCore`, `RHI`, `RHICore`, `D3D12RHI`, `VulkanRHI`, `OpenGLDrv` |
| UI | `Slate`, `SlateCore`, `SlateRHIRenderer`, `UMG`, `AdvancedWidgets`, `WidgetCarousel` |
| Runtime systems | `AudioMixer`, `AudioMixerCore`, `SignalProcessing`, `Landscape`, `Foliage`, `NavigationSystem`, `Navmesh` (Recast), `AIModule`, `GameplayTags`, `GameplayTasks`, `MovieScene`, `LevelSequence`, `AnimGraphRuntime`, `AnimationCore`, `ClothingSystemRuntime*`, `Interchange`, `GeometryCore`, `MeshDescription`, `MediaAssets`, `MoviePlayer`, `PreLoadScreen` |
| Net / online | `Net/Core`, `Sockets`, `PacketHandlers`, `Online/HTTP`, `SSL`, `Voice`, `ICMP`, `HTTPServer`, `BuildPatchServices` (patching) |
| Content | `PakFile`, `Experimental/IoStore/OnDemand` |
| Experimental | `Chaos`, `Iris` (replication), `GeometryCollectionEngine` (destruction), `Dataflow`, `ChaosVehicles`, `Animation/Constraints` |

Sample `Engine` file names show where gameplay lives:

- **Actors:** `Actor.cpp`, `ActorComponent.cpp`, `ActorReplication.cpp`.
- **Characters:** `CharacterMovementComponent.cpp`.
- **Framework:** `GameEngine.cpp`, `GameInstance.cpp`, `GameMode.cpp`, `GameState.cpp`.
- **Levels:** `LevelStreaming.cpp`, `LevelTick.cpp`.
- **Networking:** `NetDriver.cpp`, `NetConnection.cpp`, `DataChannel.cpp`, `DemoNetDriver.cpp` (replays).
- **Rendering:** `MeshPassProcessor.cpp`, `DeferredShadingRenderer.cpp`, `NaniteStreamingManager.cpp`, `LumenScreenProbeGather.cpp`, `GPUSkinCache.cpp`, `DistanceFieldAtlas.cpp`.

### 2.5 Finding: the thread roster [SCAN]

Thread and thread-related names present:

- **Main pipeline:** `GameThread`, `RenderThread` (with priority/affinity/poll settings, "RenderThread timed out waiting for GPU"), `RHIThread`.
- **Newer D3D12 submission pipeline:** `RHISubmissionThread`, `RHIInterruptThread`.
- **Watchdog:** `RTHeartBeat` (render-thread heartbeat).
- **Audio:** `AudioThread`.
- **IO:** `IoDispatcher` (with ~15 tuning cvars), `IOThreadPool`.
- **Task system:** `TaskGraphThreadNP/HP/BP` (normal/high/background priority named threads), `PoolThread`.
- **Task cvars:** `TaskGraph.UseHiPriThreads`, `TaskGraph.UseBackgroundThreads`, `TaskGraph.UseDynamicPrioritization`, `TaskGraph.TaskPriorities.{SceneRenderingTask, PhysicsTickTask, ParticleAsyncTask, UpdateCachePrimitivesTask, TickDispatchTaskPriority …}`, `ParallelForBatchSize`.

**Lesson:** a modern engine has a small set of **long-lived role threads**: game, render, RHI/submission, audio, IO. Next to them sits a **worker pool** that executes a *task graph* with priorities. Everything else is a task.

### 2.6 Finding: GPU backends are runtime-selected and vendor-aware [SCAN]

- **All backends are compiled into one exe:** `VulkanRHI`, `D3D12RHI`, `D3D11RHI`, `OpenGLDrv`, `NullDrv` (headless). The backend is chosen at startup by config or command line.
- **GPU DLLs are delay-loaded:** `d3d12.dll` (root-signature serialization + ordinal imports), `d3d11.dll` (`D3D11CreateDevice`). `dxgi.dll` is static. `vulkan-1.dll` appears as a string, so it is loaded dynamically.
- **Executable exports set GPU policy:**
  - `NvOptimusEnablement` and `AmdPowerXpressRequestHighPerformance` force the **discrete GPU on hybrid laptops** (relevant to this RTX 3050 + Radeon 680M laptop).
  - `D3D12SDKVersion` and `D3D12SDKPath` opt into the **D3D12 Agility SDK** (`D3D12Core.dll` is shipped in `Binaries/Win64/D3D12/`).
  - AMD AGS driver extensions (`ags*`) and Intel XeSS/GPU-crash-dump entry points are exported too.
- **Vendor SDKs:**
  - Neighbors: NVIDIA DLSS, Aftermath (GPU crash dumps), DirectML (NNE neural inference).
  - Marvel adds Streamline/Reflex, Intel XeSS/XeFG/XeLL, AMD FSR3/AntiLag2, PIX event runtime, OpenImageDenoise.
- **Pipeline-state management is its own subsystem:** `ShaderPipelineCache` (35 cvars), `PSOPrecache` (30 cvars), `r.D3D12.PSO.DiskCache`.
- **D3D12 submission is tunable:** `r.D3D12.Submission.MaxExecuteBatchSize.{Direct,Copy,Async}`, `r.D3D12.SubmissionTimeout`, `r.D3D12.LightweightDRED`, `r.D3D12.GPUTimeout`.
- **Lessons:**
  - Abstract the GPU behind an RHI and select the backend at runtime.
  - Treat **PSO compilation/caching** and **GPU crash diagnostics** as first-class features.
  - Expect per-vendor integrations (upscalers, latency reducers).

### 2.7 Finding: everything is a tunable [SCAN]

**Console-variable namespaces** (distinct strings, Neighbors / Marvel):

| Namespace | Neighbors | Marvel |
|---|---|---|
| `r.*` (rendering) | 3,691 | 3,807 |
| `p.*` (physics) | 780 | 1,033 |
| `fx.*` (Niagara) | 268 | 276 |
| `net.*` | 234 | 274 |
| `au.*` (audio) | 215 | 155 |
| `s.*` (streaming/loading) | 99 | 78 |
| `wp.*` (World Partition) | 65 | 60 |
| `gc.*` | 43 | — |
| `a.*` (animation) | 50 | 163 |
| `ak.*` (Wwise) | — | 149 |
| `grpc.*` | — | 108 |

**Renderer complexity by subsystem** (Neighbors, count of `r.<Subsystem>.*` cvars):

| Subsystem | cvars |
|---|---|
| Lumen + LumenScene | 378 + 129 |
| HairStrands | 222 |
| RayTracing | 208 |
| Shadow | 198 (`r.Shadow.Virtual.*`: 106) |
| Nanite | 146 |
| Vulkan | 102 |
| HeterogeneousVolumes | 99 |
| PathTracing | 91 |
| VT (virtual texturing) | 88 |
| MegaLights | 86 |
| Mobile | 67 |
| Streaming | 60 |
| FastVRam | 53 |
| VolumetricCloud | 42 |
| ShaderCompiler | 41 |
| TSR | 38 |
| ShaderPipelineCache | 35 |
| OpenGL | 34 |
| Water | 31 |
| Substrate | 30 |
| SkyAtmosphere | 30 |
| PSOPrecache | 30 |
| D3D12 | 27 |
| VolumetricFog | 22 |
| DistanceFields | 20 |
| VRS | 19 |
| DOF | 19 |
| RDG | 16 |
| DynamicRes | 16 |
| SkinCache | 15 |

**Lessons:**

- **Global illumination alone has more tuning knobs than many complete engines.** Build a **cvar registry** on day one, with typed values, help text, flags (cheat/scalability/read-only) and change callbacks.
- **Group cvars into scalability tiers** (low → epic) and device profiles.

### 2.8 Finding: modularity at gameplay level [SCAN]

- **Neighbors is built on Epic's Lyra framework.** Its packages include `LyraGame`, `CommonGame`, `CommonUI`, `CommonUser`, `CommonLoadingScreen`, `ModularGameplay`, `ModularGameplayActors`, `GameFeatures`, `GameSettings`, `UIExtension` and `GameplayMessageRuntime`, alongside ~25 studio `IW*` modules.
  - **Game Features** are plugins that inject components/abilities into actors at runtime. A game is a composition of features.
- **Marvel ships one module per hero:**
  - `Hero_1011` … `Hero_1067` (≈57 modules);
  - `HeroGAS_*` (≈40 Gameplay-Ability-System modules);
  - per-mode modules (`Mode_2201` …);
  - per-event modules (`Activity_10120` …).

  Content and code are partitioned so teams can ship heroes independently.
- **Common gameplay frameworks in both games:**
  - abilities: `GameplayAbilities` + `GameplayTags` + `GameplayTasks`;
  - input: `EnhancedInput`;
  - AI/logic: `StateTreeModule` (≈500 strings each);
  - navigation: `NavigationSystem`;
  - cinematics: `MovieScene` + `LevelSequence`;
  - VFX: `Niagara`;
  - audio: `MetasoundEngine` (Neighbors); Marvel uses **Wwise** (`AkAudio`) instead;
  - physics: `Chaos`, `ChaosCloth`, `GeometryCollectionEngine` (destruction).
- **Marvel leans hard on Mass** (UE's ECS): `MassEntity`, `MassCrowd`, `MassAIBehavior`, `MassReplication`, `MassLOD`, `MassSignals`, `MassRepresentation`, `MassSpawner`, `MassSmartObjects`, `MassZoneGraphNavigation`, plus custom `MarvelMassProjectile`. It also uses ZoneGraph, SmartObjects, Houdini Engine runtime, ReplicationGraph and embedded Python (`UnrealEnginePython`, plus a `PyRuntim` PE section).
- **Neighbors uses Iris** (`IrisCore`), UE's newer replication system. Marvel uses `ReplicationGraph` + `IrisStub`.
- **Lesson:** gameplay architecture is *layers of frameworks on top of the engine* (abilities, input mapping, state trees, crowds). Our engine needs **plugin seams** for that, not hard-coded gameplay.

### 2.9 Finding: the content pipeline is a container + IO scheduler [SCAN]

- **Neighbors ships IoStore containers:**

  | File | Size |
  |---|---|
  | `global.utoc` | 3,014 B |
  | `global.ucas` | 3.6 MB |
  | `JP-Windows.utoc` (table of contents) | 11.5 MB |
  | `JP-Windows.ucas` (chunk store) | **5.92 GB** |
  | `JP-Windows.pak` (loose, rarely accessed files) | 75.8 MB |
  | `JP-Windows.sig` (signature) | 5,152 B |

- **Code paths and settings:**
  - `CoreUObject/Private/Serialization/AsyncLoading2.cpp` (the Zen loader) and `Core/Private/IO/IoDispatcher.cpp`.
  - `Experimental/IoStore/OnDemand/*`: streaming content installs from a CDN ("CDN url to be used if a distributed endpoint cannot be reached").
  - IO-dispatcher cvars: `s.IoDispatcherSortRequestsByOffset`, `…MaxForwardSeekKB`, `…DecompressionWorkerCount`, `…CacheSizeMB`, `…BufferMemoryMB`.
  - Shaders are Oodle-compressed in groups ("Could not decompress shader group with Oodle").
- **[DOC] Zen loader design:**
  - The **dependency graph is computed offline during staging**.
  - `.utoc` describes chunk offsets/sizes/compression/encryption, and `.ucas` holds the data.
  - Chunk reads go through the **I/O dispatcher** with platform back-ends.
- **Lesson:** our cooked format = **TOC + content-addressed chunk store + precomputed dependency lists**, read by an **IO scheduler** that sorts reads, decompresses on workers and prioritizes by need.

### 2.10 Finding: pluggable global allocators + a crash-safe allocator [REA]

- **[SCAN]**
  - Neighbors references `Core/Private/HAL/MallocBinned2.cpp` and `MallocBinnedCommon.h`, plus `mimalloc` strings.
  - Marvel also has `Binned3`.
  - Even the tiny `EpicWebHelper.exe` contains **Binned2, Binned3, mimalloc, TBBmalloc 2019.0, ANSI and jemalloc** allocator code.
- **[INF]** UE selects one global `FMalloc` at boot, from a compile-time default plus overrides.
- **[REA] decompiled `FUN_14003a920` in EpicWebHelper** (532 bytes; pseudocode Evidence `ev_ad52ebeb…`). It matches the design of UE's **crash allocator** (`FMallocCrash::Realloc`):
  1. If the calling thread ≠ the stored *crashed thread id*, it calls a function that is literally `for(;;) Sleep(INFINITE)` (`FUN_1400e11c0`, Evidence `ev_afc1cd38…`). **Only the crashing thread may allocate.** Every other thread is parked forever.
  2. Memory comes from a fixed **2 MiB small pool** and a separate large pool. These are range checks against `param_1+0x38 … +0x200000` and `param_1+0x48 + size`.
  3. For pointers allocated *before* the crash, it asks the **previous allocator** (through vtable slots `+0x90` = descriptive name, `+0x40` = allocation size) for the old size. It only does this if the name is one it trusts ("binned2", "jemalloc", "ANSI").
  4. Otherwise it calls `OutputDebugStringW("Realloc from previous malloc - we don't know how to get allocation size, exiting...")` and `RequestExit(true)`.
- **[REA] decompiled `FUN_1400e0530` = `RequestExit(force)`** (Evidence `ev_3b5fb1a5…`):
  - **force** → hard-terminate path;
  - **graceful** → log "Win RequestExit", set the engine-exit flag the main loop polls, then `PostQuitMessage(0)`.
- **Lessons for us:**
  - Use `#[global_allocator]` (e.g. mimalloc) plus **frame/arena allocators** for per-frame data.
  - On crash, use a **pre-reserved emergency arena** and a reporter that lives **out-of-process**, since the heap may be corrupt.
  - Have two exit paths: *request* (cooperative, flag polled by the loop) and *terminate*.

### 2.11 Finding: the GC is incremental, parallel and cluster-aware [SCAN]

43 `gc.*` cvars, including:

- incremental marking: `gc.AllowIncrementalReachability`, `gc.IncrementalReachabilityTimeLimit`, `gc.ContinuousIncrementalGC`, `gc.IncrementalBeginDestroyEnabled`;
- parallelism: `gc.AllowParallelGC`;
- clustering: `gc.CreateGCClusters`, `gc.ActorClusteringEnabled`, `gc.AssetClustreringEnabled` (sic);
- limits and pacing: `gc.MaxObjectsInGame`, `gc.TimeBetweenPurgingPendingKillObjects`, `gc.LowMemory.*`;
- loading interaction: `gc.PerformGCWhileAsyncLoading`.

**Lesson:** a GC'd object graph needs time-slicing, clustering and many knobs. In Rust we mostly **avoid GC**: ownership + generational handles + explicit despawn. A GC (or ref-counting with cycle collection) is only needed for script VMs.

### 2.12 Finding: shipping hygiene & third-party reality [SCAN]

- **Shipping strips logging by default.** Neighbors keeps 74 log categories, while Marvel opted in and keeps 1,260. Console variables remain. Build configurations decide what survives.
- **Root launcher = anti-cheat bootstrapper.** `JP.exe` is EAC's `start_protected_game` [REA]. Marvel's binary has protector sections [INF].
- **Crash pipeline:**
  - `Engine/Binaries/Win64/CrashReportClient.exe` (a separate process);
  - Sentry + `crashpad_handler.exe` (Neighbors);
  - Aftermath for GPU crashes.
- **An engine is an integration platform.** Third-party fingerprints:
  - **Compression and media:** Oodle (compression + network), Bink (video), Opus/Vorbis (audio), libpng/libjpeg/OpenEXR, OpenColorIO.
  - **Text and fonts:** ICU (i18n), FreeType (fonts), nanosvg, libsamplerate.
  - **Network:** OpenSSL, libcurl, nghttp2 (HTTP/2), MsQuic.
  - **Gameplay infrastructure:** Recast/Detour (navmesh), mimalloc.
  - **Platform:** CEF (Chromium Embedded), Media Foundation, EOS SDK, Steamworks, Discord SDK, ONNX Runtime/DirectML.
  - **Marvel adds:** Wwise, gRPC + protobuf, Python, Houdini Engine.
  - **Plan for this:** FFI seams, a `ThirdParty` policy, license tracking.

### 2.13 Finding: the layering is real [SCAN]

`CrashReportClient.exe` (a UE desktop app with a full Slate UI) references:

| Module | Source paths |
|---|---|
| `Runtime/Core` | 280 |
| `CoreUObject` | 174 |
| `Slate` | 74 |
| `SlateCore` | 66 |
| `Developer/DerivedDataCache` | 27 |
| `ApplicationCore` | 18 |
| `Developer/StandaloneRenderer` (Slate's own D3D11/OpenGL renderer) | 16 |
| `Launch` | 2 |
| `Engine`, `Renderer`, `RHI`, any `*RHI` | **0** |

- UE's lower layers (Core, object model, UI toolkit) work **without** the game engine or the 3D renderer. The UI even has its own mini-renderer.
- `EpicWebHelper.exe` links only `Runtime/Core`.
- **Rule for us:** `core` + `reflect` + `ui` must build and run without `render` and `engine`. Tools (cooker, crash reporter, shader compiler) are small programs on the lower layers.

### 2.14 Findings → design rules

| Observation | Rule we adopt |
|---|---|
| Shared `Launch` shell across games | One engine-owned entry point; games are plugins |
| 5 RHIs in one binary, delay-loaded APIs | RHI trait + runtime backend choice; never call the graphics API outside the RHI |
| Role threads + prioritized task graph | Fixed role threads (main/game, render, audio, IO) + work-stealing task pool |
| ~3.7k render cvars, scalability | Typed cvar registry + scalability tiers + device profiles from M0 |
| Reflection packages everywhere | Derive-based reflection on every editable/serializable type |
| IoStore + Zen loader + IO dispatcher | Cooked TOC/CAS containers, offline dependency lists, IO scheduler |
| Binned/mimalloc/TBB + crash allocator | `#[global_allocator]` + frame arenas + emergency crash arena + out-of-process reporter |
| Logging stripped in shipping | Cargo features / `cfg` gate debug tooling, logging levels, cheats |
| Layering proven by CrashReportClient | Lower crates must compile & run without engine/render |

### 2.15 Limits of this analysis

- Static only. Nothing here proves runtime behavior (thread timing, frame pacing).
- The giant shipping binaries were **not** decompiled: REA's Ghidra provider only serves results after full auto-analysis and has a 330 s startup deadline (`REA_GHIDRA_STARTUP_TIMEOUT_MS`).
- Strings prove a feature's code *exists*, not that it's *enabled*.
- Engine-version estimates are inferences from feature markers.
- `/Script/X` strings may also come from redirects or soft references, not only compiled modules.

---

## 3. Unreal Engine anatomy (the reference architecture)

### 3.1 The layer cake

```
 ┌────────────────────────────────────────────────────────────────────────────┐
 │ Game modules & Game Feature plugins   (LyraGame, Hero_1011, IWCore …)      │
 ├────────────────────────────────────────────────────────────────────────────┤
 │ Gameplay frameworks: GAS · EnhancedInput · StateTree · Mass · AI · CommonUI │
 ├────────────────────────────────────────────────────────────────────────────┤
 │ Engine: UWorld · ULevel · AActor · components · ticking · streaming ·       │
 │         networking · audio device · physics scene · materials · animation   │
 │   ├── Renderer (FScene, FSceneRenderer, Nanite, Lumen, VSM…) via IRenderer  │
 │   ├── RenderCore (RDG, shader framework, render-thread utilities)           │
 │   └── RHI ──► D3D12RHI | VulkanRHI | D3D11RHI | OpenGLDrv | NullDrv         │
 │ Slate/SlateCore/UMG · AudioMixer/MetaSound · Chaos · Niagara · Interchange  │
 ├────────────────────────────────────────────────────────────────────────────┤
 │ CoreUObject: UObject · UClass/FProperty reflection · GC · packages ·        │
 │             serialization · Zen async loader · asset registry               │
 │ ApplicationCore: native windows, message pump, input devices, cursors       │
 │ Core: HAL (memory, threads, files, time) · containers · FString/FName ·     │
 │       Tasks/TaskGraph · logging · config/cvars · IO dispatcher · modules    │
 ├────────────────────────────────────────────────────────────────────────────┤
 │ Launch: WinMain → GuardedMain → FEngineLoop        Programs: UBT, UHT,      │
 │                                                    ShaderCompileWorker,     │
 │                                                    CrashReportClient …      │
 └────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 Modules, plugins and the build system [DOC]

- **Module:** a directory with a `Name.Build.cs` (C# `ModuleRules`) that declares `PublicDependencyModuleNames` / `PrivateDependencyModuleNames`. **UnrealBuildTool (UBT)** compiles only what is reachable through those dependency chains.
- **Module types:**
  - `Runtime` — always loaded, including in shipped games;
  - `RuntimeNoCommandlet`;
  - `Developer` — loaded in dev/editor builds, **never in shipping**;
  - `Editor`, `EditorNoCommandlet`, `Program`;
  - `External` — precompiled third-party code, nothing to compile.
- **Plugins** (`.uplugin`) bundle modules + content and are enabled per project.
- **Load phases:** `EarliestPossible`, `PostConfigInit`, `PreLoadingScreen`, `PreDefault`, `Default`, `PostDefault`, `PostEngineInit`, …
- **Linking:**
  - **Editor builds** are *modular*: one DLL per module, hot-reloadable via Live Coding.
  - **Shipping builds** are *monolithic*: everything is linked into one exe, which is why the games above are 190–300 MB.
- **UnrealHeaderTool (UHT)** parses `UCLASS/USTRUCT/UENUM/UPROPERTY/UFUNCTION` macros in headers and generates `*.generated.h` / `*.gen.cpp` reflection code. This is UE's answer to C++ having no reflection.
- **Build configurations:** `Debug`, `DebugGame`, `Development`, `Test`, `Shipping`. **Target types:** `Game`, `Client`, `Server`, `Editor`, `Program`.
- **Rust mapping [REC]:**
  - `Build.cs` → `Cargo.toml` crates;
  - module types → cargo **features** + separate crates (`*_editor` crates never linked into the runtime);
  - UHT → **derive/proc macros**;
  - monolithic vs modular → static linking for release and **dynamic linking for dev builds** (Bevy's `dynamic_linking` trick).

### 3.3 Engine loop & startup

- **Startup chain:** `WinMain` → `GuardedMain` → `FEngineLoop::PreInit` (split into `PreInitPreStartupScreen` / `PreInitPostStartupScreen` [SCAN]) → `Init` → `while (!IsEngineExitRequested()) Tick()` → `Exit`.
- **PreInit** does, in order:
  1. parse the command line;
  2. set up logging and the config system (`GConfig`, ini hierarchy);
  3. start the task graph and threads;
  4. load modules by **loading phase**, then plugins;
  5. initialize the RHI and load global shaders;
  6. initialize Slate and the startup screen;
  7. create `GEngine` (`UGameEngine` for games, `UEditorEngine` for the editor).
- **Tick** (simplified):
  1. compute delta time / frame pacing;
  2. pump OS messages and Slate input;
  3. `GEngine->Tick()` → `UWorld::Tick`, which runs **tick groups** in order: `TG_PrePhysics → TG_StartPhysics → TG_DuringPhysics → TG_EndPhysics → TG_PostPhysics → TG_PostUpdateWork → TG_LastDemotable`. Components and actors register tick functions with prerequisites; the tick task manager runs them as tasks.
  4. tick and draw Slate;
  5. enqueue rendering of the frame;
  6. **frame-end sync:** the game thread waits if the render thread is more than ~1 frame behind.
- **Exit** is cooperative: `RequestEngineExit(reason)` sets a flag polled by the loop. A forced exit terminates. The REA decompilation in §2.10 shows exactly this split.

### 3.4 Object model: UObject, reflection, GC, serialization [DOC]

- **`UObject`** is the base of the managed world. Each object has a `UClass` (reflection metadata), an outer (ownership/package chain), an `FName` and flags.
- **`UClass` / `FProperty` / `UFunction`** describe fields and methods at runtime. This drives:
  - the editor Details panel;
  - serialization (tagged properties, versioning);
  - Blueprint visibility;
  - network replication (`Replicated`, `RepNotify`);
  - config (`Config` properties);
  - GC reference discovery.
- **CDO (Class Default Object):** every class has a default instance. New objects are copied from it, and serialization only stores *deltas* from it.
- **Strings:**
  - `FName` — interned, case-insensitive, compares as integers; used for identifiers.
  - `FString` — mutable.
  - `FText` — localized display text.
- **Garbage collection** is reachability-based: mark from the **root set**, then sweep unreachable objects.
  - **Strong references:** `UPROPERTY` / `TObjectPtr` fields and engine-known containers.
  - **Weak / non-owning:** raw pointers are invisible to GC and dangle, so non-owning references use `TWeakObjectPtr`.
  - **Pinning and external roots:** `AddToRoot()`, `FGCObject` for non-UObjects.
  - **Destruction:** `MarkAsGarbage()` replaced `MarkPendingKill()`.
  - **Incremental reachability** requires all references to be `TObjectPtr`.
- **Packages:** `.uasset` / `.umap` files hold serialized objects plus import/export tables. In cooked builds they are stored in IoStore containers. **Soft references** (`TSoftObjectPtr`, paths) allow lazy loading. **Hard references** pull in dependencies.
- **Takeaway for Rust:**
  - Reflection is the backbone, so make it derive-based and ubiquitous.
  - **Skip** global GC for engine objects. Use explicit ownership (ECS entities/handles).
  - Keep the CDO idea as **prefab/template defaults + delta serialization**.

### 3.5 Gameplay framework [DOC]

- **Topology:** `UGameInstance` (process lifetime) → `UWorld` (a running map) → `ULevel`s (persistent + streamed) → `AActor`s → `UActorComponent`s.
  - `USceneComponent` adds a transform hierarchy.
  - `UPrimitiveComponent` adds rendering/collision.
- **Rules & roles:**
  - `AGameModeBase` — server-only rules;
  - `AGameStateBase` — replicated match state;
  - `APlayerController` — input + player intent;
  - `APawn` / `ACharacter` — the possessed body;
  - `APlayerState` — replicated per-player data;
  - `AHUD` / UMG — UI.
- **Subsystems:** auto-instanced singletons scoped to Engine / GameInstance / World / LocalPlayer. This is the clean way to add services.
- **World Partition:** the world is a streaming grid of cells, with One-File-Per-Actor, data layers and HLODs. Level streaming is the older per-sublevel approach.
- **Game Features + Modular Gameplay** (Lyra): plugins inject components and abilities into existing actors at runtime.
- **Mass (UE's ECS):**
  - **Fragments** = data only; **Tags** = data-less markers; **Chunk/Shared fragments**.
  - **Archetypes** store entities with identical fragment sets in memory chunks.
  - **Processors** are stateless logic that declare an `EntityQuery`.
  - Structural changes are **deferred through a command buffer**. **Traits** bundle fragments + processors.
  - UE 5.8 added sparse/virtual fragments and multi-core processor scheduling.
- **Takeaway:** UE runs **two object models side by side**: heavyweight Actors for designers and Mass for scale. In Rust, an ECS covers both if we add an ergonomic "actor-like" facade (prefabs, components with hooks, hierarchy).

### 3.6 Threading, tasks and the frame pipeline [DOC]

```
time ─────────────────────────────────────────────────────────────►
Game thread   [ frame N: input · tick world · enqueue render ]  [ frame N+1 … ]
Render thread       [ frame N-1: visibility · build RDG · record cmd lists ]
RHI thread                [ frame N-2: translate cmd lists · submit to GPU ]
GPU                              [ frame N-2/N-3 executing ]
```

- **Threaded rendering:** the game thread blocks at the end of `Tick()` until the render thread is within one or two frames.
- **Parallel rendering:**
  - The render thread is a **frontend** that records platform-agnostic RHI command lists.
  - The **RHI thread** translates and submits them. The GPU submission order is unchanged.
  - Debug switches: `r.rhicmdbypass`, `r.rhithread.enable`.
  - UE 5.5 reported up to **2×** gains from parallel translation.
- **Tasks:** `UE::Tasks` launches tasks with **prerequisites** (a DAG). It shares the backend (scheduler + workers) with the legacy `TaskGraph`. The guidance is: **express dependencies, don't block-wait.** Task Graph Insights visualizes critical paths.
- **Takeaway:** frame pipelining + a task DAG is the core performance architecture. In Rust:
  - the main thread runs the OS event loop + game simulation;
  - a render thread consumes an **extracted snapshot**;
  - a work-stealing pool executes system and render tasks.

### 3.7 Rendering [DOC]

- **Game ↔ render mirrors.** Each game-thread object has a render-thread twin:

  | Game thread | Render thread |
  |---|---|
  | `UWorld` | `FScene` (the render thread owns all `FScene` state) |
  | `UPrimitiveComponent` | `FPrimitiveSceneProxy` + `FPrimitiveSceneInfo` |
  | `ULightComponent` | `FLightSceneProxy` / `FLightSceneInfo` |
  | `ULocalPlayer` | `FSceneViewState` (persistent per-view data) |
  | `FSceneView` | `FViewInfo` (per frame) |

  - `FSceneRenderer` is created **every frame** for frame temporaries.
  - Gameplay changes that affect rendering call `MarkRenderStateDirty()` to re-send proxy data.
- **Module split:**
  - The Engine talks to the Renderer through `IRendererModule` / `FSceneInterface`.
  - The **RHI** is "a thin layer above the platform-specific graphics API".
- **Mesh drawing pipeline:**
  - `FMeshBatch` (from proxies) → `FMeshPassProcessor` (per pass: depth, base, shadow, velocity…) → `FMeshDrawCommand`.
  - **Static draw commands are cached** when a primitive is added and only rebuilt on change.
  - **GPUScene** keeps primitive/instance data in GPU buffers for GPU-driven culling.
- **Render Dependency Graph (RDG):** an immediate-mode recording API. Passes are added with a **parameter struct** (declaring read/write resources) and an execute lambda. The graph is compiled and executed, which provides:
  - pass culling;
  - **transient resource allocation with memory aliasing**;
  - split barriers;
  - **async-compute fencing**;
  - parallel command-list recording;
  - validation, plus RDG Insights.

  This matches Frostbite's FrameGraph (GDC 2017): extensibility, simplified async compute, automatic aliasing, big memory savings.
- **Shaders and materials:**
  - HLSL in `.usf/.ush` files, compiled by `ShaderCompileWorker` processes.
  - Permutation domains, global vs material shaders, and **shader maps** per material × vertex factory × permutation.
  - Results are cached in the **DDC** (Derived Data Cache) and shipped as **shader libraries**.
  - **PSO caching / precaching** hides pipeline-compile hitches.
  - Material graphs (`UMaterial`) generate HLSL. **Material instances** override parameters. **Substrate** is the newer layered material model (production in 5.7).
- **Nanite (virtualized geometry)** [WEB, Karis SIGGRAPH 2021]:
  - Meshes are split into **clusters of ~128 triangles**, grouped and simplified into a LOD hierarchy, so LOD is selected per cluster without cracks.
  - **GPU-driven culling** uses a two-pass HZB occlusion scheme.
  - A **visibility buffer** stores cluster+triangle IDs per pixel, and materials are shaded later in screen space.
  - A **software rasterizer** in compute handles tiny triangles; the hardware rasterizer handles big ones.
  - Geometry is **streamed in pages**.
- **Lumen (dynamic GI + reflections)** [DOC, WEB SIGGRAPH 2022]:
  - **Screen traces** first, then fallbacks.
  - **Software ray tracing** against mesh distance fields + a global distance field, or **hardware ray tracing** against triangles.
  - A **surface cache** (low-res "cards" capturing material + lighting).
  - **Screen probes** + a radiance cache for the final gather.
  - Fortnite targets ≈4 ms on consoles for GI + reflections combined, relying on TSR.
- **Other UE5 pillars:**
  - **Virtual Shadow Maps** — huge virtual resolution, cached pages;
  - **TSR** — temporal super resolution;
  - **MegaLights** — many shadowed lights stochastically; production in 5.8;
  - **Lumen Lite** — cheaper GI, 5.8;
  - virtual texturing (runtime and streaming);
  - path tracer; heterogeneous volumes; Substrate.
- **Takeaway order for us:**
  1. proxies + extract;
  2. render graph;
  3. a cached draw-command / mesh-pass system;
  4. GPU-scene buffers + indirect draws;
  5. only then meshlets/virtual geometry and GI.

### 3.8 Assets & the content pipeline [DOC]

- **Editor side:**
  - assets are packages edited in the editor;
  - imports go through **Interchange** (FBX/glTF/USD…);
  - derived data (compiled shaders, compressed textures, Nanite data, distance fields) is built once and **cached in the DDC**, keyed by input hash + processor version.
- **Cooking:** converts editor assets into platform formats.
  - Cooked data uses **unversioned property serialization**.
  - Shaders are compiled for the target RHIs.
  - Results are written to IoStore containers with offline dependency graphs (**Zen loader**).
- **Runtime:**
  - The IO dispatcher schedules chunk reads.
  - **Async loading** resolves imports via the precomputed graph.
  - Texture/mesh/Nanite **streaming managers** keep a memory budget.
  - The **Asset Registry** provides searchable metadata without loading assets.
- **Takeaway:**
  - Separate **source assets**, **derived data cache**, and **cooked containers**.
  - Key derived data by content hash + tool version.

### 3.9 Scripting

- **Blueprints:** a visual graph compiled by the Kismet compiler to **bytecode** for UE's script VM. It calls `UFUNCTION`s through reflection.
- **Verse:** Epic's functional-logic language, used in UEFN. Epic positions it as the future scripting layer [WEB, unverified detail].
- **Python:** editor automation (and some licensees ship it at runtime — Marvel embeds `UnrealEnginePython` [SCAN]).
- **Takeaway:** scripting = **reflection + a VM or hot-patched native code**. Design the reflection layer so any script backend can call engine functions.

### 3.10 Other runtime systems (what each one is)

| System | UE implementation | Notes |
|---|---|---|
| Physics | **Chaos** (rigid bodies, destruction via Geometry Collections, cloth, vehicles, async fixed-step physics) | Replaced PhysX in UE5 |
| Animation | AnimBlueprints/AnimGraph evaluated on workers (`AnimInstanceProxy`), Control Rig (RigVM), IK Rig, Motion Matching, ACL compression | Multi-threaded evaluation |
| Audio | **AudioMixer** (own audio render thread) + **MetaSounds** (DSP graph); Wwise/FMOD as alternatives | Separate thread, sample-accurate |
| VFX | **Niagara**: emitters/modules/data interfaces; CPU **VectorVM** and GPU compute simulation | Data-driven, graph-authored |
| AI | Behavior Trees + Blackboard, EQS, **StateTree**, Smart Objects, Mass AI, Recast/Detour navmesh | StateTree is the modern default |
| Networking | `NetDriver`/`NetConnection`/channels; property replication + RPCs; relevancy, priority, dormancy; ReplicationGraph; **Iris** | Iris is production-ready for licensees in 5.8 |
| UI | **Slate** (C++ declarative widgets), **UMG** (UObject/Blueprint wrapper), **CommonUI** (input routing) | Editor *and* game use Slate |
| Cinematics | Sequencer (`MovieScene`, `LevelSequence`) | Timeline of tracks bound to objects |
| Media | Electra/WMF/Bink players, Movie Render Pipeline | — |

**Iris** in detail [DOC]:

- It keeps a **quantized copy of all replicated state**, decoupling gameplay from replication.
- It is **push-based**: the game notifies it of changes.
- It does **prioritization** (float priority) and **filtering** (Owner/Connection/Group/Dynamic — dynamic is expensive).
- It is split into a **Replication Bridge** (gameplay side) and a **Replication System** (prioritize/filter/serialize).
- These are the right ideas for a Rust ECS replication layer.

### 3.11 Editor

- The editor *is* the engine + `UnrealEd` modules:
  - Slate UI with docking tabs;
  - **Details panels generated from reflection** (with per-type customization);
  - viewports with gizmos;
  - content browser + asset registry;
  - **transactions** (`FScopedTransaction`, `UObject::Modify`) for undo/redo;
  - **Play-In-Editor** (PIE): duplicates the editor world into a game world in the same process;
  - commandlets for batch jobs (cooking, resaving);
  - Live Coding for C++ hot patching.
- **Takeaway:** the editor is a *client of reflection + transactions + world duplication*. Build those three primitives first.

### 3.12 Configuration, tooling & profiling

- **Config:** `.ini` hierarchy (Base → Default → Platform → user/saved). Console variables live in a registry with flags and scalability groups (`sg.*`) and **device profiles** (`DeviceProfiles.cpp` [SCAN]).
- **Profiling:**
  - **Unreal Insights** (trace channels for CPU/GPU/loading/memory/tasks);
  - `stat` commands;
  - **LLM** (Low-Level Memory tracker, tagged allocations);
  - CSV profiler (`CsvMetrics` [SCAN]);
  - RDG Insights;
  - GPU crash tooling (DRED, Aftermath, `r.GPUCrashDebugging.*` cvars [SCAN]).

### 3.13 Version timeline (context)

| Version | Date | Highlights |
|---|---|---|
| UE 5.0 | 2022 | Nanite, Lumen, VSM, TSR, World Partition, MetaSounds, Tasks System |
| UE 5.5 | 2024 | MegaLights (experimental), RDG async-execute and parallel-translation gains |
| UE 5.7 | 2025-11-12 | **PCG** and **Substrate** production-ready, Nanite Foliage (experimental, uses Nanite skinning), MegaLights beta |
| UE 5.8 | 2026-06-17 | **Last major UE5 release.** Lumen Lite, MegaLights production, Mesh Terrain (experimental), Iris production-ready, Mass sparse fragments, an Unreal MCP server plugin |

**UE 6:** early access targeted for late 2027 [WEB].

---

## 4. The Rust game-engine landscape (Oct 2026)

### 4.1 Bevy (0.20, released 2026-10-08) — the ecosystem center of gravity

- **ECS core:**
  - archetype + sparse-set storage;
  - systems with automatic parallel scheduling (conflict analysis at schedule-build time);
  - change detection (0.20 adds per-column change ticks);
  - observers, relationships (parent/child etc.), deferred `Commands`;
  - **resources are now components on singleton entities** (0.19);
  - panics in systems/commands/observers become routed errors (0.20).
- **Rendering:**
  - A **separate render world**. Data is copied in a short **Extract** schedule, the *only* sync point. With `PipelinedRenderingPlugin`, the render app runs **in parallel with the next frame's simulation** (since 0.6).
  - **0.19 replaced the render graph with ECS schedules:** render passes are systems in schedules like `Core3d`.
  - GPU-driven work: GPU light clustering, batched morph targets, partial bindless.
  - **0.20 additions:**
    - **mesh shaders** (`draw_mesh_tasks`);
    - **Solari** real-time path tracer (ReSTIR optional);
    - DLSS-RR 4.5 via `dlss_wgpu`;
    - BCn/ASTC texture compression + mip generation in asset processing;
    - **WESL** shaders (dropped its custom WGSL dialect).
  - Experimental **virtual geometry (meshlets)** since 0.14.
- **Scenes:** **BSN** (`bsn!` macro, composable/patchable scenes, 0.19). A `.bsn` hot-reloadable file format is planned.
- **Editor:**
  - **Not shipped yet.**
  - Feathers widgets (color input, list views, dropdowns, number scrubbing), contextual theming and `PanOrbitCamera` are building blocks.
  - Entity inspector in progress.
- **Hot patching:** Dioxus **`subsecond`** integration since 0.17 (systems only, experimental; only the "tip" crate is patched).
- **Cadence:** a breaking release roughly every 3–4 months (0.18 ≈ early 2026, 0.19 2026-06-19, 0.20 2026-10-08). Plan for migration cost if building *on* Bevy.

### 4.2 Fyrox (1.0, 2026)

- Stable **1.0 after ~7 years** (formerly rg3d).
- A **scene graph** with generational `Pool<T>`/`Handle<T>`, and a **full editor** (FyroxEd).
- PBR + IBL renderer. **No GI yet** (planned for 2.0).
- New `export-cli` for CI builds (PC/WASM/Android) and a task-based scene loader.
- A good reference for **editor + scene-graph architecture in Rust**, and for "how a Rust engine with a real editor is structured".

### 4.3 Others worth knowing

| Project | What it is | Status / note |
|---|---|---|
| **Macroquad** | Simple immediate-mode 2D/3D | Small games |
| **ggez** | 2D framework | — |
| **godot-rust / gdext** | Rust bindings for Godot 4 | Use Godot's editor with Rust gameplay |
| **Comfy** | LogLog's engine | Archived |
| **Ambient** | WASM-based multiplayer engine | Archived |
| **kajiya** (Embark) | Research renderer (ReSTIR GI) | Archived — great reading |
| **rend3** | wgpu renderer | Archived |
| **Veloren** | Open-source voxel RPG with custom wgpu renderer + ECS | Real-world Rust game codebase |
| **Tiny Glade** (Pounce Light) | Reportedly a modified Bevy ECS with a custom Vulkan renderer | [WEB, secondhand] |
| **rust-gpu** | Write shaders in Rust (SPIR-V), community-owned | Still early-stage, not production-ready |

### 4.4 Lessons from the field

**LogLog Games, "Leaving Rust gamedev after 3 years" (2024-04-26)** is required reading. The pain points:

- the borrow checker forcing refactors mid-experiment;
- ECS dogma (fat components often fit game logic better), and too much indirection (events/command buffers split logic);
- weak hot reloading;
- immature game UI/editor tooling;
- the orphan rule;
- Windows incremental compile times;
- awkward global state;
- **runtime borrow panics** after refactors (overlapping queries, `RefCell`);
- lifetimes infecting context structs;
- ecosystem hype vs. substance.

**It concedes:**

- fearless large refactors;
- great generational arenas (`thunderdome`);
- improving ECS ergonomics;
- Rust's excellence for well-understood low-level code.

**What this means for us [REC]:**

1. **Iteration speed is a feature.** Add hot patching (`subsecond`), asset/shader hot reload, cvars, an in-game console and a fast dev profile from M0–M2.
2. **Gameplay code gets an easy mode.** Allow "fat" components, immediate-style APIs (raycast-and-play-sound in a few lines), and a `World` facade. Don't force everything through events.
3. **Prefer static conflict detection (scheduler) over runtime `RefCell`.** Never hold borrows across frames.
4. **Build the editor early and keep it reflection-driven.**
5. **Control compile times structurally:** crate boundaries, dynamic linking in dev, fast linker, no heavy generics in leaf crates.

### 4.5 Crate catalog by subsystem (Oct 2026)

| Subsystem | Primary | Alternatives / notes |
|---|---|---|
| Windowing | `winit` 0.30.x (0.31 in beta) | `sdl3` bindings; `raw-window-handle` 0.6 for interop |
| Gamepad | `gilrs` | Platform-specific: Windows GameInput via `windows` crate |
| GPU (portable) | `wgpu` v30 (2026-07-01; v30.0.1 2026-08-22) | Gained D3D12 Agility SDK support (v29), HDR surface color spaces, procedural BLAS (v30); ray tracing & mesh shaders are feature-gated/experimental |
| GPU (native) | `ash` (Vulkan), `windows` (D3D12), `objc2-metal` | `gpu-allocator` (Vulkan/D3D12 memory), `vk-mem` |
| Shader toolchain | `naga` (WGSL/GLSL/SPIR-V ⇄ MSL/HLSL/SPIR-V/WGSL), **WESL** (`wesl` crate) | Slang (Rust bindings immature: `slang-rs`, `slang-hal`), DXC via `hassle-rs`, `rust-gpu` (early), `wgsl_to_wgpu` (typed bindings) |
| Math | `glam` | `nalgebra` (used inside Rapier/parry), `ultraviolet` |
| ECS | `bevy_ecs` (standalone) | `hecs` (minimal), `flecs_ecs` (Flecs bindings), `shipyard` (sparse set); legion/specs are legacy |
| Handles / arenas | `slotmap`, `thunderdome` | `generational-arena` |
| Reflection | `bevy_reflect` | `facet` (newer derive-reflection), `inventory`/`linkme` (static registration) |
| Serialization | `serde` + `ron` (authoring) | `rkyv` (zero-copy cooked data), `postcard`/`bitcode` (compact binary) |
| Parallelism | `rayon` | `crossbeam`, `parking_lot`, `bevy_tasks` / `async-executor` / `smol` for async IO |
| Profiling | `tracing` + `tracing-tracy` / `tracy-client` | `puffin` (+ `puffin_egui`), RenderDoc, PIX, Nsight |
| Allocators | `mimalloc` (`#[global_allocator]`), `bumpalo` (arenas) | Tracking allocator wrapper for LLM-style tags |
| Crash handling | Embark `crash-handler`, `minidumper` | `sentry` crate, `rust-minidump` (processing) |
| Assets in | `gltf`, `image`, `ktx2`, `basis-universal`, `meshopt`, `exr`, `resvg` | `notify` (file watching), `zstd`/`lz4_flex`, `memmap2` |
| Physics | `rapier3d` 0.35.x (+`parry3d`) | `avian3d` 0.6.x (ECS-native, Bevy-only), `oxijolt` (Jolt 5.6 bindings), `rolt` (older) |
| Audio | `kira` | `cpal` (device IO), `rodio`, `fundsp` (DSP graphs), `symphonia` (decoders), `rubato` (resampling) |
| Animation | glTF skins + own blend graph | `ozz-animation-rs` (port of ozz-animation) |
| Navigation | `oxidized_navigation`, `landmass` | Recast via FFI |
| Editor UI | `egui` (0.36.x), `egui_dock`, `egui_tiles`, `transform-gizmo-egui` | `iced`, `slint`, Linebender `xilem`/`masonry` |
| Game UI | Own retained UI on the renderer + `taffy` (flex/grid layout) | `bevy_ui` design as reference |
| Text | `cosmic-text` or `parley` (layout), `swash`/`skrifa` (glyphs), `rustybuzz`/`harfrust` (shaping) | `glyphon` (wgpu text rendering) |
| Localization | `icu4x` | `fluent` |
| Scripting | `subsecond` (Rust hot patch), `mlua` (Lua/Luau), `wasmtime` (WASM mods) | `rhai`, `hot-lib-reloader` (dylib) |
| Networking | `quinn` (QUIC), `renet`, **`lightyear` 0.28** (prediction/rollback, Bevy 0.19) | `bevy_replicon`, `matchbox` (WebRTC), `steamworks` |
| HTTP / TLS | `reqwest`/`ureq`, `rustls` | — |
| Build / dist | `cargo-packager`, `cargo-dist` | `cargo-nextest`, `sccache`, `cargo-deny` |

---

## 5. Unreal → Rust concept map

| Unreal | What it does | Rust equivalent for our engine |
|---|---|---|
| `Launch` / `FEngineLoop` | Process entry, init order, main loop, exit | `engine_main` crate: `App::run()` with phases + `winit::ApplicationHandler` |
| Module + `Build.cs` | Compilation unit + deps | Crate + `Cargo.toml`; features for optional parts |
| Module load phases | Ordered init | Plugin `build()` ordering + explicit dependency declarations between plugins |
| Plugins / Game Features | Optional feature bundles | `Plugin` trait + feature crates; runtime-activatable "feature" plugins |
| UBT | Build orchestration | Cargo + `xtask` crate (cook, package, shaders) |
| UHT + `UPROPERTY` | Codegen reflection | `#[derive(Reflect, Component, Serialize)]` proc macros |
| `UClass`/`FProperty` | Runtime type info | `TypeRegistry` + `TypeInfo` (bevy_reflect) |
| CDO | Defaults + delta serialization | Prefab/template + patch-based scenes (like BSN) |
| `FName` | Interned identifiers | Interner (`u32` symbol) with `&'static str` table |
| `FText` | Localized text | `icu4x`/`fluent` message IDs |
| GC + `TObjectPtr` | Lifetime management | Ownership + `Entity`/`Handle<T>` (generational) + explicit despawn |
| `TWeakObjectPtr` | Non-owning ref | Generational handle (validated on access) |
| Soft object path | Lazy asset ref | `AssetPath`/`UntypedHandle` with on-demand load |
| `UWorld`/`ULevel` | Simulation container | ECS `World` + streaming "cells" of entities |
| `AActor` + components | Gameplay object | Entity + components (+ actor facade/prefab) |
| Tick groups | Ordered update | Schedules / system sets with ordering constraints |
| Subsystems | Scoped services | ECS resources / service plugins with lifetimes |
| Mass | Data-oriented crowds | Same ECS (archetypes, queries, command buffers) |
| Task graph / `UE::Tasks` | Job DAG | `rayon` / own task graph with prerequisites |
| Render thread + proxies | Render mirror | Render world + **extract** step (proxies are POD structs) |
| `FScene` / `FSceneRenderer` | Persistent + per-frame render state | `RenderScene` (persistent GPU scene) + `FrameRenderer` (per-frame) |
| RHI | GPU abstraction | `rhi` crate trait over wgpu (later ash/D3D12) |
| RDG | Frame graph | `render_graph` crate: passes declare reads/writes; transient pool + aliasing |
| Mesh draw commands | Cached draws | Pre-sorted, cached draw packets per pass; GPU-driven indirect later |
| Shader permutations / DDC | Variants & cache | WESL `@if` features → offline compile → content-hashed cache |
| PSO cache | Hitch avoidance | Pipeline cache (wgpu `PipelineCache` on Vulkan) + precompile at load |
| Materials / instances | Shader graph + params | Material = shader template + parameter block; instances override params |
| Nanite | Virtual geometry | `meshopt` meshlets → cluster DAG → GPU culling (late milestone) |
| Lumen | Dynamic GI | IBL → SSAO/SSGI → probe GI (DDGI) → ReSTIR/path tracing on RT GPUs |
| IoStore / Zen | Cooked containers | TOC + CAS chunk store + dependency lists (`rkyv` payloads) |
| Asset Registry | Asset metadata DB | Index file (path → GUID → deps → type → tags) |
| DDC | Derived data cache | `cache/` keyed by blake3(input + importer version) |
| Interchange | Importers | `gltf`/`image`/`ktx2` importers as plugins |
| Blueprints | Visual scripting | Later: node graph → WESL/IR; first: hot-patched Rust + Lua/WASM |
| Slate / UMG | UI | `egui` (editor) + own game UI (taffy layout) |
| Details panel | Reflection UI | Reflection-driven inspector (egui) with per-type overrides |
| Transactions | Undo/redo | Command stack recording reflected diffs |
| PIE | Play in editor | Clone/serialize editor world → run game schedules → discard |
| Chaos | Physics | `rapier3d` behind `physics` trait (Jolt optional) |
| AudioMixer / MetaSounds | Audio | `kira` + optional `fundsp` graphs on audio thread |
| Niagara | VFX | GPU particle system (compute) + CPU emitters |
| NetDriver / Iris | Replication | `quinn`/`renet` transport + Iris-style quantized state, dirty flags, priority/filter |
| cvars / ini | Tunables | Typed cvar registry + layered TOML/RON config |
| Insights / LLM | Profiling | `tracing` + Tracy + allocation tags + GPU timestamps |
| CrashReportClient | Crash UX | Out-of-process `crash_reporter` app via `minidumper` |
| Shipping config | Strip dev features | Cargo profile + `--features shipping` (no editor, logs compiled out) |

---

## 6. Architecture blueprint for our engine

### 6.1 Strategy choice

| Option | What it means | Pros | Cons |
|---|---|---|---|
| **A. Build on Bevy** | Use Bevy as the engine; add Unreal-like layers (editor, tools) | Fastest results, big ecosystem | Bevy's churn (breaking every ~3–4 months) and architecture constrain you; you learn Bevy, not engines |
| **B. Custom engine on best-of-breed crates** **[REC]** | Own engine loop, RHI, render graph, assets, editor; use `wgpu`, `winit`, `glam`, `bevy_ecs`, `egui`, `rapier`, `kira` | Unreal-like control & learning; replaceable parts; stable API you own | More work than A |
| **C. From scratch to the metal** | Own ECS, Vulkan/D3D12 via `ash`/`windows`, own UI | Maximum control/perf ceiling | Slowest; high risk of never shipping |

**Recommended path:** **B now, keep a door to C.**

- Wrap `wgpu` behind our RHI trait, so a native D3D12/Vulkan backend can be added for bindless/async compute later.
- Wrap `bevy_ecs` behind an engine API, so it can be replaced.

### 6.2 Workspace layout (placeholder prefix `ge_`; final name in Appendix C)

```
Game_Engine/
├── Cargo.toml                 # [workspace] + shared [workspace.dependencies] + profiles
├── crates/
│   ├── ge_core/               # ids/handles, interner, time, errors, cvars, config, logging/tracing init
│   ├── ge_alloc/              # global allocator choice, frame arenas, alloc tags, crash arena
│   ├── ge_platform/           # winit/gilrs wrapper, input events, VFS/file IO, dialogs, GPU-preference exports
│   ├── ge_tasks/              # thread roles, task graph (rayon-backed), main-thread queue, async IO executor
│   ├── ge_reflect/            # reflection facade (+ ge_reflect_derive proc-macros) = our UHT
│   ├── ge_ecs/                # world/entity/component facade over bevy_ecs, hierarchy, transforms, prefabs
│   ├── ge_asset/              # asset server, handles, loaders/importers, hot reload, DDC, cooking, containers
│   ├── ge_rhi/                # GPU abstraction trait + wgpu backend (later: native backends)
│   ├── ge_shader/             # WESL compile, permutations, reflection, pipeline cache, hot reload
│   ├── ge_render/             # render world, extract, render graph, passes, materials, lights, post, debug draw
│   ├── ge_physics/            # physics trait + rapier backend, fixed step, interpolation
│   ├── ge_audio/              # audio thread, kira backend, spatialization
│   ├── ge_anim/               # skeletons, clips, blend trees, state machines, GPU skinning data
│   ├── ge_ui/                 # in-game UI (taffy layout, text via cosmic-text/parley)
│   ├── ge_net/                # transport + replication (later)
│   ├── ge_script/             # hot patch glue, optional Lua/WASM host (later)
│   ├── ge_engine/             # App, Plugin trait, schedules/tick groups, gameplay framework, engine loop
│   └── ge_editor/             # egui editor: viewport, outliner, inspector, gizmos, undo, PIE, asset browser
├── apps/
│   ├── game/                  # runtime binary (thin: loads game plugin)
│   ├── editor/                # editor binary
│   ├── cooker/                # CLI: import → DDC → cook → containers
│   └── crash_reporter/        # out-of-process minidump receiver/uploader
├── games/sample/              # vertical-slice game as a plugin crate
├── assets/                    # source assets (glTF, PNG, WAV …)
├── shaders/                   # *.wesl
├── xtask/                     # cargo xtask: cook, package, shaders, ci
└── docs/
```

**Layering rule** (enforced by the crate graph; checked with `cargo-deny`/`cargo tree` in CI):

```
ge_core ← ge_alloc ← ge_platform ← ge_tasks ← ge_reflect ← ge_ecs ← ge_asset
ge_rhi (← ge_core, ge_platform) ← ge_shader ← ge_render (← ge_ecs, ge_asset)
ge_physics / ge_audio / ge_anim / ge_ui / ge_net / ge_script  (← ge_ecs, ge_asset, …)
ge_engine (← everything runtime)   ←   ge_editor (← ge_engine)   ;   apps/* (← top)
```

**Never:** `ge_render → ge_editor`, `ge_core → anything`, runtime crates → `*_editor`.

### 6.3 Frame lifecycle

```
Main thread (OS events + simulation)          Render thread                 GPU
───────────────────────────────────           ───────────────────────       ─────
poll winit events → input state
fixed-step loop: physics + gameplay (N)
variable update: animation, UI, audio cmds
EXTRACT: copy dirty render data → RenderSnapshot(N) ─────► build frame (N):
continue with frame N+1 …                       visibility/culling
                                                 build render graph
                                                 record + submit cmd buffers ───► execute (N)
                                                 present (N)
Backpressure: main waits if render is >1 frame behind (bounded latency)
```

Sketch (winit 0.30 style; illustrative, not final API):

```rust
struct EngineApp { engine: Option<Engine> }

impl winit::application::ApplicationHandler for EngineApp {
    fn resumed(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        if self.engine.is_none() { self.engine = Some(Engine::init(el)); } // create window, RHI device, worlds
    }
    fn window_event(&mut self, _el: &winit::event_loop::ActiveEventLoop,
                    id: winit::window::WindowId, ev: winit::event::WindowEvent) {
        if let Some(e) = &mut self.engine { e.platform.push_event(id, ev); }
    }
    fn about_to_wait(&mut self, el: &winit::event_loop::ActiveEventLoop) {
        let Some(e) = &mut self.engine else { return };
        e.tick();                                   // fixed-step sim + variable update + extract + hand-off
        if e.exit_requested() { el.exit(); }        // cooperative exit (flag), like UE's RequestEngineExit
        e.platform.request_redraw();
    }
}
```

### 6.4 Core layer essentials

**Handles, not pointers** (generational):

```rust
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Handle<T> { index: u32, generation: u32, _t: std::marker::PhantomData<fn() -> T> }
// Pool<T>: Vec<Slot<T>> + free list; get() checks generation → stale handles return None, never UB.
```

**Typed console variables** (registered statically, editable at runtime, scalability-aware):

```rust
pub static R_SHADOW_QUALITY: CVar<i32> =
    CVar::new("r.Shadow.Quality", 3, "Shadow quality 0..4", CVarFlags::SCALABILITY);
// registry via `inventory`/`linkme`; console commands: `r.Shadow.Quality 1`, `cvars r.Shadow`
```

**Global allocator, frame arenas and the crash path:**

```rust
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;   // UE: FMallocBinned2/mimalloc
// per-frame scratch: bumpalo::Bump reset at frame end (one per worker thread)
// crash: std::panic::set_hook → write context → minidumper client signals out-of-process server
```

**Other core pieces:**

- **Logging/profiling:** `tracing` spans on every system/pass. Use `tracing-tracy` in dev and compile out verbose levels in shipping (`max_level_*` features).
- **Config:** layered TOML/RON (engine defaults → project → platform → user). Same keys as cvars.
- **Discrete GPU on hybrid laptops:**
  - Request `PowerPreference::HighPerformance` from wgpu (DXGI GPU-preference enumeration).
  - As a safety net, export `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` like UE does.
  - In a Rust *executable* these must be force-exported, e.g. `#[unsafe(no_mangle)] pub static NvOptimusEnablement: u32 = 1;` + a `/EXPORT:NvOptimusEnablement` linker arg from `build.rs`. Verify with `dumpbin /exports` [INF — test it].

### 6.5 World / ECS

- **Use `bevy_ecs` standalone first** [REC]: archetype storage, parallel scheduler with static conflict detection, change detection, observers/hooks, relationships, commands. Wrap it: `ge_ecs::World`, `Entity`, `Query`, `Commands`, `Schedule`. This keeps the option to swap (e.g., `flecs_ecs`, or our own).
- **Hierarchy & transforms:**
  - a `Parent`/`Children` relationship;
  - `Transform` (local) and `GlobalTransform` (world), propagated in a dedicated system set after gameplay and before extract.
- **Tick groups:** schedule sets `PrePhysics → Physics (fixed) → PostPhysics → Animation → PostUpdate → Extract`.
- **Actor facade:** "prefab" templates (default components) + spawn APIs + per-entity script components. This gives designers UE-like actors without fighting the ECS (LogLog lesson: allow fat components).
- **Mass-style scale:** the same ECS handles crowds. Batch processing, chunk-level data, command-buffered structural changes.

### 6.6 Reflection & serialization

- `#[derive(Reflect)]` on every component/asset/settings type. A **TypeRegistry** with stable type names + optional UUIDs.
- **Uses:**
  - editor inspector;
  - scene/prefab serialization (RON, patch-based like BSN);
  - undo diffs;
  - network replication descriptors;
  - script bindings;
  - cvar/config binding.
- **Versioning:** each serialized type gets a schema version + migration functions. Cooked data uses **unversioned** compact binary (`rkyv`), like UE's unversioned property serialization.

### 6.7 Rendering architecture

- **RHI trait** (`ge_rhi`):
  - device, queue, buffers/textures/samplers, bind groups, pipelines, command encoding, swapchain, timestamps, debug labels;
  - backed by **wgpu** first. Keep it thin, like UE's RHI;
  - *no* scene concepts in the RHI.
- **Render world + extract:**
  - Gameplay components → **POD proxies** (`PrimitiveProxy { mesh, material, world_from_local, bounds, flags }`).
  - Copy only dirty data (change detection = `MarkRenderStateDirty`).
  - The render scene keeps persistent GPU buffers (**GPU scene**).
- **Render graph** (`ge_render::graph`). Passes declare resource reads/writes. The graph then:
  - topologically sorts;
  - culls unused passes;
  - allocates **transient** textures/buffers from a pool with lifetime-based **aliasing**;
  - inserts barriers (implicit in wgpu; explicit in native backends);
  - records passes in parallel where the backend allows.

  ```rust
  let gbuf = graph.create_texture("SceneColor", TextureDesc::hdr(size));
  graph.add_pass("Opaque", |p| {
      p.write_color(gbuf, LoadOp::Clear(Color::BLACK));
      p.write_depth(depth, LoadOp::Clear(0.0));        // reversed-Z
      p.read_buffer(scene.instances);
      move |ctx: &mut PassContext| ctx.draw_cached(&opaque_bin)
  });
  ```
- **Pipeline:**
  1. clustered **forward+** with a depth prepass (MSAA-friendly, lower bandwidth — good for a 4 GB laptop GPU). Deferred is optional later;
  2. PBR (glTF metallic-roughness);
  3. CSM directional shadows + spot/point atlas;
  4. IBL (prefiltered env + SH);
  5. HDR → bloom → tonemap (AgX/ACES/TonyMcMapface) → color grading LUT;
  6. TAA → upscaler (FSR/DLSS hooks later).
- **Shaders:**
  - WESL modules with `@if` features as **permutations**;
  - compiled at cook time (and on change in the editor), cached by content hash;
  - pipeline cache persisted;
  - a **PSO pre-warm** step at level load.
- **Draw submission:**
  - cached draw packets per pass, sorted by pipeline/material;
  - later GPU-driven: instance buffer + compute culling (frustum + HZB two-pass occlusion) + `multi_draw_indirect_count`.
- **Frontier (T3, after M12):**
  - meshlets via `meshopt` + mesh shaders (wgpu experimental, used by Bevy 0.20);
  - virtual geometry DAG;
  - SSGI → DDGI;
  - ReSTIR/path tracing on RT hardware (Bevy's Solari is a reference);
  - virtual shadow maps.
- **Debuggability:** debug labels on every pass, GPU timestamps per pass in the profiler, RenderDoc capture hotkey, wgpu validation in dev.

### 6.8 Assets & cooking

- **Handles everywhere:** `Handle<Mesh>`, `Handle<Texture>`, …, with **reference-counted strong handles + weak IDs**. The `AssetServer` loads asynchronously on IO tasks and tracks dependency trees.
- **Three stores:**
  1. **source** (`assets/`, glTF/PNG/WAV + `.meta` sidecars with GUIDs and import settings);
  2. **DDC** (derived data keyed by `blake3(source bytes + settings + importer version)`);
  3. **cooked containers** (TOC + chunk store + per-package dependency lists, compressed with zstd/lz4, payloads in `rkyv` for zero-copy).
- **Hot reload:** `notify` watches sources → re-import → swap the asset version atomically → dependents get change events. Shaders get the same treatment.
- **Registry:** an index of all assets (GUID ↔ path ↔ type ↔ deps ↔ tags) for the editor browser and the cooker.
- **Streaming:** priority-sorted IO queue, texture mips and mesh LODs on demand, a memory budget manager.

### 6.9 Other runtime systems

- **Physics:**
  - A `PhysicsBackend` trait with a Rapier implementation.
  - **Fixed timestep** (e.g., 60 Hz) with render interpolation; collision layers/filters; a character controller.
  - Determinism mode if networking/replays need it.
- **Audio:**
  - A dedicated audio thread with `kira` (buses, tweens, clocks), 3D spatialization, streaming music.
  - Gameplay sends **commands**, never touches audio state directly.
- **Animation:**
  - glTF skins/clips → sample on worker tasks → blend trees/state machines → pose buffer → GPU skinning.
  - IK later. Anim evaluation is parallel (UE `AnimInstanceProxy` idea).
- **Game UI:** a retained widget tree, `taffy` layout, text via `cosmic-text`/`parley`, drawn by our renderer. Data binding through reflection. The editor uses egui instead.
- **Scripting / hot reload:**
  1. `subsecond` hot patching for Rust gameplay systems (dev only);
  2. later, `mlua` (Lua/Luau) or `wasmtime` (sandboxed mods) bound through reflection;
  3. visual scripting only after the reflection API is stable.
- **Networking (later):**
  - `quinn`/`renet` transport;
  - server-authoritative replication with **Iris principles**: quantized state copies, push dirty-tracking, per-connection priority + filtering;
  - client prediction/rollback (study `lightyear`).

### 6.10 Editor

- **Process model:** start **in-process** (editor = engine + `ge_editor` plugin). Consider an out-of-process viewport later for crash isolation.
- **Panels:** viewport (render-to-texture), outliner (hierarchy), **inspector (reflection-driven)**, asset browser (registry), console (cvars), profiler (Tracy/puffin), log.
- **Primitives to build first:**
  1. **selection model**;
  2. **command/transaction stack** (every edit = reversible diff of reflected fields; merges for drags);
  3. **world snapshot/clone** for **Play-In-Editor**;
  4. **gizmos** (`transform-gizmo-egui`);
  5. **scene save/load** (RON + GUIDs).
- **Never** let runtime crates depend on editor crates. Editor-only data lives in `*_editor` crates or behind an `editor` feature.

### 6.11 Build, profiles, packaging

`Cargo.toml` profile suggestions [REC]:

```toml
[profile.dev]
opt-level = 1                 # playable debug builds
debug = "line-tables-only"    # faster builds, usable backtraces
[profile.dev.package."*"]
opt-level = 3                 # optimize dependencies (wgpu, rapier, glam …)
debug = false
[profile.release]
lto = "thin"
codegen-units = 16
[profile.shipping]
inherits = "release"
lto = "fat"
codegen-units = 1
panic = "abort"               # decide explicitly (abort = smaller/faster; unwind = catch per-system)
strip = "symbols"             # keep PDBs separately for crash symbolication
```

- **Fast iteration on Windows:**
  - use `rust-lld` as the linker in `.cargo/config.toml`;
  - add **Windows Defender exclusions** for `target/` and `~/.cargo`;
  - use dynamic linking of the engine crate in dev (Bevy's `dynamic_linking` pattern);
  - try `sccache`;
  - try the Cranelift backend on nightly for dev builds (still nightly-only in 2026);
  - LLD became the default linker on Linux x86-64 in Rust 1.90.
- **Configurations** (UE parity):

  | Config | What it is |
  |---|---|
  | `dev` | Editor, validation, hot reload |
  | `profile` | Optimized + Tracy |
  | `shipping` | `--features shipping`: no editor, no cheats/console, minimal logs, crash reporter on |

- **Packaging:** `cargo xtask package` → cook → containers + exe + PDBs (kept private) → `cargo-packager` installer.

### 6.12 Diagnostics & quality

- **Tracing and profiling:** `tracing` spans named like UE stats (`Frame`, `GameThread`, `Render::Opaque`), Tracy zones + GPU zones, frame-time HUD (`stat unit` analog), memory tags per subsystem.
- **Testing:**
  - unit tests for core;
  - **golden-image tests** for rendering (headless wgpu, compare with tolerance);
  - deterministic simulation tests;
  - `cargo-nextest` in CI.
- **Crash handling:** panic hook → minidump via an out-of-process server → `crash_reporter` app; symbolicate with `rust-minidump`.

---

## 7. Rust-specific pitfalls & patterns

1. **Graphs of objects with back-pointers fight the borrow checker.** Store nodes in pools/ECS and reference them by **handles/IDs**. `Rc<RefCell<…>>` webs lead to runtime panics, leaks and refactor pain.
2. **Borrow-splitting is an API design problem.** Systems declare read/write sets so the scheduler proves non-aliasing. Use `Commands` for structural changes. Avoid holding borrows across frames or callbacks.
3. **Runtime borrow checking (`RefCell`, `AtomicRefCell`, overlapping dynamic queries) moves bugs to runtime.** Prefer static scheduling (bevy_ecs detects conflicts at schedule build).
4. **Global state:**
   - use explicit resources/contexts;
   - `OnceLock`/`LazyLock` for true singletons (cvar registry, interner);
   - edition 2024 denies `static mut` references by default.
5. **Threads:**
   - Create windows and pump events on the **main thread**.
   - Keep GPU-object ownership clear (render thread) and send *commands/snapshots*, not shared mutable state.
   - Use `crossbeam` channels / triple buffers between threads.
6. **Compile times are architecture:**
   - many small crates;
   - avoid giant generic APIs in hot-edited crates;
   - limit proc-macro and `serde` derive spread;
   - dynamic linking in dev, a fast linker, `cargo build --timings` regularly;
   - Windows incremental builds are slower than Linux, so budget for it.
7. **Hot reload limits:**
   - `subsecond` patches function bodies in the *tip* crate only, and struct layout changes need a restart.
   - Design gameplay state to be **serializable** so a restart can resume (save/restore world on reload).
8. **Orphan rule:** keep traits and the types implementing them in our crates. Wrap foreign types in newtypes.
9. **Dynamic vs static dispatch:** use `dyn Trait` at plugin/editor/backend boundaries and generics in hot loops. Reflection uses `Any` + type IDs.
10. **`unsafe` budget:**
    - confine it to `ge_alloc`, ECS storage internals (if we write our own), GPU buffer mapping, FFI;
    - document invariants (`// SAFETY:`);
    - run Miri on pure-Rust unsafe code.
11. **Panics:** decide `abort` vs `unwind` for shipping. Catch at system boundaries in dev, as Bevy 0.20 does. Always emit a crash report.
12. **Async:** don't drive the frame loop with Tokio. Use async only for IO (asset loading, networking) on a small executor, and never block the main thread.
13. **FFI to C/C++ middleware** (Jolt, Steam, vendor SDKs):
    - `bindgen` / `cxx` / C shims (e.g., JoltC);
    - build with `cc`/`cmake` crates;
    - match the MSVC runtime (`/MD`);
    - keep FFI in `*-sys` crates.
14. **Floating-point determinism** (for netcode/replays):
    - fixed timestep, no fast-math;
    - avoid platform-dependent intrinsics in sim code;
    - prefer deterministic physics modes (Rapier `enhanced-determinism`, Jolt cross-platform-deterministic builds).
15. **Windows specifics:**
    - MSVC toolchain + VS Build Tools 2022 (installed);
    - long-path support;
    - `#![windows_subsystem = "windows"]` for shipping (no console);
    - per-monitor DPI (winit handles it);
    - PDB handling for crash symbolication.

---

## 8. Roadmap (milestones with exit criteria)

| # | Milestone | Exit criteria ("done when…") |
|---|---|---|
| **M0** | Workspace & window | Workspace with the crate skeleton (§6.2), CI (fmt/clippy/test), `tracing` + Tracy, cvar registry stub; window opens; RHI clears screen on the **RTX 3050** (HighPerformance adapter logged) |
| **M1** | Triangle → mesh | Camera + depth (reversed-Z), indexed mesh, uniform buffers, WGSL/WESL shader **hot reload**, resize/minimize handling, GPU timestamps in Tracy |
| **M2** | Engine loop + ECS | `App`/`Plugin`, schedules = tick groups, fixed-step + variable update, input actions mapping, transform hierarchy, cooperative exit flag, frame pacing |
| **M3** | Assets v1 | AssetServer with handles, async loading, glTF meshes/materials/textures (KTX2/BCn), `.meta` GUIDs, file-watch hot reload, DDC cache |
| **M4** | PBR renderer v1 | Clustered forward+, directional/point/spot lights, CSM + atlas shadows, IBL, HDR + bloom + tonemapping; 1080p ≥ 60 fps on a sample scene on this laptop |
| **M5** | Render architecture | Render world + **extract** + pipelined render thread (≤1 frame latency), render graph with transient aliasing + pass culling, cached draw packets, debug labels |
| **M6** | Editor shell | egui docking: viewport, outliner, **reflection inspector**, gizmos, **undo/redo**, scene save/load (RON), **Play-In-Editor** |
| **M7** | Physics & gameplay framework | Rapier backend, collision layers, character controller, "GameMode/PlayerController-like" plugin roles, subsystems/services |
| **M8** | Audio & animation | kira audio thread, 3D sounds; skeletal animation (glTF skins), blend tree/state machine, GPU skinning |
| **M9** | UI & text | In-game UI with taffy layout, text shaping, input focus/navigation (gamepad) |
| **M10** | Cook & ship | Cooker CLI → containers (TOC/CAS), shipping profile/feature, crash reporter app, installer; packaged sample runs without source assets |
| **M11** | Iteration & scripting | `subsecond` hot patch for gameplay, world save/restore on reload, optional Lua or WASM scripting via reflection |
| **M12** | Rendering T2+ | GPU-driven culling + indirect draws, TAA + upscaler hook, SSAO/SSGI, PSO pre-warm; then meshlets / virtual shadow maps / probe GI (T3) |
| **M13** | Networking (if the game needs it) | Transport, replication with priority/filtering, client prediction for the sample |

**Vertical slice:** pick a small 3D game (e.g., a third-person arena or a diorama builder) and make every milestone serve it.

---

## 9. This machine: constraints & opportunities

| Item | Value | Implication |
|---|---|---|
| OS | Windows 11 Home (26300) | D3D12 + Vulkan both available; Windows-first development |
| CPU | AMD Ryzen 7 6800H, 8C/16T | Plenty of workers for the task system; compile parallelism OK |
| RAM | 15.3 GB | Editor + game + Tracy + browser is fine; full Ghidra on 200 MB binaries is not |
| dGPU | **NVIDIA RTX 3050 Laptop, 4,096 MiB**, driver 610.74 | Ampere: hardware RT, mesh shaders, DX12 Ultimate; **4 GB VRAM** → budget ≈3 GB for content + render targets; prefer forward+ over fat G-buffers |
| iGPU | AMD Radeon 680M (RDNA2) | **Hybrid laptop:** force the discrete GPU (HighPerformance adapter + exports, §6.4); also a good second test GPU |
| Rust | 1.97.1 stable, MSVC host; `rust-src`, clippy, rustfmt; targets `x86_64-pc-windows-msvc`, `wasm32-unknown-unknown`, `wasm32-wasip1` | Edition 2024 available; WASM targets allow a WebGPU build later |
| Tools present | Git, VS Build Tools 2022, Vulkan runtime (`vulkaninfo`) | — |
| Tools missing | Vulkan SDK (validation layers), RenderDoc, DXC, sccache, cargo-nextest, Tracy profiler GUI | Install when reaching M0–M1 |
| Perf target [REC] | 1080p @ 60 fps (16.6 ms) on the RTX 3050 Laptop; 30 fps floor on the 680M | Defines scalability defaults |

---

## 10. Glossary

| Term | Meaning |
|---|---|
| **RHI** | Render Hardware Interface — thin GPU-API abstraction |
| **RDG / frame graph / render graph** | Per-frame graph of passes + resources compiled for barriers, aliasing, culling |
| **Proxy / extract** | Render-thread copy of game data, synchronized at one point per frame |
| **PSO** | Pipeline State Object (compiled shader + fixed-function state); compiling one at draw time causes hitches |
| **Permutation** | Compile-time shader variant selected by feature flags |
| **DDC** | Derived Data Cache — cached results of expensive asset processing |
| **Cook** | Convert editor assets to runtime/platform formats |
| **IoStore / Zen loader** | UE5 cooked containers (`.utoc`/`.ucas`) + loader with offline dependency graphs |
| **CDO** | Class Default Object — per-class default instance |
| **PIE** | Play-In-Editor |
| **Tick group** | Ordered phase of the frame update |
| **Archetype** | ECS storage grouping of entities with identical component sets |
| **Generational handle** | Index + generation; stale handles are detectable |
| **Visibility buffer** | G-buffer alternative storing triangle/cluster IDs per pixel |
| **HZB** | Hierarchical Z-buffer used for occlusion culling |
| **Meshlet / cluster** | Small triangle group (~64–128 tris) for GPU culling/LOD |
| **GI** | Global illumination (indirect lighting) |
| **TSR / TAA** | Temporal super resolution / temporal anti-aliasing |
| **CSM / VSM** | Cascaded shadow maps / virtual shadow maps |
| **cvar** | Console variable — named runtime tunable |
| **LLM (UE)** | Low-Level Memory tracker (tagged allocation accounting) |
| **Agility SDK** | Redistributable D3D12 runtime shipped with the game |

---

## 11. Sources & reading list

**REA / this machine**

- REA MCP (`rea-agents` 6.5.0, Ghidra 12.1.4 provider). Evidence ledger in Appendix A.

**Unreal Engine — official docs [DOC]**

- [Graphics Programming Overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/graphics-programming-overview-for-unreal-engine) — game/render class pairs, module split.
- [Threaded Rendering](https://dev.epicgames.com/documentation/en-us/unreal-engine/threaded-rendering-in-unreal-engine) · [Parallel Rendering Overview](https://dev.epicgames.com/documentation/en-us/unreal-engine/parallel-rendering-overview-for-unreal-engine)
- [Render Dependency Graph](https://dev.epicgames.com/documentation/en-us/unreal-engine/render-dependency-graph-in-unreal-engine)
- [Tasks System](https://dev.epicgames.com/documentation/en-us/unreal-engine/tasks-systems-in-unreal-engine)
- [Unreal Object Handling](https://dev.epicgames.com/documentation/unreal-engine/unreal-object-handling-in-unreal-engine) · [Incremental GC](https://dev.epicgames.com/documentation/en-us/unreal-engine/incremental-garbage-collection-in-unreal-engine) · [Object Pointers](https://dev.epicgames.com/documentation/unreal-engine/object-pointers-in-unreal-engine?lang=en-US)
- [Unreal Engine Modules](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-modules) · [Unreal Build Tool](https://dev.epicgames.com/documentation/en-us/unreal-engine/unreal-build-tool-in-unreal-engine) · [Plugins](https://dev.epicgames.com/documentation/unreal-engine/plugins-in-unreal-engine)
- [Zen Loader](https://dev.epicgames.com/documentation/en-us/unreal-engine/zen-loader-in-unreal-engine)
- [Overview of Mass Entity](https://dev.epicgames.com/documentation/en-us/unreal-engine/overview-of-mass-entity-in-unreal-engine)
- [Iris Replication System](https://dev.epicgames.com/documentation/en-us/unreal-engine/iris-replication-system-in-unreal-engine) · [Introduction to Iris](https://dev.epicgames.com/documentation/en-us/unreal-engine/introduction-to-iris-in-unreal-engine)
- [Lumen Technical Details](https://dev.epicgames.com/documentation/en-us/unreal-engine/lumen-technical-details-in-unreal-engine) · [Lumen tech blog](https://www.unrealengine.com/en-US/tech-blog/unreal-engine-5-goes-all-in-on-dynamic-global-illumination-with-lumen) · [Lumen in Fortnite Ch.4](https://www.unrealengine.com/tech-blog/lumen-brings-real-time-global-illumination-to-fortnite-battle-royale-chapter-4)
- [UE 5.7 release notes](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-7-release-notes) · [UE 5.8 release notes](https://dev.epicgames.com/documentation/unreal-engine/unreal-engine-5-8-release-notes) · [UE 5.8 announcement](https://www.unrealengine.com/news/unreal-engine-5-8-is-now-available) · [UE 5.7 coverage](https://gamefromscratch.com/unreal-engine-5-7-released/) · [UE 5.8 + UE6 timeline](https://wnhub.io/news/engines/item-51157)

**Landmark talks / papers [WEB]**

- Karis, Stubbe, Wihlidal — [A Deep Dive into Nanite Virtualized Geometry (SIGGRAPH 2021)](https://advances.realtimerendering.com/s2021/Karis_Nanite_SIGGRAPH_Advances_2021_final.pdf)
- Wright, Narkowicz, Kelly — [Lumen: Real-time GI in UE5 (SIGGRAPH 2022)](https://advances.realtimerendering.com/s2022/SIGGRAPH2022-Advances-Lumen-Wright%20et%20al.pdf)
- O'Donnell — [FrameGraph: Extensible Rendering Architecture in Frostbite (GDC 2017)](https://www.slideshare.net/DICEStudio/framegraph-extensible-rendering-architecture-in-frostbite)
- Gyrling — [Parallelizing the Naughty Dog Engine Using Fibers (GDC 2015)](https://gdcvault.com/play/1022186/)
- Mike Acton — *Data-Oriented Design and C++* (CppCon 2014)
- Sander Mertens — *Building an ECS* series (Medium; archetypes, hierarchies) — Flecs author
- Godot — [Optimization using Servers (RIDs)](https://docs.godotengine.org/en/stable/tutorials/performance/using_servers.html) · [Architecture overview](https://docs.godotengine.org/en/latest/engine_details/architecture/godot_architecture_diagram.html)

**Books**

- Jason Gregory — *Game Engine Architecture*, **4th edition (2026, two volumes**: I Foundations · II Graphics, Motion and Sound) — e.g. [retailer listing](https://www.tomlinsons-online.com/p-40157215-game-engine-architecture.aspx)
- *Real-Time Rendering* (4th ed.) · *Physically Based Rendering* (pbr-book.org) · *Game Programming Patterns* (gameprogrammingpatterns.com)

**Rust ecosystem [WEB]**

- Bevy — [0.20 release (2026-10-08)](https://bevy.org/news/bevy-0-20/) · [0.19 release (2026-06-19)](https://bevy.org/news/bevy-0-19/) · [0.6 pipelined renderer](https://bevy.org/news/bevy-0-6/) · [0.14 virtual geometry](https://bevy.org/news/bevy-0-14/) · [hot patching design note](https://hackmd.io/@bevy/SkdMQIYWle)
- wgpu — [releases (v30.0.1 latest)](https://github.com/gfx-rs/wgpu/releases)
- Fyrox — [1.0.0 announcement](https://fyrox.rs/blog/post/fyrox-game-engine-1-0-0/) · [coverage](https://gamefromscratch.com/after-7-years-of-development-fyrox-1-0-is-here/)
- WESL — [wesl-lang.dev](https://wesl-lang.dev/)
- subsecond hot patching — [Dioxus 0.7 release](https://dioxuslabs.com/blog/release-070) · [docs.rs](https://docs.rs/crate/subsecond/0.7.10)
- LogLog Games — [Leaving Rust gamedev after 3 years](https://loglog.games/blog/leaving-rust-gamedev/)
- Rust toolchain — [LLD default on Linux in 1.90](https://blog.rust-lang.org/2025/09/01/rust-lld-on-1.90.0-stable) · [2026 Fast Builds roadmap](https://goals.rust-lang.org/2026/roadmap-fast-builds.html) · [Cranelift goal](https://goals.rust-lang.org/2026/improve-cg_clif-performance.html)
- Physics — [Rapier docs](https://rapier.rs/docs/) · [Avian](https://docs.rs/avian3d) · [oxijolt](https://docs.rs/oxijolt) / [rolt](https://docs.rs/rolt)
- Networking — [lightyear releases](https://github.com/cBournhonesque/lightyear/releases)
- Crash handling — [minidumper](https://docs.rs/minidumper) · [rust-minidump at Mozilla](https://hacks.mozilla.org/2022/06/everything-is-broken-shipping-rust-minidump-at-mozilla/)
- egui — [releases](https://github.com/emilk/egui/releases) · winit — [docs.rs](https://docs.rs/crate/winit)
- rust-gpu — [project (community-owned)](https://github.com/Rust-GPU/rust-gpu)
- Learning — Learn Wgpu (sotrh.github.io/learn-wgpu) · vkguide.dev · Are We Game Yet (arewegameyet.rs)

---

## Appendix A — REA / scan evidence ledger

**REA** (MCP `rea-agents` 6.5.0; Ghidra 12.1.4; Evidence IDs truncated to 8 hex digits after `ev_`):

| Evidence | Operation | Target (SHA-256 prefix) | Observation |
|---|---|---|---|
| `ev_4ce56e57…` | `inspect_pe_resources` | `JP-Win64-Shipping.exe` (e5438e84…) | PE32+ AMD64, 192,685,568 B; resources: AFX_DIALOG_LAYOUT, RT_ICON, RT_DIALOG 101–108, RT_ACCELERATOR 101, RT_GROUP_ICON 123, RT_VERSION, RT_MANIFEST |
| `ev_098d5939…` | `inspect_pe_resources` | `Marvel-Win64-Shipping.exe` (9f52b0ee…) | 302,572,544 B; dialog/accelerator/manifest/AFX payload hashes **identical** to Neighbors |
| `ev_2f274cb3…` | `binary_overview` (Ghidra) | `JP.exe` (903e7aba…) | References PDB `start_protected_game.pdb` (EAC bootstrapper); 7,907 procedures, 5,554 strings |
| — | `search_strings` (Ghidra) | `CrashReportClient.exe` (8aa02e04…) | `provider_timeout`: Ghidra startup deadline 330,000 ms exceeded (full auto-analysis required) |
| `ev_7805fd5b…` | `search_strings` | `EpicWebHelper.exe` (37194300…) | Allocator names `binned2`, `jemalloc`, `ANSI`, `Mimalloc` |
| `ev_2e8e0d73…` | `search_strings` | same | TBBmalloc 2019.0 banner, mimalloc messages, `MallocBinned2/3.FlushThreadCacheMaxWaitTime`, `MallocCrash` strings |
| `ev_fb48c1f6…` | `xrefs` | same | "binned2" referenced at `0x14003aa10`, `0x14003aa27` |
| `ev_23452d99…` | `resolve_containing_procedure` | same | Both in `FUN_14003a920` (532 bytes) |
| `ev_ad52ebeb…` | `procedure_pseudo_code` | same | Crash-allocator realloc pattern (crashed-thread check, 2 MiB small pool, previous-allocator vtable queries, OutputDebugStringW + RequestExit) |
| `ev_afc1cd38…` | `procedure_pseudo_code` | same | `FUN_1400e11c0` = `do { Sleep(INFINITE); } while (true);` |
| `ev_3b5fb1a5…` | `procedure_pseudo_code` | same | `FUN_1400e0530` = RequestExit(force): force → terminate path; else "Win RequestExit" + flag + `PostQuitMessage(0)` |

**Static scan** (read-only Python PE parser + string dump; scratchpad `tools/pescan.py`):

- **Neighbors imports:**
  - static: `dxgi` (CreateDXGIFactory/1), `OPENGL32` (wgl*), `DSOUND`, `WINHTTP`, `WS2_32`, `UIAutomationCore`, `IMM32`, `SETUPAPI`, `MSVCP140`;
  - delay: `d3d12`, `d3d11`, `XAudio2_9Redist`, `XINPUT1_4`, `MFPlat`/`MFReadWrite`, `DirectML`, `GFSDK_Aftermath_Lib.x64`, `libcef`, `libvorbisfile_64`, `EOSSDK-Win64-Shipping`, `steam_api64`, `discord_partner_sdk`, `dbghelp`.
- **Neighbors exports (90):** `NvOptimusEnablement`, `AmdPowerXpressRequestHighPerformance`, `D3D12SDKVersion`, `D3D12SDKPath`, `ags*`, `INTC_D3D11_Xess*`, `INTC_D3D12_*`, `nsvg*`, `src_*`.
- **Marvel extras:**
  - static: `NtUniSdkBase`, `NgSocialNet`, `libenvsdk`, `CrashHunter_PC3`, `HID`, `pdh`, `wevtapi`, `XINPUT1_3`;
  - delay: `d3d9`, `dvp`, `libxell`, `libxess_fg`, `OpenImageDenoise`, `WinPixEventRuntime`, `MF`;
  - 835 exports; `PyRuntim` section.
- **Counts:** see §2.3 and §2.7. Union of embedded source paths: 3,213 files.

## Appendix B — How to reproduce the analysis

1. **REA, target-free:** `inspect_pe_resources` on each `*-Shipping.exe` (set `max_file_bytes` up to 512 MiB).
2. **REA deep (Ghidra):** `open_binary` (provider `ghidra`) on a **small** UE program (≤ ~5 MB of code fits the 330 s deadline), then `search_strings` → `xrefs` → `resolve_containing_procedure` → `procedure_pseudo_code`. For bigger targets, raise `REA_GHIDRA_STARTUP_TIMEOUT_MS` in REA's MCP server environment (e.g., `1800000`) and restart the session. Expect long analysis times and high RAM use.
3. **Static scan:** `python -I pescan.py <exe> <outdir>` writes `*.report.txt`, `*.imports.txt`, `*.exports.txt`, `*.strings.txt`. Then grep, for example:
   - `'^[AU] r\.[A-Za-z][A-Za-z0-9_.]+$'` (cvars);
   - `'/Script/[A-Za-z0-9_]+'` (reflection packages);
   - `'Engine[\\/]Source[\\/]…\.cpp'` (module map).
4. **Never** run or attach to anti-cheat-protected games. Treat every string from a target as untrusted data.

## Appendix C — Decisions to lock in before writing CLAUDE.md

1. **Engine name and crate prefix** (placeholder `ge_`).
2. **Strategy:** A (on Bevy) / **B (custom on crates, recommended)** / C (to the metal).
3. **ECS:** `bevy_ecs` standalone (recommended) vs own vs `flecs_ecs`/`hecs`.
4. **GPU:** wgpu-only for now vs a planned native D3D12/Vulkan backend (when?).
5. **Shader language:** WESL/WGSL (recommended) vs Slang vs HLSL→SPIR-V.
6. **Platforms:** Windows-first; Linux? WebGPU/WASM? (WASM targets are already installed.)
7. **Editor UI:** egui in-process (recommended) vs separate process; game UI approach.
8. **Physics backend:** Rapier (recommended) vs Avian vs Jolt (`oxijolt`).
9. **Scripting:** Rust hot patch only vs + Lua (`mlua`) vs + WASM (`wasmtime`).
10. **Asset formats:** glTF + KTX2 in; `rkyv` cooked; container design; GUID scheme.
11. **Vertical-slice game** (genre/scope) that drives milestones.
12. **Quality bar:**
    - MSRV/edition (2024; toolchain 1.97.1);
    - clippy level;
    - `unsafe` policy;
    - test strategy (golden images);
    - perf budgets (1080p60 on the RTX 3050 Laptop, 4 GB VRAM).
13. **License:** MIT OR Apache-2.0 (Rust norm) vs other.
14. **Workflow rules for Claude:** e.g., the plan/approval loop and the checks to run. Note that `c:\Users\navas\Downloads\AGENTS.md` (the Autonomous AI Desktop Agent project) is auto-loaded for any session under `Downloads`. The engine's CLAUDE.md should state that **it** takes precedence inside `Game_Engine`.
