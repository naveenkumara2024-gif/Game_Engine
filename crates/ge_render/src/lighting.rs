//! Lighting v0 (plan 0004, decision 2): punctual lights, a Principled-BSDF subset, a flat world
//! ambient and Blender-style color management. No shadows, IBL or clustering yet — M4 builds
//! those on top of these same components.
//!
//! **Units follow Blender's EEVEE/Cycles conventions** so values from a Blender scene look the
//! same here (radiometric, no photometric scale):
//! - Point / Spot: `power` in watts, radiant intensity `I = P / 4π` W/sr. A spot doesn't
//!   concentrate its power; the cone only masks it (as in Blender).
//! - Sun: `power` is the strength in W/m² (irradiance at normal incidence).
//! - Area (v0 approximation): a one-sided Lambertian emitter, `I(θ) = P/π · cos θ`. Size and
//!   shape arrive with M4.
//!
//! Lambert diffuse is `albedo / π`, so a 1000 W point light 4 m from a white wall gives a
//! pixel value of ≈ 1000 / (4π² · 16) ≈ 1.58 — the same as Cycles.

use std::f32::consts::PI;

use bytemuck::{Pod, Zeroable};
use ge_ecs::prelude::*;

// ---------------------------------------------------------------------------------------------
// Color management cvars (Render ▸ Color Management). The UI writes these; so can the console.
// ---------------------------------------------------------------------------------------------

ge_core::declare_cvar!(pub static R_EXPOSURE: f32 = (
    "r.Exposure",
    0.0,
    "Scene exposure in stops (EV) applied before the view transform; +1 doubles brightness"
));
ge_core::declare_cvar!(pub static R_GAMMA: f32 = (
    "r.Gamma",
    1.0,
    "Extra display-space gamma after the view transform (1 = none)"
));
ge_core::declare_cvar!(pub static R_VIEW_TRANSFORM: i32 = (
    "r.ViewTransform",
    0,
    "View transform: 0 = Standard (sRGB, clips), 1 = AgX-style filmic, 2 = Khronos PBR Neutral"
));
ge_core::declare_cvar!(pub static R_WHITE_BALANCE: bool = (
    "r.WhiteBalance",
    false,
    "Apply white balance (r.WhiteBalanceTemperature / r.WhiteBalanceTint)"
));
ge_core::declare_cvar!(pub static R_WHITE_BALANCE_TEMPERATURE: f32 = (
    "r.WhiteBalanceTemperature",
    6500.0,
    "White balance temperature in Kelvin; the color that should appear white"
));
ge_core::declare_cvar!(pub static R_WHITE_BALANCE_TINT: f32 = (
    "r.WhiteBalanceTint",
    10.0,
    "White balance tint (green ↔ magenta); 10 is neutral like Blender"
));
ge_core::declare_cvar!(pub static R_DIRECT_LIGHT_INTENSITY: f32 = (
    "r.DirectLightIntensity",
    1.0,
    "Multiplier on all direct light (Render ▸ Light Paths ▸ Intensity ▸ Direct Light)"
));
ge_core::declare_cvar!(pub static R_VIEWPORT_SHADING: i32 = (
    "r.ViewportShading",
    0,
    "0 = Solid (fixed studio light), 1 = Rendered (scene lights + world). Normals: r.DebugNormals"
));

/// Lights the forward pass evaluates per pixel. M4 clustering lifts the cap.
pub const MAX_LIGHTS: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ViewTransform {
    #[default]
    Standard,
    AgX,
    PbrNeutral,
}

impl ViewTransform {
    pub const ALL: [ViewTransform; 3] = [Self::Standard, Self::AgX, Self::PbrNeutral];

    pub fn from_cvar(v: i32) -> Self {
        match v {
            1 => Self::AgX,
            2 => Self::PbrNeutral,
            _ => Self::Standard,
        }
    }

