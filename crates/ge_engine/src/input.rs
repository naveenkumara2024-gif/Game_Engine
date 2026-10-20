//! Input actions (lore §6.9 "input actions mapping"). Gameplay asks "is `Jump` pressed?" or
//! "what is the `Move` axis?", never "is W down?". Bindings live in [`InputMap`]; this frame's
//! state lives in [`ActionState`], rebuilt in the `First` tick group from platform events.
//!
//! Edges (`just_pressed`) are per *frame*: read them in variable-rate groups (`PrePhysics`),
//! because `FixedUpdate` may run zero or several times in a frame. Held state and axes are safe
//! to read anywhere.

use std::collections::{HashMap, HashSet};

use ge_ecs::prelude::*;
use ge_platform::{MouseButton, PlatformEvent};

/// One physical input that can drive a button action.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// Layout-independent key name from the platform layer, e.g. `"KeyW"`, `"Escape"`.
    Key(String),
    Mouse(MouseButton),
}

impl Binding {
    pub fn key(name: &str) -> Self {
        Self::Key(name.to_owned())
    }
}

/// Four keys combined into a 2D axis: +Y is `up`, +X is `right`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Axis2dBinding {
    pub up: String,
    pub down: String,
    pub left: String,
    pub right: String,
}

impl Axis2dBinding {
    pub fn wasd() -> Self {
        Self::keys("KeyW", "KeyS", "KeyA", "KeyD")
    }

    pub fn arrows() -> Self {
        Self::keys("ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight")
    }

    pub fn keys(up: &str, down: &str, left: &str, right: &str) -> Self {
        Self {
            up: up.to_owned(),
            down: down.to_owned(),
            left: left.to_owned(),
            right: right.to_owned(),
        }
    }
}

/// Action name → bindings. Several bindings may drive one action (any of them counts).
#[derive(Resource, Clone, Debug, Default)]
pub struct InputMap {
    buttons: Vec<(String, Binding)>,
    axes: Vec<(String, Axis2dBinding)>,
}

impl InputMap {
    pub fn bind_button(&mut self, action: &str, binding: Binding) -> &mut Self {
        self.buttons.push((action.to_owned(), binding));
        self
    }

    pub fn bind_axis2d(&mut self, action: &str, binding: Axis2dBinding) -> &mut Self {
        self.axes.push((action.to_owned(), binding));
        self
    }
}

/// This frame's resolved actions.
#[derive(Resource, Clone, Debug, Default)]
pub struct ActionState {
    keys_held: HashSet<String>,
    mouse_held: HashSet<MouseButton>,
    current: HashSet<String>,
    previous: HashSet<String>,
    axes: HashMap<String, Vec2>,
    mouse_delta: Vec2,
    wheel: f32,
}

impl ActionState {
    /// Applies this frame's platform events and re-evaluates every action.
    pub fn update(&mut self, map: &InputMap, events: &[PlatformEvent]) {
        self.mouse_delta = Vec2::ZERO;
        self.wheel = 0.0;
        for event in events {
            match event {
                PlatformEvent::Key {
                    key,
                    pressed,
                    repeat: false,
                } => {
                    if *pressed {
                        self.keys_held.insert(key.clone());
                    } else {
                        self.keys_held.remove(key);
                    }
                }
                PlatformEvent::MouseButton { button, pressed } => {
                    if *pressed {
                        self.mouse_held.insert(*button);
                    } else {
                        self.mouse_held.remove(button);
                    }
                }
                PlatformEvent::MouseMotion { dx, dy } => {
                    self.mouse_delta += Vec2::new(*dx as f32, *dy as f32);
                }
                PlatformEvent::MouseWheel { lines } => self.wheel += lines,
                // Release events are lost while unfocused; drop everything so nothing sticks.
                PlatformEvent::Focused(false) => {
                    self.keys_held.clear();
                    self.mouse_held.clear();
                }
                _ => {}
            }
        }

        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
        for (action, binding) in &map.buttons {
            let held = match binding {
                Binding::Key(k) => self.keys_held.contains(k),
                Binding::Mouse(b) => self.mouse_held.contains(b),
            };
            if held {
                self.current.insert(action.clone());
            }
        }

        self.axes.clear();
        for (action, b) in &map.axes {
            let key = |k: &String| f32::from(u8::from(self.keys_held.contains(k)));
            let v = Vec2::new(key(&b.right) - key(&b.left), key(&b.up) - key(&b.down));
            let total = *self.axes.entry(action.clone()).or_default() + v;
            // Diagonals don't move faster than straight lines.
            self.axes
                .insert(action.clone(), total.clamp_length_max(1.0));
        }
    }

