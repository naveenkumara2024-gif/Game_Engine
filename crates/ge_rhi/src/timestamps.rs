//! Per-pass GPU timestamps (law 10, lore §6.7 "GPU timestamps per pass in the profiler").
//!
//! Each frame in flight gets a slot: a query set, a resolve buffer and a mappable readback
//! buffer. Results come back a few frames later through `map_async` + a non-blocking
//! `device.poll`, so the main thread never waits on the GPU. If a slot is still in flight when
//! its turn comes round again, that frame is simply not measured.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tracing::{info, warn};

/// Max passes measured per frame; extra passes run unmeasured.
const MAX_PASSES: u32 = 32;
/// Readback ring depth: frames in flight (2) + one being read.
const SLOTS: usize = 3;

const MAP_PENDING: u8 = 0;
const MAP_DONE: u8 = 1;
const MAP_FAILED: u8 = 2;

#[derive(Clone, Debug, PartialEq)]
pub struct PassTiming {
    pub name: &'static str,
    pub ms: f32,
}

/// GPU time of one completed frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuTimings {
    pub passes: Vec<PassTiming>,
    /// Sum of all measured passes.
    pub total_ms: f32,
}

pub(crate) struct GpuProfiler {
    slots: Vec<Slot>,
    frame: usize,
    period_ns: f32,
    latest: Option<GpuTimings>,
    #[cfg(feature = "tracy")]
    tracy: Option<tracy_client::GpuContext>,
}

struct Slot {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    pending: Option<Pending>,
}

struct Pending {
    names: Vec<&'static str>,
    state: Arc<AtomicU8>,
    #[cfg(feature = "tracy")]
    spans: Vec<tracy_client::GpuSpan>,
}

/// The query slot a [`crate::Frame`] writes into.
pub(crate) struct FrameQueries {
    slot: usize,
    query_set: wgpu::QuerySet,
    names: Vec<&'static str>,
    #[cfg(feature = "tracy")]
    tracy: Option<tracy_client::GpuContext>,
    #[cfg(feature = "tracy")]
    spans: Vec<tracy_client::GpuSpan>,
}

impl GpuProfiler {
    /// `None` if the device was created without `TIMESTAMP_QUERY`.
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            warn!("adapter lacks TIMESTAMP_QUERY; GPU pass timings disabled");
            return None;
        }
        let bytes = u64::from(MAX_PASSES * 2) * wgpu::QUERY_SIZE as u64;
        let slots = (0..SLOTS)
            .map(|i| Slot {
                query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some(&format!("GpuTimestamps[{i}]")),
                    ty: wgpu::QueryType::Timestamp,
                    count: MAX_PASSES * 2,
                }),
                resolve: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("GpuTimestamps::Resolve"),
                    size: bytes,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("GpuTimestamps::Readback"),
                    size: bytes,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                pending: None,
            })
            .collect();
        let period_ns = queue.get_timestamp_period();
        info!(period_ns, "GPU timestamps enabled");
        Some(Self {
            slots,
            frame: 0,
            period_ns,
            latest: None,
            #[cfg(feature = "tracy")]
            tracy: tracy::create_context(device, queue, period_ns),
        })
    }

    pub(crate) fn latest(&self) -> Option<&GpuTimings> {
        self.latest.as_ref()
    }

    /// Harvests every slot whose readback finished. Call after a non-blocking device poll.
    pub(crate) fn collect(&mut self) {
        for slot in &mut self.slots {
            let Some(pending) = &slot.pending else {
                continue;
            };
            match pending.state.load(Ordering::Acquire) {
                MAP_PENDING => continue,
                MAP_DONE => {
                    let Some(pending) = slot.pending.take() else {
                        continue;
                    };
                    let count = pending.names.len() * 2;
                    let bytes = (count * wgpu::QUERY_SIZE as usize) as u64;
                    let ticks: Option<Vec<u64>> = slot
                        .readback
                        .slice(..bytes)
                        .get_mapped_range()
                        .ok()
                        .map(|view| bytemuck::pod_collect_to_vec(&view));
                    slot.readback.unmap();
                    let Some(ticks) = ticks else {
                        warn!("GPU timestamp buffer not readable; dropping one frame of timings");
                        continue;
                    };
                    let timings = to_timings(&pending.names, &ticks, self.period_ns);
                    #[cfg(feature = "tracy")]
                    for (span, pair) in pending.spans.iter().zip(ticks.chunks_exact(2)) {
                        span.upload_timestamp_start(pair[0] as i64);
                        span.upload_timestamp_end(pair[1] as i64);
                    }
                    self.latest = Some(timings);
                }
                _ => {
                    warn!("GPU timestamp readback failed; dropping one frame of timings");
                    slot.pending = None;
                }
            }
        }
    }

    /// Hands out this frame's slot, or `None` if it is still in flight.
    pub(crate) fn begin_frame(&mut self) -> Option<FrameQueries> {
        let slot = self.frame % SLOTS;
        self.frame += 1;
        if self.slots[slot].pending.is_some() {
            return None;
        }
        Some(FrameQueries {
            slot,
            query_set: self.slots[slot].query_set.clone(),
            names: Vec::new(),
            #[cfg(feature = "tracy")]
            tracy: self.tracy.clone(),
            #[cfg(feature = "tracy")]
            spans: Vec::new(),
        })
    }

    /// Records the resolve + copy into `encoder`. Returns the slot to map after submit.
    pub(crate) fn end_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        queries: FrameQueries,
    ) -> Option<usize> {
        if queries.names.is_empty() {
            return None;
        }
        let slot = &mut self.slots[queries.slot];
        let count = queries.names.len() as u32 * 2;
        let bytes = u64::from(count) * wgpu::QUERY_SIZE as u64;
        encoder.resolve_query_set(&slot.query_set, 0..count, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.readback, 0, bytes);
        slot.pending = Some(Pending {
            names: queries.names,
            state: Arc::new(AtomicU8::new(MAP_PENDING)),
            #[cfg(feature = "tracy")]
            spans: queries.spans,
        });
        Some(queries.slot)
    }

    /// Starts the async readback. Must run after the frame's commands were submitted.
    pub(crate) fn after_submit(&mut self, slot: usize) {
        let slot = &self.slots[slot];
        let Some(pending) = &slot.pending else { return };
        let count = pending.names.len() * 2;
        let bytes = (count * wgpu::QUERY_SIZE as usize) as u64;
        let state = pending.state.clone();
        slot.readback
            .map_async(wgpu::MapMode::Read, ..bytes, move |result| {
                let s = if result.is_ok() { MAP_DONE } else { MAP_FAILED };
                state.store(s, Ordering::Release);
            });
    }
}