    pub fn as_cvar(self) -> i32 {
        match self {
            Self::Standard => 0,
            Self::AgX => 1,
            Self::PbrNeutral => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::AgX => "AgX",
            Self::PbrNeutral => "Khronos PBR Neutral",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ViewportShading {
    /// Fixed studio light, ignores scene lights (Blender's Solid mode).
    #[default]
    Solid,
    /// Scene lights + world (Blender's Material Preview / Rendered).
    Rendered,
}

/// Snapshot of the color-management cvars for one frame.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ColorManagement {
    pub exposure_ev: f32,
    pub gamma: f32,
    pub view: ViewTransform,
    /// Per-channel multiplier; `[1; 3]` when white balance is off.
    pub white_balance: [f32; 3],
}

impl Default for ColorManagement {
    fn default() -> Self {
        Self {
            exposure_ev: 0.0,
            gamma: 1.0,
            view: ViewTransform::Standard,
            white_balance: [1.0; 3],
        }
    }
}

impl ColorManagement {
    pub fn from_cvars() -> Self {
        Self {
            exposure_ev: R_EXPOSURE.get(),
            gamma: R_GAMMA.get().max(0.01),
            view: ViewTransform::from_cvar(R_VIEW_TRANSFORM.get()),
            white_balance: if R_WHITE_BALANCE.get() {
                white_balance_scale(
                    R_WHITE_BALANCE_TEMPERATURE.get(),
                    R_WHITE_BALANCE_TINT.get(),
                )
            } else {
                [1.0; 3]
            },
        }
    }

    /// Scene-linear → display-linear (what the sRGB target then encodes). Mirrors
    /// `color_manage` in `shaders/common/color.wesl`; used for the clear color and in tests.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let k = self.exposure_ev.exp2();
        let c = Vec3::from(rgb) * Vec3::from(self.white_balance) * k;
        let c = match self.view {
            ViewTransform::Standard => c.clamp(Vec3::ZERO, Vec3::ONE),
            ViewTransform::AgX => agx_style(c),
            ViewTransform::PbrNeutral => pbr_neutral(c),
        };
        if self.gamma == 1.0 {
            return c.to_array();
        }
        let g = |x: f32| srgb_to_linear(linear_to_srgb(x).powf(1.0 / self.gamma));
        [g(c.x), g(c.y), g(c.z)]
    }
}

pub fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.003_130_8 {
        x * 12.92
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

/// Our own AgX-*style* curve (clean-room, not Blender's LUT): a log2 encoding over the same
/// ≈16.5-stop range, a smooth S-curve, and a slight desaturation of bright colors so highlights
/// roll off to white instead of clipping to pure hues. The exact AgX LUT arrives with the HDR
/// post pass (M4); the tooltip says so.
pub fn agx_style(c: Vec3) -> Vec3 {
    const MIN_EV: f32 = -12.47;
    const MAX_EV: f32 = 4.03;
    // Mix a little of the luminance into each channel before the curve (the "inset" idea):
    // bright saturated colors then converge to white.
    let luma = c.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    let inset = c.lerp(Vec3::splat(luma), 0.12).max(Vec3::splat(1e-10));
    let encode = |v: f32| ((v.max(1e-10).log2() - MIN_EV) / (MAX_EV - MIN_EV)).clamp(0.0, 1.0);
    let e = Vec3::new(encode(inset.x), encode(inset.y), encode(inset.z));
    // Logistic-like S-curve through (0,0) and (1,1), pivot at middle grey (0.18 → ~0.606).
    let curve = |x: f32| {
        let p = 0.606_f32;
        let k = 3.2_f32;
        let s = |t: f32| 1.0 / (1.0 + (-k * (t - p) * 4.0).exp());
        (s(x) - s(0.0)) / (s(1.0) - s(0.0))
    };
    let display = Vec3::new(curve(e.x), curve(e.y), curve(e.z));
    // The curve outputs display-encoded values; return display-linear for the sRGB target.
    Vec3::new(
        srgb_to_linear(display.x),
        srgb_to_linear(display.y),
        srgb_to_linear(display.z),
    )
}

/// Khronos PBR Neutral tone mapper, implemented from the published specification
/// (github.com/KhronosGroup/ToneMapping, Apache-2.0).
pub fn pbr_neutral(mut c: Vec3) -> Vec3 {
    const START_COMPRESSION: f32 = 0.8 - 0.04;
    const DESATURATION: f32 = 0.15;
    let x = c.min_element();
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    c -= Vec3::splat(offset);
    let peak = c.max_element();
    if peak < START_COMPRESSION {
        return c.max(Vec3::ZERO);
    }
    let d = 1.0 - START_COMPRESSION;
    let new_peak = 1.0 - d * d / (peak + d - START_COMPRESSION);
    c *= new_peak / peak;
    let g = 1.0 - 1.0 / (DESATURATION * (peak - new_peak) + 1.0);
    c.lerp(Vec3::splat(new_peak), g).max(Vec3::ZERO)
}

/// Approximate color of a black body at `kelvin`, normalized so its brightest channel is 1.
/// A smooth fit of the Planckian locus in linear sRGB (1000 K – 40000 K), our own.
pub fn blackbody(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1000.0, 40000.0) / 100.0;
    // Fit in display space, then linearize.
    let r = if t <= 66.0 {
        1.0
    } else {
        (1.292_936 * (t - 60.0).powf(-0.133_204_76)).clamp(0.0, 1.0)
    };
    let g = if t <= 66.0 {
        (0.390_081_58 * t.ln() - 0.631_841_4).clamp(0.0, 1.0)
    } else {
        (1.129_890_9 * (t - 60.0).powf(-0.075_514_85)).clamp(0.0, 1.0)
    };
    let b = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        (0.543_206_8 * (t - 10.0).ln() - 1.196_254_1).clamp(0.0, 1.0)
    };
    let lin = Vec3::new(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    (lin / lin.max_element().max(1e-6)).to_array()
}

/// White balance as a per-channel scale: divide out the chosen white (temperature + tint) and
/// renormalize to D65-ish 6500 K / tint 10, so Blender's defaults are the identity.
pub fn white_balance_scale(kelvin: f32, tint: f32) -> [f32; 3] {
    let white = |k: f32, tint: f32| {
        let c = Vec3::from(blackbody(k));
        // Tint moves the white along green ↔ magenta (10 = neutral, higher = more magenta
        // cast removed, so the image turns greener).
        let g = 1.0 + (10.0 - tint) * 0.005;
        c * Vec3::new(1.0, g, 1.0)
    };
    let target = white(6500.0, 10.0);
    let chosen = white(kelvin, tint);
    let s = target / chosen.max(Vec3::splat(1e-4));
    // Keep overall luminance: white balance changes color, not exposure.
    let luma = |v: Vec3| v.dot(Vec3::new(0.2126, 0.7152, 0.0722));
    (s / luma(s).max(1e-4)).to_array()
}

// ---------------------------------------------------------------------------------------------
// Scene components
// ---------------------------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum LightKind {
    #[default]
    Point,
    Sun,
    Spot,
    Area,
}

