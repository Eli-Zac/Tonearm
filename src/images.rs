//! Thumbnail loading with a small, byte budgeted cache.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, VecDeque};
use std::io::Read;

// Art is fetched at screen pixels, so a scaled display needs more room.
const BUDGET: usize = 64 * 1024 * 1024;
const MAX_DOWNLOAD: u64 = 4 * 1024 * 1024;

pub type Pixels = SharedPixelBuffer<Rgba8Pixel>;

#[derive(Default)]
pub struct Cache {
    map: HashMap<String, Pixels>,
    order: VecDeque<String>,
    bytes: usize,
}

impl Cache {
    pub fn key(url: &str, px: u32) -> String {
        format!("{px}:{url}")
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

/// Downloads, decodes and center crops to a `px` square. The page clips
/// it to its rounded shape, so the pixels stay square.
pub fn fetch(http: &ureq::Agent, url: &str, px: u32) -> Option<Pixels> {
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
    let rgba = scaled.into_rgba8();
    Some(Pixels::clone_from_slice(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
    ))
}
