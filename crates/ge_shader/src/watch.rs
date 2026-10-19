//! File watching for shader hot reload. `notify` calls back on its own thread; we only forward
//! a "something changed" ping over a channel and let the main thread decide when to recompile.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

/// Editors often write a file several times per save (truncate, write, rename). Waiting for a
/// quiet period avoids compiling a half-written file.
const DEBOUNCE: Duration = Duration::from_millis(100);

pub(crate) struct ShaderWatcher {
    _watcher: notify::RecommendedWatcher,
    rx: Receiver<()>,
    debounce: Debounce,
}

impl std::fmt::Debug for ShaderWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaderWatcher").finish_non_exhaustive()
    }
}

impl ShaderWatcher {
    pub(crate) fn new(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && is_shader_edit(&event)
                {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
            debounce: Debounce::default(),
        })
    }

    pub(crate) fn poll(&mut self, now: Instant) -> bool {
        while self.rx.try_recv().is_ok() {
            self.debounce.event(now);
        }
        self.debounce.ready(now)
    }
}

fn is_shader_edit(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event.paths.iter().any(|p| {
        p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("wesl") || ext.eq_ignore_ascii_case("wgsl"))
    })
}

/// Fires once, [`DEBOUNCE`] after the last event of a burst.
#[derive(Default, Debug)]
struct Debounce {
    last: Option<Instant>,
}

impl Debounce {
    fn event(&mut self, now: Instant) {
        self.last = Some(now);
    }

    fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.duration_since(t) >= DEBOUNCE => {
                self.last = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_fires_once_after_quiet_period() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        assert!(!d.ready(t0));
        d.event(t0);
        d.event(t0 + Duration::from_millis(50));
        assert!(
            !d.ready(t0 + Duration::from_millis(100)),
            "burst still active"
        );
        assert!(d.ready(t0 + Duration::from_millis(150)));
        assert!(!d.ready(t0 + Duration::from_millis(300)), "fires only once");
    }

    #[test]
    fn only_shader_files_count() {
        let mk = |p: &str| notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: vec![p.into()],
            attrs: Default::default(),
        };
        assert!(is_shader_edit(&mk("shaders/mesh.wesl")));
        assert!(is_shader_edit(&mk("shaders/x.WGSL")));
        assert!(!is_shader_edit(&mk("shaders/mesh.wesl~")));
        assert!(!is_shader_edit(&mk("shaders/notes.txt")));
    }
}
