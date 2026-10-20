//! The vertical-slice game: a small **third-person arena** (CLAUDE.md §1, §11). Every engine
//! milestone is "done" when this game can use it. Today: walk a player around an arena with a
//! follow camera; later milestones add physics, animation, UI and audio here.

#![forbid(unsafe_code)]

pub mod arena;
pub mod camera;
pub mod player;

use ge_engine::prelude::*;

/// Input action names, shared by bindings and systems.
pub mod actions {
    pub const MOVE: &str = "Move";
    pub const ORBIT: &str = "Orbit";
    pub const QUIT: &str = "Quit";
}

#[derive(Debug, Default)]
pub struct SamplePlugin;

impl Plugin for SamplePlugin {
    fn name(&self) -> &'static str {
        "sample"
    }

    fn dependencies(&self) -> &'static [&'static str] {
        &["ge.input", "ge.transform", "ge.render"]
    }

    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<InputMap>()
            .bind_axis2d(actions::MOVE, Axis2dBinding::wasd())
            .bind_axis2d(actions::MOVE, Axis2dBinding::arrows())
            .bind_button(actions::ORBIT, Binding::Mouse(MouseButton::Right))
            .bind_button(actions::QUIT, Binding::key("Escape"));

        app.add_systems(
            TickGroup::Startup,
            (
                arena::spawn_arena,
                player::spawn_player,
                camera::spawn_camera,
            ),
        )
        .add_systems(TickGroup::PrePhysics, (camera::orbit_input, quit_on_action))
        .add_systems(TickGroup::FixedUpdate, player::move_player)
        .add_systems(
            TickGroup::PostPhysics,
            (player::interpolate_motion, camera::follow_player).chain(),
        )
        .add_systems(TickGroup::Animation, arena::spin);
    }
}

fn quit_on_action(actions: Res<ActionState>, mut exit: ResMut<ExitRequest>) {
    if actions.just_pressed(actions::QUIT) {
        exit.request("quit action");
    }
}
