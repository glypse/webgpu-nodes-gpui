//! The images project assets give, decoded on rayon's pool and uploaded to the
//! GPU by `poll` as they finish, so decoding never stalls a frame; and the
//! videos, whose frames are picked and uploaded as the times asked for move.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use shader_graph::assets::{AssetKind, VideoPlayback, video_time};
use shader_graph::passes::Size;

use super::video::{DecodedFrame, VideoStream};
use super::{DecodedImage, MediaError, decode_image};

/// Video views nothing has drawn for this long drop their texture.
const IDLE_VIEW: Duration = Duration::from_secs(10);
/// The format video frames are uploaded in: sRGB-encoded BGRA, which samples linear.
const VIDEO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

/// A project asset's file, with the hash that identifies its content.
#[derive(Clone)]
pub struct MediaSource {
    pub kind: AssetKind,
    /// SHA-256 of the file, hex. Assets sharing a hash share one texture.
    pub hash: String,
    pub bytes: Arc<Vec<u8>>,
}

/// Where an asset's image is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaStatus {
    /// The asset has no file in the library (it was never given one, or no program reads it).
    Missing,
    Loading,
    Ready(Size),
    /// Why the image can't be used: it stands in as transparent.
    Failed(String),
}

enum Entry {
    Decoding,
    Ready { texture: wgpu::Texture, size: Size },
    Failed(String),
    Video(VideoStream),
}

/// The texture one video source shows, and the frame it holds.
struct View {
    texture: Option<wgpu::Texture>,
    shown: Option<u64>,
    used: Instant,
}

struct Decoded {
    hash: String,
    result: Result<DecodedImage, MediaError>,
}

type Waker = Arc<dyn Fn() + Send + Sync>;

/// Textures of the images programs read, keyed by the content hash of their
/// file. Only what `sync` names is kept: an image no asset reads any more is
/// dropped with its texture.
pub struct MediaLibrary {
    device: wgpu::Device,
    queue: wgpu::Queue,
    max_dimension: u32,
    /// The content hash each asset reads.
    assets: HashMap<String, String>,
    entries: HashMap<String, Entry>,
    views: HashMap<String, View>,
    decoded: Sender<Decoded>,
    finished: Receiver<Decoded>,
    waker: Option<Waker>,
}

