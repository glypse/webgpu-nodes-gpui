//! Video files decoded on a thread of their own. A stream keeps the latest
//! frames decoded and picks, for each time asked, the frame showing then; the
//! decoding itself is behind `VideoDecoder`, so another platform can add one.

use std::collections::{BTreeMap, VecDeque};
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use shader_graph::assets::VideoInfo;
use tempfile::{NamedTempFile, TempPath};

use super::MediaError;
use crate::Size;

/// Frames kept for the times just before those asked for, so a view showing the frame before another's costs no decode.
const RECENT_FRAMES: usize = 4;
/// How far ahead of the last frame decoded a time must be before decoding seeks there rather than reading on.
const SEEK_AHEAD_SECONDS: f64 = 2.0;

/// A frame as a decoder produces it: BGRA pixels, rows tight and top first.
pub struct DecodedFrame {
    /// Video seconds at which the frame shows.
    pub time: f64,
    /// Seconds it shows for; positive.
    pub duration: f64,
    pub size: Size,
    pub bgra: Vec<u8>,
}

/// A decoded frame as the stream keeps it, numbered so a view can tell it has been uploaded.
pub struct Frame {
    pub seq: u64,
    pub decoded: DecodedFrame,
}

/// Decodes one video file, reading its frames in presentation order from a time.
pub trait VideoDecoder {
    fn info(&self) -> VideoInfo;
    /// Makes the next frame read the one showing at `time` or the closest after it.
    fn seek(&mut self, time: f64) -> Result<(), MediaError>;
    /// The next frame, or `None` at the end of the video.
    fn next_frame(&mut self) -> Result<Option<DecodedFrame>, MediaError>;
}

/// Opens the video file at `path`.
pub fn open_decoder(path: &Path) -> Result<Box<dyn VideoDecoder>, MediaError> {
    Ok(Box::new(super::ffmpeg_decoder::FfmpegDecoder::open(path)?))
}

/// Reads a video's metadata, as an import records it.
pub struct SystemVideoProbe;

impl shader_graph::assets::VideoProbe for SystemVideoProbe {
    fn probe(&self, bytes: &[u8]) -> Result<VideoInfo, String> {
        let file = TempVideo::write(bytes).map_err(|error| error.to_string())?;
        let decoder = open_decoder(file.path()).map_err(|error| error.to_string())?;
        Ok(decoder.info())
    }
}

type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    info: Option<VideoInfo>,
    failure: Option<String>,
    /// The video time each view wants decoded, by view id.
    wanted: BTreeMap<String, f64>,
    frames: VecDeque<Arc<Frame>>,
    /// How many frames are kept: more when several views want frames at once.
    capacity: usize,
    next_seq: u64,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fail(&self, message: String) {
        log::error!("[media] can't decode a video: {message}");
        self.lock().failure = Some(message);
    }

    /// The times to decode next, earliest first; `None` once the stream is closed.
    fn next_wanted(&self) -> Option<Vec<f64>> {
        let mut state = self.lock();
        while state.wanted.is_empty() && !state.closed {
            state = self.changed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        if state.closed {
            return None;
        }
        let mut times: Vec<f64> = std::mem::take(&mut state.wanted).into_values().collect();
        times.sort_by(f64::total_cmp);
        state.capacity = RECENT_FRAMES.max(times.len() + 1);
        Some(times)
    }

    fn closed(&self) -> bool {
        self.lock().closed
    }

    fn covered(&self, time: f64) -> bool {
        self.lock().frames.iter().any(|frame| covers(&frame.decoded, time))
    }

    fn push(&self, decoded: DecodedFrame, waker: Option<&Waker>) {
        {
            let mut state = self.lock();
            let seq = state.next_seq;
            state.next_seq += 1;
            state.frames.push_back(Arc::new(Frame { seq, decoded }));
            while state.frames.len() > state.capacity {
                state.frames.pop_front();
            }
        }
        if let Some(waker) = waker {
            waker();
        }
    }
}

fn covers(frame: &DecodedFrame, time: f64) -> bool {
    frame.time <= time && time < frame.time + frame.duration
}

/// One video file, decoding on a thread of its own, shared by every view showing it.
pub struct VideoStream {
    shared: Arc<Shared>,
}

