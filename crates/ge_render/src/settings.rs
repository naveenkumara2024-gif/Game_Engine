//! The render-settings **schema**: a stand-in for reflection (plan 0004, decision 4; finding
//! B1). Every row the editor's properties panels show for render/world/light/material settings
//! is described here — label, the EEVEE setting it mirrors, a tooltip, range, unit, default and
//! **status**:
//!
//! - [`Status::Live`] rows change the renderer now; they are backed by a cvar (law 8).
//! - [`Status::Planned`] rows are shown greyed out with the milestone that makes them real, so
//!   the panel doubles as an EEVEE study guide and a visible roadmap.
//! - [`Status::NotApplicable`] rows exist in EEVEE but not in our plans; they only show under
//!   "Show advanced".
//!
//! Panel order and names follow Blender 5.2's EEVEE properties (finding B8). Tooltips are our
//! own words (clean room: Blender is GPL). At M6 this table becomes `#[derive(Reflect)]`
//! attributes; keep its users thin.

/// What a setting does in this engine today.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Live,
    /// `(milestone, what lands then)`.
    Planned(&'static str, &'static str),
    /// Why we don't plan it.
    NotApplicable(&'static str),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Unit {
    None,
    Meters,
    Degrees,
    Pixels,
    Percent,
    Stops,
    Kelvin,
    Megabytes,
}

impl Unit {
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Meters => " m",
            Unit::Degrees => "°",
            Unit::Pixels => " px",
            Unit::Percent => "%",
            Unit::Stops => " EV",
            Unit::Kelvin => " K",
            Unit::Megabytes => " MB",
        }
    }
}

/// Widget kind, range and Blender's default.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Kind {
    Bool(bool),
    Float {
        min: f32,
        max: f32,
        default: f32,
        unit: Unit,
    },
    Int {
        min: i32,
        max: i32,
        default: i32,
    },
    /// Options and the default index.
    Enum(&'static [&'static str], usize),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Setting {
    /// Stable id (`"eevee.sampling.viewport.samples"`), used by search and tests.
    pub id: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub status: Status,
    /// The cvar a `Live` row reads and writes.
    pub cvar: Option<&'static str>,
    /// What the setting does in EEVEE (our words).
    pub help: &'static str,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Panel {
    pub id: &'static str,
    pub label: &'static str,
    /// Parent panel id for sub-panels (Blender's `bl_parent_id`).
    pub parent: Option<&'static str>,
    /// A setting drawn as a checkbox in the panel header (Blender's `use_*` toggles).
    pub header_toggle: Option<Setting>,
    /// Only shown with "Show advanced" (keeps the first view calm).
    pub advanced: bool,
    pub help: &'static str,
    pub settings: &'static [Setting],
}

const fn live(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    cvar: &'static str,
    help: &'static str,
) -> Setting {
    Setting {
        id,
        label,
        kind,
        status: Status::Live,
        cvar: Some(cvar),
        help,
    }
}

const fn planned(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    milestone: &'static str,
    lands: &'static str,
    help: &'static str,
) -> Setting {
    Setting {
        id,
        label,
        kind,
        status: Status::Planned(milestone, lands),
        cvar: None,
        help,
    }
}

const fn na(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    why: &'static str,
    help: &'static str,
) -> Setting {
    Setting {
        id,
        label,
        kind,
        status: Status::NotApplicable(why),
        cvar: None,
        help,
    }
}

const fn f(min: f32, max: f32, default: f32, unit: Unit) -> Kind {
    Kind::Float {
        min,
        max,
        default,
        unit,
    }
}

const fn i(min: i32, max: i32, default: i32) -> Kind {
    Kind::Int { min, max, default }
}

const fn panel(
    id: &'static str,
    label: &'static str,
    parent: Option<&'static str>,
    help: &'static str,
    settings: &'static [Setting],
) -> Panel {
    Panel {
        id,
        label,
        parent,
        header_toggle: None,
        advanced: false,
        help,
        settings,
    }
}

const fn toggled(mut p: Panel, toggle: Setting) -> Panel {
    p.header_toggle = Some(toggle);
    p
}

const fn advanced(mut p: Panel) -> Panel {
    p.advanced = true;
    p
}

const RES: &[&str] = &["1:1", "1:2", "1:4", "1:8", "1:16"];
const TAA: (&str, &str) = ("M12", "temporal anti-aliasing (TAA)");
const SHADOWS: (&str, &str) = ("M4", "cascaded + atlas shadow maps");
const RAYTRACE: (&str, &str) = ("M12", "screen-space reflections and GI");
const POST: (&str, &str) = ("M12", "post-processing stack");
const VOLUMES: (&str, &str) = ("T3", "volumetrics (after M12)");