impl MediaLibrary {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let (decoded, finished) = mpsc::channel();
        Self {
            device: device.clone(),
            queue: queue.clone(),
            max_dimension: device.limits().max_texture_dimension_2d,
            assets: HashMap::new(),
            entries: HashMap::new(),
            views: HashMap::new(),
            decoded,
            finished,
            waker: None,
        }
    }

    /// Called from the decoding threads when an image finishes, so the owner polls.
    pub fn set_waker(&mut self, waker: impl Fn() + Send + Sync + 'static) {
        self.waker = Some(Arc::new(waker));
    }

    /// Makes the library read exactly `sources`: decodes the images it doesn't
    /// have yet and drops those no source reads any more. Decoding finishes in
    /// `poll`.
    pub fn sync(&mut self, sources: &BTreeMap<String, MediaSource>) {
        self.assets = sources
            .iter()
            .map(|(asset, source)| (asset.clone(), source.hash.clone()))
            .collect();
        let live: HashSet<&String> = self.assets.values().collect();
        self.entries.retain(|hash, _| live.contains(hash));
        let mut new = Vec::new();
        for source in sources.values() {
            if self.entries.contains_key(&source.hash) {
                continue;
            }
            if source.kind == AssetKind::Video {
                let stream = VideoStream::open(source.bytes.clone(), self.waker.clone());
                self.entries
                    .insert(source.hash.clone(), Entry::Video(stream));
            } else {
                self.entries.insert(source.hash.clone(), Entry::Decoding);
                new.push((source.hash.clone(), source.bytes.clone()));
            }
        }
        for (hash, bytes) in new {
            self.spawn_decode(hash, bytes);
        }
    }

    fn spawn_decode(&self, hash: String, bytes: Arc<Vec<u8>>) {
        let decoded = self.decoded.clone();
        let waker = self.waker.clone();
        let max_dimension = self.max_dimension;
        rayon::spawn(move || {
            // A panic here would abort the process from the pool, so it's
            // reported as a failed image like any other.
            let result = catch_unwind(AssertUnwindSafe(|| decode_image(&bytes, max_dimension)))
                .unwrap_or_else(|_| Err(MediaError::Decode("the decoder failed".to_owned())));
            // The library may have been dropped meanwhile; nothing then reads the result.
            let _ = decoded.send(Decoded { hash, result });
            if let Some(waker) = waker {
                waker();
            }
        });
    }

    /// Uploads the images decoded since the last call. Returns whether any
    /// image became ready or failed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(Decoded { hash, result }) = self.finished.try_recv() {
            if !matches!(self.entries.get(&hash), Some(Entry::Decoding)) {
                continue;
            }
            let entry = match result {
                Ok(image) => Entry::Ready {
                    texture: self.upload(&image),
                    size: image.size,
                },
                Err(error) => {
                    log::error!("[media] can't decode an image: {error}");
                    Entry::Failed(format!("An image can't be used. {error}"))
                }
            };
            self.entries.insert(hash, entry);
            changed = true;
        }
        changed
    }

    pub fn status(&self, asset: &str) -> MediaStatus {
        let Some(hash) = self.assets.get(asset) else {
            return MediaStatus::Missing;
        };
        match self.entries.get(hash) {
            None | Some(Entry::Decoding) => MediaStatus::Loading,
            Some(Entry::Ready { size, .. }) => MediaStatus::Ready(*size),
            Some(Entry::Failed(message)) => MediaStatus::Failed(message.clone()),
            Some(Entry::Video(stream)) => match stream.info() {
                Ok(None) => MediaStatus::Loading,
                Ok(Some(info)) => MediaStatus::Ready(Size::new(info.width, info.height)),
                Err(message) => MediaStatus::Failed(format!("A video can't be used. {message}")),
            },
        }
    }

    /// The texture of an asset's image, once it is ready.
    pub fn texture(&self, asset: &str) -> Option<&wgpu::Texture> {
        match self.entries.get(self.assets.get(asset)?)? {
            Entry::Ready { texture, .. } => Some(texture),
            Entry::Decoding | Entry::Failed(_) | Entry::Video(_) => None,
        }
    }

    /// The texture of a video view at project time `time`: the frame that a
    /// video played by `playback` shows then, uploaded only when it's a frame
    /// the view doesn't hold yet. `None` while nothing shows or no frame is
    /// decoded. `view` names the node showing the video, and each view keeps
    /// its own texture.
    pub fn video_texture(
        &mut self,
        asset: &str,
        view: &str,
        playback: &VideoPlayback,
        time: f64,
    ) -> Option<wgpu::Texture> {
        let now = Instant::now();
        self.views
            .retain(|_, v| now.duration_since(v.used) < IDLE_VIEW);
        let Self {
            assets,
            entries,
            views,
            device,
            queue,
            ..
        } = self;
        let Some(Entry::Video(stream)) = assets.get(asset).and_then(|hash| entries.get(hash))
        else {
            return None;
        };
        let Ok(Some(info)) = stream.info() else {
            return None;
        };
        let t = video_time(playback, time, info.duration)?;
        let frame = stream.frame_at(view, t);
        let entry = views.entry(view.to_owned()).or_insert(View {
            texture: None,
            shown: None,
            used: now,
        });
        entry.used = now;
        if let Some(frame) = frame.filter(|frame| entry.shown != Some(frame.seq)) {
            entry.texture = Some(upload_frame(
                device,
                queue,
                entry.texture.take(),
                &frame.decoded,
            ));
            entry.shown = Some(frame.seq);
        }
        entry.texture.clone()
    }

    fn upload(&self, image: &DecodedImage) -> wgpu::Texture {
        let size = image.size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("media"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            texture.as_image_copy(),
            &image.texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 8),
                rows_per_image: None,
            },
            texture.size(),
        );
        texture
    }
}

/// Writes a frame into `reuse` when that is the frame's size, else into a new texture.
fn upload_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    reuse: Option<wgpu::Texture>,
    frame: &DecodedFrame,
) -> wgpu::Texture {
    let size = frame.size;
    let texture = match reuse {
        Some(texture) if texture.width() == size.width && texture.height() == size.height => {
            texture
        }
        _ => device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: VIDEO_FORMAT,
            // COPY_SRC lets a frame be read back, as the renderer does for exports and tests.
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }),
    };
    queue.write_texture(
        texture.as_image_copy(),
        &frame.bgra,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.width * 4),
            rows_per_image: None,
        },
        texture.size(),
    );
    texture
}
