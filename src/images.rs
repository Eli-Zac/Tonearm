//! Thumbnail loading with a small, byte budgeted cache.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, VecDeque};
use std::io::Read;

const BUDGET: usize = 12 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Default)]
pub struct Cache {
    map: HashMap<String, Pixels>,
    order: VecDeque<String>,
    bytes: usize,
}

impl Cache {
    pub fn key(url: &str, px: u32, round: bool) -> String {
        format!("{px}{}{url}", if round { 'o' } else { ':' })
    }

    pub fn get(&self, key: &str) -> Option<Pixels> {
        self.map.get(key).cloned()
    }

    pub fn put(&mut self, key: String, pixels: Pixels) {
        if self.map.contains_key(&key) {
            return;
        }
        self.bytes += pixels.as_bytes().len();
        self.order.push_back(key.clone());
        self.map.insert(key, pixels);
        while self.bytes > BUDGET {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&oldest) {
                self.bytes -= old.as_bytes().len();
            }
        }
    }
}

/// Google's image hosts resize on request. Asking for the size we draw
/// saves both download and decode work.
pub fn sized(url: &str, px: u32) -> String {
    let resizable = url.contains("googleusercontent.com") || url.contains("ggpht.com");
    match url.rfind('=') {
        Some(i) if resizable => format!("{}=w{px}-h{px}-l90-rj", &url[..i]),
        _ => url.to_string(),
    }
}

/// The cache key of a wide picture, kept apart from the squares.
pub fn wide_key(url: &str, width: u32) -> String {
    format!("{width}w{url}")
}

/// Downloads a wide picture, such as an artist banner, at `width` pixels
/// across. It keeps its own shape. The page crops it to fit and covers
/// the corners, so nothing is cut or rounded here.
pub fn fetch_wide(http: &ureq::Agent, url: &str, width: u32) -> Option<Pixels> {
    let resizable = url.contains("googleusercontent.com") || url.contains("ggpht.com");
    let url = match url.rfind('=') {
        // Banners come at 12 by 5. Asking for that shape keeps the whole picture.
        Some(i) if resizable => format!("{}=w{width}-h{}-p-l90-rj", &url[..i], width * 5 / 12),
        _ if url.starts_with("//") => format!("https:{url}"),
        _ => url.to_string(),
    };
    let response = http
        .get(&url)
        .set("User-Agent", crate::innertube::USER_AGENT)
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut bytes)
        .ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    let scaled = if decoded.width() > width {
        let height = decoded.height() * width / decoded.width();
        decoded.resize_exact(width, height.max(1), image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let rgba = scaled.into_rgba8();
    Some(Pixels::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}

/// Rounds the corners by fading alpha. The software renderer cannot clip
/// an image to a rounded shape, so we bake the shape into the pixels.
pub fn round_corners(rgba: &mut image::RgbaImage, radius: f32) {
    let (w, h) = (rgba.width() as f32, rgba.height() as f32);
    for (x, y, pixel) in rgba.enumerate_pixels_mut() {
        // Distance from the pixel centre to the nearest corner circle.
        let cx = (x as f32 + 0.5).clamp(radius, w - radius);
        let cy = (y as f32 + 0.5).clamp(radius, h - radius);
        let distance = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
        let cover = (radius - distance + 0.5).clamp(0.0, 1.0);
        if cover < 1.0 {
            pixel.0[3] = (pixel.0[3] as f32 * cover) as u8;
        }
    }
}

/// Downloads, decodes, center crops to a `px` square and rounds the
/// corners. `round` makes a full circle, used for artists.
pub fn fetch(http: &ureq::Agent, url: &str, px: u32, round: bool) -> Option<Pixels> {
    // Covers of downloaded songs live on disk, so they show offline.
    let bytes = if let Some(path) = url.strip_prefix("file:") {
        std::fs::read(path).ok()?
    } else {
        let url = if url.starts_with("//") {
            format!("https:{url}")
        } else {
            sized(url, px)
        };
        let response = http
            .get(&url)
            .set("User-Agent", crate::innertube::USER_AGENT)
            .call()
            .ok()?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_DOWNLOAD)
            .read_to_end(&mut bytes)
            .ok()?;
        bytes
    };
    let decoded = image::load_from_memory(&bytes).ok()?;
    let side = decoded.width().min(decoded.height());
    let (x, y) = ((decoded.width() - side) / 2, (decoded.height() - side) / 2);
    let square = decoded.crop_imm(x, y, side, side);
    let scaled = if side > px {
        square.resize_exact(px, px, image::imageops::FilterType::Triangle)
    } else {
        square
    };
    let mut rgba = scaled.into_rgba8();
    let side = rgba.width() as f32;
    round_corners(&mut rgba, if round { side / 2.0 } else { side * 0.065 });
    Some(Pixels::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}
