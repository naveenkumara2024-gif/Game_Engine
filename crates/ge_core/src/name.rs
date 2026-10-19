//! Interned identifiers — the `FName` analog (lore §5). Compare as integers; the string table
//! lives for the whole process. Case-sensitive (unlike UE) to keep lookups trivial.

use std::collections::HashMap;
use std::fmt;
use std::sync::{LazyLock, RwLock};

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(u32);

#[derive(Default)]
struct Interner {
    map: HashMap<&'static str, u32>,
    strings: Vec<&'static str>,
}

static INTERNER: LazyLock<RwLock<Interner>> = LazyLock::new(Default::default);

impl Name {
    pub fn new(s: &str) -> Self {
        if let Some(&id) = INTERNER.read().expect("interner poisoned").map.get(s) {
            return Name(id);
        }
        let mut interner = INTERNER.write().expect("interner poisoned");
        if let Some(&id) = interner.map.get(s) {
            return Name(id);
        }
        // Leaked on purpose: names are process-lifetime and few.
        let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
        let id = u32::try_from(interner.strings.len()).expect("too many names");
        interner.strings.push(leaked);
        interner.map.insert(leaked, id);
        Name(id)
    }

    pub fn as_str(self) -> &'static str {
        INTERNER.read().expect("interner poisoned").strings[self.0 as usize]
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.as_str())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_string_same_name() {
        let a = Name::new("Render::Opaque");
        let b = Name::new("Render::Opaque");
        let c = Name::new("Render::Shadow");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.as_str(), "Render::Opaque");
    }
}
