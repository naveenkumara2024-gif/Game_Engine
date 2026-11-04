//! Human-readable entity names (UE's actor label, Blender's object name). Shown by the editor's
//! outliner and in logs. Plain `String`, not an interned `ge_core::Name`: the editor renames on
//! every keystroke, and the interner never frees.

use std::borrow::Cow;
use std::fmt;

use bevy_ecs::prelude::*;

#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Name(pub Cow<'static, str>);

impl Name {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn set(&mut self, name: impl Into<Cow<'static, str>>) {
        self.0 = name.into();
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&'static str> for Name {
    fn from(s: &'static str) -> Self {
        Self::new(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}
