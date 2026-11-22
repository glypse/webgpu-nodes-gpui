use serde::{Deserialize, Serialize};

use crate::graph::{MAX_ASSET_SIDE, MAX_ID_LENGTH};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Video,
}

impl AssetKind {
    /// Where a project bundle keeps the kind's files.
    pub const fn folder(self) -> &'static str {
        match self {
            Self::Image => "assets/images",
            Self::Video => "assets/videos",
        }
    }
}

/// What every asset has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetFile {
    pub id: String,
    /// Shown to the user; the file's name without its extension when imported.
    pub name: String,
    /// The file's path in the project bundle (see `asset_file`).
    pub file: String,
    pub mime: String,
    pub bytes: u64,
    /// SHA-256 of the file, hex: importing the same file twice reuses the asset.
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

/// A video's own metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFile {
    #[serde(flatten)]
    pub file: AssetFile,
    /// Seconds.
    pub duration: f64,
    /// Frames per second, averaged for a variable frame rate.
    pub frame_rate: f64,
}

/// A file the project uses, e.g. an image. Its bytes live outside the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Asset {
    Image(AssetFile),
    Video(VideoFile),
}

impl Asset {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Image(_) => AssetKind::Image,
            Self::Video(_) => AssetKind::Video,
        }
    }

    pub fn file_info(&self) -> &AssetFile {
        match self {
            Self::Image(file) => file,
            Self::Video(video) => &video.file,
        }
    }

    pub fn file_info_mut(&mut self) -> &mut AssetFile {
        match self {
            Self::Image(file) => file,
            Self::Video(video) => &mut video.file,
        }
    }

    pub fn id(&self) -> &str {
        &self.file_info().id
    }

    pub fn name(&self) -> &str {
        &self.file_info().name
    }

    /// The file's path in the project bundle.
    pub fn file(&self) -> &str {
        &self.file_info().file
    }

    pub fn hash(&self) -> &str {
        &self.file_info().hash
    }

    pub fn video(&self) -> Option<&VideoFile> {
        match self {
            Self::Video(video) => Some(video),
            Self::Image(_) => None,
        }
    }

    /// Whether the metadata is what `asset_file` and `asset_kind_of` could have made.
    pub fn is_valid(&self) -> bool {
        let info = self.file_info();
        let side = 1..=MAX_ASSET_SIDE;
        let finite = self.video().is_none_or(|v| {
            v.duration.is_finite() && v.duration >= 0.0 && v.frame_rate.is_finite() && v.frame_rate >= 0.0
        });
        !info.id.is_empty()
            && info.id.len() <= MAX_ID_LENGTH
            && info.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && info.hash.len() == 64
            && info.hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
            && side.contains(&info.width)
            && side.contains(&info.height)
            && finite
            && asset_kind_of(&info.mime) == Some(self.kind())
            && asset_file(self.kind(), &info.id, &info.file) == info.file
    }
}

/// The extension a bundle path keeps: tame ones only, as lowercase.
fn extension_of(filename: &str) -> String {
    let Some(dot) = filename.rfind('.').filter(|d| *d > 0) else { return String::new() };
    let extension = filename[dot..].to_ascii_lowercase();
    let tame = extension.len() >= 2
        && extension.len() <= 9
        && extension[1..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if tame { extension } else { String::new() }
}

/// The bundle path of an asset's file: named by id, so names never collide
/// and renaming an asset never moves its file.
pub fn asset_file(kind: AssetKind, id: &str, filename: &str) -> String {
    format!("{}/{id}{}", kind.folder(), extension_of(filename))
}

/// Whether `path` is one `asset_file` gives. Bundle paths come from files,
/// and end up as zip entries: anything else could stand for the bundle's
/// JSON, or lead out of the folder it's extracted to.
pub fn is_asset_path(path: &str) -> bool {
    [AssetKind::Image, AssetKind::Video].iter().any(|kind| {
        path.strip_prefix(kind.folder())
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(is_file_name)
    })
}

/// A file's name in its folder: its asset's id, then the extension `asset_file` kept, if any.
fn is_file_name(name: &str) -> bool {
    let (stem, extension) = match name.find('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    let tame = extension.is_empty()
        || (extension.len() >= 2
            && extension.len() <= 9
            && extension[1..].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    !stem.is_empty() && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && tame
}

/// The kind of asset a file of `mime` type makes, if any. SVG is a
/// document: opened as one, it runs its scripts.
pub fn asset_kind_of(mime: &str) -> Option<AssetKind> {
    let (kind, subtype) = mime.split_once('/')?;
    let valid = subtype.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && subtype.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '+' | '-'));
    if !valid || mime == "image/svg+xml" {
        return None;
    }
    match kind {
        "image" => Some(AssetKind::Image),
        "video" => Some(AssetKind::Video),
        _ => None,
    }
}

/// A file's name without its extension.
pub fn asset_name(filename: &str) -> &str {
    match filename.rfind('.') {
        Some(dot) if dot > 0 => &filename[..dot],
        _ => filename,
    }
}

/// What a video shows past its end: its start again, its last frame, or nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoEnd {
    Loop,
    Hold,
    Transparent,
}

/// How a Video node plays its file along the project's time.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VideoPlayback {
    /// Project seconds at which the video's first frame shows.
    pub start: f64,
    /// Video seconds per project second.
    pub speed: f64,
    pub end: VideoEnd,
}

pub const DEFAULT_PLAYBACK: VideoPlayback = VideoPlayback { start: 0.0, speed: 1.0, end: VideoEnd::Loop };

impl Default for VideoPlayback {
    fn default() -> Self {
        DEFAULT_PLAYBACK
    }
}

