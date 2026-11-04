//! Play-In-Editor v0 (lore §6.10 primitive #3, plan 0004 decision 10).
//!
//! **Play** clones every scene tree (`clone_tree`) into a *stash* and disables the copies, so
//! no query sees them. The game then runs on the live scene. **Stop** despawns the live scene
//! and re-enables the stash: the world is back exactly as edited, including game components
//! like the sample's `Player`, as long as they implement `Clone` (anything that doesn't is
//! skipped by the cloner — the full reflection-based world snapshot comes at M6).
//!
//! Entity ids change across Play/Stop, so selection and undo history are cleared.

use ge_ecs::clone_tree;
use ge_ecs::prelude::*;
use ge_engine::{EXIT_WINDOW_CLOSED, ExitRequest, SimulationState};
use ge_render::CameraOverride;
use tracing::info;

use crate::commands::set_enabled_recursive;
use crate::state::{Editor, EditorOnly};

/// Marks the root of a stashed (disabled) copy of the scene.
#[derive(Component, Copy, Clone, Debug, Default)]
pub struct PieStash;

#[derive(Debug, Default)]
pub struct PieState {
    stash: Vec<Entity>,
}

/// Live scene roots: everything with a transform that isn't the editor's and has no parent.
fn scene_roots(world: &mut World) -> Vec<Entity> {
    let mut q = world.query_filtered::<Entity, (
        With<Transform>,
        Without<ChildOf>,
        Without<EditorOnly>,
        Without<PieStash>,
    )>();
    q.iter(world).collect()
}

pub fn state(world: &World) -> SimulationState {
    *world.resource::<SimulationState>()
}

/// Play from Editing (stash + run), or resume from Paused.
pub fn play(world: &mut World, ed: &mut Editor) {
    match state(world) {
        SimulationState::Playing => {}
        SimulationState::Paused => {
            world.insert_resource(SimulationState::Playing);
            ed.toast("Resumed");
        }
        SimulationState::Editing => {
            ed.history.clear(world);
            ed.selected = None;
            ed.pie.stash.clear();
            for root in scene_roots(world) {
                let copy = clone_tree(world, root);
                set_enabled_recursive(world, copy, false);
                world.entity_mut(copy).insert(PieStash);
                ed.pie.stash.push(copy);
            }
            info!(roots = ed.pie.stash.len(), "play: scene stashed");
            world.insert_resource(CameraOverride(None));
            world.insert_resource(SimulationState::Playing);
            ed.toast("Playing · Shift+Esc to stop · Alt+P to pause");
        }
    }
}

pub fn pause(world: &mut World, ed: &mut Editor) {
    if state(world) == SimulationState::Playing {
        world.insert_resource(SimulationState::Paused);
        ed.toast("Paused · inspect the game, Alt+P resumes");
    }
}

/// Stop: throw away the played scene and bring back the stashed one.
pub fn stop(world: &mut World, ed: &mut Editor) {
    if state(world) == SimulationState::Editing {
        return;
    }
    for root in scene_roots(world) {
        if let Ok(e) = world.get_entity_mut(root) {
            e.despawn();
        }
    }
    for copy in ed.pie.stash.drain(..) {
        if world.get_entity(copy).is_ok() {
            set_enabled_recursive(world, copy, true);
            world.entity_mut(copy).remove::<PieStash>();
        }
    }
    ed.history.clear(world);
    ed.selected = None;
    world.insert_resource(CameraOverride(Some(ed.camera)));
    world.insert_resource(SimulationState::Editing);
    info!("stop: scene restored");
    ed.toast("Stopped · scene restored");
}

/// While playing, a game's own quit request means "stop playing", not "close the editor".
pub fn intercept_game_exit(world: &mut World, ed: &mut Editor) {
    if state(world) == SimulationState::Editing {
        return;
    }
    let pending = world.resource::<ExitRequest>().reason().map(str::to_owned);
    if let Some(reason) = pending
        && reason != EXIT_WINDOW_CLOSED
    {
        world.resource_mut::<ExitRequest>().cancel();
        info!(reason, "game asked to quit; stopping play instead");
        stop(world, ed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component, Clone, Debug, PartialEq)]
    struct Health(i32);

    fn editor(world: &mut World) -> Editor {
        let camera = world.spawn((EditorOnly, Transform::default())).id();
        crate::new_editor(camera)
    }

    #[test]
    fn stop_restores_the_edited_scene() {
        let mut world = World::new();
        world.init_resource::<ExitRequest>();
        world.insert_resource(SimulationState::Editing);
        let mut ed = editor(&mut world);
        let player = world
            .spawn((
                Name::new("Player"),
                Transform::from_xyz(1.0, 0.0, 0.0),
                Health(10),
            ))
            .id();
        world.spawn((Transform::default(), ChildOf(player), Name::new("Hat")));

        play(&mut world, &mut ed);
        assert_eq!(state(&world), SimulationState::Playing);
        // The game moves, hurts and spawns things.
        world.get_mut::<Transform>(player).unwrap().translation.x = 9.0;
        world.get_mut::<Health>(player).unwrap().0 = 1;
        world.spawn((Transform::default(), Name::new("Bullet")));
        let mut names = world.query::<&Name>();
        assert_eq!(
            names.iter(&world).count(),
            3,
            "stash is invisible to queries"
        );

        stop(&mut world, &mut ed);
        assert_eq!(state(&world), SimulationState::Editing);
        let mut q = world.query::<(&Name, &Transform, Option<&Health>)>();
        let mut rows: Vec<_> = q
            .iter(&world)
            .map(|(n, t, h)| (n.as_str().to_owned(), t.translation.x, h.cloned()))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            rows,
            [
                ("Hat".to_owned(), 0.0, None),
                ("Player".to_owned(), 1.0, Some(Health(10))),
            ]
        );
        let mut stash = world.query_filtered::<Entity, With<PieStash>>();
        assert_eq!(stash.iter(&world).count(), 0);
    }

    #[test]
    fn game_quit_stops_play_but_window_close_still_exits() {
        let mut world = World::new();
        world.init_resource::<ExitRequest>();
        world.insert_resource(SimulationState::Editing);
        let mut ed = editor(&mut world);
        play(&mut world, &mut ed);
        world.resource_mut::<ExitRequest>().request("quit action");
        intercept_game_exit(&mut world, &mut ed);
        assert_eq!(world.resource::<ExitRequest>().reason(), None);
        assert_eq!(state(&world), SimulationState::Editing);

        play(&mut world, &mut ed);
        world
            .resource_mut::<ExitRequest>()
            .request(EXIT_WINDOW_CLOSED);
        intercept_game_exit(&mut world, &mut ed);
        assert_eq!(
            world.resource::<ExitRequest>().reason(),
            Some(EXIT_WINDOW_CLOSED)
        );
    }
}
