//! The cast button, as the desktop's `casting`: the Cast devices on the
//! network, and the cast to the one picked. The engine's sound goes to the
//! device as a stream served from this phone (the core's `cast`), so
//! crossfades carry over.
//!
//! Android finds the devices (NsdManager) and tells the core through
//! `cast_seen` / `cast_lost`. The app polls `cast_state`.

use crate::Tonearm;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tonearm_core::cast;

/// What the device shows as playing until it hears of a song.
const TITLE: &str = "Tonearm";

#[derive(uniffi::Enum, Clone, Copy, PartialEq, Debug)]
pub enum CastKind {
    Speaker,
    Screen,
    Group,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct CastDevice {
    pub name: String,
    pub model: String,
    pub kind: CastKind,
}

/// Where the cast stands, for the Devices sheet and the cast button.
#[derive(uniffi::Record, Clone, Debug, PartialEq)]
pub struct CastState {
    /// By name. The one cast to stays listed while it is.
    pub devices: Vec<CastDevice>,
    /// The device in `devices` cast to; -1 for this phone.
    pub active: i32,
    /// Cast to, but not playing there yet.
    pub connecting: bool,
    /// The device's volume, 0 to 1, while casting.
    pub volume: f32,
}

#[derive(Default)]
struct Casting {
    finder: Option<cast::Finder>,
    /// The devices as last listed, in `cast_state`'s order.
    devices: Vec<cast::Device>,
    session: Option<cast::Session>,
    connecting: bool,
    volume: f32,
    /// Counts casts, so word from one that has ended is ignored.
    generation: u64,
    /// The lyrics the device has, by video id and line count.
    lyrics_sent: Option<String>,
    /// The song whose lyrics the cast last asked for itself, so each is
    /// asked for once.
    lyrics_asked: Option<String>,
    /// When the device was last told the song playing.
    told: Option<Instant>,
    /// Said once, then taken by the app as a toast.
    note: Option<String>,
}

static CASTING: Mutex<Option<Casting>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Casting) -> R) -> R {
    f(CASTING.lock().unwrap().get_or_insert_with(Casting::default))
}

fn ip(address: &str) -> Option<std::net::IpAddr> {
    address.trim_start_matches('/').parse().ok()
}

#[uniffi::export]
impl Tonearm {
    /// A Cast device Android found: its service's instance name, TXT
    /// record as keys and values, address and port.
    pub fn cast_seen(&self, instance: String, keys: Vec<String>, values: Vec<String>, address: String, port: u16) {
        let Some(ip) = ip(&address) else { return };
        let txt: Vec<(String, String)> = keys.into_iter().zip(values).collect();
        // The bindings are made from a Windows build, which finds devices
        // itself and has neither.
        #[cfg(not(windows))]
        cast::seen(&instance, &txt, ip, port);
        #[cfg(windows)]
        let _ = (instance, txt, ip, port);
    }

    pub fn cast_lost(&self, instance: String) {
        #[cfg(not(windows))]
        cast::lost(&instance);
        #[cfg(windows)]
        let _ = instance;
    }

    /// The devices found and the cast, if any. Starts the finder the
    /// first time.
    pub fn cast_state(&self) -> CastState {
        with(|c| {
            if c.finder.is_none() {
                c.finder = Some(cast::Finder::start(|| {}));
            }
            let mut devices = c.finder.as_ref().map(|f| f.devices()).unwrap_or_default();
            if let Some(session) = &c.session {
                if !devices.iter().any(|d| d.id == session.device.id) {
                    devices.push(session.device.clone());
                }
            }
            let active = c
                .session
                .as_ref()
                .and_then(|s| devices.iter().position(|d| d.id == s.device.id))
                .map_or(-1, |i| i as i32);
            c.devices = devices;
            CastState {
                devices: c
                    .devices
                    .iter()
                    .map(|d| CastDevice {
                        name: d.name.clone(),
                        model: d.model.clone(),
                        kind: if d.group {
                            CastKind::Group
                        } else if d.screen {
                            CastKind::Screen
                        } else {
                            CastKind::Speaker
                        },
                    })
                    .collect(),
                active,
                connecting: c.connecting,
                volume: c.volume,
            }
        })
    }

    /// A line of the Devices sheet: a device by its place in the last
    /// `cast_state`, or -1 for this phone. `phone` is what the device shows
    /// it plays from.
    pub fn cast_pick(self: Arc<Self>, index: i32, phone: String) {
        let device = usize::try_from(index).ok().and_then(|i| with(|c| c.devices.get(i).cloned()));
        let current = with(|c| c.session.as_ref().map(|s| s.device.id.clone()));
        if current.is_some() && current == device.as_ref().map(|d| d.id.clone()) {
            return;
        }
        // From one device straight to another, without a moment here.
        self.end_cast(None, false, device.is_none());
        let Some(device) = device else { return };
        let pipe = cast::Pipe::new();
        // The song stops here at once and carries on from the same place
        // on the device once it plays. Routed before the cast starts, so
        // a cast that fails at once routes it back here after, not before.
        self.audio.route(Some(pipe.clone()));
        let me = Arc::downgrade(&self);
        // Started and kept under one lock: a cast that fails at once (a
        // device gone from its address refuses in a moment) has its end
        // wait for the session to be kept. Kept after, the end found no
        // session, and the sheet said "Connecting…" for ever.
        with(|c| {
            c.generation += 1;
            let generation = c.generation;
            c.session = Some(cast::Session::start(
                device,
                pipe,
                TITLE.into(),
                format!("Playing from {phone}"),
                move |event| cast_event(&me, generation, event),
            ));
            c.connecting = true;
            c.volume = 1.0;
            c.lyrics_sent = None;
            c.lyrics_asked = None;
            c.told = None;
        });
    }

