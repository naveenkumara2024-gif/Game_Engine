//! The engine's built-in plugins. `DefaultPlugins` is what a normal game adds.

use ge_ecs::propagate_transforms;
use ge_render::{RenderSnapshot, extract_render_snapshot};

use crate::input::{ActionState, InputMap, update_actions};
use crate::{App, Plugin, Plugins, TickGroup};

/// Input actions: `InputMap` + `ActionState`, updated in `First`.
#[derive(Debug, Default)]
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn name(&self) -> &'static str {
        "ge.input"
    }

    fn build(&self, app: &mut App) {
        app.init_resource::<InputMap>()
            .init_resource::<ActionState>()
            .add_systems(TickGroup::First, update_actions);
    }
}

/// `Transform` → `GlobalTransform` propagation in `PostUpdate`.
#[derive(Debug, Default)]
pub struct TransformPlugin;

impl Plugin for TransformPlugin {
    fn name(&self) -> &'static str {
        "ge.transform"
    }

    fn build(&self, app: &mut App) {
        app.add_systems(TickGroup::PostUpdate, propagate_transforms);
    }
}

/// Extract of `MeshRenderer` / `Camera3d` into the `RenderSnapshot`.
#[derive(Debug, Default)]
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn name(&self) -> &'static str {
        "ge.render"
    }

    fn dependencies(&self) -> &'static [&'static str] {
        &["ge.transform"]
    }

    fn build(&self, app: &mut App) {
        app.init_resource::<RenderSnapshot>()
            .add_systems(TickGroup::Extract, extract_render_snapshot);
    }
}

/// Input, transforms and rendering.
#[derive(Debug, Default)]
pub struct DefaultPlugins;

impl Plugins for DefaultPlugins {
    fn add_to(self, app: &mut App) {
        app.add_plugins((InputPlugin, TransformPlugin, RenderPlugin));
    }
}
