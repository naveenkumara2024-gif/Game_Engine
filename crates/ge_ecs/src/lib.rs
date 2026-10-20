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

mod time;
mod transform;

pub use time::{FixedTime, MAX_DELTA, Time};
pub use transform::{GlobalTransform, Transform, propagate_transforms};

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

    pub use crate::{FixedTime, GlobalTransform, Time, Transform};
    pub use glam::{Affine3A, Mat4, Quat, Vec2, Vec3, Vec4};
}
