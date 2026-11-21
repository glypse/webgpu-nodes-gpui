//! Draws the shader programs `shader-graph` generates, with wgpu. Also keeps
//! project time (`Clock`, `Player`) and measures playback (`FrameMeter`).

mod clock;
mod frame_meter;
mod gpu_timer;
mod player;
mod present;
mod refresh_rate;
mod renderer;
mod uniforms;

pub use clock::{Clock, FrameInputs, Timing};
pub use frame_meter::{FrameMeter, FrameRate};
pub use player::Player;
pub use refresh_rate::RefreshRateMeter;
pub use renderer::{
    Frame, PixelFormat, Program, ProgramDiagnostic, ProgramError, RenderOptions,
    Renderer, Size,
};
pub use uniforms::{UniformBlock, UniformField, UniformScalar, pack_block};
