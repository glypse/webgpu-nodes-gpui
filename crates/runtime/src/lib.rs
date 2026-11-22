//! Draws the shader programs `shader-graph` generates, with wgpu. Also keeps
//! project time (`Clock`, `Player`) and measures playback (`FrameMeter`).

mod clock;
mod formats;
mod frame_meter;
mod gpu_timer;
mod media;
mod mips;
mod player;
mod present;
mod refresh_rate;
mod renderer;
mod simulation;
#[cfg(target_os = "macos")]
mod surface;
mod uniforms;

pub use clock::{Clock, FrameInputs, Timing};
pub use frame_meter::{FrameMeter, FrameRate};
pub use media::{DecodedFrame, MediaError, MediaLibrary, MediaSource, MediaStatus, SystemVideoProbe, VideoDecoder, open_decoder};
pub use player::Player;
pub use refresh_rate::RefreshRateMeter;
pub use renderer::{
    Frame, PixelFormat, Presented, Program, ProgramDiagnostic, ProgramError, RenderOptions,
    Renderer, Size,
};
#[cfg(target_os = "macos")]
pub use surface::Surface;
pub use simulation::{Step, StepKind, StepRecord, next_step};
pub use uniforms::{UniformBlock, UniformField, UniformScalar, pack_block};