/// The Render tab: EEVEE 5.2's panels in Blender's order.
pub const RENDER_PANELS: &[Panel] = &[
    panel(
        "eevee.sampling",
        "Sampling",
        None,
        "EEVEE renders the same frame several times with tiny camera jitters and averages them. \
         More samples = smoother edges, shadows and noise, at the cost of time.",
        &[],
    ),
    panel(
        "eevee.sampling.viewport",
        "Viewport",
        Some("eevee.sampling"),
        "Sampling while you work in the viewport.",
        &[
            planned(
                "eevee.sampling.viewport.samples",
                "Samples",
                i(0, 4096, 16),
                TAA.0,
                TAA.1,
                "How many jittered frames the viewport accumulates when nothing moves. 0 keeps \
                 accumulating forever.",
            ),
            planned(
                "eevee.sampling.viewport.reprojection",
                "Temporal Reprojection",
                Kind::Bool(true),
                TAA.0,
                TAA.1,
                "Reuse samples from previous frames while the camera moves, so the viewport \
                 stays smooth instead of restarting accumulation every frame.",
            ),
            planned(
                "eevee.sampling.viewport.jittered_shadows",
                "Jittered Shadows",
                Kind::Bool(false),
                SHADOWS.0,
                SHADOWS.1,
                "Move each light's shadow origin a little every sample so soft shadows converge \
                 to their real penumbra (needs several samples).",
            ),
        ],
    ),
    panel(
        "eevee.sampling.render",
        "Render",
        Some("eevee.sampling"),
        "Sampling for final renders (F12).",
        &[planned(
            "eevee.sampling.render.samples",
            "Samples",
            i(1, 65536, 64),
            TAA.0,
            TAA.1,
            "Jittered frames averaged for a final render. Doubling this roughly halves the \
             visible noise and doubles render time.",
        )],
    ),
    advanced(panel(
        "eevee.sampling.advanced",
        "Advanced",
        Some("eevee.sampling"),
        "Fine control over how lights are sampled.",
        &[planned(
            "eevee.sampling.advanced.light_threshold",
            "Light Threshold",
            f(0.0, 1.0, 0.01, Unit::None),
            "M4",
            "light culling by influence radius",
            "Below this brightness a light's influence is treated as zero. It decides how far \
             each light reaches, which keeps lights cheap to cull.",
        )],
    )),
    toggled(
        panel(
            "eevee.sampling.shadows",
            "Shadows",
            Some("eevee.sampling"),
            "Global shadow quality. EEVEE uses virtual shadow maps traced with a few rays.",
            &[
                planned(
                    "eevee.shadows.rays",
                    "Rays",
                    i(1, 4, 1),
                    SHADOWS.0,
                    SHADOWS.1,
                    "Shadow rays per pixel per sample. More rays = softer, less noisy shadows \
                     from big lights.",
                ),
                planned(
                    "eevee.shadows.steps",
                    "Steps",
                    i(1, 16, 6),
                    SHADOWS.0,
                    SHADOWS.1,
                    "How many steps each shadow ray takes through the shadow map. More steps \
                     reduce light leaking at contact points.",
                ),
                planned(
                    "eevee.shadows.volume",
                    "Volume Shadows",
                    Kind::Bool(false),
                    VOLUMES.0,
                    VOLUMES.1,
                    "Let volumes (fog, smoke) cast shadows onto themselves and the scene.",
                ),
                planned(
                    "eevee.shadows.volume_steps",
                    "Volume Steps",
                    i(1, 128, 16),
                    VOLUMES.0,
                    VOLUMES.1,
                    "Samples taken through a volume when computing its shadow.",
                ),
                planned(
                    "eevee.shadows.resolution",
                    "Resolution",
                    f(0.0, 1.0, 1.0, Unit::Percent),
                    SHADOWS.0,
                    SHADOWS.1,
                    "Scales the resolution of every shadow map. Lower is faster and blurrier.",
                ),
            ],
        ),
        planned(
            "eevee.shadows.enabled",
            "Shadows",
            Kind::Bool(true),
            SHADOWS.0,
            SHADOWS.1,
            "Turn all shadows on or off.",
        ),
    ),
    panel(
        "eevee.light_paths",
        "Light Paths",
        None,
        "How strong direct and bounced (indirect) light are, and how they are clamped.",
        &[],
    ),
    advanced(panel(
        "eevee.clamping",
        "Clamping",
        Some("eevee.light_paths"),
        "Caps very bright samples ('fireflies'). 0 disables a clamp.",
        &[
            planned(
                "eevee.clamp.surface_direct",
                "Surface Direct Light",
                f(0.0, 1000.0, 0.0, Unit::None),
                "M4",
                "HDR pipeline",
                "Maximum brightness a single direct-light sample may contribute on surfaces.",
            ),
            planned(
                "eevee.clamp.surface_indirect",
                "Surface Indirect Light",
                f(0.0, 1000.0, 10.0, Unit::None),
                RAYTRACE.0,
                RAYTRACE.1,
                "Maximum brightness of bounced-light samples on surfaces. Lower removes \
                 sparkles from rough reflections, at the cost of energy.",
            ),
            planned(
                "eevee.clamp.volume_direct",
                "Volume Direct Light",
                f(0.0, 1000.0, 0.0, Unit::None),
                VOLUMES.0,
                VOLUMES.1,
                "Same as surface direct clamping, inside volumes.",
            ),
            planned(
                "eevee.clamp.volume_indirect",
                "Volume Indirect Light",
                f(0.0, 1000.0, 0.0, Unit::None),
                VOLUMES.0,
                VOLUMES.1,
                "Same as surface indirect clamping, inside volumes.",
            ),
        ],
    )),
    panel(
        "eevee.light_paths.intensity",
        "Intensity",
        Some("eevee.light_paths"),
        "Global multipliers for artistic control. 1 is physically based.",
        &[
            live(
                "eevee.intensity.direct",
                "Direct Light",
                f(0.0, 10.0, 1.0, Unit::None),
                "r.DirectLightIntensity",
                "Multiplies light that reaches a surface straight from a lamp. Try 0 to see only \
                 the world (ambient) light, or 2 to double every lamp at once.",
            ),
            planned(
                "eevee.intensity.indirect",
                "Indirect Light",
                f(0.0, 10.0, 1.0, Unit::None),
                RAYTRACE.0,
                RAYTRACE.1,
                "Multiplies light that bounced off other surfaces first (global illumination).",
            ),
        ],
    ),
    toggled(
        panel(
            "eevee.raytracing",
            "Raytracing",
            None,
            "Traces rays against the screen (and light probes as fallback) for reflections, \
             refraction and bounced light. Off = reflections come only from light probes.",
            &[
                planned(
                    "eevee.raytracing.method",
                    "Method",
                    Kind::Enum(&["Light Probe", "Screen-Trace"], 1),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Light Probe: reflections from baked probes only (fast, approximate). \
                     Screen-Trace: march rays through the depth buffer, falling back to probes \
                     for what's off-screen.",
                ),
                planned(
                    "eevee.raytracing.resolution",
                    "Resolution",
                    Kind::Enum(RES, 1),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Trace at a fraction of the screen resolution and upscale. 1:2 traces one \
                     ray per 2×2 pixels.",
                ),
                planned(
                    "eevee.raytracing.max_roughness",
                    "Max Roughness",
                    f(0.0, 1.0, 0.5, Unit::None),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Surfaces rougher than this skip ray tracing and use probes instead: rough \
                     reflections are blurry anyway, so rays are wasted on them.",
                ),
            ],
        ),
        planned(
            "eevee.raytracing.enabled",
            "Raytracing",
            Kind::Bool(false),
            RAYTRACE.0,
            RAYTRACE.1,
            "Enable ray-traced reflections, refraction and indirect light.",
        ),
    ),
    panel(
        "eevee.screen_trace",
        "Screen Tracing",
        Some("eevee.raytracing"),
        "Settings for rays marched through the depth buffer.",
        &[
            planned(
                "eevee.screen_trace.precision",
                "Precision",
                f(0.0, 1.0, 0.25, Unit::None),
                RAYTRACE.0,
                RAYTRACE.1,
                "Step quality of each screen ray. Higher finds thin objects more reliably but \
                 costs more.",
            ),
            planned(
                "eevee.screen_trace.thickness",
                "Thickness",
                f(0.0, 10.0, 0.2, Unit::Meters),
                RAYTRACE.0,
                RAYTRACE.1,
                "The depth buffer has no back faces, so every pixel is assumed to be this thick. \
                 Too small: rays slip behind objects. Too big: halos appear.",
            ),
            planned(
                "eevee.screen_trace.backface_hit",
                "Backface Hit",
                Kind::Bool(false),
                RAYTRACE.0,
                RAYTRACE.1,
                "Let rays hit the back side of surfaces, with their radiance scaled (useful for \
                 thin geometry like leaves).",
            ),
        ],
    ),
    advanced(panel(
        "eevee.denoise",
        "Denoising",
        Some("eevee.raytracing"),
        "Cleans up noisy ray-traced results.",
        &[
            planned(
                "eevee.denoise.spatial",
                "Spatial Reuse",
                Kind::Bool(true),
                RAYTRACE.0,
                RAYTRACE.1,
                "Share ray hits between neighbouring pixels.",
            ),
            planned(
                "eevee.denoise.temporal",
                "Temporal Accumulation",
                Kind::Bool(true),
                RAYTRACE.0,
                RAYTRACE.1,
                "Blend with previous frames' results (stable, can ghost on fast motion).",
            ),
            planned(
                "eevee.denoise.bilateral",
                "Bilateral Filter",
                Kind::Bool(true),
                RAYTRACE.0,
                RAYTRACE.1,
                "Blur that stops at edges (depth/normal changes) to remove leftover noise.",
            ),
        ],
    )),
    toggled(
        panel(
            "eevee.fast_gi",
            "Fast GI Approximation",
            Some("eevee.raytracing"),
            "A cheap screen-space stand-in for bounced light and ambient occlusion on rough \
             surfaces.",
            &[
                planned(
                    "eevee.fast_gi.threshold",
                    "Threshold",
                    f(0.0, 1.0, 0.5, Unit::None),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Roughness above which surfaces use the fast approximation instead of full \
                     rays.",
                ),
                planned(
                    "eevee.fast_gi.method",
                    "Method",
                    Kind::Enum(&["Ambient Occlusion", "Global Illumination"], 1),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Ambient Occlusion only darkens creases. Global Illumination also picks up \
                     colored light bounced from nearby surfaces.",
                ),
                planned(
                    "eevee.fast_gi.resolution",
                    "Resolution",
                    Kind::Enum(RES, 1),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Resolution the approximation runs at.",
                ),
                planned(
                    "eevee.fast_gi.rays",
                    "Rays",
                    i(1, 16, 2),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Directions sampled per pixel.",
                ),
                planned(
                    "eevee.fast_gi.steps",
                    "Steps",
                    i(1, 64, 8),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Samples along each direction.",
                ),
                planned(
                    "eevee.fast_gi.precision",
                    "Precision",
                    f(0.0, 1.0, 0.25, Unit::None),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Quality of each step.",
                ),
                planned(
                    "eevee.fast_gi.distance",
                    "Distance",
                    f(0.0, 100.0, 0.0, Unit::Meters),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "How far to look for occluders. 0 = unlimited (screen-space only).",
                ),
                planned(
                    "eevee.fast_gi.thickness_near",
                    "Thickness Near",
                    f(0.0, 100.0, 0.25, Unit::Meters),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Assumed thickness of nearby occluders.",
                ),
                planned(
                    "eevee.fast_gi.thickness_far",
                    "Far",
                    f(0.0, 90.0, 45.0, Unit::Degrees),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Angular thickness for distant occluders.",
                ),
                planned(
                    "eevee.fast_gi.bias",
                    "Bias",
                    f(0.0, 0.1, 0.05, Unit::None),
                    RAYTRACE.0,
                    RAYTRACE.1,
                    "Pushes samples off the surface to avoid self-occlusion artifacts.",
                ),
            ],
        ),
        planned(
            "eevee.fast_gi.enabled",
            "Fast GI Approximation",
            Kind::Bool(false),
            RAYTRACE.0,
            RAYTRACE.1,
            "Enable the fast approximation for rough surfaces.",
        ),
    ),
    panel(
        "eevee.volumes",
        "Volumes",
        None,
        "Fog, smoke and other participating media, evaluated in a camera-aligned 3D grid.",
        &[
            planned(
                "eevee.volumes.resolution",
                "Resolution",
                Kind::Enum(RES, 3),
                VOLUMES.0,
                VOLUMES.1,
                "Size of the volume grid's cells relative to the screen. Smaller cells = \
                 sharper volumes and more memory.",
            ),
            planned(
                "eevee.volumes.steps",
                "Steps",
                i(1, 256, 64),
                VOLUMES.0,
                VOLUMES.1,
                "Slices of the grid along the view direction.",
            ),
            planned(
                "eevee.volumes.distribution",
                "Distribution",
                f(0.0, 1.0, 0.8, Unit::None),
                VOLUMES.0,
                VOLUMES.1,
                "How much the slices crowd towards the camera (where detail matters most).",
            ),
            planned(
                "eevee.volumes.max_depth",
                "Max Depth",
                i(1, 16, 16),
                VOLUMES.0,
                VOLUMES.1,
                "How many overlapping volume objects a pixel can see.",
            ),
        ],
    ),
    toggled(
        advanced(panel(
            "eevee.volumes.range",
            "Custom Range",
            Some("eevee.volumes"),
            "Limit the volume grid to a depth range instead of the camera's clip range.",
            &[
                planned(
                    "eevee.volumes.start",
                    "Start",
                    f(0.0, 10000.0, 0.1, Unit::Meters),
                    VOLUMES.0,
                    VOLUMES.1,
                    "Where the grid begins.",
                ),
                planned(
                    "eevee.volumes.end",
                    "End",
                    f(0.0, 10000.0, 100.0, Unit::Meters),
                    VOLUMES.0,
                    VOLUMES.1,
                    "Where the grid ends.",
                ),
            ],
        )),
        planned(
            "eevee.volumes.custom_range",
            "Custom Range",
            Kind::Bool(false),
            VOLUMES.0,
            VOLUMES.1,
            "Use the custom start/end.",
        ),
    ),
    advanced(panel(
        "eevee.curves",
        "Curves",
        None,
        "How hair and curve objects are drawn.",
        &[
            na(
                "eevee.curves.shape",
                "Shape",
                Kind::Enum(&["Strand", "Strip"], 1),
                "the sample game has no hair or curves",
                "Strand: thin lines. Strip: camera-facing ribbons.",
            ),
            na(
                "eevee.curves.subdivision",
                "Additional Subdivision",
                i(0, 3, 0),
                "the sample game has no hair or curves",
                "Extra smoothing of each curve.",
            ),
        ],
    )),
    advanced(panel(
        "eevee.simplify",
        "Simplify",
        None,
        "Global quality caps for heavy scenes.",
        &[
            na(
                "eevee.simplify.subdivision",
                "Max Subdivision",
                i(0, 6, 6),
                "no subdivision modifier",
                "Upper limit on subdivision-surface levels.",
            ),
            na(
                "eevee.simplify.child_particles",
                "Max Child Particles",
                f(0.0, 1.0, 1.0, Unit::Percent),
                "no particle system",
                "Fraction of child particles shown.",
            ),
            na(
                "eevee.simplify.volume_resolution",
                "Volume Resolution",
                f(0.0, 1.0, 1.0, Unit::Percent),
                "no volume objects",
                "Scales volume object resolution.",
            ),
            planned(
                "eevee.simplify.normals",
                "Normals",
                Kind::Bool(false),
                "M12",
                "LODs",
                "Skip expensive normal computations (custom normals) in the viewport.",
            ),
        ],
    )),
    panel(
        "eevee.dof",
        "Depth of Field",
        None,
        "Blurs what is out of focus, like a real lens. Focus distance and f-stop live on the \
         camera (Camera tab).",
        &[
            planned(
                "eevee.dof.max_size",
                "Max Size",
                f(0.0, 2000.0, 100.0, Unit::Pixels),
                POST.0,
                POST.1,
                "Largest blur radius on screen; caps the cost of very out-of-focus areas.",
            ),
            planned(
                "eevee.dof.sprite_threshold",
                "Sprite Threshold",
                f(0.0, 10.0, 1.0, Unit::None),
                POST.0,
                POST.1,
                "Bright pixels above this become bokeh sprites (the visible lens-shaped \
                 highlights).",
            ),
            planned(
                "eevee.dof.neighbor_rejection",
                "Neighbor Rejection",
                f(0.0, 40.0, 10.0, Unit::None),
                POST.0,
                POST.1,
                "Stops a single very bright pixel from becoming a huge sprite.",
            ),
            planned(
                "eevee.dof.jitter",
                "Jitter Camera",
                Kind::Bool(false),
                POST.0,
                POST.1,
                "Move the camera across the lens per sample for physically correct blur \
                 (needs many samples).",
            ),
            planned(
                "eevee.dof.overblur",
                "Overblur",
                f(0.0, 100.0, 5.0, Unit::Percent),
                POST.0,
                POST.1,
                "Extra blur on top of jittering to hide the sample pattern.",
            ),
        ],
    ),
    toggled(
        panel(
            "eevee.motion_blur",
            "Motion Blur",
            None,
            "Smears moving objects along their motion during the shutter time.",
            &[
                planned(
                    "eevee.motion_blur.position",
                    "Position",
                    Kind::Enum(&["Start on Frame", "Center on Frame", "End on Frame"], 1),
                    POST.0,
                    POST.1,
                    "Whether the shutter opens at, around, or before the frame.",
                ),
                planned(
                    "eevee.motion_blur.shutter",
                    "Shutter",
                    f(0.0, 2.0, 0.5, Unit::None),
                    POST.0,
                    POST.1,
                    "Shutter open time as a fraction of a frame. 0.5 is the classic 180° film \
                     shutter.",
                ),
            ],
        ),
        planned(
            "eevee.motion_blur.enabled",
            "Motion Blur",
            Kind::Bool(false),
            POST.0,
            POST.1,
            "Enable motion blur.",
        ),
    ),
    panel(
        "eevee.film",
        "Film",
        None,
        "How samples are turned into pixels.",
        &[
            planned(
                "eevee.film.filter_size",
                "Filter Size",
                f(0.0, 100.0, 1.5, Unit::Pixels),
                TAA.0,
                TAA.1,
                "Width of the reconstruction filter. Larger is softer, smaller is sharper but \
                 can alias.",
            ),
            planned(
                "eevee.film.transparent",
                "Transparent",
                Kind::Bool(false),
                "M5",
                "render targets with alpha",
                "Render the world background as transparent (alpha 0) for compositing.",
            ),
            planned(
                "eevee.film.overscan",
                "Overscan",
                f(0.0, 50.0, 3.0, Unit::Percent),
                RAYTRACE.0,
                RAYTRACE.1,
                "Render a border outside the frame so screen-space effects have data at the \
                 edges.",
            ),
        ],
    ),
    advanced(panel(
        "eevee.performance",
        "Performance",
        None,
        "Quality/speed trade-offs.",
        &[
            planned(
                "eevee.performance.hq_normals",
                "High Quality Normals",
                Kind::Bool(false),
                "M3",
                "mesh cooking",
                "Store normals at higher precision (fixes banding on dense, smooth meshes).",
            ),
            planned(
                "eevee.performance.anisotropic",
                "Anisotropic Filtering",
                Kind::Enum(&["Off", "2×", "4×", "8×", "16×"], 1),
                "M3",
                "textures and samplers",
                "Keeps textures sharp on surfaces seen at a grazing angle (floors, roads).",
            ),
            planned(
                "eevee.performance.shadow_pool",
                "Shadow Pool",
                i(16, 4096, 512),
                SHADOWS.0,
                SHADOWS.1,
                "GPU memory (MB) reserved for shadow map pages. Too small and distant shadows \
                 lose resolution.",
            ),
            planned(
                "eevee.performance.probe_pool",
                "Light Probes Volume Pool",
                i(16, 4096, 16),
                RAYTRACE.0,
                RAYTRACE.1,
                "GPU memory (MB) for baked volume light probes.",
            ),
            planned(
                "eevee.performance.pixel_size",
                "Viewport Pixel Size",
                Kind::Enum(&["Automatic", "1×", "2×", "4×", "8×"], 0),
                "M5",
                "render scale",
                "Render the viewport at lower resolution to stay interactive.",
            ),
            na(
                "eevee.performance.compositor",
                "Compositor Device",
                Kind::Enum(&["CPU", "GPU"], 1),
                "we have no node compositor",
                "Where Blender's compositor runs.",
            ),
        ],
    )),
    panel(
        "color",
        "Color Management",
        None,
        "Turns the scene's physical light values into colors your display can show. This is \
         where most of a render's look comes from.",
        &[
            planned(
                "color.display",
                "Display Device",
                Kind::Enum(&["sRGB", "Display P3", "Rec.2100 PQ"], 0),
                "M12",
                "wide-gamut / HDR output",
                "The color space of your monitor. We always output sRGB today, which every \
                 desktop monitor understands; wide-gamut and HDR displays come later.",
            ),
            live(
                "color.view",
                "View Transform",
                Kind::Enum(&["Standard", "AgX", "Khronos PBR Neutral"], 0),
                "r.ViewTransform",
                "How scene brightness (which has no upper limit) is squeezed into 0–1. Standard \
                 clips anything above 1 to flat white. AgX rolls highlights off smoothly and \
                 desaturates them, like film (Blender's default). PBR Neutral keeps material \
                 colors truest. (Our AgX is an AgX-style curve; the exact LUT arrives at M4.)",
            ),
            planned(
                "color.look",
                "Look",
                Kind::Enum(
                    &["None", "Punchy", "Base Contrast", "Very High Contrast"],
                    0,
                ),
                "M4",
                "HDR post pass with LUTs",
                "An artistic contrast preset applied on top of the view transform.",
            ),
            live(
                "color.exposure",
                "Exposure",
                f(-32.0, 32.0, 0.0, Unit::Stops),
                "r.Exposure",
                "Brightens or darkens the whole image in photographic stops: +1 doubles the \
                 light, −1 halves it. Applied before the view transform, so highlights still \
                 roll off nicely.",
            ),
            live(
                "color.gamma",
                "Gamma",
                f(0.0, 5.0, 1.0, Unit::None),
                "r.Gamma",
                "Extra gamma on the final display values. Above 1 lifts the mid-tones, below 1 \
                 deepens them; black and white stay put.",
            ),
        ],
    ),
    toggled(
        panel(
            "color.white_balance",
            "White Balance",
            Some("color"),
            "Pick which light color should look white, like a camera's white-balance setting.",
            &[
                live(
                    "color.white_balance.temperature",
                    "Temperature",
                    f(1800.0, 100000.0, 6500.0, Unit::Kelvin),
                    "r.WhiteBalanceTemperature",
                    "The color temperature that should appear neutral. Set it to your main \
                     light's temperature (e.g. 3200 K for tungsten) to remove its orange cast.",
                ),
                live(
                    "color.white_balance.tint",
                    "Tint",
                    f(-500.0, 500.0, 10.0, Unit::None),
                    "r.WhiteBalanceTint",
                    "Shifts the neutral point between green and magenta (10 is neutral).",
                ),
            ],
        ),
        live(
            "color.white_balance.enabled",
            "White Balance",
            Kind::Bool(false),
            "r.WhiteBalance",
            "Apply white balance.",
        ),
    ),
    advanced(panel(
        "color.curves",
        "Curves",
        Some("color"),
        "A tone curve applied after the view transform.",
        &[planned(
            "color.curves.enabled",
            "Use Curves",
            Kind::Bool(false),
            "M4",
            "HDR post pass",
            "Hand-draw a brightness curve (like Photoshop's Curves).",
        )],
    )),
    advanced(panel(
        "color.working_space",
        "Working Space",
        Some("color"),
        "The color space lighting math happens in.",
        &[planned(
            "color.working_space.space",
            "Working Space",
            Kind::Enum(&["Linear Rec.709", "Linear Rec.2020", "ACEScg"], 0),
            "M4",
            "HDR post pass",
            "We light in linear Rec.709 (sRGB primaries), the same as Blender's default.",
        )],
    )),
];

