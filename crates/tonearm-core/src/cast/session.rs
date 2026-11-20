//! One cast: opens the receiver app on the device, has it play our
//! stream, and keeps the two in step until it ends.
//!
//! Devices get Tonearm's own receiver (the page in `receiver/`) when the
//! build has its id in `TONEARM_CAST_APP`, and are told the song playing:
//! a screen shows it, and the media status carries it to a smart clock's
//! display, the Google Home app and phone notifications. Speaker groups,
//! and any device that won't open ours, get Google's Default Media
//! Receiver.
//!
//! Pause and play follow the engine, through the pipe. Volume goes to the
//! device, apart from TVs known to ignore it (LG's), where it scales the
//! stream instead. When someone pauses or changes the volume on the device
//! itself (its remote, or the Google Home app), the app hears of it.

use super::channel::{Channel, Message, CONNECTION, HEARTBEAT, MEDIA, PLATFORM, RECEIVER};
use super::stream::{Pipe, Server};
use super::Device;
use crate::lyrics::Syllable;
use serde_json::{json, Value};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Google's Default Media Receiver. It needs no registration.
const DEFAULT_APP: &str = "CC1AD845";
/// Tonearm's own receiver, when the build has its id.
const TONEARM_APP: Option<&str> = match option_env!("TONEARM_CAST_APP") {
    Some(id) if !id.is_empty() => Some(id),
    _ => None,
};
/// Where Tonearm's receiver hears about the song playing.
const TONEARM: &str = "urn:x-cast:app.tonearm";
/// A song's position this far from where the receiver's own clock puts
/// it is a seek, and the receiver is told.
const DRIFT: f32 = 1.5;
const PING_EVERY: Duration = Duration::from_secs(5);
/// No word at all from the device for this long: it has gone.
const SILENT: Duration = Duration::from_secs(20);
/// How often to ask how far the device has played.
const STATUS_EVERY: Duration = Duration::from_secs(1);
/// How long a device has to take up a volume change before the cast
/// scales the stream instead.
const VOLUME_IGNORED: Duration = Duration::from_millis(1500);
/// A device that has not fetched the stream this long after being told
/// the link never will: something between is turning it away.
const UNREACHED: Duration = Duration::from_secs(12);

/// What happens to a cast, for the app.
#[derive(Debug)]
pub enum Event {
    /// The device has started playing the stream.
    Playing,
    /// The device's volume, from 0 to 1, as it stands now.
    Volume(f32),
    /// Paused or played on the device itself.
    Paused(bool),
    /// Skip forward or back a song, asked for on the device or a phone
    /// following the cast. Only Tonearm's own receiver passes these on.
    Next,
    Previous,
    /// The cast is over. With a reason when it did not end at our asking.
    Ended(Option<String>),
}

/// The song playing, for a receiver with a screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    /// Tells songs apart. Its video id.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The cover's link.
    pub cover: String,
    /// Seconds. Zero when unknown.
    pub duration: f32,
    /// Seconds into the song, as heard on the device.
    pub position: f32,
    pub paused: bool,
}

/// The receiver to open on a device: ours, apart from speaker groups,
/// which play Google's.
fn app_for(device: &Device) -> &'static str {
    match TONEARM_APP {
        Some(app) if !device.group => app,
        _ => DEFAULT_APP,
    }
}

/// Whether the device's volume is no use, so the cast scales the stream
/// instead. LG's TVs say their volume can be set, and even report the new
/// level back, but play on as loud as before. They don't say who made
/// them: their model is a sales code like "86UT8050PSB.AAUFLJD", where the
/// dot is LG's.
fn ignores_its_volume(device: &Device) -> bool {
    let model = device.model.to_ascii_lowercase();
    device.screen && !device.group && (model.starts_with("lg") || model.contains("webos") || model.contains('.'))
}

/// A line of synced lyrics, for a receiver with a screen.
#[derive(Clone, Debug, Default)]
pub struct LyricLine {
    /// Milliseconds into the song.
    pub start: u32,
    pub end: u32,
    pub text: String,
    /// Backing vocals sung with it.
    pub background: String,
    /// The translation on show under it, if any.
    pub translation: String,
    /// The line's own timing of each syllable, when the lyrics have it.
    /// A syllable that ends a word ends with a space.
    pub syllables: Vec<Syllable>,
    /// The same for the backing vocals.
    pub background_syllables: Vec<Syllable>,
    /// Where the singer's lines sit: 0 left, 1 right, 2 centred.
    pub align: u8,
}