impl LightKind {
    pub const ALL: [LightKind; 4] = [Self::Point, Self::Sun, Self::Spot, Self::Area];

    pub fn label(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::Sun => "Sun",
            Self::Spot => "Spot",
            Self::Area => "Area",
        }
    }

    /// Blender's default power for a newly added light of this kind.
    pub fn default_power(self) -> f32 {
        match self {
            Self::Point | Self::Spot => 1000.0,
            Self::Sun => 1.0,
            Self::Area => 1000.0,
        }
    }
}

/// A light at the entity's `GlobalTransform`, shining down its local −Z (like Blender's).
/// Fields mirror Blender's Light properties; the ones marked *(M4)* are stored and shown in the
/// editor but not yet used by the renderer.
#[derive(Component, Copy, Clone, Debug, PartialEq)]
#[require(Transform)]
pub struct Light {
    pub kind: LightKind,
    /// Linear RGB. With `use_temperature` it tints the black-body color.
    pub color: Vec3,
    pub use_temperature: bool,
    pub temperature: f32,
    /// Watts (point, spot, area) or W/m² (sun "Strength").
    pub power: f32,
    /// Extra stops: the power is multiplied by 2^exposure.
    pub exposure: f32,
    /// Area lights: keep emitted power independent of size *(M4)*.
    pub normalize: bool,
    /// Soft-shadow radius for point/spot *(M4)*.
    pub radius: f32,
    pub soft_falloff: bool,
    /// Sun angular diameter in radians *(M4)*.
    pub angle: f32,
    /// Area light width × height in metres *(M4)*.
    pub size: Vec2,
    pub use_custom_distance: bool,
    /// Light influence ends here (smooth window) when `use_custom_distance` is set.
    pub custom_distance: f32,
    /// Spot cone full angle in radians.
    pub spot_size: f32,
    /// 0 = hard cone edge, 1 = softest.
    pub spot_blend: f32,
    pub show_cone: bool,
    pub diffuse_factor: f32,
    pub specular_factor: f32,
    pub transmission_factor: f32,
    pub volume_factor: f32,
    pub cast_shadow: bool,
}

