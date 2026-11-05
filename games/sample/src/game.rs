//! **Orb Run**, the sample's game loop: collect glowing orbs before the timer runs out while
//! sentinels hunt you down. Every few orbs another sentinel joins and they all get faster.
//! Three hits and the run is over; `R` starts a new one.
//!
//! No physics (M7) or game UI (M9) yet: contacts are distance checks on the ground plane and
//! the HUD is the window title ([`WindowStatus`]).

use ge_engine::prelude::*;

use crate::actions;
use crate::arena::{ARENA_HALF_SIZE, Spin};
use crate::player::{FixedMotion, Player, PlayerPart, START};

pub const ROUND_SECS: f32 = 60.0;
pub const LIVES: u32 = 3;
/// Orbs on the field at once.
const ORBS: usize = 3;
/// Orbs spawn inside the pillar ring.
const SPAWN_RADIUS: f32 = ARENA_HALF_SIZE - 4.0;
const COLLECT_RADIUS: f32 = 1.2;
const HIT_RADIUS: f32 = 1.1;
/// Seconds of invulnerability (the player blinks) after a hit.
const INVULNERABLE_SECS: f32 = 2.0;
const START_SENTINELS: u32 = 2;
const MAX_SENTINELS: u32 = 6;
/// One more sentinel every this many orbs.
const ORBS_PER_SENTINEL: u32 = 5;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Playing,
    /// The run ended: `true` = the timer ran out (survived), `false` = out of lives.
    Over {
        survived: bool,
    },
}

/// The state of the current run.
#[derive(Resource, Clone, Debug)]
pub struct Round {
    pub phase: Phase,
    pub score: u32,
    pub best: u32,
    pub lives: u32,
    pub time_left: f32,
    /// Seconds of hit invulnerability left.
    pub invulnerable: f32,
    /// Spawn the orbs and sentinels (and clear old ones) on the next gameplay frame.
    pub needs_setup: bool,
    rng: u64,
}

