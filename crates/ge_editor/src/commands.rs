//! Undo / redo (lore §6.10 primitive #2, finding B3). Every edit the editor makes is a
//! reversible [`Edit`] pushed onto the [`History`]. Continuous edits (dragging a slider or the
//! gizmo, typing in a field) **merge** into one step while the widget stays active.
//!
//! Without reflection (M6) edits are typed: [`SetComponent<T>`] snapshots a whole component
//! before/after. Spawning and deleting never despawn: a deleted entity is *disabled* (bevy's
//! `Disabled`, which default queries skip), so undo is just re-enabling it and entity ids stay
//! stable. Entities are only truly despawned when their edit falls out of the history.

use std::any::Any;

use ge_ecs::prelude::*;

/// Hidden from every default query; the stand-in for "deleted but undoable".
pub use ge_ecs::Disabled;

const MAX_STEPS: usize = 200;

pub trait Edit: Send + Sync + 'static {
    fn label(&self) -> String;
    fn apply(&mut self, world: &mut World);
    fn revert(&mut self, world: &mut World);
    /// Absorb `next` (a later edit of the same widget) into `self`. Return false to refuse.
    fn merge(&mut self, _next: &dyn Edit) -> bool {
        false
    }
    /// The edit leaves the history. `applied` says whether it is currently in effect.
    fn discard(&mut self, _world: &mut World, _applied: bool) {}
    fn as_any(&self) -> &dyn Any;
}

struct Step {
    edit: Box<dyn Edit>,
    /// Merge key of the widget that produced it (`None` = never merges).
    merge_id: Option<u64>,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The widget currently being dragged/typed into: edits from it merge into the top step.
    open_merge: Option<u64>,
    kept_open: bool,
}

impl std::fmt::Debug for History {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("History")
            .field("undo", &self.undo.len())
            .field("redo", &self.redo.len())
            .finish()
    }
}

impl History {
    /// Applies `edit` and records it.
    pub fn execute(&mut self, world: &mut World, mut edit: Box<dyn Edit>, merge_id: Option<u64>) {
        edit.apply(world);
        self.record(world, edit, merge_id);
    }

    /// Records an edit that has already been applied (e.g. by a widget writing in place).
    pub fn record(&mut self, world: &mut World, edit: Box<dyn Edit>, merge_id: Option<u64>) {
        for mut step in self.redo.drain(..) {
            step.edit.discard(world, false);
        }
        if let (Some(id), Some(top)) = (merge_id, self.undo.last_mut())
            && self.open_merge == Some(id)
            && top.merge_id == Some(id)
            && top.edit.merge(edit.as_ref())
        {
            self.kept_open = true;
            return;
        }
        self.undo.push(Step { edit, merge_id });
        self.open_merge = merge_id;
        self.kept_open = merge_id.is_some();
        if self.undo.len() > MAX_STEPS {
            let mut oldest = self.undo.remove(0);
            oldest.edit.discard(world, true);
        }
    }

    /// Call while a widget is still being interacted with so its next edit merges.
    pub fn keep_open(&mut self, merge_id: u64) {
        if self.open_merge == Some(merge_id) {
            self.kept_open = true;
        }
    }

    /// Once per UI frame: closes the merge window if nothing kept it open.
    pub fn end_frame(&mut self) {
        if !self.kept_open {
            self.open_merge = None;
        }
        self.kept_open = false;
    }

    pub fn undo(&mut self, world: &mut World) -> Option<String> {
        self.open_merge = None;
        let mut step = self.undo.pop()?;
        step.edit.revert(world);
        let label = step.edit.label();
        self.redo.push(step);
        Some(label)
    }

