//! Tasks (lore §2.5, §3.6): a few long-lived **IO threads** for blocking file work and a
//! **work-stealing pool** (`rayon`) for CPU work. Results come back as [`Task<T>`]s that the
//! main thread *polls* each frame — there is no async runtime and nothing ever blocks the frame
//! loop (CLAUDE.md §7). The render thread joins this crate at M5.

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, TryRecvError};
use tracing::{debug, info_span};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A result that will arrive later. Poll it; never wait on it from the frame loop.
#[derive(Debug)]
pub struct Task<T> {
    rx: Receiver<std::thread::Result<T>>,
    done: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll<T> {
    Ready(T),
    Pending,
    /// The job panicked (the panic is caught and logged; the pool keeps running).
    Panicked,
}

impl<T> Task<T> {
    pub fn poll(&mut self) -> Poll<T> {
        if self.done {
            return Poll::Pending;
        }
        match self.rx.try_recv() {
            Ok(Ok(v)) => {
                self.done = true;
                Poll::Ready(v)
            }
            Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                self.done = true;
                Poll::Panicked
            }
            Err(TryRecvError::Empty) => Poll::Pending,
        }
    }

    /// Blocks until the result arrives. **Tests and tools only.**
    pub fn wait(self) -> Option<T> {
        self.rx.recv().ok().and_then(Result::ok)
    }
}

fn run_caught<T: Send + 'static>(
    name: &'static str,
    f: impl FnOnce() -> T + Send + 'static,
    tx: Sender<std::thread::Result<T>>,
) -> impl FnOnce() + Send + 'static {
    move || {
        let _span = info_span!("Task", name).entered();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if result.is_err() {
            tracing::error!(task = name, "task panicked");
        }
        let _ = tx.send(result);
    }
}

struct Inner {
    io_tx: Option<Sender<Job>>,
    io_threads: Vec<JoinHandle<()>>,
    compute: rayon::ThreadPool,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Closing the channel ends the IO threads after their current job.
        self.io_tx = None;
        for t in self.io_threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Cheap to clone; all clones share the same threads.
#[derive(Clone)]
pub struct Tasks {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Tasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tasks")
            .field("io_threads", &self.inner.io_threads.len())
            .field("compute_threads", &self.inner.compute.current_num_threads())
            .finish()
    }
}

impl Default for Tasks {
    fn default() -> Self {
        Self::new(2, None)
    }
}

impl Tasks {
    /// `io_threads` blocking-IO threads and a compute pool (`None` = one thread per core,
    /// minus one for the main thread).
    pub fn new(io_threads: usize, compute_threads: Option<usize>) -> Self {
        let (io_tx, io_rx) = crossbeam_channel::unbounded::<Job>();
        let io_threads = (0..io_threads.max(1))
            .map(|i| {
                let rx = io_rx.clone();
                std::thread::Builder::new()
                    .name(format!("IO {i}"))
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            job();
                        }
                    })
                    .expect("spawn IO thread")
            })
            .collect();
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let compute = rayon::ThreadPoolBuilder::new()
            .num_threads(compute_threads.unwrap_or(cores.saturating_sub(1).max(1)))
            .thread_name(|i| format!("Worker {i}"))
            .build()
            .expect("build compute pool");
        debug!(cores, "task pools started");
        Self {
            inner: Arc::new(Inner {
                io_tx: Some(io_tx),
                io_threads,
                compute,
            }),
        }
    }

    /// Runs blocking IO (file reads, directory scans) on an IO thread.
    pub fn io<T: Send + 'static>(
        &self,
        name: &'static str,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Task<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if let Some(io) = &self.inner.io_tx {
            let _ = io.send(Box::new(run_caught(name, f, tx)));
        }
        Task { rx, done: false }
    }

    /// Runs CPU work (decoding, importing) on the compute pool.
    pub fn compute<T: Send + 'static>(
        &self,
        name: &'static str,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Task<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.inner.compute.spawn(run_caught(name, f, tx));
        Task { rx, done: false }
    }

    /// Data-parallel helper on the compute pool (e.g. mip levels, mesh primitives).
    pub fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        self.inner.compute.install(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_ready<T>(task: &mut Task<T>) -> Poll<T> {
        let start = Instant::now();
        loop {
            match task.poll() {
                Poll::Pending if start.elapsed() < Duration::from_secs(5) => {
                    std::thread::yield_now();
                }
                other => return other,
            }
        }
    }

    #[test]
    fn io_and_compute_return_results_without_blocking() {
        let tasks = Tasks::new(2, Some(2));
        let (gate_tx, gate_rx) = crossbeam_channel::bounded::<()>(0);
        let mut slow = tasks.io("slow", move || {
            gate_rx.recv().ok();
            7
        });
        // Polling returns immediately while the job is still waiting.
        assert_eq!(slow.poll(), Poll::Pending);
        let mut fast = tasks.compute("fast", || 6 * 7);
        assert_eq!(wait_ready(&mut fast), Poll::Ready(42));
        gate_tx.send(()).unwrap();
        assert_eq!(wait_ready(&mut slow), Poll::Ready(7));
        assert_eq!(slow.poll(), Poll::Pending, "a task yields its value once");
    }

    #[test]
    fn panics_are_contained() {
        let tasks = Tasks::new(1, Some(1));
        let mut bad = tasks.compute("bad", || -> u32 { panic!("boom") });
        assert_eq!(wait_ready(&mut bad), Poll::Panicked);
        // The pool still works.
        assert_eq!(tasks.compute("ok", || 1).wait(), Some(1));
        assert_eq!(tasks.io("ok", || 2).wait(), Some(2));
    }
}
