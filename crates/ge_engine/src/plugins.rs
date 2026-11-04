//! The engine's built-in plugins. `DefaultPlugins` is what a normal game adds.

use ge_asset::AssetServer;
use ge_ecs::prelude::*;
use ge_ecs::propagate_transforms;
use ge_render::{RenderSnapshot, extract_render_snapshot, spawn_scenes};
use ge_tasks::Tasks;

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

/// Assets (plan 0005): the `AssetServer` over `assets/` with its DDC, updated at the start of
/// every frame (in Edit mode too), plus model instancing (`SceneRoot`).
#[derive(Debug, Default)]
pub struct AssetPlugin;

impl Plugin for AssetPlugin {
    fn name(&self) -> &'static str {
        "ge.asset"
    }

    fn build(&self, app: &mut App) {
        let mut server = AssetServer::new(
            AssetServer::default_root(),
            AssetServer::default_ddc(),
            Tasks::default(),
        );
        if cfg!(feature = "hot-reload") {
            server.watch();
        }
        app.insert_resource(server)
            .add_systems(TickGroup::First, (update_assets, spawn_scenes).chain());
    }
}

fn update_assets(mut server: ResMut<AssetServer>) {
    server.update();
}

/// Assets, input, transforms and rendering.
#[derive(Debug, Default)]
pub struct DefaultPlugins;

impl Plugins for DefaultPlugins {
    fn add_to(self, app: &mut App) {
        app.add_plugins((AssetPlugin, InputPlugin, TransformPlugin, RenderPlugin));
    }
}
