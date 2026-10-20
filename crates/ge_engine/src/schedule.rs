//! Tick groups (UE's `ETickingGroup`, lore §3.3, §6.5). Each group is one ECS schedule; the
//! engine runs them in [`TickGroup::FRAME`] order every frame.

use ge_ecs::prelude::*;

#[derive(ScheduleLabel, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum TickGroup {
    /// Once, after the window, RHI and renderer exist. Spawn the initial world here.
    Startup,
    /// Engine bookkeeping that everything else reads: input actions.
    First,
    /// Variable-rate gameplay before the simulation step (camera input, AI decisions).
    PrePhysics,
    /// Fixed-rate simulation (`p.FixedHz`). Runs 0..N times per frame; integrate with
    /// `FixedTime::step_secs`, never `Time::delta`.
    FixedUpdate,
    /// Reacts to the simulation result (follow cameras, interpolation, triggers).
    PostPhysics,
    /// Animation evaluation (M8).
    Animation,
    /// Transform propagation and other "finalize the frame's state" work.
    PostUpdate,
    /// Copies render data into the `RenderSnapshot`: the only game → render sync point (law 2).
    Extract,
}

impl TickGroup {
    pub const ALL: [TickGroup; 8] = [
        TickGroup::Startup,
        TickGroup::First,
        TickGroup::PrePhysics,
        TickGroup::FixedUpdate,
        TickGroup::PostPhysics,
        TickGroup::Animation,
        TickGroup::PostUpdate,
        TickGroup::Extract,
    ];

    /// Per-frame order (CLAUDE.md §6). `FixedUpdate` runs in its slot as many times as owed.
    pub const FRAME: [TickGroup; 7] = [
        TickGroup::First,
        TickGroup::PrePhysics,
        TickGroup::FixedUpdate,
        TickGroup::PostPhysics,
        TickGroup::Animation,
        TickGroup::PostUpdate,
        TickGroup::Extract,
    ];
}