impl VideoStream {
    /// Starts decoding `bytes`, a video file's contents. `waker` is called when a frame arrives or the video fails.
    pub fn open(bytes: Arc<Vec<u8>>, waker: Option<Waker>) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State { capacity: RECENT_FRAMES, ..State::default() }),
            changed: Condvar::new(),
        });
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new().name("video-decode".to_owned()).spawn(move || {
            if let Err(message) = decode(&bytes, &thread_shared, waker.as_ref()) {
                log::error!("[media] can't decode a video: {message}");
                thread_shared.lock().failure = Some(message);
                if let Some(waker) = &waker {
                    waker();
                }
            }
        });
        if let Err(error) = spawned {
            shared.fail(format!("the decoding thread couldn't start: {error}"));
        }
        Self { shared }
    }

    /// The video's size and length once it's open; `None` while it opens.
    pub fn info(&self) -> Result<Option<VideoInfo>, String> {
        let state = self.shared.lock();
        match (&state.failure, state.info) {
            (Some(failure), _) => Err(failure.clone()),
            (None, info) => Ok(info),
        }
    }

    /// The frame showing at video time `time`, or else the closest one read before it, or the first one read.
    /// Asks the decoder for `time` when no frame covers it yet, for `view`.
    pub fn frame_at(&self, view: &str, time: f64) -> Option<Arc<Frame>> {
        let mut state = self.shared.lock();
        if !state.frames.iter().any(|frame| covers(&frame.decoded, time)) {
            state.wanted.insert(view.to_owned(), time);
            self.shared.changed.notify_all();
        }
        let covering = state.frames.iter().find(|frame| covers(&frame.decoded, time));
        let before = state.frames.iter().rev().find(|frame| frame.decoded.time <= time);
        covering.or(before).or(state.frames.front()).cloned()
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.changed.notify_all();
    }
}

/// Where a decoder's reading stands: the start of the last frame it read.
#[derive(Default)]
struct Position {
    last_time: Option<f64>,
}

/// Decodes until the stream closes, or fails with why the video can't be read.
fn decode(bytes: &[u8], shared: &Shared, waker: Option<&Waker>) -> Result<(), String> {
    // Declared first so it's removed after the decoder, which reads the file.
    let file = TempVideo::write(bytes).map_err(|error| error.to_string())?;
    let mut decoder = open_decoder(file.path()).map_err(|error| error.to_string())?;
    shared.lock().info = Some(decoder.info());
    if let Some(waker) = waker {
        waker();
    }
    let mut position = Position::default();
    while let Some(times) = shared.next_wanted() {
        for time in times {
            read_to(decoder.as_mut(), &mut position, shared, waker, time).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// Reads frames until one covers `time`, seeking when that's behind the reading or far ahead of it.
fn read_to(
    decoder: &mut dyn VideoDecoder,
    position: &mut Position,
    shared: &Shared,
    waker: Option<&Waker>,
    time: f64,
) -> Result<(), MediaError> {
    if shared.covered(time) {
        return Ok(());
    }
    let seek = match position.last_time {
        None => true,
        Some(from) => time < from || time > from + SEEK_AHEAD_SECONDS,
    };
    if seek {
        decoder.seek(time)?;
        position.last_time = None;
    }
    loop {
        if shared.closed() {
            return Ok(());
        }
        let Some(frame) = decoder.next_frame()? else { return Ok(()) };
        position.last_time = Some(frame.time);
        let reached = frame.time + frame.duration > time;
        shared.push(frame, waker);
        if reached {
            return Ok(());
        }
    }
}

/// A video's bytes written to a file a decoder can open, removed when dropped.
struct TempVideo(Option<TempPath>);

impl TempVideo {
    fn write(bytes: &[u8]) -> Result<Self, MediaError> {
        let copy = |error: std::io::Error| MediaError::Video(format!("it couldn't be copied to a file: {error}"));
        let mut file = NamedTempFile::new().map_err(copy)?;
        file.write_all(bytes).map_err(copy)?;
        file.flush().map_err(copy)?;
        Ok(Self(Some(file.into_temp_path())))
    }

    fn path(&self) -> &Path {
        self.0.as_deref().unwrap_or(Path::new(""))
    }
}

impl Drop for TempVideo {
    fn drop(&mut self) {
        if let Some(path) = self.0.take()
            && let Err(error) = path.close()
        {
            log::error!("[media] the video's temporary file couldn't be removed: {error}");
        }
    }
}
