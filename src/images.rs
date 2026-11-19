//! Thumbnail loading with a small, byte budgeted cache in memory and a
//! larger one on disk.

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

    /// A hit moves to the back, so the art in use is the last to go.
    pub fn get(&mut self, key: &str) -> Option<Pixels> {
        let pixels = self.map.get(key)?.clone();
        if let Some(at) = self.order.iter().rposition(|k| k == key) {
            if at + 1 != self.order.len() {
                let key = self.order.remove(at)?;
                self.order.push_back(key);
            }
        }
        Some(pixels)
    }

    /// The largest copy of `url` held, at whatever size it was fetched:
    /// a stand-in while the size wanted is on its way.
    pub fn largest(&self, url: &str) -> Option<Pixels> {
        let tail = format!(":{url}");
        self.map
            .iter()
            .filter(|(key, _)| key.ends_with(&tail))
            .max_by_key(|(_, pixels)| pixels.width())
            .map(|(_, pixels)| pixels.clone())
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

// ------------------------------------------------------------- disk cache

/// Art already fetched is kept on disk as it came, so the next launch, or
/// a page opened offline, shows it without the network. The URL asks for
/// the size, so each size is its own file. Least recently shown go first.
const DISK_BUDGET: u64 = 128 * 1024 * 1024;
/// Trims after this many new files, not after every one.
const TRIM_EVERY: u32 = 64;

fn disk_dir() -> Option<PathBuf> {
    Some(crate::store::data_dir()?.join("art-cache"))
}

fn disk_file(url: &str) -> Option<PathBuf> {
    use sha1::{Digest, Sha1};
    let hash: String = Sha1::digest(url.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Some(disk_dir()?.join(format!("{hash}.img")))
}

/// The bytes of a picture, from the disk cache or the network.
fn download(http: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let path = disk_file(url);
    if let Some(path) = &path {
        if let Ok(bytes) = std::fs::read(path) {
            if !bytes.is_empty() {
                // Marks it as recently shown, for trimming.
                if let Ok(file) = std::fs::File::options().write(true).open(path) {
                    let _ = file.set_modified(SystemTime::now());
                }
                return Some(bytes);
            }
        }
    }
    let response = http
        .get(url)
        .set("User-Agent", crate::innertube::USER_AGENT)
        .call()
        .ok()?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut bytes)
        .ok()?;
    // Only a picture that decodes is worth keeping.
    if let Some(path) = path.filter(|_| image::guess_format(&bytes).is_ok()) {
        store(&path, &bytes);
    }
    Some(bytes)
}

fn store(path: &Path, bytes: &[u8]) {
    static WRITES: AtomicU32 = AtomicU32::new(0);
    let Some(folder) = path.parent() else { return };
    if std::fs::create_dir_all(folder).is_err() {
        return;
    }
    // Through a part file, so a crash never leaves half a picture.
    let partial = path.with_extension("part");
    if std::fs::write(&partial, bytes).is_err() || std::fs::rename(&partial, path).is_err() {
        let _ = std::fs::remove_file(&partial);
        return;
    }
    if WRITES.fetch_add(1, Ordering::Relaxed) % TRIM_EVERY == 0 {
        trim(folder);
    }
}

/// Drops the least recently shown pictures until the folder fits.
fn trim(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut files: Vec<(PathBuf, u64, SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file()
                .then(|| (e.path(), meta.len(), meta.modified().unwrap_or(UNIX_EPOCH)))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= DISK_BUDGET {
        return;
    }
    files.sort_by_key(|f| f.2);
    for (path, size, _) in files {
        if total <= DISK_BUDGET {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

pub use crate::innertube::sized;

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
    let bytes = download(http, &url)?;
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
        download(http, &url)?
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