/// Cast messages stop at 64 KB. Lyrics larger than this lose their
/// syllable timing, then their translations, then their backing vocals,
/// then don't go at all.
const LYRICS_MAX: usize = 56 * 1024;

/// Syllables, small: `[start, end, text]` each.
fn timed(syllables: &[Syllable]) -> Value {
    Value::Array(syllables.iter().map(|s| json!([s.start, s.end, s.text])).collect())
}

/// The message that gives the receiver a song's lyrics.
fn lyrics_message(id: &str, lines: &[LyricLine]) -> Option<Value> {
    let none = || Value::Array(Vec::new());
    for (syllables, translations, backing) in [(true, true, true), (false, true, true), (false, false, true), (false, false, false)] {
        let lines: Vec<Value> = lines
            .iter()
            .map(|l| {
                json!({
                    "start": l.start,
                    "end": l.end,
                    "text": l.text,
                    "align": l.align,
                    "syllables": if syllables { timed(&l.syllables) } else { none() },
                    "background": if backing { l.background.as_str() } else { "" },
                    "backgroundSyllables": if syllables && backing { timed(&l.background_syllables) } else { none() },
                    "translation": if translations { l.translation.as_str() } else { "" },
                })
            })
            .collect();
        let message = json!({"type": "LYRICS", "id": id, "lines": lines});
        if message.to_string().len() <= LYRICS_MAX {
            return Some(message);
        }
    }
    None
}

enum Command {
    Volume(f32),
    Now(NowPlaying),
    Lyrics(Value),
    Stop,
}

/// A cast in progress. Dropping it ends the cast.
pub struct Session {
    tx: Sender<Command>,
    /// Closes when the cast thread is done.
    done: Receiver<()>,
    pub device: Device,
    /// Set once Tonearm's own receiver is up, which shows the song.
    shows_songs: Arc<AtomicBool>,
}

impl Session {
    /// Starts casting to `device`. The engine should route to `pipe`
    /// straight after. `title` and `subtitle` are what the device shows.
    pub fn start(
        device: Device,
        pipe: Arc<Pipe>,
        title: String,
        subtitle: String,
        events: impl Fn(Event) + Send + 'static,
    ) -> Session {
        let (tx, rx) = mpsc::channel();
        let (finished, done) = mpsc::channel::<()>();
        let target = device.clone();
        let shows_songs = Arc::new(AtomicBool::new(false));
        let shows = shows_songs.clone();
        std::thread::Builder::new()
            .name("cast".into())
            .spawn(move || {
                let _finished = finished;
                let mut cast = Cast {
                    app: app_for(&target),
                    shows_songs: shows,
                    device: target,
                    pipe: pipe.clone(),
                    events: Box::new(events),
                    request: 0,
                };
                let result = cast.run(&rx, &title, &subtitle);
                pipe.close();
                (cast.events)(Event::Ended(result.err()));
            })
            .expect("cannot start the cast thread");
        Session {
            tx,
            done,
            device,
            shows_songs,
        }
    }

    pub fn volume(&self, level: f32) {
        let _ = self.tx.send(Command::Volume(level));
    }

    /// A song's synced lyrics, by the id its `NowPlaying` has. The receiver
    /// keeps them for when that song shows. Send them again after a cast
    /// starts playing: ones sent while it was starting can be lost.
    pub fn lyrics(&self, id: &str, lines: &[LyricLine]) {
        if self.shows_songs() {
            if let Some(message) = lyrics_message(id, lines) {
                let _ = self.tx.send(Command::Lyrics(message));
            }
        }
    }

    /// Whether the device shows the song playing, so wants `now_playing`.
    pub fn shows_songs(&self) -> bool {
        self.shows_songs.load(Ordering::Relaxed)
    }

    /// The song playing now. Cheap, so call it as often as the playhead
    /// moves: the device is told only of changes.
    pub fn now_playing(&self, now: NowPlaying) {
        if self.shows_songs() {
            let _ = self.tx.send(Command::Now(now));
        }
    }

    /// Ends the cast and waits a moment for the device to be told, as
    /// when Tonearm quits.
    pub fn finish(self) {
        let _ = self.tx.send(Command::Stop);
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}

struct Cast {
    /// The receiver app opened on the device.
    app: &'static str,
    shows_songs: Arc<AtomicBool>,
    device: Device,
    pipe: Arc<Pipe>,
    events: Box<dyn Fn(Event) + Send>,
    request: u64,
}

impl Cast {
    fn next_request(&mut self) -> u64 {
        self.request += 1;
        self.request
    }

