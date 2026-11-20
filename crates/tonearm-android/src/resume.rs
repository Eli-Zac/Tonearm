//! What was playing last, back in the player when the app opens, as the
//! YouTube Music app does it.
//!
//! The queue, the song in it and how far in it was are saved as they
//! change (in the desktop's session format), and put back paused on start.
//! Play then opens the song at that place.
//!
//! When signed in, the top of the account's YouTube Music history is read
//! too. If it is a song this phone didn't play and hadn't seen there
//! before, it was played somewhere else since (the website, the app on
//! another device, a TV), so that song takes the player instead, from its
//! start, with its radio after it.

use crate::{song, Song, Tonearm};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tonearm_core::innertube::model::{Item, Kind};
use tonearm_core::session::Session;
use tonearm_core::{audio, store};

/// How often the place in a playing song is saved.
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// A song saved this close to its end starts over instead.
const NEAR_END_MS: u64 = 5000;
/// The history's top song as last read, so it counts as new only once.
const HISTORY_TOP: &str = "history-top";
/// Songs this phone started lately, newest last.
const PLAYED_HERE: &str = "played-here";
const PLAYED_HERE_KEPT: usize = 30;

/// When the session was last saved, and what it looked like then.
pub(crate) struct Saved {
    version: u64,
    state: u32,
    at: Instant,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            version: u64::MAX,
            state: u32::MAX,
            at: Instant::now(),
        }
    }
}

impl Tonearm {
    /// Puts back the queue saved last time, paused at its place.
    pub(crate) fn restore(&self) {
        let Some(saved) = Session::load() else { return };
        let start = saved.queue.iter().take(saved.index).filter(|i| i.video_id.is_some()).count();
        let songs: Vec<Song> = saved.queue.iter().filter_map(from_saved).collect();
        let Some(current) = songs.get(start).cloned() else { return };
        let ms = (saved.position.max(0.0) * 1000.0) as u64;
        let length = (crate::lyrics::seconds_of(&current.duration) * 1000.0) as u64;
        let ms = if length > 0 && ms + NEAR_END_MS > length { 0 } else { ms };
        {
            let mut queue = self.queue.lock().unwrap();
            queue.restore(songs, start, saved.shuffle);
            queue.repeat = saved.repeat;
        }
        *self.resume_at.lock().unwrap() = Some((current.video_id, ms));
        self.changed();
    }

    /// The restored song waiting for play, and where it starts, while it
    /// is still the queue's song and nothing else has played.
    pub(crate) fn waiting(&self) -> Option<(Song, u64)> {
        let at = self.resume_at.lock().unwrap().clone()?;
        let current = self.queue.lock().unwrap().current().cloned()?;
        (current.video_id == at.0).then_some((current, at.1))
    }

    /// Saves the session if the queue changed, the song started or
    /// stopped, or a while has passed playing. From the queue thread.
    pub(crate) fn save_session(&self, state: u32) {
        let version = self.version.load(Ordering::SeqCst);
        {
            let mut saved = self.saved.lock().unwrap();
            let due = version != saved.version
                || state != saved.state
                || (state == audio::PLAYING && saved.at.elapsed() >= SAVE_EVERY);
            if !due {
                return;
            }
            *saved = Saved {
                version,
                state,
                at: Instant::now(),
            };
        }
        let position_ms = match self.waiting() {
            Some((_, ms)) => ms,
            None if !self.loading.load(Ordering::SeqCst)
                && matches!(state, audio::PLAYING | audio::PAUSED)
                && self.audio.status.generation.load(Ordering::SeqCst) == self.generation.load(Ordering::SeqCst) =>
            {
                self.audio.status.position_ms.load(Ordering::SeqCst)
            }
            None => 0,
        };
        // A song found without its length (a search's top result, say)
        // keeps the length the engine measured, for the progress line.
        let status = &self.audio.status;
        let measured = (status.generation.load(Ordering::SeqCst) == self.generation.load(Ordering::SeqCst))
            .then(|| status.duration_ms.load(Ordering::SeqCst) / 1000)
            .filter(|s| *s > 0)
            .map(|s| format!("{}:{:02}", s / 60, s % 60));
        let text = {
            let queue = self.queue.lock().unwrap();
            if queue.items().is_empty() {
                return;
            }
            let mut items: Vec<Item> = queue.items().iter().map(to_saved).collect();
            if let (Some(length), Some(current)) = (measured, items.get_mut(queue.index())) {
                if current.duration.is_empty() {
                    current.duration = length;
                }
            }
            Session {
                queue: items,
                index: queue.index(),
                position: position_ms as f32 / 1000.0,
                volume: 1.0,
                shuffle: queue.shuffle(),
                repeat: queue.repeat,
            }
            .serialize()
        };
        Session::save(text);
    }