impl Default for Round {
    fn default() -> Self {
        Self {
            phase: Phase::Playing,
            score: 0,
            best: 0,
            lives: LIVES,
            time_left: ROUND_SECS,
            invulnerable: 0.0,
            needs_setup: true,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl Round {
    /// A fresh run that keeps the best score and the random stream.
    pub fn restart(&mut self) {
        *self = Self {
            best: self.best,
            rng: self.rng,
            ..Self::default()
        };
    }

    pub fn playing(&self) -> bool {
        self.phase == Phase::Playing
    }

    /// Sentinels that should be on the field for the current score.
    pub fn wanted_sentinels(&self) -> u32 {
        (START_SENTINELS + self.score / ORBS_PER_SENTINEL).min(MAX_SENTINELS)
    }

    /// Sentinel speed (m/s) for the current score; the player runs at 6.
    pub fn sentinel_speed(&self) -> f32 {
        (2.5 + self.score as f32 * 0.12).min(5.2)
    }

    /// Ends the run after a hit, or starts the invulnerability window. Returns true on a hit
    /// that counted.
    pub fn hit(&mut self) -> bool {
        if !self.playing() || self.invulnerable > 0.0 {
            return false;
        }
        self.lives = self.lives.saturating_sub(1);
        self.invulnerable = INVULNERABLE_SECS;
        if self.lives == 0 {
            self.finish(false);
        }
        true
    }

    /// Counts down the timer; ends the run at zero.
    pub fn tick(&mut self, dt: f32) {
        if !self.playing() {
            return;
        }
        self.time_left = (self.time_left - dt).max(0.0);
        self.invulnerable = (self.invulnerable - dt).max(0.0);
        if self.time_left == 0.0 {
            self.finish(true);
        }
    }

    fn finish(&mut self, survived: bool) {
        self.phase = Phase::Over { survived };
        self.best = self.best.max(self.score);
        self.invulnerable = 0.0;
    }

    /// xorshift64*: uniform in [0, 1). Deterministic per run, no extra crate.
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 40) as f32 / (1u64 << 24) as f32
    }

    /// A point on the floor inside the pillar ring.
    fn random_point(&mut self) -> Vec3 {
        let angle = self.random() * std::f32::consts::TAU;
        let r = self.random().sqrt() * SPAWN_RADIUS;
        Vec3::new(angle.cos() * r, 0.0, angle.sin() * r)
    }

    /// The window-title HUD.
    pub fn status(&self) -> String {
        let hearts: String = (0..LIVES)
            .map(|i| if i < self.lives { '♥' } else { '·' })
            .collect();
        match self.phase {
            Phase::Playing => format!(
                "Orbs {}  ·  {:.0} s  ·  {hearts}  ·  best {}",
                self.score,
                self.time_left.ceil(),
                self.best
            ),
            Phase::Over { survived } => format!(
                "{}  Orbs {} (best {})  ·  press R to play again",
                if survived { "TIME!" } else { "CAUGHT!" },
                self.score,
                self.best
            ),
        }
    }
}

/// A collectible orb. `home` is where it floats (it bobs around that height).
#[derive(Component, Copy, Clone, Debug)]
pub struct Orb {
    pub home: Vec3,
    pub phase: f32,
}

/// An enemy cube that chases the player.
#[derive(Component, Copy, Clone, Debug)]
pub struct Sentinel;

/// A short-lived visual: grows and fades, then despawns.
#[derive(Component, Copy, Clone, Debug)]
pub struct Burst {
    pub age: f32,
    pub color: Vec3,
}

const BURST_SECS: f32 = 0.45;
const ORB_COLOR: Vec3 = Vec3::new(0.25, 0.85, 1.0);
const SENTINEL_COLOR: Vec3 = Vec3::new(1.0, 0.18, 0.08);

fn orb_bundle(home: Vec3, phase: f32) -> impl Bundle {
    (
        Name::new("Orb"),
        Orb { home, phase },
        Transform::from_translation(home + Vec3::Y * 0.9).with_scale(Vec3::splat(0.35)),
        MeshRenderer {
            cast_shadow: false,
            ..MeshRenderer::new(
                MeshKind::Primitive(PrimitiveMesh::new(Primitive::IcoSphere {
                    subdivisions: 2,
                    radius: 1.0,
                })),
                [1.0; 3],
            )
        },
        Material {
            base_color: ORB_COLOR,
            emission_color: ORB_COLOR,
            emission_strength: 6.0,
            roughness: 0.2,
            ..Default::default()
        },
        Light {
            power: 80.0,
            color: ORB_COLOR,
            cast_shadow: false,
            use_custom_distance: true,
            custom_distance: 5.0,
            ..Light::new(LightKind::Point)
        },
        Spin {
            axis: Vec3::Y,
            speed: 2.0,
        },
    )
}

fn sentinel_bundle(position: Vec3) -> impl Bundle {
    (
        Name::new("Sentinel"),
        Sentinel,
        Transform::from_translation(position + Vec3::Y * 0.5).with_scale(Vec3::splat(0.9)),
        MeshRenderer::new(MeshKind::Cube, [1.0; 3]),
        Material {
            base_color: Vec3::new(0.25, 0.05, 0.04),
            metallic: 0.7,
            roughness: 0.35,
            emission_color: SENTINEL_COLOR,
            emission_strength: 0.6,
            ..Default::default()
        },
        Light {
            power: 60.0,
            color: SENTINEL_COLOR,
            cast_shadow: false,
            use_custom_distance: true,
            custom_distance: 3.5,
            ..Light::new(LightKind::Point)
        },
    )
}

/// A spot on the arena edge, opposite the player, for a new or knocked-back sentinel.
fn edge_point(round: &mut Round, player: Vec3) -> Vec3 {
    let away = if player.length_squared() > 1.0 {
        (-player).normalize()
    } else {
        let a = round.random() * std::f32::consts::TAU;
        Vec3::new(a.cos(), 0.0, a.sin())
    };
    let spread = (round.random() - 0.5) * 1.6;
    let dir = Quat::from_rotation_y(spread) * away;
    dir * (ARENA_HALF_SIZE - 3.0)
}

/// An orb spot away from the player and the other orbs.
fn orb_point(round: &mut Round, player: Vec3, others: &[Vec3]) -> Vec3 {
    let mut best = Vec3::ZERO;
    for _ in 0..16 {
        best = round.random_point();
        let clear = best.distance(player) > 4.0 && others.iter().all(|o| o.distance(best) > 3.0);
        if clear {
            break;
        }
    }
    best
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Starts a fresh run when the editor enters Play from Edit (PIE Stop despawns everything the
/// run spawned, so the next Play must set up again). Runs in `First`, which is not gated.
pub fn track_play_state(
    state: Option<Res<SimulationState>>,
    mut previous: Local<Option<SimulationState>>,
    mut round: ResMut<Round>,
    mut status: ResMut<WindowStatus>,
) {
    let state = state.map_or(SimulationState::Playing, |s| *s);
    if *previous == Some(SimulationState::Editing) && state == SimulationState::Playing {
        round.restart();
    }
    *previous = Some(state);
    let text = if state == SimulationState::Editing {
        String::new()
    } else {
        round.status()
    };
    if status.0 != text {
        status.0 = text;
    }
}

pub fn restart_input(actions: Res<ActionState>, mut round: ResMut<Round>) {
    if !round.playing() && actions.just_pressed(actions::RESTART) {
        round.restart();
    }
}

/// Entities that belong to one run and are cleared by the next.
type RunEntities = Or<(With<Orb>, With<Sentinel>, With<Burst>)>;

/// Clears the previous run's orbs and sentinels and spawns new ones; puts the player back.
pub fn setup_round(
    mut commands: Commands,
    mut round: ResMut<Round>,
    old: Query<Entity, RunEntities>,
    mut players: Query<(&mut FixedMotion, &mut Transform), With<Player>>,
) {
    if !round.needs_setup {
        return;
    }
    round.needs_setup = false;
    for e in &old {
        commands.entity(e).despawn();
    }
    for (mut motion, mut transform) in &mut players {
        *motion = FixedMotion {
            previous: START,
            current: START,
        };
        transform.translation = START;
    }
    let mut spots = Vec::new();
    for i in 0..ORBS {
        let p = orb_point(&mut round, START, &spots);
        spots.push(p);
        commands.spawn(orb_bundle(p, i as f32 * 2.1));
    }
    for _ in 0..round.wanted_sentinels() {
        let p = edge_point(&mut round, START);
        commands.spawn(sentinel_bundle(p));
    }
}

/// The timer, orb pickups, sentinel chasing and hits. After the player's interpolated
/// transform is known (PostPhysics).
#[allow(clippy::type_complexity)]
pub fn play_round(
    mut commands: Commands,
    time: Res<Time>,
    mut round: ResMut<Round>,
    players: Query<&Transform, (With<Player>, Without<Orb>, Without<Sentinel>)>,
    mut orbs: Query<(Entity, &mut Orb), Without<Sentinel>>,
    mut sentinels: Query<(Entity, &mut Transform), (With<Sentinel>, Without<Player>)>,
) {
    let dt = time.delta_secs();
    let Some(player) = players.iter().next().map(|t| flat(t.translation)) else {
        return;
    };
    round.tick(dt);
    if !round.playing() {
        return;
    }

    // Orbs: pick up on contact, respawn elsewhere.
    let homes: Vec<Vec3> = orbs.iter().map(|(_, o)| o.home).collect();
    for (_, mut orb) in &mut orbs {
        if flat(orb.home).distance(player) < COLLECT_RADIUS {
            round.score += 1;
            tracing::info!(score = round.score, orb = %orb.home, player = %player, "orb collected");
            commands.spawn(burst(orb.home + Vec3::Y * 0.9, ORB_COLOR));
            let others: Vec<Vec3> = homes.iter().copied().filter(|h| *h != orb.home).collect();
            orb.home = orb_point(&mut round, player, &others);
        }
    }

    // Sentinels: steer toward the player, keep apart from each other.
    let speed = round.sentinel_speed();
    let positions: Vec<(Entity, Vec3)> = sentinels
        .iter()
        .map(|(e, t)| (e, flat(t.translation)))
        .collect();
    let mut hit = false;
    for (e, mut transform) in &mut sentinels {
        let here = flat(transform.translation);
        let mut steer = (player - here).normalize_or_zero();
        for &(other, there) in &positions {
            let d = here.distance(there);
            if other != e && d < 1.8 && d > 1e-3 {
                steer += (here - there) / d * (1.8 - d);
            }
        }
        let next = here + steer.normalize_or_zero() * speed * dt;
        transform.translation = Vec3::new(next.x, 0.5, next.z);
        transform.rotation *= Quat::from_rotation_y(speed * dt * 0.8);
        if next.distance(player) < HIT_RADIUS && round.hit() {
            hit = true;
            commands.spawn(burst(transform.translation, SENTINEL_COLOR));
            let back = edge_point(&mut round, player);
            transform.translation = back + Vec3::Y * 0.5;
        }
    }
    if hit {
        tracing::info!(lives = round.lives, score = round.score, "player hit");
    }

    // Difficulty: another sentinel every few orbs.
    let have = positions.len() as u32;
    if round.playing() && have < round.wanted_sentinels() {
        let p = edge_point(&mut round, player);
        commands.spawn(sentinel_bundle(p));
    }
    if let Phase::Over { survived } = round.phase {
        tracing::info!(score = round.score, best = round.best, survived, "run over");
    }
}

fn burst(at: Vec3, color: Vec3) -> impl Bundle {
    (
        Name::new("Burst"),
        Burst { age: 0.0, color },
        Transform::from_translation(at).with_scale(Vec3::splat(0.3)),
        MeshRenderer {
            cast_shadow: false,
            ..MeshRenderer::new(
                MeshKind::Primitive(PrimitiveMesh::new(Primitive::IcoSphere {
                    subdivisions: 1,
                    radius: 1.0,
                })),
                [1.0; 3],
            )
        },
        Material {
            base_color: Vec3::ZERO,
            emission_color: color,
            emission_strength: 10.0,
            alpha: 0.8,
            render_method: RenderMethod::Blended,
            ..Default::default()
        },
    )
}

/// Visuals: orbs bob, bursts grow and fade, the player blinks while invulnerable.
#[allow(clippy::type_complexity)]
pub fn animate(
    mut commands: Commands,
    time: Res<Time>,
    round: Res<Round>,
    mut orbs: Query<(&Orb, &mut Transform), Without<Burst>>,
    mut bursts: Query<(Entity, &mut Burst, &mut Transform, &mut Material), Without<Orb>>,
    mut parts: Query<&mut Visibility, With<PlayerPart>>,
) {
    let t = time.elapsed_secs();
    let dt = time.delta_secs();
    for (orb, mut transform) in &mut orbs {
        let bob = (t * 2.5 + orb.phase).sin() * 0.25;
        transform.translation = orb.home + Vec3::Y * (0.9 + bob);
    }
    for (e, mut burst, mut transform, mut material) in &mut bursts {
        burst.age += dt;
        let k = (burst.age / BURST_SECS).min(1.0);
        transform.scale = Vec3::splat(0.3 + k * 1.6);
        material.alpha = 0.8 * (1.0 - k);
        material.emission_strength = 10.0 * (1.0 - k);
        material.emission_color = burst.color;
        if k >= 1.0 {
            commands.entity(e).despawn();
        }
    }
    // Blink at 10 Hz while invulnerable.
    let visible =
        round.invulnerable <= 0.0 || ((round.invulnerable * 10.0) as u32).is_multiple_of(2);
    for mut v in &mut parts {
        if v.visible != visible {
            v.visible = visible;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_hits_end_the_run_with_invulnerability_between() {
        let mut r = Round::default();
        assert!(r.hit());
        assert!(!r.hit(), "invulnerable right after a hit");
        r.tick(INVULNERABLE_SECS + 0.01);
        assert!(r.hit());
        r.tick(INVULNERABLE_SECS + 0.01);
        r.score = 7;
        assert!(r.hit());
        assert_eq!(r.phase, Phase::Over { survived: false });
        assert_eq!(r.best, 7);
        assert!(!r.hit(), "no hits after the run ends");
    }

    #[test]
    fn timer_ends_the_run_as_survived_and_restart_keeps_best() {
        let mut r = Round {
            score: 4,
            ..Default::default()
        };
        r.tick(ROUND_SECS + 1.0);
        assert_eq!(r.phase, Phase::Over { survived: true });
        r.restart();
        assert!(r.playing() && r.needs_setup);
        assert_eq!((r.score, r.best, r.lives), (0, 4, LIVES));
        assert_eq!(r.time_left, ROUND_SECS);
    }

    #[test]
    fn difficulty_ramps_and_caps() {
        let mut r = Round::default();
        assert_eq!(r.wanted_sentinels(), START_SENTINELS);
        r.score = ORBS_PER_SENTINEL * 2;
        assert_eq!(r.wanted_sentinels(), START_SENTINELS + 2);
        r.score = 1000;
        assert_eq!(r.wanted_sentinels(), MAX_SENTINELS);
        assert!(
            r.sentinel_speed() < 6.0,
            "the player can always outrun them"
        );
    }

    #[test]
    fn spawn_points_stay_in_the_ring_and_away_from_the_player() {
        let mut r = Round::default();
        for _ in 0..200 {
            let v = r.random();
            assert!((0.0..1.0).contains(&v));
            let p = orb_point(&mut r, Vec3::ZERO, &[]);
            assert!(p.length() <= SPAWN_RADIUS + 1e-3);
            let e = edge_point(&mut r, Vec3::new(3.0, 0.0, 0.0));
            assert!(e.x < 0.0, "sentinels arrive from the far side: {e}");
        }
    }
}
