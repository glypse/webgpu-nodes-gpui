//! Draws the shader programs `shader-graph` generates, with wgpu. Also keeps
//! project time (`Clock`, `Player`) and measures playback (`FrameMeter`).

mod clock;
mod frame_meter;
mod player;
mod refresh_rate;

pub use clock::{Clock, FrameInputs, Timing};
pub use frame_meter::{FrameMeter, FrameRate};
pub use player::Player;
pub use refresh_rate::RefreshRateMeter;