/// World tab rows that aren't live yet (Color and Strength are hand-built, live).
pub const WORLD_PANELS: &[Panel] = &[
    panel(
        "world.volume",
        "Volume",
        None,
        "Fog filling the whole world.",
        &[planned(
            "world.volume.density",
            "Density",
            f(0.0, 1.0, 0.0, Unit::None),
            VOLUMES.0,
            VOLUMES.1,
            "How thick the world fog is.",
        )],
    ),
    panel(
        "world.light_probe",
        "Light Probe",
        None,
        "The world is captured into a probe for reflections and image-based lighting.",
        &[planned(
            "world.probe.resolution",
            "Resolution",
            Kind::Enum(&["128", "256", "512", "1024", "2048"], 3),
            "M4",
            "image-based lighting (IBL)",
            "Resolution of the captured world probe. Higher = sharper reflections of the sky.",
        )],
    ),
    panel(
        "world.sun",
        "Sun",
        None,
        "EEVEE can extract the sun from a sky texture and light the scene with it.",
        &[
            planned(
                "world.sun.threshold",
                "Threshold",
                f(0.0, 100.0, 10.0, Unit::None),
                "M4",
                "image-based lighting (IBL)",
                "Sky pixels brighter than this are treated as the sun.",
            ),
            planned(
                "world.sun.angle",
                "Angle",
                f(0.0, 180.0, 0.526, Unit::Degrees),
                "M4",
                "image-based lighting (IBL)",
                "Angular size of the extracted sun (affects shadow softness).",
            ),
        ],
    ),
    advanced(panel(
        "world.mist",
        "Mist Pass",
        None,
        "A depth-based fog pass for compositing.",
        &[na(
            "world.mist.start",
            "Start",
            f(0.0, 10000.0, 5.0, Unit::Meters),
            "we have no render passes for compositing",
            "Distance where mist begins.",
        )],
    )),
];