    fn run(&mut self, rx: &Receiver<Command>, title: &str, subtitle: &str) -> Result<(), String> {
        let name = self.device.name.clone();
        let mut channel = Channel::connect(self.device.addr).map_err(|e| format!("Couldn't connect to {name}: {e}"))?;
        let server = Server::start(channel.local_ip, self.pipe.clone())?;
        channel.send(PLATFORM, CONNECTION, &json!({"type": "CONNECT"}))?;
        let mut launch = self.next_request();
        channel.send(PLATFORM, RECEIVER, &json!({"type": "LAUNCH", "appId": self.app, "requestId": launch}))?;

        // Wait for the receiver app to come up and say where to reach it.
        // Only once the device has answered our launch: until then a status
        // can still name the session of an earlier cast, which the launch
        // is about to replace.
        let started = Instant::now();
        // The device's volume as it last reported it, to tell its own
        // changes from ours.
        let mut volume: Option<f32> = None;
        // The volume is the stream's scale, not the device's. It starts
        // full, which is how loud the device's own remote has it.
        let mut scaled = ignores_its_volume(&self.device);
        let mut answered = false;
        let (transport, session) = loop {
            if started.elapsed() > Duration::from_secs(20) {
                return Err(format!("{name} didn't start the player"));
            }
            if let Ok(Command::Stop) = rx.try_recv() {
                return Ok(());
            }
            let Some(message) = channel.poll()? else { continue };
            self.ping_back(&mut channel, &message)?;
            let p = &message.payload;
            if std::env::var_os("TONEARM_CAST_DEBUG").is_some() {
                eprintln!("cast launch {launch}: {} {p}", message.namespace);
            }
            match p["type"].as_str() {
                Some("RECEIVER_STATUS") => {
                    // The slider shows the device's volume from the start.
                    if let Some(level) = p["status"]["volume"]["level"].as_f64() {
                        if volume.is_none() {
                            (self.events)(Event::Volume(if scaled { 1.0 } else { level as f32 }));
                        }
                        volume = Some(level as f32);
                    }
                    answered |= p["requestId"].as_u64() == Some(launch);
                    if let Some(app) = our_app(p, self.app).filter(|_| answered) {
                        break app;
                    }
                }
                // A device that won't open ours, such as one not yet allowed
                // to while it is unpublished, gets Google's instead.
                Some("LAUNCH_ERROR") if self.app != DEFAULT_APP => {
                    eprintln!("cast: {name} wouldn't open {} ({}), using the default receiver", self.app, p["reason"]);
                    self.app = DEFAULT_APP;
                    launch = self.next_request();
                    channel.send(PLATFORM, RECEIVER, &json!({"type": "LAUNCH", "appId": self.app, "requestId": launch}))?;
                }
                Some("LAUNCH_ERROR") => {
                    return Err(format!("{name} couldn't start the player ({})", p["reason"].as_str().unwrap_or("no reason")))
                }
                _ => {}
            }
        };
        channel.send(&transport, CONNECTION, &json!({"type": "CONNECT"}))?;
        self.shows_songs.store(self.app != DEFAULT_APP, Ordering::Relaxed);
        let load = self.next_request();
        channel.send(
            &transport,
            MEDIA,
            &json!({
                "type": "LOAD",
                "requestId": load,
                "sessionId": session,
                "media": {
                    "contentId": server.url,
                    "contentUrl": server.url,
                    "contentType": "audio/wav",
                    "streamType": "LIVE",
                    "metadata": {"metadataType": 3, "title": title, "artist": subtitle},
                },
                "autoplay": true,
            }),
        )?;

        let mut media: Option<u64> = None;
        let mut playing = false;
        // The pause state the device was last told, to notice the engine's.
        let mut told_paused = false;
        let mut wanted_volume: Option<f32> = None;
        let mut volume_sent = Instant::now() - Duration::from_secs(1);
        // A device that doesn't take up a level by this long after it was
        // sent moves the cast to scaling the stream.
        let mut volume_check: Option<(f32, Instant)> = None;
        // The level last asked for here.
        let mut sent_volume: Option<f32> = None;
        let mut heard = Instant::now();
        let mut pinged = Instant::now();
        let mut asked = Instant::now();
        let loaded = Instant::now();
        // Until the device plays, the song isn't moving there.
        let mut screen = Screen {
            held: true,
            ..Screen::default()
        };
        loop {
            if !self.pipe.reached() && loaded.elapsed() > UNREACHED {
                return Err(format!(
                    "{name} couldn't reach this computer. Allow Tonearm through Windows Firewall, \
                     or set this network to private in Windows settings"
                ));
            }
            match rx.try_recv() {
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    let id = self.next_request();
                    let _ = channel.send(PLATFORM, RECEIVER, &json!({"type": "STOP", "sessionId": session, "requestId": id}));
                    return Ok(());
                }
                Ok(Command::Volume(level)) => {
                    let level = level.clamp(0.0, 1.0);
                    if scaled {
                        self.pipe.set_gain(gain_for(level));
                    }
                    wanted_volume = Some(level);
                }
                Ok(Command::Now(now)) => screen.update(now, self.pipe.lead()),
                Ok(Command::Lyrics(message)) => channel.send(&transport, TONEARM, &message)?,
                Err(TryRecvError::Empty) => {}
            }
            if let Some(now) = screen.due() {
                channel.send(
                    &transport,
                    TONEARM,
                    &json!({
                        "type": "NOW",
                        "id": now.id,
                        "title": now.title,
                        "artist": now.artist,
                        "album": now.album,
                        "cover": now.cover,
                        "duration": now.duration,
                        "position": now.position,
                        "paused": now.paused,
                    }),
                )?;
            }
            // A slider dragged sends a stream of levels. The device gets
            // one every tenth of a second.
            if volume_check.is_some_and(|(_, at)| at.elapsed() > VOLUME_IGNORED) {
                let (level, _) = volume_check.take().unwrap();
                eprintln!("cast: {name} ignored a volume change, scaling the stream instead");
                scaled = true;
                self.pipe.set_gain(gain_for(level));
            }
            // The device is told the level even when it ignores it, so a
            // phone following the cast shows the same as the slider.
            if let Some(level) = wanted_volume {
                if volume_sent.elapsed() >= Duration::from_millis(100) {
                    let id = self.next_request();
                    channel.send(PLATFORM, RECEIVER, &json!({"type": "SET_VOLUME", "volume": {"level": level}, "requestId": id}))?;
                    if !scaled {
                        volume_check.get_or_insert((level, Instant::now())).0 = level;
                        volume = Some(level);
                    }
                    // A device that ignores its volume reports its own level
                    // on, unchanged: `volume` stays that, and ours is kept
                    // apart, so neither report reads as someone else's change.
                    sent_volume = Some(level);
                    wanted_volume = None;
                    volume_sent = Instant::now();
                }
            }
            if let Some(media) = media {
                let paused = self.pipe.paused();
                if paused != told_paused {
                    told_paused = paused;
                    let id = self.next_request();
                    let kind = if paused { "PAUSE" } else { "PLAY" };
                    channel.send(&transport, MEDIA, &json!({"type": kind, "mediaSessionId": media, "requestId": id}))?;
                }
                if asked.elapsed() >= STATUS_EVERY {
                    asked = Instant::now();
                    let id = self.next_request();
                    channel.send(&transport, MEDIA, &json!({"type": "GET_STATUS", "mediaSessionId": media, "requestId": id}))?;
                }
            }
            if pinged.elapsed() >= PING_EVERY {
                pinged = Instant::now();
                channel.send(PLATFORM, HEARTBEAT, &json!({"type": "PING"}))?;
            }
            if heard.elapsed() >= SILENT {
                return Err(format!("Lost the connection to {name}"));
            }

            let Some(message) = channel.poll()? else { continue };
            heard = Instant::now();
            self.ping_back(&mut channel, &message)?;
            let p = &message.payload;
            match (message.namespace.as_str(), p["type"].as_str()) {
                (CONNECTION, Some("CLOSE")) if message.source == transport => {
                    return Err(format!("{name} stopped playing"));
                }
                (RECEIVER, Some("RECEIVER_STATUS")) => {
                    if our_app(p, self.app).is_none_or(|(_, s)| s != session) {
                        return Err(format!("{name} is playing something else now"));
                    }
                    if let Some(level) = p["status"]["volume"]["level"].as_f64() {
                        let level = level as f32;
                        if volume_check.is_some_and(|(want, _)| (want - level).abs() < 0.02) {
                            volume_check = None;
                        }
                        // Ours coming back changes nothing. Anyone else's,
                        // such as a phone's slider, moves ours, and with a
                        // device that ignores its volume, the stream's.
                        let theirs = volume.is_none_or(|v| (v - level).abs() > 0.01)
                            && sent_volume.is_none_or(|v| (v - level).abs() > 0.01);
                        if scaled && !theirs {
                            volume = Some(level);
                        }
                        if wanted_volume.is_none() && volume_check.is_none() && theirs {
                            volume = Some(level);
                            if scaled {
                                self.pipe.set_gain(gain_for(level));
                            }
                            (self.events)(Event::Volume(level));
                        }
                    }
                }
                (MEDIA, Some("MEDIA_STATUS")) => {
                    let Some(status) = p["status"].as_array().and_then(|s| s.first()) else {
                        // An empty status: the media has gone.
                        if playing {
                            return Err(format!("{name} stopped playing"));
                        }
                        continue;
                    };
                    if let Some(id) = status["mediaSessionId"].as_u64() {
                        media = Some(id);
                    }
                    let state = status["playerState"].as_str().unwrap_or("");
                    if let Some(seconds) = status["currentTime"].as_f64() {
                        self.pipe.heard(seconds, state == "PLAYING");
                    }
                    match state {
                        "PLAYING" if !playing => {
                            playing = true;
                            screen.held = false;
                            (self.events)(Event::Playing);
                        }
                        // Idle with no reason before it plays is the player
                        // starting up.
                        "IDLE" if playing || status["idleReason"].is_string() => {
                            let reason = status["idleReason"].as_str().unwrap_or("");
                            return Err(match reason {
                                "ERROR" if !self.pipe.reached() => format!(
                                    "{name} couldn't reach this computer. Allow Tonearm through Windows \
                                     Firewall, or set this network to private in Windows settings"
                                ),
                                "ERROR" => format!("{name} couldn't play the stream"),
                                "INTERRUPTED" | "CANCELLED" => format!("{name} is playing something else now"),
                                _ => format!("{name} stopped playing"),
                            });
                        }
                        _ => {}
                    }
                    // Paused or played on the device, not at our asking:
                    // only those come with no request id.
                    let ours = p["requestId"].as_u64().unwrap_or(0) != 0;
                    if !ours && playing && (state == "PAUSED" || state == "PLAYING") {
                        let paused = state == "PAUSED";
                        if paused != told_paused {
                            told_paused = paused;
                            (self.events)(Event::Paused(paused));
                        }
                    }
                }
                (TONEARM, Some("NEXT")) => (self.events)(Event::Next),
                (TONEARM, Some("PREVIOUS")) => (self.events)(Event::Previous),
                (MEDIA, Some("LOAD_FAILED" | "LOAD_CANCELLED" | "INVALID_REQUEST")) => {
                    return Err(format!("{name} couldn't play the stream ({})", p["type"].as_str().unwrap_or("")));
                }
                _ => {}
            }
        }
    }

