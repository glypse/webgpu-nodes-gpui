//! The media library on the real device: images decode off the main thread,
//! are uploaded when polled, and are dropped once no asset reads them.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use runtime::{MediaSource, MediaStatus, Renderer, Size};
use shader_graph::assets::{AssetKind, asset_hash};

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 10, 10, 255]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("encodes");
    out.into_inner()
}

fn source(kind: AssetKind, bytes: &[u8]) -> MediaSource {
    MediaSource { kind, hash: asset_hash(bytes), bytes: Arc::new(bytes.to_vec()) }
}

/// Syncs `sources` and waits until none of the assets is still decoding.
fn settle(renderer: &mut Renderer, sources: &BTreeMap<String, MediaSource>) {
    let (wake, woken) = mpsc::channel();
    renderer.set_media_waker(move || {
        let _ = wake.send(());
    });
    renderer.sync_media(sources);
    loop {
        renderer.poll_media();
        if sources.keys().all(|asset| renderer.media_status(asset) != MediaStatus::Loading) {
            return;
        }
        woken.recv_timeout(Duration::from_secs(20)).expect("the images decode");
    }
}

#[test]
fn decodes_an_image_to_its_size_off_the_main_thread() {
    let mut renderer = Renderer::new().expect("a GPU");
    let sources = BTreeMap::from([("photo".to_owned(), source(AssetKind::Image, &png(5, 3)))]);
    assert_eq!(renderer.media_status("photo"), MediaStatus::Missing);
    settle(&mut renderer, &sources);
    assert_eq!(renderer.media_status("photo"), MediaStatus::Ready(Size::new(5, 3)));
}

#[test]
fn a_file_that_isnt_an_image_fails_with_its_reason() {
    let mut renderer = Renderer::new().expect("a GPU");
    let sources = BTreeMap::from([("broken".to_owned(), source(AssetKind::Image, b"not an image"))]);
    settle(&mut renderer, &sources);
    assert!(
        matches!(renderer.media_status("broken"), MediaStatus::Failed(m) if m.starts_with("An image can't be used.")),
        "{:?}",
        renderer.media_status("broken")
    );
}

#[test]
fn assets_with_one_file_share_its_image_and_unused_images_are_dropped() {
    let mut renderer = Renderer::new().expect("a GPU");
    let bytes = png(2, 2);
    let both = BTreeMap::from([
        ("first".to_owned(), source(AssetKind::Image, &bytes)),
        ("second".to_owned(), source(AssetKind::Image, &bytes)),
    ]);
    settle(&mut renderer, &both);
    assert_eq!(renderer.media_status("second"), MediaStatus::Ready(Size::new(2, 2)));

    let one = BTreeMap::from([("second".to_owned(), source(AssetKind::Image, &bytes))]);
    renderer.sync_media(&one);
    assert_eq!(renderer.media_status("second"), MediaStatus::Ready(Size::new(2, 2)));
    assert_eq!(renderer.media_status("first"), MediaStatus::Missing);

    renderer.sync_media(&BTreeMap::new());
    assert_eq!(renderer.media_status("second"), MediaStatus::Missing);
    assert!(!renderer.poll_media());
}
