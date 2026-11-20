//! What the queue thread does a few times a second while songs play, as the
//! desktop's tick does: levelling each song's loudness, crossfading into
//! the next song (or starting it gaplessly with crossfade off), and telling
//! Last.fm and the listening history what was played.

use crate::{song_item, Tonearm};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tonearm_core::listen::Moment;
use tonearm_core::{audio, history, offline, services, transition};

/// How long before the fade the next song starts opening. Its link and
/// first bytes can take a while to come.
const PREPARE_AHEAD: f32 = 30.0;
/// A fade shorter than this, left after a late start, is not worth it.
const SHORTEST_FADE: f32 = 0.5;

/// The song opened ahead, to crossfade or follow into.
pub(crate) struct FadeNext {
    /// Names it to the audio engine.
    token: u64,
    /// The engine generation of the song it follows.
    generation: u64,
    /// Its queue place and video id, to notice the queue changing.
    index: usize,
    video_id: String,
    /// Handed to the engine to start gaplessly when this song ends.
    armed: bool,
}

impl Tonearm {
    /// One tick of the queue thread.
    pub(crate) fn tick(self: &Arc<Self>) {
        let status = &self.audio.status;
        // A song the engine started gaplessly: the queue moves on to it.
        let followed = status.followed.swap(0, Ordering::SeqCst);
        if followed != 0 {
            self.followed(followed);
        }
        let generation = status.generation.load(Ordering::SeqCst);
        let state = status.state.load(Ordering::SeqCst);
        let playing = state == audio::PLAYING;
        let duration = status.duration_ms.load(Ordering::SeqCst) as f32 / 1000.0;
        if playing && self.gained.swap(generation, Ordering::SeqCst) != generation {
            self.apply_gain(generation);
        }
        self.track_listening(generation, playing, duration);
        self.cast_now_playing();
        if playing && !self.loading.load(Ordering::SeqCst) {
            self.crossfade_check(generation, duration);
        }
    }

    // ------------------------------------------------------------ levelling

    /// The gain that brings `video_id` to the chosen loudness. One while
    /// levelling is off or its loudness is not known yet.
    fn gain_for(&self, video_id: &str) -> f32 {
        let lkfs = self.loudness.lock().unwrap().get(video_id).copied().flatten();
        tonearm_core::settings::levelling_gain(self.levelling.load(Ordering::SeqCst), lkfs)
    }

    /// Sends the song under `generation` its levelling gain, looking its
    /// loudness up first when that is not known yet.
    pub(crate) fn apply_gain(self: &Arc<Self>, generation: u64) {
        let Some(video_id) = self.now_playing().map(|s| s.video_id) else { return };
        let known = self.loudness.lock().unwrap().contains_key(&video_id);
        if known {
            self.audio.gain(generation, self.gain_for(&video_id));
            return;
        }
        // Asked once: None marks it asked.
        self.loudness.lock().unwrap().insert(video_id.clone(), None);
        let worker = self.clone();
        std::thread::spawn(move || {
            let lkfs = worker.client.loudness(&video_id).ok().flatten();
            {
                let mut known = worker.loudness.lock().unwrap();
                if known.len() > 500 {
                    known.clear();
                }
                known.insert(video_id.clone(), lkfs);
            }
            worker.audio.gain(generation, worker.gain_for(&video_id));
        });
    }

    // ------------------------------------------------------------ crossfade

    /// Opens the next song ahead of the end, and once the playing song
    /// reaches the fade, mixes into it, as the desktop's `crossfade_check`
    /// does without beat matching. With crossfade off the next song is
    /// handed to the engine to start the moment this one ends.
    fn crossfade_check(self: &Arc<Self>, generation: u64, duration: f32) {
        if duration <= 0.0 {
            return;
        }
        let target = {
            let queue = self.queue.lock().unwrap();
            queue.follows().and_then(|i| queue.items().get(i).map(|s| (i, s.video_id.clone())))
        };
        let Some((index, video_id)) = target else {
            // Nothing should follow any more, as under repeat one.
            if self.fade.lock().unwrap().take().is_some_and(|n| n.armed) {
                self.audio.unfollow();
            }
            return;
        };
        let status = &self.audio.status;
        let position = status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0;
        let left = duration - position;
        let seconds = self.crossfade.load(Ordering::SeqCst) as f32;
        let opened = self
            .fade
            .lock()
            .unwrap()
            .as_ref()
            .filter(|n| n.generation == generation && n.index == index && n.video_id == video_id)
            .map(|n| (n.token, n.armed));
        let Some((token, armed)) = opened else {
            if left <= seconds + PREPARE_AHEAD {
                self.prepare_next(generation, index, video_id);
            }
            return;
        };
        let ready = status.prepared.load(Ordering::SeqCst) == token;
        if seconds <= 0.0 {
            // Gapless: the engine starts it to the frame.
            if !armed && ready {
                self.audio.follow(token, self.gain_for(&video_id));
                if let Some(next) = self.fade.lock().unwrap().as_mut() {
                    next.armed = true;
                }
            }
            return;
        }
        if left > seconds || !ready || left < SHORTEST_FADE.min(seconds) {
            return;
        }
        // Reached late, it mixes over what is left rather than cutting the
        // old song short.
        let settings = transition::Settings::auto(None, None, false);
        let plan = transition::plan(settings, None, duration as f64, None, left.min(seconds) as f64);
        self.fade.lock().unwrap().take();
        let generation = self.move_on(index);
        self.audio.crossfade(token, generation, plan);
    }