    /// Answers the device's heartbeat, which it sends every few seconds.
    fn ping_back(&self, channel: &mut Channel, message: &Message) -> Result<(), String> {
        if message.namespace == HEARTBEAT && message.payload["type"] == "PING" {
            channel.send(&message.source, HEARTBEAT, &json!({"type": "PONG"}))?;
        }
        Ok(())
    }
}

/// What the receiver's screen was last told, and what it is to be told.
#[derive(Default)]
struct Screen {
    /// Last sent, and when.
    shown: Option<(NowPlaying, Instant)>,
    /// A new song, held back until the device plays it.
    next: Option<(NowPlaying, Instant)>,
    /// A change to the song shown, to send now.
    change: Option<NowPlaying>,
    /// Shown as paused whatever the app says: the device hasn't started.
    held: bool,
}

impl Screen {
    /// `lead` is how far the device trails the engine. A new song waits
    /// that long, so the screen changes with the sound.
    fn update(&mut self, mut now: NowPlaying, lead: Duration) {
        now.paused |= self.held;
        match (&mut self.next, &self.shown) {
            (Some((next, _)), _) if next.id == now.id => *next = now,
            (_, Some((shown, at))) if shown.id == now.id => {
                self.next = None;
                let expected = if shown.paused {
                    shown.position
                } else {
                    shown.position + at.elapsed().as_secs_f32()
                };
                if now.paused != shown.paused || (now.position - expected).abs() > DRIFT {
                    self.change = Some(now);
                }
            }
            (_, None) => self.change = Some(now),
            _ => self.next = Some((now, Instant::now() + lead)),
        }
    }

