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
mod compile;
mod renderer;
mod simulation;
mod target;
#[cfg(target_os = "macos")]
mod surface;
mod uniforms;

pub use clock::{FrameInputs, Timing};
pub use media::{MediaLibrary, MediaSource, MediaStatus, SystemVideoProbe};
pub use player::Player;
pub use refresh_rate::RefreshRateMeter;
pub use compile::{CompileQueue, Compiled};
pub use renderer::{
    Compiler, Frame, PixelFormat, Presented, Program, ProgramDiagnostic, ProgramError, RenderOptions,
    Renderer, Size,
};
#[cfg(target_os = "macos")]
pub use surface::Surface;
