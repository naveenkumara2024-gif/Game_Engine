# 0007 — Orb Run (the sample becomes a game)

**Status:** done 2026-11-05. The user asked for it directly ("create a simple game with our engine"), so this was written alongside the work rather than approved first. · **Milestone:** none. This is gameplay on the M4 engine; it doesn't advance M5.

## Goal

Turn `games/sample` from a walk-around demo into a small game that uses what the engine has today: input actions, the fixed step, the transform hierarchy, the M4 renderer (emission, bloom, point lights, shadows) and Play-In-Editor.

## Rules of the game

- You have **60 s**. Walk into the glowing **orbs** (3 on the field) to score. Each orb you collect reappears somewhere else.
- **Sentinels** (red cubes) chase you and steer apart from each other. Two start on the field; one more joins every 5 orbs, up to 6. Their speed rises from 2.5 to 5.2 m/s; you run at 6.
- A hit costs a life (♥ ×3), knocks the sentinel back to the far edge and makes you blink, invulnerable for 2 s.
- The run ends at 0 lives ("CAUGHT!") or when time runs out ("TIME!"). **R** or **Enter** restarts; the best score is kept.
- HUD: the window title (`WindowStatus`), until game UI arrives at M9.

## Changes

- `games/sample/src/game.rs` (new):
  - `Round` resource: the phase, score, best, lives, timer and a seeded xorshift RNG.
  - Components: `Orb`, `Sentinel`, `Burst`.
  - Systems: `track_play_state`, `restart_input`, `setup_round`, `play_round`, `animate`.
- `player.rs`:
  - a `PlayerPart` marker (for the blink);
  - movement freezes when the run is over;
  - `START` is now public.
- `lib.rs`: the Restart binding and the system registration. `arena.rs` is unchanged.
- `ge_engine`:
  - a `WindowStatus` resource: the runner shows it in the title as soon as it changes, ahead of the stats readout;
  - `RenderMethod` added to the prelude.
- `apps/game` defaults to Rendered shading (`-cvar r.ViewportShading=0` still overrides it) and is titled "Orb Run".
- PIE: entering Play from Edit starts a fresh run. Stop already despawns everything the run spawned.

## Decisions

- **Contacts:** distance checks on the ground plane. Physics is M7; swap these for Rapier sensors then.
- **Timing:** game logic runs in PostPhysics with the variable `Time` step (only the player's movement is fixed-step). That's fine for a toy; revisit if it needs to be deterministic or networked.
- **No new third-party crates.** The RNG is xorshift64*.

## Tests

- Unit tests (`game.rs`):
  - lives and invulnerability;
  - the timer and restart;
  - the difficulty ramp and its cap;
  - spawn points stay inside the pillar ring, and sentinels arrive from the far side.
- Headless (`tests/headless.rs`):
  - an idle player scores nothing and is caught within the round; the HUD says CAUGHT, and R restarts;
  - steering with WASD into an orb scores;
  - a 700 ms startup hitch doesn't score.
- Manual: `cargo run -p game`. The title reads e.g. `Orb Run — Orbs 2 · 52 s · ♥♥· · best 0 — CPU … | GPU 1.7 ms | 144 fps`.
