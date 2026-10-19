//! Typed console variables (law 8, lore §2.7, §6.4). Declared as statics anywhere in the
//! engine with [`declare_cvar!`], collected at startup via `inventory`, looked up
//! case-insensitively by name (UE convention: `r.*`, `p.*`, `au.*`, `s.*`, `net.*`).

use std::collections::HashMap;
use std::fmt;
use std::ops::BitOr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, RwLock};

#[doc(hidden)]
pub use inventory as __inventory;

#[derive(Copy, Clone, PartialEq, Eq, Default, Debug)]
pub struct CVarFlags(u32);

impl CVarFlags {
    pub const NONE: Self = Self(0);
    /// Only settable in dev builds (enforced once `shipping` exists).
    pub const CHEAT: Self = Self(1 << 0);
    /// Part of a scalability group (low → epic).
    pub const SCALABILITY: Self = Self(1 << 1);
    /// Settable from the command line / config at startup, not from the console.
    pub const READ_ONLY: Self = Self(1 << 2);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for CVarFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Where a set request came from; `READ_ONLY` cvars reject [`SetBy::Console`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SetBy {
    CommandLine,
    Config,
    Console,
    Code,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CVarError {
    #[error("unknown cvar '{0}'")]
    Unknown(String),
    #[error("cvar '{name}' is read-only")]
    ReadOnly { name: &'static str },
    #[error("cvar '{name}': cannot parse '{value}' as {expected}")]
    Parse {
        name: &'static str,
        value: String,
        expected: &'static str,
    },
}

/// Value types a cvar can hold.
pub trait CVarValue: Copy + Send + Sync + 'static {
    const TYPE_NAME: &'static str;
    fn parse(s: &str) -> Option<Self>;
    fn format(&self) -> String;
}

impl CVarValue for bool {
    const TYPE_NAME: &'static str = "bool";
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" => Some(true),
            "0" | "false" | "off" => Some(false),
            _ => None,
        }
    }
    fn format(&self) -> String {
        (*self as u8).to_string()
    }
}

macro_rules! impl_cvar_value_fromstr {
    ($($t:ty),*) => {$(
        impl CVarValue for $t {
            const TYPE_NAME: &'static str = stringify!($t);
            fn parse(s: &str) -> Option<Self> { s.trim().parse().ok() }
            fn format(&self) -> String { self.to_string() }
        }
    )*};
}
impl_cvar_value_fromstr!(i32, u32, f32);

/// Comma-separated floats, e.g. a color `0.1,0.2,0.4`.
impl<const N: usize> CVarValue for [f32; N] {
    const TYPE_NAME: &'static str = "float list";
    fn parse(s: &str) -> Option<Self> {
        let mut out = [0.0; N];
        let mut parts = s.split(',');
        for slot in &mut out {
            *slot = parts.next()?.trim().parse().ok()?;
        }
        parts.next().is_none().then_some(out)
    }
    fn format(&self) -> String {
        self.iter()
            .map(f32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

pub struct CVar<T: CVarValue> {
    name: &'static str,
    help: &'static str,
    flags: CVarFlags,
    default: T,
    value: RwLock<T>,
    /// Bumped on every successful set so consumers can cheaply detect changes.
    version: AtomicU32,
}

impl<T: CVarValue> CVar<T> {
    pub const fn new(name: &'static str, default: T, help: &'static str, flags: CVarFlags) -> Self {
        Self {
            name,
            help,
            flags,
            default,
            value: RwLock::new(default),
            version: AtomicU32::new(0),
        }
    }

    pub fn get(&self) -> T {
        *self.value.read().expect("cvar lock poisoned")
    }

    pub fn set(&self, value: T) {
        *self.value.write().expect("cvar lock poisoned") = value;
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn default_value(&self) -> T {
        self.default
    }

    pub fn version(&self) -> u32 {
        self.version.load(Ordering::Acquire)
    }
}

impl<T: CVarValue + fmt::Debug> fmt::Debug for CVar<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CVar")
            .field("name", &self.name)
            .field("value", &self.get())
            .finish()
    }
}

/// Type-erased view used by the registry, console and config loaders.
pub trait AnyCVar: Sync {
    fn name(&self) -> &'static str;
    fn help(&self) -> &'static str;
    fn flags(&self) -> CVarFlags;
    fn value_string(&self) -> String;
    fn set_from_str(&self, value: &str, by: SetBy) -> Result<(), CVarError>;
}

impl<T: CVarValue> AnyCVar for CVar<T> {
    fn name(&self) -> &'static str {
        self.name
    }
    fn help(&self) -> &'static str {
        self.help
    }
    fn flags(&self) -> CVarFlags {
        self.flags
    }
    fn value_string(&self) -> String {
        self.get().format()
    }
    fn set_from_str(&self, value: &str, by: SetBy) -> Result<(), CVarError> {
        if by == SetBy::Console && self.flags.contains(CVarFlags::READ_ONLY) {
            return Err(CVarError::ReadOnly { name: self.name });
        }
        let parsed = T::parse(value).ok_or_else(|| CVarError::Parse {
            name: self.name,
            value: value.to_owned(),
            expected: T::TYPE_NAME,
        })?;
        self.set(parsed);
        Ok(())
    }
}

#[doc(hidden)]
pub struct CVarEntry(pub &'static dyn AnyCVar);
inventory::collect!(CVarEntry);

impl fmt::Debug for CVarEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CVarEntry").field(&self.0.name()).finish()
    }
}

