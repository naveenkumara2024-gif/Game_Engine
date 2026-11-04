//! File watching for asset hot reload (plan 0005, decision 10). `notify` calls back on its own
//! thread; changed paths are forwarded over a channel and handed out after a quiet period, so
//! a half-written export never gets imported.

use crossbeam_channel::Receiver;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

/// Exporters (Blender, image editors) often write several times per save.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

pub struct AssetWatcher {
    _watcher: notify::RecommendedWatcher,
    rx: Receiver<PathBuf>,
    pending: BTreeSet<PathBuf>,
    last_event: Option<Instant>,
}

impl std::fmt::Debug for AssetWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetWatcher")
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl AssetWatcher {
    pub fn new(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    )
                {
                    for p in event.paths {
                        // Our own atomic writes (`*.tmpNNN`) are noise.
                        let tmp = p
                            .extension()
                            .is_some_and(|e| e.to_string_lossy().contains("tmp"));
                        if !tmp {
                            let _ = tx.send(p);
                        }
                    }
                }
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
            pending: BTreeSet::new(),
            last_event: None,
        })
    }

    /// The set of changed files once a burst has settled; empty otherwise.
    pub fn poll(&mut self, now: Instant) -> Vec<PathBuf> {
        while let Ok(p) = self.rx.try_recv() {
            self.pending.insert(p);
            self.last_event = Some(now);
        }
        match self.last_event {
            Some(t) if now.duration_since(t) >= DEBOUNCE => {
                self.last_event = None;
                std::mem::take(&mut self.pending).into_iter().collect()
            }
            _ => Vec::new(),
        }
    }
}
