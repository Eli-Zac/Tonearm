//! Android Auto's side: the queue a choice in the car's browse list plays,
//! worked out as the phone's own taps work it out, but handed back rather
//! than started, since Media3 starts it through the player.

use crate::pages::{self, Item};
use crate::{song, songs_of, Song, Tonearm, TonearmError};

#[uniffi::export]
impl Tonearm {
    /// A song picked in a list (an album's tracks, a playlist, Downloads):
    /// the list's songs, to play from that one, as `play_list`.
    pub fn list_songs(&self, items: Vec<Item>) -> Vec<Song> {
        items.iter().map(pages::core_item).filter_map(|i| song(&i)).collect()
    }

    /// A song card picked: the song, then its radio, as `play_radio`.
    /// Blocks on the network.
    pub fn radio_songs(&self, item: Item) -> Result<Vec<Song>, TonearmError> {
        let core = pages::core_item(&item);
        let core = self.client.song_for(&core).unwrap_or(core);
        let first = song(&core).ok_or_else(|| "Nothing to play".to_string())?;
        let id = first.video_id.clone();
        let (items, _) = self.client.queue(Some(&id), None)?;
        let mut songs = vec![first];
        songs.extend(items.iter().filter_map(song).filter(|s| s.video_id != id));
        Ok(songs)
    }

    /// An album, playlist or mix picked to play: its songs, as `play_item`.
    /// Blocks on the network.
    pub fn item_songs(&self, item: Item) -> Result<Vec<Song>, TonearmError> {
        if item.video_id.is_some() {
            return self.radio_songs(item);
        }
        let items = match (&item.playlist_id, &item.browse_id) {
            (Some(playlist), _) => self.client.queue(None, Some(playlist))?.0,
            (None, Some(browse)) => songs_of(&self.client.browse(browse, item.params.as_deref())?),
            (None, None) => Vec::new(),
        };
        let songs: Vec<Song> = items.iter().filter_map(song).collect();
        if songs.is_empty() {
            return Err("Nothing to play".to_string().into());
        }
        Ok(songs)
    }
}
