//! One cast: opens the receiver app on the device, has it play our
//! stream, and keeps the two in step until it ends.
//!
//! Devices with a screen get Tonearm's own receiver (the page in
//! `receiver/`) when the build has its id in `TONEARM_CAST_APP`, and are
//! told the song playing so the screen can show it. Everything else, and
//! any device that won't open ours, gets Google's Default Media Receiver.
//!
//! Pause and play follow the engine, through the pipe. Volume goes to the
//! device. When someone pauses or changes the volume on the device itself
//! (its remote, or the Google Home app), the app hears of it.

use super::channel::{Channel, Message, CONNECTION, HEARTBEAT, MEDIA, PLATFORM, RECEIVER};
use super::stream::{Pipe, Server};
use super::Device;
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

/// The receiver to open on a device: ours where there is a screen to
/// show the song on. A speaker gains nothing from it.
fn app_for(device: &Device) -> &'static str {
    match TONEARM_APP {
        Some(app) if device.screen && !device.group => app,
        _ => DEFAULT_APP,
    }
}

enum Command {
    Volume(f32),
    Now(NowPlaying),
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
        let launch = self.next_request();
        channel.send(PLATFORM, RECEIVER, &json!({"type": "LAUNCH", "appId": self.app, "requestId": launch}))?;

        // Wait for the receiver app to come up and say where to reach it.
        let started = Instant::now();
        let mut volume: Option<f32> = None;
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
            match p["type"].as_str() {
                Some("RECEIVER_STATUS") => {
                    // The slider shows the device's volume from the start.
                    if let Some(level) = p["status"]["volume"]["level"].as_f64() {
                        if volume.is_none() {
                            (self.events)(Event::Volume(level as f32));
                        }
                        volume = Some(level as f32);
                    }
                    if let Some(app) = our_app(p, self.app) {
                        break app;
                    }
                }
                // A device that won't open ours, such as one not yet allowed
                // to while it is unpublished, gets Google's instead.
                Some("LAUNCH_ERROR") if self.app != DEFAULT_APP => {
                    eprintln!("cast: {name} wouldn't open {} ({}), using the default receiver", self.app, p["reason"]);
                    self.app = DEFAULT_APP;
                    let launch = self.next_request();
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
        let mut heard = Instant::now();
        let mut pinged = Instant::now();
        let mut asked = Instant::now();
        let loaded = Instant::now();
        let mut screen = Screen::default();
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
                Ok(Command::Volume(level)) => wanted_volume = Some(level.clamp(0.0, 1.0)),
                Ok(Command::Now(now)) => screen.update(now, self.pipe.lead()),
                Err(TryRecvError::Empty) => {}
            }
            if let Some(now) = screen.due() {
                channel.send(
                    &transport,
                    TONEARM,
                    &json!({
                        "type": "NOW",
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
            if let Some(level) = wanted_volume {
                if volume_sent.elapsed() >= Duration::from_millis(100) {
                    let id = self.next_request();
                    channel.send(PLATFORM, RECEIVER, &json!({"type": "SET_VOLUME", "volume": {"level": level}, "requestId": id}))?;
                    volume = Some(level);
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
                        // Ours coming back changes nothing. Anyone else's
                        // moves the slider.
                        if wanted_volume.is_none() && volume.is_none_or(|v| (v - level).abs() > 0.01) {
                            volume = Some(level);
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
}

impl Screen {
    /// `lead` is how far the device trails the engine. A new song waits
    /// that long, so the screen changes with the sound.
    fn update(&mut self, now: NowPlaying, lead: Duration) {
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
}
