//! Engine (lore §3.3, §6.3, §6.5): the [`App`], plugins, tick groups, input actions and the
//! frame loop. Games are plugins and never write their own `main` loop:
//!
//! ```ignore
//! App::new().add_plugins(DefaultPlugins).add_plugins(MyGamePlugin).run()
//! ```
//!
//! Game crates import [`prelude`] only. It includes the ECS vocabulary from `ge_ecs` (and the
//! hidden path the ECS derives need), so they never depend on `bevy_ecs` directly.

#![forbid(unsafe_code)]

mod app;
pub mod input;
mod pacing;
mod plugin;
mod plugins;
mod runner;
mod schedule;

pub use app::{App, ExitRequest, P_FIXED_HZ};
pub use ge_platform::{MouseButton, WindowDesc};
pub use pacing::T_MAX_FPS;
pub use plugin::{Plugin, PluginError, Plugins};
pub use plugins::{DefaultPlugins, InputPlugin, RenderPlugin, TransformPlugin};
pub use schedule::TickGroup;

ge_core::declare_cvar!(pub static R_CLEAR_COLOR: [f32; 3] =
    ("r.ClearColor", [0.10, 0.20, 0.40], "Background color behind the scene, linear RGB"));

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(transparent)]
    Platform(#[from] ge_platform::PlatformError),
    #[error(transparent)]
    Plugin(#[from] PluginError),
}

/// Everything a game crate needs.
pub mod prelude {
    pub use ge_ecs::prelude::*;

    pub use crate::input::{ActionState, Axis2dBinding, Binding, InputMap};
    pub use crate::{
        App, DefaultPlugins, ExitRequest, MouseButton, Plugin, Plugins, TickGroup, WindowDesc,
    };
    pub use ge_render::{Camera3d, MeshKind, MeshRenderer};
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_cvars_registered() {
        for name in ["r.ClearColor", "p.FixedHz", "t.MaxFps"] {
            assert!(ge_core::cvar::find(name).is_some(), "{name}");
        }
    }
}