impl FrameQueries {
    /// Reserves a begin/end query pair for a pass. `None` once the frame is out of queries.
    pub(crate) fn pass_writes(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let index = self.names.len() as u32;
        if index >= MAX_PASSES {
            return None;
        }
        self.names.push(label);
        #[cfg(feature = "tracy")]
        if let Some(ctx) = &self.tracy
            && let Ok(span) = ctx.span_alloc(label, "render_pass", file!(), line!())
        {
            self.spans.push(span);
        }
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(index * 2),
            end_of_pass_write_index: Some(index * 2 + 1),
        })
    }

    /// Marks the CPU-side end of the most recent pass for Tracy.
    pub(crate) fn end_pass(&mut self) {
        #[cfg(feature = "tracy")]
        if let Some(span) = self.spans.last_mut() {
            span.end_zone();
        }
    }
}

/// Converts raw begin/end tick pairs to milliseconds. Pairs where the end precedes the begin
/// (seen on some drivers after a power-state change) count as zero.
fn to_timings(names: &[&'static str], ticks: &[u64], period_ns: f32) -> GpuTimings {
    let passes: Vec<PassTiming> = names
        .iter()
        .zip(ticks.chunks_exact(2))
        .map(|(&name, pair)| PassTiming {
            name,
            ms: (pair[1].saturating_sub(pair[0]) as f64 * f64::from(period_ns) / 1e6) as f32,
        })
        .collect();
    let total_ms = passes.iter().map(|p| p.ms).sum();
    GpuTimings { passes, total_ms }
}

#[cfg(feature = "tracy")]
mod tracy {
    //! Tracy GPU context. Calibrated once with a blocking timestamp readback at startup,
    //! which is the only place we ever wait on the GPU for timings.

    use tracing::warn;

    pub(super) fn create_context(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        period_ns: f32,
    ) -> Option<tracy_client::GpuContext> {
        let client = tracy_client::Client::running()?;
        let now = read_gpu_timestamp(device, queue)?;
        let ty = match device.adapter_info().backend {
            wgpu::Backend::Vulkan => tracy_client::GpuContextType::Vulkan,
            wgpu::Backend::Dx12 => tracy_client::GpuContextType::Direct3D12,
            _ => tracy_client::GpuContextType::Invalid,
        };
        client
            .new_gpu_context(Some("GPU"), ty, now as i64, period_ns)
            .inspect_err(|e| warn!(error = %e, "could not create Tracy GPU context"))
            .ok()
    }

    /// Writes one timestamp from an empty compute pass and waits for it.
    fn read_gpu_timestamp(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<u64> {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("TracyCalibration"),
            ty: wgpu::QueryType::Timestamp,
            count: 1,
        });
        let size = wgpu::QUERY_SIZE as u64;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TracyCalibration::Resolve"),
            size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TracyCalibration::Readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("TracyCalibration"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &set,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: None,
            }),
        });
        encoder.resolve_query_set(&set, 0..1, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, size);
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let value = {
            let view = readback.get_mapped_range(..).ok()?;
            bytemuck::pod_read_unaligned::<u64>(&view[..8])
        };
        readback.unmap();
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_to_ms() {
        let t = to_timings(&["A", "B"], &[1_000, 2_000_000, 2_000_000, 1_000], 1.0);
        assert_eq!(t.passes.len(), 2);
        assert!((t.passes[0].ms - 1.999).abs() < 1e-3);
        assert_eq!(t.passes[1].ms, 0.0, "end before begin clamps to zero");
        assert!((t.total_ms - 1.999).abs() < 1e-3);
    }

    #[test]
    fn period_scales_ticks() {
        // 1e6 ticks at 2 ns/tick = 2 ms.
        let t = to_timings(&["A"], &[0, 1_000_000], 2.0);
        assert!((t.total_ms - 2.0).abs() < 1e-6);
    }
}