impl Light {
    pub fn new(kind: LightKind) -> Self {
        Self {
            kind,
            power: kind.default_power(),
            ..Self::default()
        }
    }

    /// Final emitted color × intensity with temperature and exposure applied (see module docs
    /// for units).
    pub fn radiance(&self) -> Vec3 {
        let base = if self.use_temperature {
            self.color * Vec3::from(blackbody(self.temperature))
        } else {
            self.color
        };
        let p = self.power.max(0.0) * self.exposure.exp2();
        let intensity = match self.kind {
            LightKind::Point | LightKind::Spot => p / (4.0 * PI),
            LightKind::Sun => p,
            LightKind::Area => p / PI,
        };
        base * intensity
    }
}

impl Default for Light {
    fn default() -> Self {
        Self {
            kind: LightKind::Point,
            color: Vec3::ONE,
            use_temperature: false,
            temperature: 6500.0,
            power: 1000.0,
            exposure: 0.0,
            normalize: true,
            radius: 0.1,
            soft_falloff: true,
            angle: 0.526_f32.to_radians(),
            size: Vec2::splat(1.0),
            use_custom_distance: false,
            custom_distance: 40.0,
            spot_size: 45f32.to_radians(),
            spot_blend: 0.15,
            show_cone: false,
            diffuse_factor: 1.0,
            specular_factor: 1.0,
            transmission_factor: 1.0,
            volume_factor: 1.0,
            cast_shadow: true,
        }
    }
}

/// Principled BSDF subset (Material tab). Multiplies the mesh's vertex color and
/// `MeshRenderer::tint`. Entities without one use [`Material::default`].
#[derive(Component, Copy, Clone, Debug, PartialEq)]
pub struct Material {
    /// Linear RGB.
    pub base_color: Vec3,
    pub metallic: f32,
    pub roughness: f32,
    pub emission_color: Vec3,
    pub emission_strength: f32,
    /// Blender's default is off (both sides drawn and lit).
    pub backface_culling: bool,
}

impl Default for Material {
    fn default() -> Self {
        // Blender's Principled BSDF defaults.
        Self {
            base_color: Vec3::splat(0.8),
            metallic: 0.0,
            roughness: 0.5,
            emission_color: Vec3::ONE,
            emission_strength: 0.0,
            backface_culling: false,
        }
    }
}

/// The world (World tab): background color and a flat ambient term in Rendered shading.
#[derive(Resource, Copy, Clone, Debug, PartialEq)]
pub struct WorldSettings {
    /// Linear RGB.
    pub color: Vec3,
    pub strength: f32,
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self {
            color: Vec3::new(0.10, 0.20, 0.40),
            strength: 1.0,
        }
    }
}

impl WorldSettings {
    pub fn radiance(&self) -> Vec3 {
        self.color * self.strength.max(0.0)
    }
}

/// Extracted light: plain data in world space.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LightItem {
    pub kind: LightKind,
    pub position: Vec3,
    /// Direction the light travels (local −Z in world space), normalized.
    pub direction: Vec3,
    pub radiance: Vec3,
    /// 0 = unlimited.
    pub cutoff: f32,
    pub spot_cos: f32,
    pub spot_blend: f32,
    pub diffuse: f32,
    pub specular: f32,
}

impl LightItem {
    pub fn from_light(light: &Light, global: &GlobalTransform) -> Self {
        // Area lights are a 180° cone with cosine emission in v0 (see module docs).
        let (spot_cos, spot_blend) = match light.kind {
            LightKind::Spot => {
                let c = (light.spot_size.clamp(1e-3, PI) * 0.5).cos();
                (c, (1.0 - c) * light.spot_blend.clamp(0.0, 1.0))
            }
            _ => (-1.0, 0.0),
        };
        Self {
            kind: light.kind,
            position: global.translation(),
            direction: global.forward().normalize_or(Vec3::NEG_Y),
            radiance: light.radiance(),
            cutoff: if light.use_custom_distance {
                light.custom_distance.max(1e-3)
            } else {
                0.0
            },
            spot_cos,
            spot_blend,
            diffuse: light.diffuse_factor.max(0.0),
            specular: light.specular_factor.max(0.0),
        }
    }
}

