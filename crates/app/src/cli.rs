use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;

/// The node editor for WGSL shaders.
///
/// With no flags the app opens the project and waits for input. The flags below
/// let a script or an agent run it for a bounded time, screenshot it and profile it.
#[derive(Debug, Clone, Parser)]
#[command(name = "webgpu-nodes", version)]
pub struct Cli {
    /// Project to open: bare JSON or a bundle. Defaults to the last one opened.
    pub project: Option<PathBuf>,

    /// Start playback as soon as the window opens.
    #[arg(long)]
    pub play: bool,

    /// Quit cleanly after this many seconds, counted from launch.
    #[arg(long, value_name = "SECONDS")]
    pub seconds: Option<f64>,

    /// Quit cleanly once this many preview frames have been presented. Without
    /// --play the preview draws one frame, so pair this with --seconds.
    #[arg(long, value_name = "N")]
    pub frames: Option<u64>,

    /// Save a PNG of the app window just before quitting.
    #[arg(long, value_name = "PATH")]
    pub screenshot: Option<PathBuf>,

    /// Window size in points, as WIDTHxHEIGHT (e.g. 1400x860).
    #[arg(long, value_name = "WxH", value_parser = Size::from_str)]
    pub window: Option<Size>,

    /// Record spans to a Chrome trace at PATH (open it in ui.perfetto.dev) and
    /// print a summary when the app quits.
    #[arg(long, value_name = "PATH")]
    pub profile: Option<PathBuf>,

    /// Open an output window beside the project window; --screenshot then captures it.
    #[arg(long)]
    pub output: bool,

    /// Zoom the first graph area to FACTOR on startup (1 is 100%), centred on its nodes.
    #[arg(long, value_name = "FACTOR", value_parser = parse_zoom)]
    pub zoom: Option<f32>,
}

fn parse_zoom(text: &str) -> Result<f32, String> {
    text.trim()
        .parse::<f32>()
        .ok()
        .filter(|z| z.is_finite() && *z > 0.0)
        .ok_or_else(|| format!("`{text}` is not a positive zoom factor"))
}

/// A window size in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl FromStr for Size {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (w, h) = text
            .split_once(['x', 'X'])
            .ok_or_else(|| format!("`{text}` is not WIDTHxHEIGHT"))?;
        let parse = |part: &str| {
            part.trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 1.0)
                .ok_or_else(|| format!("`{text}` is not WIDTHxHEIGHT with sizes of at least 1"))
        };
        Ok(Self { width: parse(w)?, height: parse(h)? })
    }
}

impl Cli {
    /// Whether any flag asks for measurements, which turns span recording on.
    pub fn records(&self) -> bool {
        self.profile.is_some() || self.frames.is_some()
    }

    /// Whether the app should quit by itself.
    pub fn bounded(&self) -> bool {
        self.seconds.is_some() || self.frames.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_flags() {
        let cli = Cli::try_parse_from([
            "webgpu-nodes",
            "scene.json",
            "--play",
            "--seconds",
            "10",
            "--screenshot",
            "out.png",
            "--window",
            "800x600",
            "--profile",
            "trace.json",
        ])
        .expect("parses");
        assert_eq!(cli.project, Some(PathBuf::from("scene.json")));
        assert!(cli.play);
        assert_eq!(cli.seconds, Some(10.0));
        assert_eq!(cli.window, Some(Size { width: 800.0, height: 600.0 }));
        assert!(cli.records());
        assert!(cli.bounded());
    }

    #[test]
    fn rejects_bad_window_sizes() {
        assert!(Cli::try_parse_from(["webgpu-nodes", "--window", "800"]).is_err());
        assert!(Cli::try_parse_from(["webgpu-nodes", "--window", "0x600"]).is_err());
    }

    #[test]
    fn parses_a_positive_zoom() {
        let cli = Cli::try_parse_from(["webgpu-nodes", "--zoom", "0.83"]).expect("parses");
        assert_eq!(cli.zoom, Some(0.83));
        assert!(Cli::try_parse_from(["webgpu-nodes", "--zoom", "0"]).is_err());
        assert!(Cli::try_parse_from(["webgpu-nodes", "--zoom", "-1"]).is_err());
    }
}