/// Light tab rows that aren't live yet.
pub const LIGHT_PANELS: &[Panel] = &[toggled(
    panel(
        "light.shadow",
        "Shadow",
        None,
        "Per-light shadow settings.",
        &[
            planned(
                "light.shadow.jitter",
                "Jitter",
                Kind::Bool(false),
                SHADOWS.0,
                SHADOWS.1,
                "Randomize the shadow origin each sample for accurate soft shadows.",
            ),
            planned(
                "light.shadow.overblur",
                "Overblur",
                f(0.0, 1.0, 0.2, Unit::None),
                SHADOWS.0,
                SHADOWS.1,
                "Extra blur to hide jitter noise with few samples.",
            ),
            planned(
                "light.shadow.filter",
                "Filter",
                f(0.0, 10.0, 1.0, Unit::None),
                SHADOWS.0,
                SHADOWS.1,
                "Blur the shadow edge to hide shadow-map pixels.",
            ),
            planned(
                "light.shadow.resolution_limit",
                "Resolution Limit",
                f(0.0, 100.0, 0.001, Unit::Meters),
                SHADOWS.0,
                SHADOWS.1,
                "Smallest shadow-map texel size in world units (or relative to distance).",
            ),
        ],
    ),
    planned(
        "light.shadow.enabled",
        "Shadow",
        Kind::Bool(true),
        SHADOWS.0,
        SHADOWS.1,
        "Whether this light casts shadows.",
    ),
)];

