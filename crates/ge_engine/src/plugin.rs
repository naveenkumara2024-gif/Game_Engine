//! Plugins (law 9, lore §3.2). Features and games are plugins; the app builds them in
//! dependency order, like UE loads modules by declared dependencies and loading phase.

use crate::App;

pub trait Plugin: 'static {
    /// Unique, stable name. Other plugins refer to it in [`Plugin::dependencies`].
    fn name(&self) -> &'static str;

    /// Plugins whose `build` must run before this one's.
    fn dependencies(&self) -> &'static [&'static str] {
        &[]
    }

    /// Registers resources and systems. Runs once, before the window and RHI exist.
    fn build(&self, app: &mut App);
}

/// Anything `App::add_plugins` accepts: one plugin, a tuple of them, or a group.
pub trait Plugins {
    fn add_to(self, app: &mut App);
}

impl<P: Plugin> Plugins for P {
    fn add_to(self, app: &mut App) {
        app.push_plugin(Box::new(self));
    }
}

macro_rules! impl_plugins_tuple {
    ($($p:ident),+) => {
        impl<$($p: Plugins),+> Plugins for ($($p,)+) {
            #[allow(non_snake_case)]
            fn add_to(self, app: &mut App) {
                let ($($p,)+) = self;
                $($p.add_to(app);)+
            }
        }
    };
}
impl_plugins_tuple!(A);
impl_plugins_tuple!(A, B);
impl_plugins_tuple!(A, B, C);
impl_plugins_tuple!(A, B, C, D);
impl_plugins_tuple!(A, B, C, D, E);
impl_plugins_tuple!(A, B, C, D, E, F);
impl_plugins_tuple!(A, B, C, D, E, F, G);
impl_plugins_tuple!(A, B, C, D, E, F, G, H);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    #[error("plugin '{0}' was added twice")]
    Duplicate(&'static str),
    #[error("plugin '{plugin}' depends on '{dependency}', which was not added")]
    MissingDependency {
        plugin: &'static str,
        dependency: &'static str,
    },
    #[error("plugin dependency cycle among: {}", .0.join(", "))]
    Cycle(Vec<&'static str>),
    #[error("plugin '{0}' added more plugins from build(); add them from the app instead")]
    AddedDuringBuild(&'static str),
}

/// Returns plugin indices in build order: dependencies first, otherwise insertion order.
pub(crate) fn build_order(
    plugins: &[(&'static str, &'static [&'static str])],
) -> Result<Vec<usize>, PluginError> {
    let index_of = |name: &str| plugins.iter().position(|(n, _)| *n == name);
    for (i, (name, deps)) in plugins.iter().enumerate() {
        if index_of(name) != Some(i) {
            return Err(PluginError::Duplicate(name));
        }
        if let Some(dep) = deps.iter().find(|d| index_of(d).is_none()) {
            return Err(PluginError::MissingDependency {
                plugin: name,
                dependency: dep,
            });
        }
    }

    let mut order = Vec::with_capacity(plugins.len());
    let mut done = vec![false; plugins.len()];
    while order.len() < plugins.len() {
        // First not-yet-built plugin whose dependencies are all built (keeps insertion order).
        let next = (0..plugins.len()).find(|&i| {
            !done[i]
                && plugins[i]
                    .1
                    .iter()
                    .all(|d| index_of(d).is_some_and(|j| done[j]))
        });
        match next {
            Some(i) => {
                done[i] = true;
                order.push(i);
            }
            None => {
                let stuck = (0..plugins.len())
                    .filter(|&i| !done[i])
                    .map(|i| plugins[i].0)
                    .collect();
                return Err(PluginError::Cycle(stuck));
            }
        }
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependencies_build_first_otherwise_insertion_order() {
        let p = [
            ("game", &["render", "input"][..]),
            ("input", &[][..]),
            ("render", &["transform"][..]),
            ("transform", &[][..]),
        ];
        let order: Vec<_> = build_order(&p).unwrap().iter().map(|&i| p[i].0).collect();
        assert_eq!(order, ["input", "transform", "render", "game"]);
    }

    #[test]
    fn missing_dependency_is_named() {
        let p = [("game", &["physics"][..])];
        assert_eq!(
            build_order(&p),
            Err(PluginError::MissingDependency {
                plugin: "game",
                dependency: "physics"
            })
        );
    }

    #[test]
    fn cycles_and_duplicates_are_errors() {
        let p = [("a", &["b"][..]), ("b", &["a"][..]), ("c", &[][..])];
        assert_eq!(build_order(&p), Err(PluginError::Cycle(vec!["a", "b"])));
        let p = [("a", &[][..]), ("a", &[][..])];
        assert_eq!(build_order(&p), Err(PluginError::Duplicate("a")));
    }
}