/// Matches `Light` in `shaders/common/lighting.wesl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub(crate) struct LightUniform {
    position_cutoff: [f32; 4],
    direction_kind: [f32; 4],
    radiance_diffuse: [f32; 4],
    spot_specular: [f32; 4],
}

impl LightUniform {
    pub(crate) fn pack(item: &LightItem) -> Self {
        let kind = match item.kind {
            LightKind::Point => 0.0,
            LightKind::Sun => 1.0,
            LightKind::Spot => 2.0,
            LightKind::Area => 3.0,
        };
        Self {
            position_cutoff: item.position.extend(item.cutoff).to_array(),
            direction_kind: item.direction.extend(kind).to_array(),
            radiance_diffuse: item.radiance.extend(item.diffuse).to_array(),
            spot_specular: [item.spot_cos, item.spot_blend, item.specular, 0.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_is_identity_in_range_and_clips() {
        let cm = ColorManagement::default();
        for v in [0.0, 0.18, 0.5, 1.0] {
            assert_eq!(cm.apply([v; 3]), [v; 3]);
        }
        assert_eq!(cm.apply([4.0, -1.0, 0.5]), [1.0, 0.0, 0.5]);
    }

    #[test]
    fn exposure_doubles_per_stop() {
        let cm = ColorManagement {
            exposure_ev: 1.0,
            ..Default::default()
        };
        let [r, ..] = cm.apply([0.25; 3]);
        assert!((r - 0.5).abs() < 1e-6);
    }

    #[test]
    fn filmic_curves_are_monotonic_and_bounded() {
        for f in [agx_style, pbr_neutral] {
            let mut prev = -1.0;
            for i in 0..400 {
                let x = (i as f32 / 40.0 - 8.0).exp2(); // 2^-8 .. 2^2
                let y = f(Vec3::splat(x)).x;
                assert!(y >= prev - 1e-6, "not monotonic at {x}");
                assert!((0.0..=1.0 + 1e-5).contains(&y), "out of range at {x}: {y}");
                prev = y;
            }
        }
        // Middle grey lands near the middle of the display range.
        let grey = linear_to_srgb(agx_style(Vec3::splat(0.18)).x);
        assert!((0.35..0.7).contains(&grey), "{grey}");
    }

    #[test]
    fn gamma_one_is_skipped_and_gamma_brightens() {
        let mut cm = ColorManagement::default();
        let a = cm.apply([0.2; 3])[0];
        cm.gamma = 2.0;
        let b = cm.apply([0.2; 3])[0];
        assert_eq!(a, 0.2);
        assert!(b > a);
    }

    #[test]
    fn blackbody_is_warm_to_cool() {
        let warm = blackbody(2000.0);
        let cool = blackbody(12000.0);
        assert!(warm[0] > warm[2] && cool[2] > cool[0]);
        for c in [warm, cool, blackbody(6500.0)] {
            assert!((c.iter().copied().fold(0.0, f32::max) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn white_balance_defaults_are_identity() {
        let s = white_balance_scale(6500.0, 10.0);
        for c in s {
            assert!((c - 1.0).abs() < 1e-4, "{s:?}");
        }
        // Choosing a warm white makes the image cooler (less red).
        let w = white_balance_scale(3000.0, 10.0);
        assert!(w[0] < w[2]);
    }

    #[test]
    fn point_light_matches_cycles_units() {
        let l = Light::new(LightKind::Point);
        // 1000 W point → I = 1000/4π W/sr; white wall at 4 m, normal incidence:
        // L = albedo/π · I / d² ≈ 1.58.
        let pixel = l.radiance().x / PI / 16.0;
        assert!((pixel - 1000.0 / (4.0 * PI * PI * 16.0)).abs() < 1e-5);
        assert!((pixel - 1.583).abs() < 1e-3);
    }

    #[test]
    fn light_uniform_is_64_bytes() {
        assert_eq!(size_of::<LightUniform>(), 64);
    }
}