    pub fn redo(&mut self, world: &mut World) -> Option<String> {
        self.open_merge = None;
        let mut step = self.redo.pop()?;
        step.edit.apply(world);
        let label = step.edit.label();
        self.undo.push(step);
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Labels oldest → newest, and how many of them are applied (Edit ▸ Undo History).
    pub fn labels(&self) -> (Vec<String>, usize) {
        let applied = self.undo.len();
        let labels = self
            .undo
            .iter()
            .chain(self.redo.iter().rev())
            .map(|s| s.edit.label())
            .collect();
        (labels, applied)
    }

    /// Forgets everything (entity ids are about to change, e.g. Play/Stop swaps the scene).
    pub fn clear(&mut self, world: &mut World) {
        for mut step in self.undo.drain(..) {
            step.edit.discard(world, true);
        }
        for mut step in self.redo.drain(..) {
            step.edit.discard(world, false);
        }
        self.open_merge = None;
    }
}

/// Replaces one component on one entity.
pub struct SetComponent<T: Component + Clone + PartialEq> {
    pub entity: Entity,
    pub before: T,
    pub after: T,
    pub label: &'static str,
}

impl<T: Component + Clone + PartialEq> Edit for SetComponent<T> {
    fn label(&self) -> String {
        self.label.to_owned()
    }

    fn apply(&mut self, world: &mut World) {
        if let Ok(mut e) = world.get_entity_mut(self.entity) {
            e.insert(self.after.clone());
        }
    }

    fn revert(&mut self, world: &mut World) {
        if let Ok(mut e) = world.get_entity_mut(self.entity) {
            e.insert(self.before.clone());
        }
    }