    /// What to send now, if anything.
    fn due(&mut self) -> Option<NowPlaying> {
        let now = match self.change.take() {
            Some(now) => now,
            None if self.next.as_ref().is_some_and(|(_, at)| Instant::now() >= *at) => self.next.take()?.0,
            None => return None,
        };
        self.shown = Some((now.clone(), Instant::now()));
        Some(now)
    }
}

/// The stream's scale for a slider level, on the same curve as this
/// computer's volume.
fn gain_for(level: f32) -> f32 {
    level * level
}

/// Our receiver app's transport id and session id, from a receiver status.
fn our_app(payload: &Value, app: &str) -> Option<(String, String)> {
    payload["status"]["applications"]
        .as_array()?
        .iter()
        .find(|a| a["appId"] == app)
        .and_then(|a| Some((a["transportId"].as_str()?.to_string(), a["sessionId"].as_str()?.to_string())))
}

/// What a device says about itself, without changing anything on it: the
/// app it is running and its volume. For `--probe cast`.
pub fn status(device: &Device) -> Result<Value, String> {
    let mut channel = Channel::connect(device.addr)?;
    channel.send(PLATFORM, CONNECTION, &json!({"type": "CONNECT"}))?;
    channel.send(PLATFORM, RECEIVER, &json!({"type": "GET_STATUS", "requestId": 1}))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if let Some(message) = channel.poll()? {
            if message.payload["type"] == "RECEIVER_STATUS" {
                let _ = channel.send(PLATFORM, CONNECTION, &json!({"type": "CLOSE"}));
                return Ok(message.payload["status"].clone());
            }
        }
    }
    Err("no answer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, position: f32, paused: bool) -> NowPlaying {
        NowPlaying {
            id: id.into(),
            title: id.into(),
            position,
            paused,
            ..NowPlaying::default()
        }
    }

    #[test]
    fn the_screen_hears_only_of_changes() {
        let mut screen = Screen::default();
        screen.update(song("a", 0.0, false), Duration::ZERO);
        assert_eq!(screen.due().map(|n| n.id), Some("a".into()));
        // Playing on as expected: nothing to say.
        screen.update(song("a", 0.1, false), Duration::ZERO);
        assert!(screen.due().is_none());
        // Paused, then a seek.
        screen.update(song("a", 0.1, true), Duration::ZERO);
        assert!(screen.due().is_some_and(|n| n.paused));
        screen.update(song("a", 60.0, true), Duration::ZERO);
        assert!(screen.due().is_some_and(|n| n.position == 60.0));
    }

    #[test]
    fn lg_tvs_get_the_streams_volume() {
        let device = |model: &str, screen: bool| Device {
            id: String::new(),
            name: String::new(),
            model: model.into(),
            addr: "127.0.0.1:8009".parse().unwrap(),
            group: false,
            screen,
        };
        // The user's own devices, as they name themselves.
        assert!(ignores_its_volume(&device("86UT8050PSB.AAUFLJD", true)));
        assert!(!ignores_its_volume(&device("HK Citation One", false)));
        assert!(!ignores_its_volume(&device("Lenovo Smart Clock", false)));
        assert!(!ignores_its_volume(&device("Chromecast", true)));
        assert!(!ignores_its_volume(&device("Google TV Streamer", true)));
        assert!(ignores_its_volume(&device("LG webOS TV", true)));
        assert_eq!(gain_for(0.5), 0.25);
    }

    #[test]
    fn lyrics_too_big_lose_their_syllables_first() {
        let line = |syllables: usize| LyricLine {
            start: 1000,
            end: 4000,
            text: "a line of lyrics about this long, give or take".into(),
            translation: "una línea".into(),
            syllables: (0..syllables)
                .map(|n| Syllable {
                    start: 1000 + n as u32 * 100,
                    end: 1100 + n as u32 * 100,
                    text: "syl ".into(),
                })
                .collect(),
            align: 1,
            ..LyricLine::default()
        };
        let small = lyrics_message("a", &vec![line(8); 40]).unwrap();
        assert_eq!(small["lines"][0]["syllables"][1], json!([1100, 1200, "syl "]));
        assert_eq!(small["lines"][0]["align"], 1);

        let big = lyrics_message("a", &vec![line(60); 200]).unwrap();
        assert!(big.to_string().len() <= LYRICS_MAX);
        assert_eq!(big["lines"][0]["syllables"], json!([]));
        assert_eq!(big["lines"][0]["translation"], "una línea");
    }

    #[test]
    fn a_new_song_waits_for_the_sound() {
        let mut screen = Screen::default();
        screen.update(song("a", 0.0, false), Duration::ZERO);
        screen.due();
        screen.update(song("b", 0.0, false), Duration::from_secs(60));
        assert!(screen.due().is_none());
        // A later word on the same new song keeps its place in line.
        screen.update(song("b", 0.5, false), Duration::ZERO);
        assert!(screen.due().is_none());

        screen.update(song("c", 0.0, false), Duration::ZERO);
        assert_eq!(screen.due().map(|n| n.id), Some("c".into()));
    }

    /// Casts silence to a real device and checks that Tonearm's receiver
    /// opened, heard the song and put it in the media status. Needs a
    /// build with TONEARM_CAST_APP and TONEARM_CAST_TEST=ip:port, so it
    /// is ignored by default:
    /// `cargo test -p tonearm-core cast_to_a_real_device -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cast_to_a_real_device() {
        let addr = std::env::var("TONEARM_CAST_TEST").expect("TONEARM_CAST_TEST=ip:port");
        let device = Device {
            id: "test".into(),
            name: "test device".into(),
            model: String::new(),
            addr: addr.parse().expect("an ip:port"),
            group: false,
            screen: true,
        };
        let pipe = Pipe::new();
        pipe.set_rate(48_000);
        let (tx, rx) = mpsc::channel();
        let session = Session::start(device.clone(), pipe, "Tonearm".into(), "cast test".into(), move |event| {
            let _ = tx.send(event);
        });
        let started = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_secs(40)).expect("no word from the device") {
                Event::Playing => break,
                Event::Ended(reason) => panic!("the cast ended: {reason:?}"),
                other => println!("event {other:?}"),
            }
        }
        println!("playing after {:?}", started.elapsed());
        assert!(session.shows_songs(), "the device fell back to the default receiver");

        let title = "Tonearm receiver test";
        for position in [0.0f32, 1.0, 2.0] {
            session.now_playing(NowPlaying {
                id: "test".into(),
                title: title.into(),
                artist: "Claude".into(),
                album: "Cast check".into(),
                cover: "https://picsum.photos/seed/tonearm/720/720".into(),
                duration: 200.0,
                position,
                paused: false,
            });
            std::thread::sleep(Duration::from_secs(1));
        }
        std::thread::sleep(Duration::from_secs(3));

        // A second connection, as the Google Home app would see it.
        let mut channel = Channel::connect(device.addr).unwrap();
        channel.send(PLATFORM, CONNECTION, &json!({"type": "CONNECT"})).unwrap();
        channel.send(PLATFORM, RECEIVER, &json!({"type": "GET_STATUS", "requestId": 1})).unwrap();
        let mut transport = None;
        let mut shown = None;
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_secs(10) && shown.is_none() {
            let Some(message) = channel.poll().unwrap() else { continue };
            let p = &message.payload;
            match p["type"].as_str() {
                Some("RECEIVER_STATUS") if transport.is_none() => {
                    let (id, _) = our_app(p, APP_UNDER_TEST).expect("Tonearm's receiver is not running");
                    println!("receiver running: {}", p["status"]["applications"][0]["displayName"]);
                    channel.send(&id, CONNECTION, &json!({"type": "CONNECT"})).unwrap();
                    channel.send(&id, MEDIA, &json!({"type": "GET_STATUS", "requestId": 2})).unwrap();
                    transport = Some(id);
                }
                Some("MEDIA_STATUS") => {
                    let meta = &p["status"][0]["media"]["metadata"];
                    println!("media status metadata: {meta}");
                    shown = meta["title"].as_str().map(str::to_string);
                }
                _ => {}
            }
        }
        session.finish();
        assert_eq!(shown.as_deref(), Some(title), "the receiver didn't announce the song");
    }

    const APP_UNDER_TEST: &str = match TONEARM_APP {
        Some(app) => app,
        None => DEFAULT_APP,
    };

    /// Plays a real song to a real device through the engine, the way the
    /// app casts, and keeps the receiver told of it. For a look at the
    /// receiver on a TV. `TONEARM_CAST_SONG` is what to search for and
    /// `TONEARM_CAST_SECONDS` how long to play (default 120):
    /// `cargo test -p tonearm-core cast_a_song_to_a_real_device -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cast_a_song_to_a_real_device() {
        use crate::audio::{Audio, Input, PLAYING};
        use crate::innertube::model::Kind;
        let addr = std::env::var("TONEARM_CAST_TEST").expect("TONEARM_CAST_TEST=ip:port");
        let query = std::env::var("TONEARM_CAST_SONG").unwrap_or_else(|_| "Party with the Boys Don Toliver".into());
        let seconds: u64 = std::env::var("TONEARM_CAST_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);

        let client = crate::innertube::Client::new();
        let page = client.search(&query).expect("search");
        let item = page
            .shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .find(|i| i.kind == Kind::Song && i.video_id.is_some())
            .expect("no song found")
            .clone();
        let video_id = item.video_id.clone().unwrap();
        println!("song: {} / {} ({video_id})", item.title, item.subtitle);
        let input = match crate::offline::local_file(&video_id) {
            Some(path) => Input::File(path),
            None => {
                let resolver = crate::resolver::default().expect("no resolver");
                let stream = resolver.resolve(&video_id).expect("resolve");
                Input::Http {
                    url: stream.url,
                    headers: stream.headers,
                    cache_as: None,
                }
            }
        };

        let audio = Audio::new();
        let status = audio.status.clone();
        audio.load(input, 1);
        let started = Instant::now();
        while status.state.load(Ordering::SeqCst) != PLAYING {
            assert!(started.elapsed() < Duration::from_secs(30), "the song never started");
            std::thread::sleep(Duration::from_millis(50));
        }
        let device = Device {
            id: "test".into(),
            name: "test device".into(),
            model: String::new(),
            addr: addr.parse().expect("an ip:port"),
            group: false,
            screen: true,
        };
        let pipe = Pipe::new();
        let (tx, rx) = mpsc::channel();
        let session = Session::start(device, pipe.clone(), "Tonearm".into(), "cast test".into(), move |event| {
            let _ = tx.send(event);
        });
        audio.route(Some(pipe));

        let artist = if item.artists.is_empty() {
            item.subtitle.split(" • ").next().unwrap_or_default().to_string()
        } else {
            item.artists.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        };
        let cover = crate::innertube::sized(&item.thumb, 720);
        let cover = if cover.starts_with("//") { format!("https:{cover}") } else { cover };
        let until = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < until {
            while let Ok(event) = rx.try_recv() {
                println!("event {event:?}");
                if let Event::Ended(reason) = event {
                    panic!("the cast ended: {reason:?}");
                }
            }
            let shown = status.position_ms.load(Ordering::SeqCst).saturating_sub(status.lag_ms.load(Ordering::SeqCst));
            session.now_playing(NowPlaying {
                id: video_id.clone(),
                title: item.title.clone(),
                artist: artist.clone(),
                album: item.album.as_ref().map(|(n, _)| n.clone()).unwrap_or_default(),
                cover: cover.clone(),
                duration: status.duration_ms.load(Ordering::SeqCst) as f32 / 1000.0,
                position: shown as f32 / 1000.0,
                paused: status.state.load(Ordering::SeqCst) != PLAYING,
            });
            std::thread::sleep(Duration::from_millis(250));
        }
        println!("shows songs: {}", session.shows_songs());
        session.finish();
    }

    /// Asks a real device to open Tonearm's receiver and nothing else: no
    /// fallback, no sound. Closes it again if it opens. For checking when a
    /// newly published receiver has reached a device:
    /// `cargo test -p tonearm-core receiver_opens_on_a_real_device -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn receiver_opens_on_a_real_device() {
        let addr = std::env::var("TONEARM_CAST_TEST").expect("TONEARM_CAST_TEST=ip:port");
        let app = TONEARM_APP.expect("a build with TONEARM_CAST_APP");
        let mut channel = Channel::connect(addr.parse().expect("an ip:port")).unwrap();
        channel.send(PLATFORM, CONNECTION, &json!({"type": "CONNECT"})).unwrap();
        channel.send(PLATFORM, RECEIVER, &json!({"type": "LAUNCH", "appId": app, "requestId": 1})).unwrap();
        let asked = Instant::now();
        while asked.elapsed() < Duration::from_secs(20) {
            let Some(message) = channel.poll().unwrap() else { continue };
            let p = &message.payload;
            match p["type"].as_str() {
                Some("LAUNCH_ERROR") => panic!("the device won't open it: {}", p["reason"]),
                Some("RECEIVER_STATUS") => {
                    if let Some((_, session)) = our_app(p, app) {
                        println!("opened: {}", p["status"]["applications"][0]["displayName"]);
                        let _ = channel.send(PLATFORM, RECEIVER, &json!({"type": "STOP", "sessionId": session, "requestId": 2}));
                        std::thread::sleep(Duration::from_millis(300));
                        return;
                    }
                }
                _ => {}
            }
        }
        panic!("no answer from the device");
    }

    /// Prints a real device's receiver status: what it runs and how its
    /// volume can be set. `cargo test -p tonearm-core device_status -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn device_status() {
        let addr = std::env::var("TONEARM_CAST_TEST").expect("TONEARM_CAST_TEST=ip:port");
        let device = Device {
            id: "test".into(),
            name: "test device".into(),
            model: String::new(),
            addr: addr.parse().expect("an ip:port"),
            group: false,
            screen: false,
        };
        let status = status(&device).unwrap();
        println!("volume: {}", status["volume"]);
        for app in status["applications"].as_array().into_iter().flatten() {
            println!("app: {} {} {}", app["appId"], app["displayName"], app["statusText"]);
        }
    }
}