    /// Opens the song at `index` in the engine, ready to fade in.
    fn prepare_next(self: &Arc<Self>, generation: u64, index: usize, video_id: String) {
        let token = self.fade_token.fetch_add(1, Ordering::SeqCst) + 1;
        let old = self.fade.lock().unwrap().replace(FadeNext {
            token,
            generation,
            index,
            video_id: video_id.clone(),
            armed: false,
        });
        // A song armed for the old order would start in its place.
        if old.is_some_and(|n| n.armed) {
            self.audio.unfollow();
        }
        if let Some(path) = offline::local_file(&video_id) {
            self.audio.prepare(audio::Input::File(path), token);
            return;
        }
        let worker = self.clone();
        std::thread::spawn(move || {
            let result = worker.resolver.resolve(&video_id);
            let still = worker.fade.lock().unwrap().as_ref().is_some_and(|n| n.token == token);
            match result {
                Ok(stream) if still => worker.audio.prepare(
                    audio::Input::Http {
                        url: stream.url,
                        headers: stream.headers,
                        cache_as: Some(video_id),
                    },
                    token,
                ),
                // Left in place, so it is not tried again every tick.
                Err(message) => eprintln!("tonearm: crossfade: {message}"),
                _ => {}
            }
        });
    }

    /// The engine started the armed next song gaplessly.
    fn followed(self: &Arc<Self>, token: u64) {
        let next = self.fade.lock().unwrap().take();
        let Some(next) = next.filter(|n| n.token == token) else { return };
        let generation = self.move_on(next.index);
        self.audio.adopt(generation);
    }

    /// Moves the queue on to `index`, which the engine is starting, under a
    /// new generation.
    fn move_on(&self, index: usize) -> u64 {
        self.queue.lock().unwrap().go(index);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.seek_hold.clear();
        self.changed();
        generation
    }

    /// The crossfade setting changed: the next song's place is planned again.
    pub(crate) fn forget_fade(&self) {
        if self.fade.lock().unwrap().take().is_some_and(|n| n.armed) {
            self.audio.unfollow();
        }
    }

    // ------------------------------------------------------------ listening

    /// Tells Last.fm what is going on, and keeps the listening history, as
    /// the desktop's `track_listening`.
    fn track_listening(self: &Arc<Self>, generation: u64, playing: bool, duration: f32) {
        let moments = self.listen.lock().unwrap().tick(generation, playing, duration);
        for moment in moments {
            match moment {
                Moment::Started => {
                    let Some(song) = self.now_playing() else { continue };
                    self.remember(&song, 0.25);
                    self.played_here(&song.video_id);
                    let seconds = if duration > 0.0 { duration } else { crate::lyrics::seconds_of(&song.duration) };
                    let item = song_item(&song);
                    self.services.send(services::Event::Started(services::Track {
                        title: song.title.clone(),
                        lead_artist: tonearm_core::listen::lead_artist(&item.artists, &song.artist),
                        artist: song.artist.clone(),
                        album: song.album.clone(),
                        seconds,
                        art: song.cover.clone(),
                    }));
                }
                Moment::Scrobble => self.services.send(services::Event::Scrobble),
                Moment::Counted => {
                    let Some(song) = self.now_playing() else { continue };
                    self.remember(&song, 0.75);
                    // And in the account's YouTube Music history, as a play
                    // in YouTube Music's own player would be.
                    if self.client.signed_in() {
                        let worker = self.clone();
                        std::thread::spawn(move || {
                            match worker.client.add_to_history(&song.video_id) {
                                Ok(()) => worker.history_top_is(&song.video_id),
                                Err(message) => eprintln!("tonearm: history: {message}"),
                            }
                        });
                    }
                }
                Moment::Paused => self.services.send(services::Event::Paused),
                Moment::Resumed => {
                    let position = self.audio.status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0;
                    self.services.send(services::Event::Resumed { position });
                }
            }
        }
    }

    fn remember(&self, song: &crate::Song, weight: f64) {
        let text = {
            let mut taste = self.taste.lock().unwrap();
            taste.record(&song_item(song), weight);
            taste.serialize()
        };
        history::History::save(text);
    }
}