/// Material tab rows that aren't live yet.
pub const MATERIAL_PANELS: &[Panel] = &[
    panel(
        "material.principled_more",
        "More Principled Inputs",
        None,
        "The rest of the Principled BSDF. Each one adds a physical layer to the surface.",
        &[
            planned(
                "material.alpha",
                "Alpha",
                f(0.0, 1.0, 1.0, Unit::None),
                "M4",
                "transparency (dithered + blended)",
                "Opacity. 0 is fully see-through.",
            ),
            planned(
                "material.ior",
                "IOR",
                f(1.0, 4.0, 1.5, Unit::None),
                "M12",
                "refraction",
                "Index of refraction: how much light bends entering the material (glass ≈ 1.5, \
                 water ≈ 1.33). Also sets how reflective a dielectric is head-on.",
            ),
            planned(
                "material.subsurface",
                "Subsurface Weight",
                f(0.0, 1.0, 0.0, Unit::None),
                "M12",
                "subsurface scattering",
                "Light entering the surface, scattering inside and leaving elsewhere (skin, wax, \
                 marble).",
            ),
            planned(
                "material.specular_ior_level",
                "Specular IOR Level",
                f(0.0, 1.0, 0.5, Unit::None),
                "M4",
                "full PBR",
                "Artistic tweak of reflection strength; 0.5 keeps the physical IOR.",
            ),
            planned(
                "material.transmission",
                "Transmission Weight",
                f(0.0, 1.0, 0.0, Unit::None),
                "M12",
                "refraction",
                "Light passing through the surface (glass, water).",
            ),
            planned(
                "material.coat",
                "Coat Weight",
                f(0.0, 1.0, 0.0, Unit::None),
                "M12",
                "layered BSDF",
                "A clear lacquer layer on top (car paint, varnished wood).",
            ),
            planned(
                "material.sheen",
                "Sheen Weight",
                f(0.0, 1.0, 0.0, Unit::None),
                "M12",
                "layered BSDF",
                "Soft rim highlight of fabrics like velvet.",
            ),
            planned(
                "material.thin_film",
                "Thin Film Thickness",
                f(0.0, 100000.0, 0.0, Unit::None),
                "M12",
                "layered BSDF",
                "Rainbow interference of soap bubbles and oil slicks (nanometres).",
            ),
        ],
    ),
    advanced(panel(
        "material.settings_more",
        "Settings (planned)",
        None,
        "Render settings of the material.",
        &[
            planned(
                "material.render_method",
                "Render Method",
                Kind::Enum(&["Dithered", "Blended"], 0),
                "M4",
                "transparency (dithered + blended)",
                "Dithered: transparency as a stochastic pattern, sorted correctly. Blended: \
                 classic alpha blending, needs sorting.",
            ),
            planned(
                "material.backface_culling_shadow",
                "Backface Culling (Shadow)",
                Kind::Bool(false),
                SHADOWS.0,
                SHADOWS.1,
                "Don't let back faces cast shadows.",
            ),
            planned(
                "material.transparent_shadows",
                "Transparent Shadows",
                Kind::Bool(false),
                SHADOWS.0,
                SHADOWS.1,
                "Shadows respect the material's alpha.",
            ),
            planned(
                "material.displacement",
                "Displacement",
                Kind::Enum(
                    &["Bump Only", "Displacement Only", "Displacement and Bump"],
                    0,
                ),
                "M12",
                "tessellation / displacement",
                "Whether the displacement output fakes detail with normals or moves vertices.",
            ),
            planned(
                "material.raytraced_transmission",
                "Raytraced Transmission",
                Kind::Bool(false),
                "M12",
                "refraction",
                "Trace refraction rays through the screen instead of using probes.",
            ),
            na(
                "material.pass_index",
                "Pass Index",
                i(0, 32767, 0),
                "we have no render passes for compositing",
                "An id for masking this material in the compositor.",
            ),
        ],
    )),
];

