//! Third-person follow camera: right-drag orbits, the wheel zooms, it always looks at the player.

use ge_engine::prelude::*;

use crate::actions;
use crate::player::Player;

/// Radians per raw mouse unit.
const ROTATE_SPEED: f32 = 0.005;
/// Distance multiplier per wheel line (towards the player).
const ZOOM_STEP: f32 = 0.9;
const PITCH_RANGE: (f32, f32) = (-0.2, 1.4);
const DISTANCE_RANGE: (f32, f32) = (3.0, 40.0);
/// Look at the body's center, not its feet.
const LOOK_OFFSET: Vec3 = Vec3::new(0.0, 0.8, 0.0);

#[derive(Component, Copy, Clone, Debug)]
pub struct OrbitCamera {
    /// Radians around +Y; 0 puts the camera on the player's +Z side.
    pub yaw: f32,
    /// Radians above the horizon.
    pub pitch: f32,
    pub distance: f32,
}

impl OrbitCamera {
    pub fn offset(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }
}

pub fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        OrbitCamera {
            yaw: 0.6,
            pitch: 0.45,
            distance: 10.0,
        },
        Transform::default(),
    ));
}

pub fn orbit_input(actions: Res<ActionState>, mut cameras: Query<&mut OrbitCamera>) {
    for mut cam in &mut cameras {
        if actions.pressed(actions::ORBIT) {
            let d = actions.mouse_delta();
            cam.yaw -= d.x * ROTATE_SPEED;
            cam.pitch = (cam.pitch + d.y * ROTATE_SPEED).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        }
        if actions.wheel() != 0.0 {
            cam.distance = (cam.distance * ZOOM_STEP.powf(actions.wheel()))
                .clamp(DISTANCE_RANGE.0, DISTANCE_RANGE.1);
        }
    }
}

pub fn follow_player(
    players: Query<&Transform, (With<Player>, Without<OrbitCamera>)>,
    mut cameras: Query<(&OrbitCamera, &mut Transform)>,
) {
    let Some(player) = players.iter().next() else {
        return;
    };
    let target = player.translation + LOOK_OFFSET;
    for (orbit, mut transform) in &mut cameras {
        *transform =
            Transform::from_translation(target + orbit.offset()).looking_at(target, Vec3::Y);
    }
}
