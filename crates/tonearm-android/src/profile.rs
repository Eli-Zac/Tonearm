//! The profile page, as the desktop's: what the history on this phone says
//! you play (the songs played lately, your top artists, how many songs,
//! artists and pages), read from the core's `history`. No network.

use crate::pages::{self, Item};
use crate::Tonearm;
use tonearm_core::history;

/// One of the artists played most, and their page when the history links it.
#[derive(uniffi::Record, Clone, Debug)]
pub struct TopArtist {
    pub name: String,
    pub id: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct Profile {
    /// Songs played lately, newest first, as the desktop's Played lately.
    pub played: Vec<Item>,
    /// Best first.
    pub artists: Vec<TopArtist>,
    pub songs: u32,
    pub artist_count: u32,
    /// Albums, playlists and artist pages opened.
    pub opened: u32,
}

#[uniffi::export]
impl Tonearm {
    /// What the profile page shows. Quick: it only reads the history.
    pub fn profile(&self) -> Profile {
        let taste = self.taste.lock().unwrap();
        let (songs, artist_count, opened) = taste.counts();
        Profile {
            played: taste.recent(true, 12).iter().map(pages::item).collect(),
            artists: taste
                .top_artist_names(5)
                .into_iter()
                .map(|name| TopArtist { id: taste.artist_id(&name), name })
                .collect(),
            songs: songs as u32,
            artist_count: artist_count as u32,
            opened: opened as u32,
        }
    }

    /// An album, artist or playlist page was opened: that says something
    /// about taste, as on the desktop. Songs are counted when they play.
    pub fn remember_open(&self, item: Item) {
        if item.video_id.is_some() {
            return;
        }
        let text = {
            let mut taste = self.taste.lock().unwrap();
            taste.record(&pages::core_item(&item), 0.5);
            taste.serialize()
        };
        std::thread::spawn(move || history::History::save(text));
    }
}
