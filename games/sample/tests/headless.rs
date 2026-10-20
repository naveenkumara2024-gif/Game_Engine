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
    // floor + 10 pillars + 6 cluster cubes + player + 2 shoulders + antenna
    assert_eq!(snap.items.len(), 1 + 10 + 6 + 1 + 2 + 1);
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