    /// Plays the restored song from where it was left, off the caller's
    /// thread (it finds a stream). False when no song is waiting.
    pub(crate) fn play_waiting(self: &Arc<Self>) -> bool {
        if self.waiting().is_none() {
            return false;
        }
        self.play_hold.set(true);
        self.loading.store(true, Ordering::SeqCst);
        let worker = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = worker.play_current() {
                eprintln!("tonearm: the saved song didn't start: {e}");
            }
        });
        true
    }

    /// A song this phone started, for telling plays elsewhere apart.
    pub(crate) fn played_here(&self, video_id: &str) {
        let mut ids: Vec<String> = store::load(PLAYED_HERE)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.is_empty() && *l != video_id)
            .map(String::from)
            .collect();
        ids.push(video_id.to_string());
        let skip = ids.len().saturating_sub(PLAYED_HERE_KEPT);
        let _ = store::save(PLAYED_HERE, &ids[skip..].join("\n"));
    }

    /// This phone's play reached the account's history, at its top.
    pub(crate) fn history_top_is(&self, video_id: &str) {
        let _ = store::save(HISTORY_TOP, video_id);
    }

    /// Reads the top of the account's history and, when something new was
    /// played elsewhere, shows that song in the player. Blocks.
    fn check_elsewhere(&self) {
        if !self.client.signed_in() || self.request.load(Ordering::SeqCst) != 0 {
            return;
        }
        let page = match self.client.browse("FEmusic_history", None) {
            Ok(page) => page,
            Err(message) => {
                eprintln!("tonearm: history: {message}");
                return;
            }
        };
        let Some(top) = page
            .shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .find(|i| i.video_id.is_some())
            .and_then(song)
        else {
            return;
        };
        let seen = store::load(HISTORY_TOP);
        self.history_top_is(&top.video_id);
        let here = store::load(PLAYED_HERE).unwrap_or_default();
        let current = self.queue.lock().unwrap().current().map(|s| s.video_id.clone());
        if seen.as_deref() == Some(top.video_id.as_str())
            || here.lines().any(|l| l == top.video_id)
            || current.as_deref() == Some(top.video_id.as_str())
        {
            return;
        }
        // Only while nothing has been played since the app opened.
        if self.request.load(Ordering::SeqCst) != 0 {
            return;
        }
        let id = top.video_id.clone();
        {
            let mut queue = self.queue.lock().unwrap();
            queue.restore(vec![top], 0, false);
        }
        *self.resume_at.lock().unwrap() = Some((id.clone(), 0));
        self.changed();
        // Its radio after it, as a song card tapped plays.
        let seed = Item {
            kind: Kind::Song,
            video_id: Some(id.clone()),
            ..Item::default()
        };
        let seed = self.client.song_for(&seed).and_then(|s| s.video_id).unwrap_or(id.clone());
        let Ok((items, _)) = self.client.queue(Some(&seed), None) else { return };
        let more: Vec<Song> = items.iter().filter_map(song).filter(|s| s.video_id != id).collect();
        let mut queue = self.queue.lock().unwrap();
        if queue.items().len() == 1 && queue.current().is_some_and(|s| s.video_id == id) {
            queue.extend(more);
            drop(queue);
            self.changed();
        }
    }
}

/// Looks at the account's history once the app has started.
pub(crate) fn check_elsewhere(tonearm: Weak<Tonearm>) {
    std::thread::Builder::new()
        .name("last played".into())
        .spawn(move || {
            if let Some(tonearm) = tonearm.upgrade() {
                tonearm.check_elsewhere();
            }
        })
        .expect("cannot start the history check");
}

/// A queued song as the session keeps it. The album's name is kept even
/// without its page.
fn to_saved(song: &Song) -> Item {
    Item {
        kind: Kind::Song,
        title: song.title.clone(),
        subtitle: song.artist.clone(),
        thumb: song.cover.clone(),
        duration: song.duration.clone(),
        video_id: Some(song.video_id.clone()),
        artists: song.artists.iter().map(|a| (a.name.clone(), a.id.clone())).collect(),
        album: (!song.album.is_empty()).then(|| (song.album.clone(), song.album_id.clone().unwrap_or_default())),
        ..Item::default()
    }
}

fn from_saved(item: &Item) -> Option<Song> {
    let mut song = song(item)?;
    if song.album_id.as_deref() == Some("") {
        song.album_id = None;
    }
    if song.artist.is_empty() {
        song.artist = item.subtitle.clone();
    }
    Some(song)
}