    pub fn pressed(&self, action: &str) -> bool {
        self.current.contains(action)
    }

    pub fn just_pressed(&self, action: &str) -> bool {
        self.current.contains(action) && !self.previous.contains(action)
    }

    pub fn just_released(&self, action: &str) -> bool {
        !self.current.contains(action) && self.previous.contains(action)
    }

    /// Zero for unknown actions. Length ≤ 1.
    pub fn axis2d(&self, action: &str) -> Vec2 {
        self.axes.get(action).copied().unwrap_or(Vec2::ZERO)
    }

    /// Raw mouse movement this frame (+X right, +Y down), in device units.
    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_delta
    }

    /// Wheel lines this frame, positive away from the user.
    pub fn wheel(&self) -> f32 {
        self.wheel
    }
}

/// OS events gathered since the last frame. Filled by the engine, read in `First`, cleared at
/// the end of the frame.
#[derive(Resource, Clone, Debug, Default)]
pub struct PlatformEvents(pub Vec<PlatformEvent>);

pub(crate) fn update_actions(
    mut state: ResMut<ActionState>,
    map: Res<InputMap>,
    events: Res<PlatformEvents>,
) {
    state.update(&map, &events.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str, pressed: bool) -> PlatformEvent {
        PlatformEvent::Key {
            key: k.into(),
            pressed,
            repeat: false,
        }
    }

    fn map() -> InputMap {
        let mut m = InputMap::default();
        m.bind_button("Jump", Binding::key("Space"))
            .bind_button("Jump", Binding::Mouse(MouseButton::Left))
            .bind_axis2d("Move", Axis2dBinding::wasd());
        m
    }

    #[test]
    fn button_edges() {
        let m = map();
        let mut s = ActionState::default();
        s.update(&m, &[key("Space", true)]);
        assert!(s.pressed("Jump") && s.just_pressed("Jump"));
        s.update(&m, &[]);
        assert!(s.pressed("Jump") && !s.just_pressed("Jump"));
        // OS key repeat doesn't re-trigger.
        s.update(
            &m,
            &[PlatformEvent::Key {
                key: "Space".into(),
                pressed: true,
                repeat: true,
            }],
        );
        assert!(!s.just_pressed("Jump"));
        s.update(&m, &[key("Space", false)]);
        assert!(!s.pressed("Jump") && s.just_released("Jump"));
        s.update(&m, &[]);
        assert!(!s.just_released("Jump"));
    }

    #[test]
    fn any_binding_drives_the_action() {
        let m = map();
        let mut s = ActionState::default();
        s.update(
            &m,
            &[PlatformEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true,
            }],
        );
        assert!(s.just_pressed("Jump"));
    }

    #[test]
    fn wasd_axis_normalizes_diagonals() {
        let m = map();
        let mut s = ActionState::default();
        s.update(&m, &[key("KeyW", true)]);
        assert_eq!(s.axis2d("Move"), Vec2::Y);
        s.update(&m, &[key("KeyD", true)]);
        let v = s.axis2d("Move");
        assert!((v.length() - 1.0).abs() < 1e-6 && v.x > 0.0 && v.y > 0.0);
        s.update(&m, &[key("KeyS", true)]); // W and S cancel
        assert_eq!(s.axis2d("Move"), Vec2::X);
        assert_eq!(s.axis2d("Unknown"), Vec2::ZERO);
    }

    #[test]
    fn focus_loss_releases_everything() {
        let m = map();
        let mut s = ActionState::default();
        s.update(&m, &[key("KeyW", true), key("Space", true)]);
        s.update(&m, &[PlatformEvent::Focused(false)]);
        assert_eq!(s.axis2d("Move"), Vec2::ZERO);
        assert!(!s.pressed("Jump") && s.just_released("Jump"));
    }

    #[test]
    fn mouse_delta_and_wheel_are_per_frame() {
        let m = map();
        let mut s = ActionState::default();
        s.update(
            &m,
            &[
                PlatformEvent::MouseMotion { dx: 3.0, dy: -1.0 },
                PlatformEvent::MouseMotion { dx: 2.0, dy: 0.0 },
                PlatformEvent::MouseWheel { lines: 1.5 },
            ],
        );
        assert_eq!(s.mouse_delta(), Vec2::new(5.0, -1.0));
        assert_eq!(s.wheel(), 1.5);
        s.update(&m, &[]);
        assert_eq!(s.mouse_delta(), Vec2::ZERO);
        assert_eq!(s.wheel(), 0.0);
    }
}