/// Every panel in every schema table.
pub fn all_panels() -> impl Iterator<Item = &'static Panel> {
    RENDER_PANELS
        .iter()
        .chain(WORLD_PANELS)
        .chain(LIGHT_PANELS)
        .chain(MATERIAL_PANELS)
}

/// Every setting, including header toggles.
pub fn all_settings() -> impl Iterator<Item = &'static Setting> {
    all_panels().flat_map(|p| p.header_toggle.iter().chain(p.settings))
}

pub fn find(id: &str) -> Option<&'static Setting> {
    all_settings().find(|s| s.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_rows_documented() {
        let mut ids = HashSet::new();
        for p in all_panels() {
            assert!(ids.insert(p.id), "duplicate panel id {}", p.id);
            assert!(!p.help.is_empty(), "panel {} has no help", p.id);
            if let Some(parent) = p.parent {
                assert!(
                    all_panels().any(|q| q.id == parent),
                    "{}: unknown parent {parent}",
                    p.id
                );
            }
        }
        let mut setting_ids = HashSet::new();
        for s in all_settings() {
            assert!(setting_ids.insert(s.id), "duplicate setting id {}", s.id);
            assert!(s.help.len() > 10, "{} needs a real tooltip", s.id);
        }
    }

    #[test]
    fn live_rows_have_registered_cvars_others_have_none() {
        // Make sure the declaring module is linked into the test binary.
        let _ = crate::lighting::R_EXPOSURE.get();
        for s in all_settings() {
            match s.status {
                Status::Live => {
                    let name = s
                        .cvar
                        .unwrap_or_else(|| panic!("{} live without cvar", s.id));
                    assert!(
                        ge_core::cvar::find(name).is_some(),
                        "{}: cvar {name} not registered",
                        s.id
                    );
                }
                _ => assert!(s.cvar.is_none(), "{} is not live but names a cvar", s.id),
            }
        }
    }

    #[test]
    fn defaults_are_in_range() {
        for s in all_settings() {
            match s.kind {
                Kind::Float {
                    min, max, default, ..
                } => assert!((min..=max).contains(&default), "{}", s.id),
                Kind::Int { min, max, default } => {
                    assert!((min..=max).contains(&default), "{}", s.id);
                }
                Kind::Enum(options, default) => assert!(default < options.len(), "{}", s.id),
                Kind::Bool(_) => {}
            }
        }
    }

    #[test]
    fn render_tab_covers_eevee_5_2_panels() {
        // Top-level panels in Blender 5.2's EEVEE Render tab order (finding B8).
        let top: Vec<_> = RENDER_PANELS
            .iter()
            .filter(|p| p.parent.is_none())
            .map(|p| p.label)
            .collect();
        assert_eq!(
            top,
            [
                "Sampling",
                "Light Paths",
                "Raytracing",
                "Volumes",
                "Curves",
                "Simplify",
                "Depth of Field",
                "Motion Blur",
                "Film",
                "Performance",
                "Color Management"
            ]
        );
    }
}
