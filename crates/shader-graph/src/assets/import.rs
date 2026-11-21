//! Describing the files a user imports as assets: their size, hash and
//! bundle path, as the importer of the original makes them.

use std::io::Cursor;

use image::{ImageDecoder as _, ImageReader, metadata::Orientation};
use thiserror::Error;

use super::asset::{Asset, AssetFile, AssetKind, MAX_ASSET_SIDE, asset_file, asset_kind_of, asset_name};
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
    #[error("videos can't be imported yet.")]
    VideoUnsupported,
    #[error("the project has too many assets.")]
    TooManyAssets,
}

/// Describes an image file as an asset with `id`. Its size is the size it is
/// shown at, after its EXIF orientation.
pub fn describe_asset(id: &str, file: ImportFile) -> Result<Asset, ImportError> {
    match asset_kind_of(file.mime) {
        Some(AssetKind::Image) => {}
        Some(AssetKind::Video) => return Err(ImportError::VideoUnsupported),
        None => return Err(ImportError::Unreadable),
    }
    let (width, height) = image_size(file.bytes).ok_or(ImportError::Unreadable)?;
    Ok(Asset::Image(AssetFile {
        id: id.to_owned(),
        name: asset_name(file.name).to_owned(),
        file: asset_file(AssetKind::Image, id, file.name),
        mime: file.mime.to_owned(),
        bytes: file.bytes.len() as u64,
        hash: asset_hash(file.bytes),
        width,
        height,
    }))
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
        let Ok(Asset::Image(asset)) = describe_asset("a1", file("Holiday.PNG", "image/png", &bytes)) else {
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
        assert_eq!(describe_asset("a", file("x.svg", "image/svg+xml", &bytes)), Err(ImportError::Unreadable));
        assert_eq!(describe_asset("a", file("x.txt", "text/plain", &bytes)), Err(ImportError::Unreadable));
        assert_eq!(describe_asset("a", file("x.png", "image/png", b"not an image")), Err(ImportError::Unreadable));
        assert_eq!(describe_asset("a", file("c.mp4", "video/mp4", b"")), Err(ImportError::VideoUnsupported));
    }
}
