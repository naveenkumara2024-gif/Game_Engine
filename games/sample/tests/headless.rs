//! Drives the sample game through the real plugin stack without a window: input actions →
//! fixed-step movement → interpolation → camera → transform propagation → extract.

use std::time::Duration;

use ge_engine::input::PlatformEvents;
use ge_engine::prelude::*;
use ge_engine::{InputPlugin, RenderPlugin, TransformPlugin};
use ge_render::RenderSnapshot;
use sample::SamplePlugin;
use sample::camera::OrbitCamera;
use sample::player::{Player, camera_basis};

/// Queues a key event as if the OS had sent it this frame.
fn key(events: &mut PlatformEvents, name: &str, pressed: bool) {
    events.0.push(ge_platform::PlatformEvent::Key {
        key: name.into(),
        pressed,
        repeat: false,
    });
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((InputPlugin, TransformPlugin, RenderPlugin, SamplePlugin));
    app.finish().expect("plugins build");
    app.startup();
    app
}

fn player_position(app: &mut App) -> Vec3 {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&Transform, With<Player>>();
    q.single(world).unwrap().translation
}

fn run_for(app: &mut App, seconds: f64, fps: f64) {
    let frame = Duration::from_secs_f64(1.0 / fps);
    for _ in 0..(seconds * fps).round() as u32 {
        app.update(frame);
    }
}

fn walk_forward_one_second(fps: f64) -> Vec3 {
    let mut app = app();
    run_for(&mut app, 0.1, fps); // settle
    let start = player_position(&mut app);
    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "KeyW",
        true,
    );
    run_for(&mut app, 1.0, fps);
    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "KeyW",
        false,
    );
    run_for(&mut app, 0.1, fps);
    player_position(&mut app) - start
}

#[test]
fn w_walks_away_from_the_camera_at_the_same_speed_at_any_fps() {
    let world_forward = {
        let mut app = app();
        let world = app.world_mut();
        let yaw = world.query::<&OrbitCamera>().single(world).unwrap().yaw;
        camera_basis(yaw).0
    };
    let mut distances = Vec::new();
    for fps in [30.0, 144.0] {
        let moved = walk_forward_one_second(fps);
        assert!(
            moved.normalize().abs_diff_eq(world_forward, 1e-3),
            "{fps} fps moved {moved}, expected along {world_forward}"
        );
        distances.push(moved.length());
    }
    // 6 units/s for ~1 s; both rates within two fixed steps of each other.
    let step = 6.0 / 60.0;
    assert!(
        (distances[0] - distances[1]).abs() <= 2.0 * step,
        "{distances:?}"
    );
    assert!((distances[0] - 6.0).abs() <= 2.0 * step, "{distances:?}");
}

#[test]
fn children_follow_and_extract_sees_everything() {
    let mut app = app();
    run_for(&mut app, 0.05, 60.0);
    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "KeyD",
        true,
    );
    run_for(&mut app, 0.5, 60.0);

    let world = app.world_mut();
    let (player, children) = world
        .query_filtered::<(Entity, &Children), With<Player>>()
        .single(world)
        .map(|(e, c)| (e, c.to_vec()))
        .unwrap();
    let player_global = *world.get::<GlobalTransform>(player).unwrap();
    for child in children {
        let local = world.get::<Transform>(child).unwrap().to_affine();
        let global = world.get::<GlobalTransform>(child).unwrap();
        assert!(
            global.0.abs_diff_eq(player_global.0 * local, 1e-4),
            "child is not attached to the moved player"
        );
    }

    let snap = world.resource::<RenderSnapshot>();
    assert!(snap.camera.is_some(), "camera extracted");
    // floor + 10 pillars + 6 cluster cubes + player + 2 shoulders + antenna, plus Orb Run's
    // 3 orbs and 2 starting sentinels. Without a renderer nothing drains the delta, so it
    // holds one upsert per visible mesh.
    assert_eq!(snap.scene.upserts().count(), 1 + 10 + 6 + 1 + 2 + 1 + 3 + 2);
}

#[test]
fn escape_requests_exit() {
    let mut app = app();
    app.update(Duration::from_millis(16));
    assert_eq!(app.exit_requested(), None);
    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "Escape",
        true,
    );
    app.update(Duration::from_millis(16));
    assert_eq!(app.exit_requested(), Some("quit action"));
}

fn round(app: &App) -> sample::game::Round {
    app.world().resource::<sample::game::Round>().clone()
}

/// Orb Run: standing still scores nothing, sentinels catch the player within the round, and R
/// starts a fresh run.
#[test]
fn idle_player_is_caught_and_r_restarts() {
    use sample::game::{LIVES, Phase};
    let mut app = app();
    run_for(&mut app, 3.0, 60.0);
    let r = round(&app);
    assert_eq!(r.score, 0, "no orbs without moving");
    let start = player_position(&mut app);
    assert!(
        start.distance(Vec3::new(0.0, 0.5, 0.0)) < 1e-3,
        "player stays put: {start}"
    );

    run_for(&mut app, 40.0, 60.0);
    let r = round(&app);
    assert_eq!(r.phase, Phase::Over { survived: false }, "{r:?}");
    assert_eq!(r.lives, 0);
    assert!(
        app.world().resource::<WindowStatus>().0.contains("CAUGHT"),
        "HUD: {:?}",
        app.world().resource::<WindowStatus>().0
    );

    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "KeyR",
        true,
    );
    run_for(&mut app, 0.1, 60.0);
    key(
        &mut app.world_mut().resource_mut::<PlatformEvents>(),
        "KeyR",
        false,
    );
    let r = round(&app);
    assert!(r.playing(), "restarted");
    assert_eq!(r.lives, LIVES);
}

/// Walking straight to an orb collects it, and it reappears elsewhere.
#[test]
fn walking_into_an_orb_scores() {
    use sample::game::Orb;
    let mut app = app();
    run_for(&mut app, 0.1, 60.0);
    let target = {
        let world = app.world_mut();
        let mut q = world.query::<&Orb>();
        q.iter(world).next().unwrap().home
    };
    // Teleport-free: steer with WASD toward the orb using the camera basis each frame.
    for _ in 0..(10 * 60) {
        if round(&app).score > 0 {
            break;
        }
        let here = player_position(&mut app);
        let yaw = {
            let world = app.world_mut();
            let mut q = world.query::<&OrbitCamera>();
            q.single(world).unwrap().yaw
        };
        let (forward, right) = camera_basis(yaw);
        let to = Vec3::new(target.x - here.x, 0.0, target.z - here.z);
        let events = &mut app.world_mut().resource_mut::<PlatformEvents>();
        for (k, on) in [
            ("KeyW", to.dot(forward) > 0.2),
            ("KeyS", to.dot(forward) < -0.2),
            ("KeyD", to.dot(right) > 0.2),
            ("KeyA", to.dot(right) < -0.2),
        ] {
            key(events, k, on);
        }
        app.update(Duration::from_secs_f64(1.0 / 60.0));
    }
    assert!(round(&app).score >= 1, "collected: {:?}", round(&app));
}

/// A long first frame (startup hitch) must not move the player or score.
#[test]
fn startup_hitch_does_not_score() {
    let mut app = app();
    app.update(Duration::from_millis(700));
    run_for(&mut app, 2.0, 144.0);
    let r = round(&app);
    assert_eq!(r.score, 0, "{r:?} player at {}", player_position(&mut app));
}