/// The seconds between a video's frames: 0 when its frame rate isn't known.
pub fn frame_interval(asset: Option<&Asset>) -> f64 {
    match asset.and_then(Asset::video) {
        Some(video) if video.frame_rate > 0.0 => f64::from((1.0 / video.frame_rate) as f32),
        _ => 0.0,
    }
}

/// The time in a video of `duration` seconds shown at project time `time`,
/// or `None` when nothing shows (before its start, or past its end when
/// `end` is `Transparent`).
pub fn video_time(playback: &VideoPlayback, time: f64, duration: f64) -> Option<f64> {
    if time < playback.start {
        return None;
    }
    let t = (time - playback.start) * playback.speed;
    if duration <= 0.0 {
        return Some(0.0);
    }
    if (0.0..duration).contains(&t) {
        return Some(t);
    }
    match playback.end {
        VideoEnd::Loop => Some(((t % duration) + duration) % duration),
        VideoEnd::Hold => Some(t.clamp(0.0, duration)),
        VideoEnd::Transparent => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Asset {
        Asset::Image(AssetFile {
            id: "a1".into(),
            name: "Photo".into(),
            file: "assets/images/a1.png".into(),
            mime: "image/png".into(),
            bytes: 3,
            hash: "a".repeat(64),
            width: 4,
            height: 2,
        })
    }

    fn with(f: impl FnOnce(&mut AssetFile)) -> Asset {
        let mut asset = image();
        f(asset.file_info_mut());
        asset
    }

    #[test]
    fn files_assets_by_kind_and_id_keeping_the_extension() {
        assert_eq!(asset_file(AssetKind::Image, "a1", "Photo.JPG"), "assets/images/a1.jpg");
        assert_eq!(asset_file(AssetKind::Video, "v", "clip"), "assets/videos/v");
        assert_eq!(asset_file(AssetKind::Image, "a", "x/../../evil"), "assets/images/a");
        assert_eq!(asset_file(AssetKind::Image, "a", "b.tar/../../etc"), "assets/images/a");
    }

    #[test]
    fn only_takes_paths_of_the_assets_own_folder_and_name() {
        assert!(image().is_valid());
        for file in [
            "project.json",
            "groups.json",
            "../../evil",
            "assets/images/../../project.json",
            "assets/images/a1/../../../evil.png",
            "assets/videos/a1.png",
            "assets/images/other.png",
            "assets/images/a1.png/x",
            "__proto__",
        ] {
            assert!(!with(|a| a.file = file.into()).is_valid(), "{file}");
        }
        assert!(!with(|a| a.id = "../a1".into()).is_valid());
        assert!(is_asset_path("assets/images/a.png"));
        assert!(!is_asset_path("assets/images/"));
        assert!(!is_asset_path("project.json"));
    }

    #[test]
    fn only_takes_hashes_and_media_types_that_can_be_real() {
        assert!(!with(|a| a.hash = "abc".into()).is_valid());
        assert!(!with(|a| a.mime = "text/html".into()).is_valid());
        assert!(!with(|a| a.mime = "image/svg+xml".into()).is_valid());
        assert!(!with(|a| a.mime = "video/mp4".into()).is_valid());
        assert_eq!(asset_kind_of("image/png; charset=x"), None);
    }

    #[test]
    fn round_trips_through_json() {
        let video = Asset::Video(VideoFile {
            file: AssetFile {
                id: "v".into(),
                name: "clip".into(),
                file: "assets/videos/v.mp4".into(),
                mime: "video/mp4".into(),
                bytes: 1,
                hash: "0".repeat(64),
                width: 64,
                height: 32,
            },
            duration: 2.0,
            frame_rate: 25.0,
        });
        for asset in [image(), video] {
            let json = serde_json::to_string(&asset).expect("serialises");
            assert_eq!(serde_json::from_str::<Asset>(&json).expect("parses"), asset);
        }
        let json = r#"{"kind":"video","id":"v","name":"c","file":"assets/videos/v.mp4","mime":"video/mp4","bytes":1,"hash":"0000000000000000000000000000000000000000000000000000000000000000","width":1,"height":1,"duration":2,"frameRate":25}"#;
        assert!(serde_json::from_str::<Asset>(json).is_ok());
        assert!(serde_json::from_str::<Asset>(&json.replace(",\"frameRate\":25", "")).is_err());
    }

    #[test]
    fn video_frame_interval_is_known_with_a_frame_rate() {
        let video = |frame_rate| {
            Asset::Video(VideoFile {
                file: AssetFile { mime: "video/mp4".into(), ..image().file_info().clone() },
                duration: 2.0,
                frame_rate,
            })
        };
        assert_eq!(frame_interval(Some(&video(25.0))), f64::from(0.04f32));
        assert_eq!(frame_interval(Some(&video(0.0))), 0.0);
        assert_eq!(frame_interval(None), 0.0);
    }

    #[test]
    fn videos_play_along_the_project_time() {
        let loops = DEFAULT_PLAYBACK;
        assert_eq!(video_time(&loops, 0.5, 2.0), Some(0.5));
        assert_eq!(video_time(&loops, 2.5, 2.0), Some(0.5));
        assert_eq!(video_time(&VideoPlayback { start: 1.0, ..loops }, 0.5, 2.0), None);
        let hold = VideoPlayback { end: VideoEnd::Hold, ..loops };
        assert_eq!(video_time(&hold, 5.0, 2.0), Some(2.0));
        let transparent = VideoPlayback { end: VideoEnd::Transparent, ..loops };
        assert_eq!(video_time(&transparent, 5.0, 2.0), None);
        assert_eq!(video_time(&loops, 3.0, 0.0), Some(0.0));
    }
}