/// Declare a cvar static and register it.
///
/// ```ignore
/// ge_core::declare_cvar!(pub static R_VSYNC: bool = ("r.VSync", true, "Wait for vblank"));
/// ge_core::declare_cvar!(static R_Q: i32 = ("r.Quality", 3, "0..4", CVarFlags::SCALABILITY));
/// ```
#[macro_export]
macro_rules! declare_cvar {
    ($vis:vis static $ident:ident : $ty:ty = ($name:literal, $default:expr, $help:literal $(,)?)) => {
        $crate::declare_cvar!($vis static $ident: $ty =
            ($name, $default, $help, $crate::cvar::CVarFlags::NONE));
    };
    ($vis:vis static $ident:ident : $ty:ty =
        ($name:literal, $default:expr, $help:literal, $flags:expr $(,)?)) => {
        $vis static $ident: $crate::cvar::CVar<$ty> =
            $crate::cvar::CVar::new($name, $default, $help, $flags);
        $crate::cvar::__inventory::submit! { $crate::cvar::CVarEntry(&$ident) }
    };
}

static REGISTRY: LazyLock<HashMap<String, &'static dyn AnyCVar>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for entry in inventory::iter::<CVarEntry> {
        let key = entry.0.name().to_ascii_lowercase();
        if map.insert(key, entry.0).is_some() {
            tracing::error!(
                cvar = entry.0.name(),
                "duplicate cvar registration; last one wins"
            );
        }
    }
    map
});

pub fn find(name: &str) -> Option<&'static dyn AnyCVar> {
    REGISTRY.get(&name.to_ascii_lowercase()).copied()
}

pub fn set(name: &str, value: &str, by: SetBy) -> Result<(), CVarError> {
    find(name)
        .ok_or_else(|| CVarError::Unknown(name.to_owned()))?
        .set_from_str(value, by)
}

/// All cvars whose name starts with `prefix` (case-insensitive), sorted by name.
pub fn list(prefix: &str) -> Vec<&'static dyn AnyCVar> {
    let prefix = prefix.to_ascii_lowercase();
    let mut out: Vec<_> = REGISTRY
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| *v)
        .collect();
    out.sort_by_key(|c| c.name());
    out
}

/// Applies `-cvar name=value` pairs (also `-cvar=name=value`) from process arguments.
/// Returns the errors so the caller decides whether they are fatal.
pub fn apply_command_line<I, S>(args: I) -> Vec<CVarError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut errors = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        let pair = if arg == "-cvar" {
            match args.next() {
                Some(next) => next.as_ref().to_owned(),
                None => break,
            }
        } else if let Some(rest) = arg.strip_prefix("-cvar=") {
            rest.to_owned()
        } else {
            continue;
        };
        let (name, value) = pair.split_once('=').unwrap_or((pair.as_str(), ""));
        if let Err(e) = set(name, value, SetBy::CommandLine) {
            errors.push(e);
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    declare_cvar!(static T_INT: i32 = ("test.Int", 3, "test int"));
    declare_cvar!(static T_COLOR: [f32; 3] = ("test.Color", [0.0, 0.5, 1.0], "test color"));
    declare_cvar!(static T_RO: bool = ("test.ReadOnly", false, "ro", CVarFlags::READ_ONLY));
    declare_cvar!(static T_CMD: u32 = ("test.FromCmd", 0, "cmd line"));

    #[test]
    fn registered_and_case_insensitive() {
        assert_eq!(find("TEST.int").map(|c| c.name()), Some("test.Int"));
        assert!(list("test.").len() >= 4);
    }

    #[test]
    fn set_and_get_typed() {
        set("test.Int", "7", SetBy::Console).unwrap();
        assert_eq!(T_INT.get(), 7);
        assert_eq!(T_INT.default_value(), 3);
        assert!(T_INT.version() >= 1);
    }

    #[test]
    fn float_list_parse() {
        set("test.Color", "1, 0,0.25", SetBy::Code).unwrap();
        assert_eq!(T_COLOR.get(), [1.0, 0.0, 0.25]);
        assert!(matches!(
            set("test.Color", "1,2", SetBy::Code),
            Err(CVarError::Parse { .. })
        ));
        assert!(matches!(
            set("test.Color", "1,2,3,4", SetBy::Code),
            Err(CVarError::Parse { .. })
        ));
    }

    #[test]
    fn read_only_rejects_console_only() {
        assert_eq!(
            set("test.ReadOnly", "1", SetBy::Console),
            Err(CVarError::ReadOnly {
                name: "test.ReadOnly"
            })
        );
        set("test.ReadOnly", "1", SetBy::CommandLine).unwrap();
        assert!(T_RO.get());
    }

    #[test]
    fn unknown_cvar() {
        assert_eq!(
            set("nope.Nope", "1", SetBy::Code),
            Err(CVarError::Unknown("nope.Nope".into()))
        );
    }

    #[test]
    fn command_line_forms() {
        let errs = apply_command_line(["game.exe", "-cvar", "test.FromCmd=5", "-cvar=nope.X=1"]);
        assert_eq!(T_CMD.get(), 5);
        assert_eq!(errs, vec![CVarError::Unknown("nope.X".into())]);
    }
}
