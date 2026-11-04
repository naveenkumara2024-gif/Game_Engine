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
    /// Immediate-mode UI (the editor, debug panels). After gameplay so it shows this frame's
    /// state, before `PostUpdate` so its edits propagate the same frame.
    Ui,
    /// Transform propagation and other "finalize the frame's state" work.
    PostUpdate,
    /// Copies render data into the `RenderSnapshot`: the only game → render sync point (law 2).
    Extract,
}

impl TickGroup {
    pub const ALL: [TickGroup; 9] = [
        TickGroup::Startup,
        TickGroup::First,
        TickGroup::PrePhysics,
        TickGroup::FixedUpdate,
        TickGroup::PostPhysics,
        TickGroup::Animation,
        TickGroup::Ui,
        TickGroup::PostUpdate,
        TickGroup::Extract,
    ];

    /// Per-frame order (CLAUDE.md §6). `FixedUpdate` runs in its slot as many times as owed.
    pub const FRAME: [TickGroup; 8] = [
        TickGroup::First,
        TickGroup::PrePhysics,
        TickGroup::FixedUpdate,
        TickGroup::PostPhysics,
        TickGroup::Animation,
        TickGroup::Ui,
        TickGroup::PostUpdate,
        TickGroup::Extract,
    ];

    /// Groups that advance the game. They are skipped while [`SimulationState`] is not
    /// `Playing` (the editor's Edit and Pause modes).
    pub fn is_gameplay(self) -> bool {
        matches!(
            self,
            TickGroup::PrePhysics
                | TickGroup::FixedUpdate
                | TickGroup::PostPhysics
                | TickGroup::Animation
        )
    }
}

/// Whether gameplay runs this frame (lore §6.10 Play-In-Editor). A standalone game is always
/// `Playing`; the editor starts in `Editing` and toggles it with Play / Pause / Stop.
#[derive(Resource, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SimulationState {
    /// Authoring: gameplay tick groups don't run.
    Editing,
    #[default]
    Playing,
    /// Play mode frozen: gameplay groups don't run, the game's state is kept.
    Paused,
}

impl SimulationState {
    pub fn runs_gameplay(self) -> bool {
        self == SimulationState::Playing
    }
}
