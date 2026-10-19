//! Generational handles into a pool — "handles, not pointers" (law 5, lore §6.4).
//! A stale handle (slot freed and possibly reused) returns `None`, never another object.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

pub struct Handle<T> {
    index: u32,
    generation: u32,
    _t: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

// Manual impls: derives would wrongly require `T: Clone` etc.
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Handle<T> {}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}

impl<T> Eq for Handle<T> {}

impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({}v{})", self.index, self.generation)
    }
}

#[derive(Debug)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

#[derive(Debug)]
pub struct Pool<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }
}

impl<T> Pool<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, value: T) -> Handle<T> {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            return Handle {
                index,
                generation: slot.generation,
                _t: PhantomData,
            };
        }
        let index = u32::try_from(self.slots.len()).expect("Pool exceeded u32::MAX slots");
        self.slots.push(Slot {
            generation: 0,
            value: Some(value),
        });
        Handle {
            index,
            generation: 0,
            _t: PhantomData,
        }
    }

    pub fn remove(&mut self, handle: Handle<T>) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        let value = slot.value.take()?;
        // Bump on free so every outstanding handle to this slot goes stale.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        self.len -= 1;
        Some(value)
    }

    pub fn get(&self, handle: Handle<T>) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        (slot.generation == handle.generation).then_some(())?;
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, handle: Handle<T>) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        (slot.generation == handle.generation).then_some(())?;
        slot.value.as_mut()
    }

    pub fn contains(&self, handle: Handle<T>) -> bool {
        self.get(handle).is_some()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle<T>, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            let handle = Handle {
                index: i as u32,
                generation: s.generation,
                _t: PhantomData,
            };
            s.value.as_ref().map(|v| (handle, v))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove() {
        let mut pool = Pool::new();
        let a = pool.insert("a");
        let b = pool.insert("b");
        assert_eq!(pool.get(a), Some(&"a"));
        assert_eq!(pool.get(b), Some(&"b"));
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.remove(a), Some("a"));
        assert_eq!(pool.len(), 1);
        assert!(!pool.contains(a));
    }

    #[test]
    fn stale_handle_after_reuse_returns_none() {
        let mut pool = Pool::new();
        let old = pool.insert(1);
        pool.remove(old);
        let new = pool.insert(2);
        assert_eq!(old.index(), new.index(), "slot should be reused");
        assert_eq!(pool.get(old), None);
        assert_eq!(pool.remove(old), None);
        assert_eq!(pool.get(new), Some(&2));
    }

    #[test]
    fn double_remove_is_harmless() {
        let mut pool = Pool::new();
        let h = pool.insert(());
        assert!(pool.remove(h).is_some());
        assert!(pool.remove(h).is_none());
        assert!(pool.is_empty());
    }

    #[test]
    fn iter_skips_free_slots() {
        let mut pool = Pool::new();
        let a = pool.insert(10);
        let _b = pool.insert(20);
        pool.remove(a);
        let values: Vec<_> = pool.iter().map(|(_, v)| *v).collect();
        assert_eq!(values, vec![20]);
    }
}