    fn merge(&mut self, next: &dyn Edit) -> bool {
        match next.as_any().downcast_ref::<Self>() {
            Some(next) if next.entity == self.entity => {
                self.after = next.after.clone();
                true
            }
            _ => false,
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Adds a component that wasn't there (undo removes it).
pub struct AddComponent<T: Component + Clone> {
    pub entity: Entity,
    pub value: T,
    pub label: &'static str,
}

impl<T: Component + Clone> Edit for AddComponent<T> {
    fn label(&self) -> String {
        self.label.to_owned()
    }
    fn apply(&mut self, world: &mut World) {
        if let Ok(mut e) = world.get_entity_mut(self.entity) {
            e.insert(self.value.clone());
        }
    }
    fn revert(&mut self, world: &mut World) {
        if let Ok(mut e) = world.get_entity_mut(self.entity) {
            e.remove::<T>();
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Changes a resource (World tab settings).
pub struct SetResource<T: Resource + Clone + PartialEq> {
    pub before: T,
    pub after: T,
    pub label: &'static str,
}

impl<T: Resource + Clone + PartialEq> Edit for SetResource<T> {
    fn label(&self) -> String {
        self.label.to_owned()
    }
    fn apply(&mut self, world: &mut World) {
        world.insert_resource(self.after.clone());
    }
    fn revert(&mut self, world: &mut World) {
        world.insert_resource(self.before.clone());
    }
    fn merge(&mut self, next: &dyn Edit) -> bool {
        match next.as_any().downcast_ref::<Self>() {
            Some(next) => {
                self.after = next.after.clone();
                true
            }
            None => false,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Changes a cvar (Render tab settings), stored as text so any cvar type works.
pub struct SetCVar {
    pub name: &'static str,
    pub before: String,
    pub after: String,
    pub label: String,
}

impl Edit for SetCVar {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn apply(&mut self, _world: &mut World) {
        let _ = ge_core::cvar::set(self.name, &self.after, ge_core::cvar::SetBy::Console);
    }
    fn revert(&mut self, _world: &mut World) {
        let _ = ge_core::cvar::set(self.name, &self.before, ge_core::cvar::SetBy::Console);
    }
    fn merge(&mut self, next: &dyn Edit) -> bool {
        match next.as_any().downcast_ref::<Self>() {
            Some(next) if next.name == self.name => {
                self.after = next.after.clone();
                true
            }
            _ => false,
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Enables or disables a whole subtree (spawn = enable, delete = disable).
pub struct SetAlive {
    pub entity: Entity,
    /// True for "spawned" (applied = alive), false for "deleted" (applied = disabled).
    pub alive_when_applied: bool,
    pub label: String,
}

impl Edit for SetAlive {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn apply(&mut self, world: &mut World) {
        set_enabled_recursive(world, self.entity, self.alive_when_applied);
    }
    fn revert(&mut self, world: &mut World) {
        set_enabled_recursive(world, self.entity, !self.alive_when_applied);
    }
    fn discard(&mut self, world: &mut World, applied: bool) {
        // Despawn for good only when the entity is currently dead and can't come back.
        let alive = applied == self.alive_when_applied;
        if !alive && let Ok(e) = world.get_entity_mut(self.entity) {
            e.despawn();
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Inserts or removes `Disabled` on an entity and all its descendants.
pub fn set_enabled_recursive(world: &mut World, root: Entity, enabled: bool) {
    let mut stack = vec![root];
    while let Some(e) = stack.pop() {
        let Ok(mut entity) = world.get_entity_mut(e) else {
            continue;
        };
        if let Some(children) = entity.get::<Children>() {
            stack.extend(children.iter());
        }
        if enabled {
            entity.remove::<Disabled>();
        } else {
            entity.insert(Disabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component, Clone, PartialEq, Debug)]
    struct Value(i32);

    fn set(entity: Entity, before: i32, after: i32) -> Box<dyn Edit> {
        Box::new(SetComponent {
            entity,
            before: Value(before),
            after: Value(after),
            label: "Set value",
        })
    }

    #[test]
    fn undo_redo_round_trip_and_redo_cleared_by_new_edit() {
        let mut world = World::new();
        let e = world.spawn(Value(0)).id();
        let mut h = History::default();
        h.execute(&mut world, set(e, 0, 1), None);
        h.execute(&mut world, set(e, 1, 2), None);
        assert_eq!(world.get::<Value>(e), Some(&Value(2)));
        h.undo(&mut world);
        assert_eq!(world.get::<Value>(e), Some(&Value(1)));
        h.undo(&mut world);
        assert_eq!(world.get::<Value>(e), Some(&Value(0)));
        assert!(h.undo(&mut world).is_none());
        h.redo(&mut world);
        assert_eq!(world.get::<Value>(e), Some(&Value(1)));
        h.execute(&mut world, set(e, 1, 5), None);
        assert!(!h.can_redo(), "a new edit drops the redo branch");
    }

    #[test]
    fn drags_merge_into_one_step_until_released() {
        let mut world = World::new();
        let e = world.spawn(Value(0)).id();
        let mut h = History::default();
        for v in 1..=10 {
            h.execute(&mut world, set(e, v - 1, v), Some(42));
            h.keep_open(42);
            h.end_frame();
        }
        assert_eq!(h.labels().0.len(), 1, "one drag = one undo step");
        h.end_frame(); // released: nothing kept it open
        h.execute(&mut world, set(e, 10, 11), Some(42));
        assert_eq!(h.labels().0.len(), 2, "a new drag starts a new step");
        h.undo(&mut world);
        h.undo(&mut world);
        assert_eq!(world.get::<Value>(e), Some(&Value(0)));
    }

    #[test]
    fn delete_disables_and_undo_restores_subtree() {
        let mut world = World::new();
        let parent = world.spawn(Value(1)).id();
        let child = world.spawn((Value(2), ChildOf(parent))).id();
        let mut h = History::default();
        h.execute(
            &mut world,
            Box::new(SetAlive {
                entity: parent,
                alive_when_applied: false,
                label: "Delete".into(),
            }),
            None,
        );
        let mut q = world.query::<&Value>();
        assert_eq!(
            q.iter(&world).count(),
            0,
            "both hidden from default queries"
        );
        h.undo(&mut world);
        let mut q = world.query::<&Value>();
        assert_eq!(q.iter(&world).count(), 2);
        assert!(world.get_entity(child).is_ok());
    }

    #[test]
    fn discarded_spawn_is_despawned() {
        let mut world = World::new();
        let e = world.spawn(Value(1)).id();
        let mut h = History::default();
        h.record(
            &mut world,
            Box::new(SetAlive {
                entity: e,
                alive_when_applied: true,
                label: "Add".into(),
            }),
            None,
        );
        h.undo(&mut world); // disabled
        h.execute(&mut world, set(e, 1, 2), None); // drops the redo branch → despawn
        assert!(world.get_entity(e).is_err());
    }
}