    /// Sets the device's volume, 0 to 1, while casting.
    pub fn cast_volume(&self, level: f32) {
        with(|c| {
            if let Some(session) = &c.session {
                let level = level.clamp(0.0, 1.0);
                session.volume(level);
                c.volume = level;
            }
        });
    }

    pub fn casting(&self) -> bool {
        with(|c| c.session.is_some())
    }

    /// What to tell the user about the cast, once: why it ended.
    pub fn take_cast_note(&self) -> Option<String> {
        with(|c| c.note.take())
    }
}

fn cast_event(me: &Weak<Tonearm>, generation: u64, event: cast::Event) {
    let Some(tonearm) = me.upgrade() else { return };
    if with(|c| c.generation) != generation {
        return;
    }
    match event {
        cast::Event::Playing => with(|c| {
            c.connecting = false;
            // Lyrics sent while the device was starting may be lost.
            c.lyrics_sent = None;
            c.told = None;
        }),
        cast::Event::Volume(level) => with(|c| c.volume = level.clamp(0.0, 1.0)),
        cast::Event::Paused(true) => tonearm.pause(),
        cast::Event::Paused(false) => tonearm.clone().resume(),
        cast::Event::Next => {
            let _ = tonearm.next();
        }
        cast::Event::Previous => {
            let _ = tonearm.previous();
        }
        cast::Event::Ended(reason) => {
            let name = with(|c| c.session.as_ref().map(|s| s.device.name.clone())).unwrap_or_default();
            tonearm.end_cast(Some(reason.unwrap_or_else(|| format!("{name} stopped playing"))), true, true);
        }
    }
}

impl Tonearm {
    /// Stops casting, if casting, and with `here` plays on this phone
    /// again. A cast that ended on its own pauses first, so the music
    /// doesn't jump to the phone's speaker.
    fn end_cast(&self, note: Option<String>, pause: bool, here: bool) {
        let session = with(|c| {
            c.generation += 1;
            c.connecting = false;
            if c.session.is_some() {
                c.note = note;
            }
            c.session.take()
        });
        let Some(session) = session else { return };
        // Dropped on its own thread: ending says goodbye to the device.
        std::thread::spawn(move || drop(session));
        if pause {
            self.pause();
        }
        if here {
            self.audio.route(None);
        }
        self.changed();
    }

    /// How far behind the engine the device plays, to take off the time
    /// shown. Zero when not casting.
    pub(crate) fn cast_lag_ms(&self) -> u64 {
        if with(|c| c.session.is_none()) {
            return 0;
        }
        self.audio.status.lag_ms.load(Ordering::SeqCst)
    }

    /// Tells Tonearm's own receiver the song playing, about once a second,
    /// and its synced lyrics once found. From the queue thread's tick.
    pub(crate) fn cast_now_playing(self: &Arc<Self>) {
        let shows = with(|c| {
            c.session.as_ref().is_some_and(|s| s.shows_songs())
                && c.told.is_none_or(|t| t.elapsed() >= Duration::from_secs(1))
        });
        if !shows {
            return;
        }
        let Some(song) = self.now_playing() else { return };
        // The app finds lyrics only while its screen is open. With the
        // phone locked or the app in the background the cast asks for
        // them itself, once a song, so the device still shows them.
        let known = self.lyrics.lock().unwrap().contains_key(&song.video_id);
        let ask = !known
            && with(|c| {
                let new = c.lyrics_asked.as_deref() != Some(song.video_id.as_str());
                if new {
                    c.lyrics_asked = Some(song.video_id.clone());
                }
                new
            });
        if ask {
            let me = self.clone();
            let (id, title, artist) = (song.video_id.clone(), song.title.clone(), song.artist.clone());
            std::thread::spawn(move || {
                me.core_lyrics(id, title, artist, 0.0);
            });
        }
        let status = &self.audio.status;
        let position = status.position_ms.load(Ordering::SeqCst).saturating_sub(status.lag_ms.load(Ordering::SeqCst));
        let playing = status.state.load(Ordering::SeqCst) == tonearm_core::audio::PLAYING;
        let lines: Option<(String, Vec<cast::LyricLine>)> = self.lyrics.lock().unwrap().get(&song.video_id).filter(|l| l.synced).map(|found| {
            let translated = found.lines.iter().filter(|l| !l.translation.is_empty()).count();
            (
                format!("{}/{}/{translated}", song.video_id, found.lines.len()),
                found
                    .lines
                    .iter()
                    .map(|l| cast::LyricLine {
                        start: l.start,
                        end: l.end,
                        text: l.text.clone(),
                        background: l.background_text.clone(),
                        translation: l.translation.clone(),
                        syllables: l.syllables.clone(),
                        background_syllables: l.background.clone(),
                        align: l.align,
                    })
                    .collect(),
            )
        });
        with(|c| {
            let Some(session) = &c.session else { return };
            if let Some((key, lines)) = lines {
                if c.lyrics_sent.as_deref() != Some(key.as_str()) {
                    session.lyrics(&song.video_id, &lines);
                    c.lyrics_sent = Some(key);
                }
            }
            session.now_playing(cast::NowPlaying {
                id: song.video_id.clone(),
                title: song.title.clone(),
                artist: song.artist.clone(),
                album: song.album.clone(),
                // A downloaded song's cover is a file here, out of the device's reach.
                cover: if song.cover.starts_with("http") { song.cover.clone() } else { String::new() },
                duration: status.duration_ms.load(Ordering::SeqCst) as f32 / 1000.0,
                position: position as f32 / 1000.0,
                paused: !playing,
            });
            c.told = Some(Instant::now());
        });
    }
}
