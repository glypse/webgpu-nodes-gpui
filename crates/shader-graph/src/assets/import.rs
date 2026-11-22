//! Describing the files a user imports as assets: their size, hash and
//! bundle path, as the importer of the original makes them.

use std::io::Cursor;

use image::{ImageDecoder as _, ImageReader, metadata::Orientation};
use thiserror::Error;

use super::asset::{
    Asset, AssetFile, AssetKind, MAX_ASSET_SIDE, VideoFile, asset_file, asset_kind_of, asset_name,
};
use super::bundle::asset_hash;

/// A file picked to import: its name and media type as the platform gave them, and its bytes.
#[derive(Clone, Copy, Debug)]
pub struct ImportFile<'a> {
    pub name: &'a str,
    pub mime: &'a str,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ImportError {
    #[error("it isn't an image or video this editor can read.")]
    Unreadable,
    #[error("the project has too many assets.")]
    TooManyAssets,
}

/// A video's metadata as a probe reads it. The duration is in seconds and the
/// frame rate averages the frames' spacing, as the original's importer does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub frame_rate: f64,
}

/// Reads a video file's metadata. Videos are decoded by the runtime, so the
/// platform that decodes them provides the probe.
pub trait VideoProbe {
    fn probe(&self, bytes: &[u8]) -> Result<VideoInfo, String>;
}

/// A probe for an import that has no videos to read: every video is unreadable.
pub struct NoVideos;

impl VideoProbe for NoVideos {
    fn probe(&self, _bytes: &[u8]) -> Result<VideoInfo, String> {
        Err("videos can't be read here".to_owned())
    }
}

/// Describes an image or video file as an asset with `id`. An image's size is
/// the size it is shown at, after its EXIF orientation; a video's is its
/// display size, as `probe` reads it.
pub fn describe_asset(id: &str, file: ImportFile, probe: &dyn VideoProbe) -> Result<Asset, ImportError> {
    let kind = asset_kind_of(file.mime).ok_or(ImportError::Unreadable)?;
    let common = |width, height| AssetFile {
        id: id.to_owned(),
        name: asset_name(file.name).to_owned(),
        file: asset_file(kind, id, file.name),
        mime: file.mime.to_owned(),
        bytes: file.bytes.len() as u64,
        hash: asset_hash(file.bytes),
        width,
        height,
    };
    match kind {
        AssetKind::Image => {
            let (width, height) = image_size(file.bytes).ok_or(ImportError::Unreadable)?;
            Ok(Asset::Image(common(width, height)))
        }
        AssetKind::Video => {
            let info = probe.probe(file.bytes).map_err(|_| ImportError::Unreadable)?;
            let side = 1..=MAX_ASSET_SIDE;
            if !side.contains(&info.width) || !side.contains(&info.height) {
                return Err(ImportError::Unreadable);
            }
            let video = VideoFile {
                file: common(info.width, info.height),
                duration: info.duration,
                frame_rate: info.frame_rate,
            };
            Ok(Asset::Video(video))
        }
    }
}

/// The size an image's pixels have once upright, read from its header; `None`
/// when it can't be read or is beyond what an asset can be.
fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut decoder = reader.into_decoder().ok()?;
    let (width, height) = decoder.dimensions();
    let turned = matches!(
        decoder.orientation().ok()?,
        Orientation::Rotate90 | Orientation::Rotate270 | Orientation::Rotate90FlipH | Orientation::Rotate270FlipH
    );
    let (width, height) = if turned { (height, width) } else { (width, height) };
    let side = 1..=MAX_ASSET_SIDE;
    (side.contains(&width) && side.contains(&height)).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
        out.into_inner()
    }

    fn file<'a>(name: &'a str, mime: &'a str, bytes: &'a [u8]) -> ImportFile<'a> {
        ImportFile { name, mime, bytes }
    }

    #[test]
    fn describes_an_image_by_its_header_and_its_bytes() {
        let bytes = png(3, 2);
        let Ok(Asset::Image(asset)) = describe_asset("a1", file("Holiday.PNG", "image/png", &bytes), &NoVideos) else {
            panic!("an image");
        };
        assert_eq!(asset.name, "Holiday");
        assert_eq!(asset.file, "assets/images/a1.png");
        assert_eq!((asset.width, asset.height), (3, 2));
        assert_eq!(asset.bytes, bytes.len() as u64);
        assert_eq!(asset.hash, asset_hash(&bytes));
    }

    #[test]
    fn refuses_what_isnt_an_image_it_can_read() {
        let bytes = png(1, 1);
        assert_eq!(describe_asset("a", file("x.svg", "image/svg+xml", &bytes), &NoVideos), Err(ImportError::Unreadable));
        assert_eq!(describe_asset("a", file("x.txt", "text/plain", &bytes), &NoVideos), Err(ImportError::Unreadable));
        assert_eq!(describe_asset("a", file("x.png", "image/png", b"not an image"), &NoVideos), Err(ImportError::Unreadable));
    }

    struct Probe(VideoInfo);

    impl VideoProbe for Probe {
        fn probe(&self, _bytes: &[u8]) -> Result<VideoInfo, String> {
            Ok(self.0)
        }
    }

    #[test]
    fn describes_a_video_by_what_the_probe_reads() {
        let info = VideoInfo { width: 640, height: 360, duration: 2.5, frame_rate: 24.0 };
        let bytes = b"a movie";
        let Ok(Asset::Video(video)) = describe_asset("v1", file("Clip.MP4", "video/mp4", bytes), &Probe(info)) else {
            panic!("a video");
        };
        assert_eq!(video.file.name, "Clip");
        assert_eq!(video.file.file, "assets/videos/v1.mp4");
        assert_eq!((video.file.width, video.file.height), (640, 360));
        assert_eq!(video.file.bytes, bytes.len() as u64);
        assert_eq!(video.file.hash, asset_hash(bytes));
        assert_eq!((video.duration, video.frame_rate), (2.5, 24.0));
    }

    #[test]
    fn refuses_a_video_the_probe_cant_read() {
        assert_eq!(describe_asset("v", file("c.mp4", "video/mp4", b""), &NoVideos), Err(ImportError::Unreadable));
    }
}
