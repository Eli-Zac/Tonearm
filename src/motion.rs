//! Animated album covers.
//!
//! The player bar can show a looping cover instead of a still. This module
//! decodes animated WebP and GIF into frames. Frames are small and capped,
//! so a moving cover costs a few megabytes at most.
//!
//! Where the clips come from is a separate question. The catalogues that
//! carry motion artwork (Apple Music, Spotify Canvas, Tidal) publish video,
//! which needs a Windows Media Foundation decoder we have not written yet.
//! Until then, `TONEARM_TEST_COVER` points every track at one animated file.

use crate::images::Pixels;
use image::codecs::{gif::GifDecoder, webp::WebPDecoder};
use image::{AnimationDecoder, ImageFormat};
use std::io::{Cursor, Read};
use std::time::Duration;

const SIDE: u32 = 128;
const MAX_FRAMES: usize = 90;
const MAX_DOWNLOAD: u64 = 12 * 1024 * 1024;

pub struct Frame {
    pub pixels: Pixels,
    pub delay: Duration,
}

/// The animated cover for a track, if we know one.
pub fn lookup(_title: &str, _artist: &str) -> Option<String> {
    std::env::var("TONEARM_TEST_COVER").ok()
}

pub fn fetch(http: &ureq::Agent, url: &str) -> Option<Vec<Frame>> {
    let mut bytes = Vec::new();
    http.get(url)
        .call()
        .ok()?
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut bytes)
        .ok()?;
    decode(&bytes)
}

pub fn decode(bytes: &[u8]) -> Option<Vec<Frame>> {
    let frames = match image::guess_format(bytes).ok()? {
        ImageFormat::WebP => WebPDecoder::new(Cursor::new(bytes)).ok()?.into_frames(),
        ImageFormat::Gif => GifDecoder::new(Cursor::new(bytes)).ok()?.into_frames(),
        _ => return None,
    };
    let mut out = Vec::new();
    for frame in frames.take(MAX_FRAMES) {
        let frame = frame.ok()?;
        let (ms, denominator) = frame.delay().numer_denom_ms();
        let delay = Duration::from_millis((ms / denominator.max(1)).clamp(20, 1000) as u64);
        let rgba = image::DynamicImage::ImageRgba8(frame.into_buffer());
        let side = rgba.width().min(rgba.height());
        let square = rgba.crop_imm(
            (rgba.width() - side) / 2,
            (rgba.height() - side) / 2,
            side,
            side,
        );
        let mut rgba = square
            .resize_exact(SIDE, SIDE, image::imageops::FilterType::Triangle)
            .into_rgba8();
        crate::images::round_corners(&mut rgba, SIDE as f32 * 0.065);
        out.push(Frame {
            pixels: Pixels::clone_from_slice(rgba.as_raw(), SIDE, SIDE),
            delay,
        });
    }
    (out.len() > 1).then_some(out)
}
