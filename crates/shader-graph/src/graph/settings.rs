//! How the project renders: saved with it, and part of its undo history.

use serde::{Deserialize, Serialize};

use super::model::GraphModel;

/// The key of the settings in the project's JSON.
const KEY: &str = "settings";

/// The largest texture side every GPU supports.
pub const MAX_CANVAS_SIZE: u32 = 8192;
pub const MAX_FPS: f64 = 240.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    #[default]
    #[serde(rename = "srgb")]
    Srgb,
    #[serde(rename = "display-p3")]
    DisplayP3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasSettings {
    pub width: u32,
    pub height: u32,
    /// Render at the preview's size rather than `width` × `height` (which are kept for later).
    pub follow_preview: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimingSettings {
    pub fps: f64,
    /// Draw on every display refresh, time following the wall clock, rather than at `fps`.
    pub realtime: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSettings {
    pub canvas: CanvasSettings,
    pub timing: TimingSettings,
    /// The colour space frames are presented and exported in.
    pub color_space: ColorSpace,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            canvas: CanvasSettings { width: 1920, height: 1080, follow_preview: false },
            timing: TimingSettings { fps: 60.0, realtime: true },
            color_space: ColorSpace::Srgb,
        }
    }
}

impl ProjectSettings {
    pub fn is_valid(&self) -> bool {
        let side = 1..=MAX_CANVAS_SIZE;
        side.contains(&self.canvas.width)
            && side.contains(&self.canvas.height)
            && self.timing.fps.is_finite()
            && (1.0..=MAX_FPS).contains(&self.timing.fps)
    }
}

impl GraphModel {
    /// The project's settings; the defaults when it has none, or ones that can't be used.
    pub fn settings(&self) -> ProjectSettings {
        let Some(value) = self.project.extra.get(KEY) else { return ProjectSettings::default() };
        match serde_json::from_value::<ProjectSettings>(value.clone()) {
            Ok(settings) if settings.is_valid() => settings,
            _ => {
                log::warn!("the project's settings can't be used, so the defaults apply");
                ProjectSettings::default()
            }
        }
    }

    /// Replaces the settings. Refused (`false`) when a value is out of range.
    pub fn update_settings(&mut self, settings: ProjectSettings) -> bool {
        if !settings.is_valid() {
            return false;
        }
        if settings == self.settings() {
            return true;
        }
        match serde_json::to_value(settings) {
            Ok(value) => self.project.extra.insert(KEY.to_owned(), value),
            Err(_) => return false,
        };
        self.changed();
        true
    }
}
