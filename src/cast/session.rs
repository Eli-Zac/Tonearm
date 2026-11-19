//! One cast: opens Google's Default Media Receiver on the device, has it
//! play our stream, and keeps the two in step until it ends.
//!
//! Pause and play follow the engine, through the pipe. Volume goes to the
//! device. When someone pauses or changes the volume on the device itself
//! (its remote, or the Google Home app), the app hears of it.

use super::channel::{Channel, Message, CONNECTION, HEARTBEAT, MEDIA, PLATFORM, RECEIVER};
use super::stream::{Pipe, Server};
use super::Device;
use serde_json::{json, Value};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Google's Default Media Receiver. It needs no registration.
const APP: &str = "CC1AD845";
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

enum Command {
    Volume(f32),
    Stop,
}

/// A cast in progress. Dropping it ends the cast.
pub struct Session {
    tx: Sender<Command>,
    /// Closes when the cast thread is done.
    done: Receiver<()>,
    pub device: Device,
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
        std::thread::Builder::new()
            .name("cast".into())
            .spawn(move || {
                let _finished = finished;
                let mut cast = Cast {
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
        Session { tx, done, device }
    }

    pub fn volume(&self, level: f32) {
        let _ = self.tx.send(Command::Volume(level));
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
        channel.send(PLATFORM, RECEIVER, &json!({"type": "LAUNCH", "appId": APP, "requestId": launch}))?;

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
                    if let Some(app) = our_app(p) {
                        break app;
                    }
                }
                Some("LAUNCH_ERROR") => {
                    return Err(format!("{name} couldn't start the player ({})", p["reason"].as_str().unwrap_or("no reason")))
                }
                _ => {}
            }
        };
        channel.send(&transport, CONNECTION, &json!({"type": "CONNECT"}))?;
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
                Err(TryRecvError::Empty) => {}
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
                    if our_app(p).is_none_or(|(_, s)| s != session) {
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

/// Our receiver app's transport id and session id, from a receiver status.
fn our_app(payload: &Value) -> Option<(String, String)> {
    payload["status"]["applications"]
        .as_array()?
        .iter()
        .find(|a| a["appId"] == APP)
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
