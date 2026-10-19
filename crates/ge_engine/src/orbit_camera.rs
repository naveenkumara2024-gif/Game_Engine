//! Temporary orbit camera controller for M1: right-drag orbits, the wheel zooms. Replaced by
//! input actions and a camera component once the ECS and input mapping exist (M2).

use ge_platform::{MouseButton, PlatformEvent};
use ge_render::Camera;
use glam::Vec3;

/// Radians per raw mouse unit.
const ROTATE_SPEED: f32 = 0.005;
/// Distance multiplier per wheel line (towards the target).
const ZOOM_STEP: f32 = 0.9;
/// Stay just short of straight up/down so `look_at` keeps a stable up vector.
const MAX_PITCH: f32 = 1.5;
const MIN_DISTANCE: f32 = 1.0;
const MAX_DISTANCE: f32 = 200.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OrbitCamera {
    base: Camera,
    yaw: f32,
    pitch: f32,
    distance: f32,
    dragging: bool,
}

impl OrbitCamera {
    /// Starts orbiting `camera.target` from wherever `camera` currently is.
    pub(crate) fn new(camera: Camera) -> Self {
        let offset = camera.position - camera.target;
        let distance = offset.length().max(MIN_DISTANCE);
        Self {
            base: camera,
            yaw: offset.x.atan2(offset.z),
            pitch: (offset.y / distance).clamp(-1.0, 1.0).asin(),
            distance,
            dragging: false,
        }
    }

    pub(crate) fn event(&mut self, event: &PlatformEvent) {
        match *event {
            PlatformEvent::MouseButton {
                button: MouseButton::Right,
                pressed,
            } => self.dragging = pressed,
            PlatformEvent::Focused(false) => self.dragging = false,
            PlatformEvent::MouseMotion { dx, dy } if self.dragging => {
                self.yaw -= dx as f32 * ROTATE_SPEED;
                self.pitch = (self.pitch + dy as f32 * ROTATE_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
            }
            PlatformEvent::MouseWheel { lines } => {
                self.distance =
                    (self.distance * ZOOM_STEP.powf(lines)).clamp(MIN_DISTANCE, MAX_DISTANCE);
            }
            _ => {}
        }
    }

    pub(crate) fn camera(&self) -> Camera {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let offset = Vec3::new(cos_pitch * sin_yaw, sin_pitch, cos_pitch * cos_yaw) * self.distance;
        Camera {
            position: self.base.target + offset,
            ..self.base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Camera {
        Camera {
            position: Vec3::new(7.0, 6.0, 9.0),
            target: Vec3::new(0.0, 0.5, 0.0),
            ..Default::default()
        }
    }

    #[test]
    fn starts_where_the_camera_is() {
        let orbit = OrbitCamera::new(base());
        assert!(orbit.camera().position.distance(base().position) < 1e-4);
    }

    #[test]
    fn motion_only_rotates_while_dragging() {
        let mut orbit = OrbitCamera::new(base());
        let before = orbit.camera();
        orbit.event(&PlatformEvent::MouseMotion { dx: 100.0, dy: 0.0 });
        assert_eq!(orbit.camera(), before);

        orbit.event(&PlatformEvent::MouseButton {
            button: MouseButton::Right,
            pressed: true,
        });
        orbit.event(&PlatformEvent::MouseMotion { dx: 100.0, dy: 0.0 });
        let after = orbit.camera();
        assert_ne!(after.position, before.position);
        // Orbiting keeps the distance to the target.
        let d0 = before.position.distance(before.target);
        let d1 = after.position.distance(after.target);
        assert!((d0 - d1).abs() < 1e-4);
    }

    #[test]
    fn zoom_and_pitch_are_clamped() {
        let mut orbit = OrbitCamera::new(base());
        for _ in 0..1000 {
            orbit.event(&PlatformEvent::MouseWheel { lines: 1.0 });
        }
        assert!((orbit.distance - MIN_DISTANCE).abs() < 1e-6);
        orbit.event(&PlatformEvent::MouseButton {
            button: MouseButton::Right,
            pressed: true,
        });
        orbit.event(&PlatformEvent::MouseMotion { dx: 0.0, dy: 1.0e6 });
        assert_eq!(orbit.pitch, MAX_PITCH);
    }
}
