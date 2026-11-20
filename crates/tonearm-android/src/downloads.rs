//! Songs saved on the phone, to play with no connection: the core's
//! `offline` downloads, as the desktop keeps them, in what Kotlin can read.
//!
//! The core downloads one song at a time on its own worker and says how it
//! went through `offline::listen`. Here that bumps a counter and keeps the
//! notes to show; the app polls `downloads_version` and reads the marks
//! again when it moves, as it polls `playback`.

use crate::pages::{self, Item, Page};
use crate::{Tonearm, TonearmError};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tonearm_core::innertube;
use tonearm_core::offline::{self, Event};

/// Bumped by every download event, so the app knows to look again.
static VERSION: AtomicU64 = AtomicU64::new(1);
/// What to tell the user, oldest first, until the app takes them.
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Where each song on a page stands, read in one go.
#[derive(uniffi::Record, Clone, Debug, Default)]
pub struct DownloadMarks {
    /// Saved on the phone.
    pub saved: Vec<String>,
    /// Being downloaded or waiting their turn, with how far (0 to 1).
    pub busy: Vec<DownloadProgress>,
    /// As `downloads_version` was when these were read.
    pub version: u64,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct DownloadProgress {
    pub video_id: String,
    pub progress: f32,
}

fn note(text: String) {
    let mut notes = NOTES.lock().unwrap();
    // Nobody reads a pile of them; the latest matter.
    if notes.len() >= 8 {
        notes.remove(0);
    }
    notes.push(text);
}

/// Hears the core's download worker. Set once, as the core starts.
pub fn listen() {
    offline::listen(|event| {
        match event {
            Event::Progress(_) => {}
            // As the desktop's toasts: one for a single song, one for a
            // whole batch once its last song is in, and every failure.
            Event::Finished { item, result, n, total } => match result {
                Ok(()) if total == 1 => note(format!("Downloaded {}", item.title)),
                Ok(()) if n + 1 == total => note(format!("Downloaded {total} songs")),
                Ok(()) => {}
                Err(message) => note(format!("Couldn't download {}: {message}", item.title)),
            },
        }
        VERSION.fetch_add(1, Ordering::SeqCst);
    });
}

fn changed() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

/// A download's cover is a file on the phone: its path, which the image
/// loader reads as it is.
fn local_art(mut item: Item) -> Item {
    if let Some(path) = item.thumb.strip_prefix("file:") {
        item.thumb = path.to_string();
    }
    item
}

#[uniffi::export]
impl Tonearm {
    /// Moves whenever a download starts, gets further, finishes or is
    /// removed. Cheap, for polling.
    pub fn downloads_version(&self) -> u64 {
        VERSION.load(Ordering::SeqCst)
    }

    /// Which songs are saved, and which are on their way and how far.
    pub fn download_marks(&self) -> DownloadMarks {
        let version = VERSION.load(Ordering::SeqCst);
        let saved = offline::downloaded_ids();
        let busy = offline::queued()
            .into_iter()
            .filter(|id| !offline::is_downloaded(id))
            .map(|id| DownloadProgress { progress: offline::progress(&id), video_id: id })
            .collect();
        DownloadMarks { saved, busy, version }
    }

    /// What finished or failed since last asked, to show the user.
    pub fn take_download_notes(&self) -> Vec<String> {
        std::mem::take(&mut *NOTES.lock().unwrap())
    }

    /// Downloads songs in the background, skipping any saved or on their
    /// way. Gives what to tell the user.
    pub fn download(&self, items: Vec<Item>) -> String {
        let items: Vec<_> = items.iter().map(pages::core_item).collect();
        let http = innertube::agent(Duration::from_secs(20));
        let fresh = offline::queue(Some(self.resolver.clone()), &http, items);
        if !fresh.is_empty() {
            changed();
        }
        match fresh.as_slice() {
            [] => "Already downloaded".into(),
            [one] => format!("Downloading {}", one.title),
            all => format!("Downloading {} songs", all.len()),
        }
    }

    /// An album's or playlist's menu "Download all": fetches its songs,
    /// then downloads them. Blocks on the network.
    pub fn download_collection(&self, item: Item) -> Result<String, TonearmError> {
        let songs = tonearm_core::menu::collection_songs(&self.client, &pages::core_item(&item))?;
        if songs.is_empty() {
            return Err("There are no songs in it".to_string().into());
        }
        Ok(self.download(songs.iter().map(pages::item).collect()))
    }

    /// A song's menu "Download" or "Remove download". Gives what to tell
    /// the user.
    pub fn toggle_download(&self, item: Item) -> String {
        let Some(video_id) = item.video_id.clone() else {
            return String::new();
        };
        if offline::is_downloaded(&video_id) {
            self.remove_download(video_id)
        } else if offline::is_downloading(&video_id) {
            format!("{} is downloading", item.title)
        } else {
            self.download(vec![item])
        }
    }

    /// Deletes a song's download. Blocks on the disk.
    pub fn remove_download(&self, video_id: String) -> String {
        let title = offline::title_of(&video_id).unwrap_or_default();
        offline::remove(&video_id);
        changed();
        format!("Removed {title} from downloads")
    }

    /// The title of a song being downloaded or saved, for the download
    /// notification. Empty when unknown.
    pub fn download_title(&self, video_id: String) -> String {
        offline::title_of(&video_id)
            .or_else(|| offline::queued_title(&video_id))
            .unwrap_or_default()
    }

    /// Deletes the downloads of several songs, such as an album's, from its
    /// page header. Songs not downloaded are passed over. Blocks on the disk.
    pub fn remove_downloads(&self, video_ids: Vec<String>) -> String {
        let saved: Vec<String> = video_ids.into_iter().filter(|id| offline::is_downloaded(id)).collect();
        for id in &saved {
            offline::remove(id);
        }
        changed();
        match saved.len() {
            0 => "Nothing here is downloaded".into(),
            1 => "Removed 1 song from downloads".into(),
            n => format!("Removed {n} songs from downloads"),
        }
    }

    /// The Downloads page: every saved song, newest first, read from the
    /// phone alone, so it opens with no connection.
    pub fn downloads_page(&self) -> Page {
        let mut page = pages::page(&offline::downloads_page());
        for shelf in &mut page.shelves {
            shelf.items = std::mem::take(&mut shelf.items).into_iter().map(local_art).collect();
        }
        page
    }

    /// How many songs are saved, and the room they take, such as
    /// "12 songs • 96 MB".
    pub fn downloads_summary(&self) -> String {
        let count = offline::downloads().len();
        let (_, bytes) = offline::usage();
        match count {
            0 => "Songs you download play here, even offline".into(),
            1 => format!("1 song • {}", offline::size_text(bytes)),
            n => format!("{n} songs • {}", offline::size_text(bytes)),
        }
    }
}
