const TIMESTAMP_BYTES: u64 = 8;
/// Bytes a frame's timing takes in its readback buffer: a start and an end timestamp.
pub(crate) const TIMING_BYTES: u64 = 2 * TIMESTAMP_BYTES;

/// Times a frame's passes, as one, on the GPU, with timestamp queries. Only
/// exists on devices with the `TIMESTAMP_QUERY` feature.
///
/// One query set serves every frame: each resolves it into its own readback
/// buffer before the next frame's pass writes it again, in queue order.
pub(crate) struct GpuTimer {
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    /// Nanoseconds per timestamp tick.
    period: f64,
}

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        Some(Self {
            query_set: device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("frame timing"),
                ty: wgpu::QueryType::Timestamp,
                count: 2,
            }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frame timing"),
                size: TIMING_BYTES,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            period: f64::from(queue.get_timestamp_period()),
        })
    }

    /// The timestamps a pass writes when it's the first and/or the last of
    /// the passes timed as one.
    pub fn writes(&self, first: bool, last: bool) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: first.then_some(0),
            end_of_pass_write_index: last.then_some(1),
        }
    }

    /// Encodes copying the pass's timestamps into `buffer` at `offset`.
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder, buffer: &wgpu::Buffer, offset: u64) {
        encoder.resolve_query_set(&self.query_set, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, buffer, offset, TIMING_BYTES);
    }

    /// The seconds between the timestamps `resolve` copied, as read back.
    pub fn seconds(&self, bytes: &[u8]) -> Option<f64> {
        let stamp = |i: usize| {
            let bytes = bytes.get(i * 8..i * 8 + 8)?;
            Some(u64::from_le_bytes(bytes.try_into().ok()?))
        };
        let (start, end) = (stamp(0)?, stamp(1)?);
        // Some drivers write timestamps out of order across a pass.
        (end > start).then(|| (end - start) as f64 * self.period / 1e9)
    }
}
