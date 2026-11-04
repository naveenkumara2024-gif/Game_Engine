//! Asset ids and handles (lore §6.8).
//!
//! - [`AssetId<T>`]: a weak, `Copy` generational id. Components and the render snapshot store
//!   these; a stale id simply resolves to `None`.
//! - [`Handle<T>`]: a **strong** handle. It keeps its whole *source file* (and every sub-asset
//!   imported from it) loaded. When the last strong handle to a source drops, the server
//!   unloads it at the end of the frame.

use std::any::TypeId;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::sync::Arc;

/// A weak, typed asset id.
pub struct AssetId<T> {
    pub(crate) index: u32,
    pub(crate) generation: u32,
    _marker: PhantomData<fn() -> T>,
}

impl<T> AssetId<T> {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            _marker: PhantomData,
        }
    }

    /// Builds an id from raw parts. For tests and tools that drive the renderer without an
    /// `AssetServer`; real ids come from [`crate::AssetServer::load`].
    #[doc(hidden)]
    pub fn from_raw(index: u32, generation: u32) -> Self {
        Self::new(index, generation)
    }

    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl<T: 'static> AssetId<T> {
    pub fn untyped(self) -> UntypedId {
        UntypedId {
            type_id: TypeId::of::<T>(),
            index: self.index,
            generation: self.generation,
        }
    }
}

impl<T> Clone for AssetId<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for AssetId<T> {}
impl<T> PartialEq for AssetId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}
impl<T> Eq for AssetId<T> {}
impl<T> Hash for AssetId<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}
impl<T> fmt::Debug for AssetId<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = std::any::type_name::<T>()
            .rsplit("::")
            .next()
            .unwrap_or("?");
        write!(f, "AssetId<{name}>({}v{})", self.index, self.generation)
    }
}

/// An id of any asset type (events, garbage collection).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct UntypedId {
    pub type_id: TypeId,
    pub index: u32,
    pub generation: u32,
}

impl UntypedId {
    pub fn typed<T: 'static>(self) -> Option<AssetId<T>> {
        (self.type_id == TypeId::of::<T>()).then(|| AssetId::new(self.index, self.generation))
    }
}

/// Shared by every strong handle of one source file; its strong count is the source's
/// reference count.
#[derive(Debug)]
pub(crate) struct SourceToken {
    pub(crate) source: crate::AssetPath,
}

/// A strong handle: keeps the asset (and its source file's other sub-assets) loaded.
pub struct Handle<T> {
    id: AssetId<T>,
    pub(crate) token: Arc<SourceToken>,
}

impl<T> Handle<T> {
    pub(crate) fn new(id: AssetId<T>, token: Arc<SourceToken>) -> Self {
        Self { id, token }
    }

    pub fn id(&self) -> AssetId<T> {
        self.id
    }

    /// The source file this asset came from.
    pub fn source(&self) -> &crate::AssetPath {
        &self.token.source
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            token: self.token.clone(),
        }
    }
}

impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({:?}, {})", self.id, self.token.source)
    }
}
