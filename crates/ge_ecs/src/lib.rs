//! World / ECS facade (lore §6.5). Wraps `bevy_ecs` so that **only this crate** names it in a
//! `Cargo.toml`; engine and game crates import everything from [`prelude`].
//!
//! Engine-shaped types are ours ([`Transform`], [`GlobalTransform`], [`Time`], [`FixedTime`]).
//! The ECS vocabulary (`Query`, `Commands`, `Res`, derives …) is a curated re-export of bevy's,
//! so a bevy upgrade is contained here unless bevy changes that vocabulary itself (plan 0003,
//! decision 2).
//!
//! **Derives:** bevy's `#[derive(Component)]` / `#[derive(Resource)]` / `#[derive(ScheduleLabel)]`
//! expand to `bevy_ecs::…` paths when the crate isn't in the user's `Cargo.toml`. The prelude
//! carries a hidden `bevy_ecs` re-export to satisfy them, so any module that uses these derives
//! must `use ge_ecs::prelude::*;`.

#![forbid(unsafe_code)]

mod name;
mod time;
mod transform;

pub use name::Name;
pub use time::{FixedTime, MAX_DELTA, Time};
pub use transform::{GlobalTransform, Transform, propagate_transforms};

#[cfg(test)]
mod tests {
    use super::prelude::*;

    #[derive(Component, Clone, PartialEq, Debug)]
    struct Tag(u32);

    #[test]
    fn clone_tree_copies_children_and_keeps_originals() {
        let mut world = World::new();
        let root = world.spawn(Tag(1)).id();
        world.spawn((Tag(2), ChildOf(root)));
        let copy = super::clone_tree(&mut world, root);
        assert_ne!(copy, root);
        assert_eq!(world.get::<Tag>(copy), Some(&Tag(1)));
        let kids = world.get::<Children>(copy).expect("children cloned");
        assert_eq!(kids.len(), 1);
        let kid = kids[0];
        assert_eq!(world.get::<Tag>(kid), Some(&Tag(2)));
        assert_eq!(world.get::<ChildOf>(kid).map(|c| c.parent()), Some(copy));
        assert_eq!(world.get::<Children>(root).map(|c| c.len()), Some(1));
    }
}

/// Marks an entity as switched off: default queries skip it (bevy's entity disabling). The
/// editor uses it for undoable deletes and for the Play-In-Editor scene stash.
pub use bevy_ecs::entity_disabling::Disabled;

/// Spawns a deep copy of `root` and its descendants (`Children`), with every component that
/// implements `Clone`. Components that don't are skipped. Returns the copy's root.
pub fn clone_tree(
    world: &mut bevy_ecs::world::World,
    root: bevy_ecs::entity::Entity,
) -> bevy_ecs::entity::Entity {
    world
        .entity_mut(root)
        .clone_and_spawn_with_opt_out(|builder| {
            builder.linked_cloning(true);
        })
}

/// Scheduling types the engine needs to build its tick groups. Games use the prelude.
pub mod schedule {
    pub use bevy_ecs::schedule::{IntoScheduleConfigs, Schedule, ScheduleLabel, Schedules};
    pub use bevy_ecs::system::ScheduleSystem;
}

pub mod prelude {
    // Derive output refers to `bevy_ecs::…`; this makes that path resolve (see crate docs).
    #[doc(hidden)]
    pub use bevy_ecs;

    pub use bevy_ecs::prelude::{
        Added, Bundle, Changed, ChildOf, ChildSpawnerCommands, Children, Commands, Component,
        DetectChanges, DetectChangesMut, Entity, EntityCommands, Has, IntoScheduleConfigs, Local,
        Mut, Or, Query, Ref, Res, ResMut, Resource, Single, SystemSet, With, Without, World,
    };
    pub use bevy_ecs::schedule::ScheduleLabel;

    pub use crate::{Disabled, FixedTime, GlobalTransform, Name, Time, Transform};
    pub use glam::{Affine3A, EulerRot, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};
}
