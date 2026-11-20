//! Audio engine. One thread owns the output device and the decoder.
//!
//! Tracks stream over HTTP into memory while they play. A four minute AAC
//! track is about 4 MB, and we free it as soon as the next track loads. A
//! track that finishes downloading is kept in the offline cache, and cached
//! or downloaded tracks play straight from their file.

use crate::cast::Pipe;
use crate::mix::{self, Transition};
use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const CHUNK: u64 = 512 * 1024;

/// How every download error starts, so the player can tell them from
/// broken files and try a fresh link.
pub const STREAM_FAILED: &str = "Stream failed";

/// A download error as people can read it. Never the link itself: it is
/// long, says nothing useful, and is tied to this computer.
fn stream_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(403, _) => format!("{STREAM_FAILED}: YouTube refused the link (403)"),
        ureq::Error::Status(code, _) => format!("{STREAM_FAILED}: YouTube answered {code}"),
        ureq::Error::Transport(t) => format!("{STREAM_FAILED}: {}", t.kind()),
    }
}

pub const IDLE: u32 = 0;
pub const LOADING: u32 = 1;
pub const PLAYING: u32 = 2;
pub const PAUSED: u32 = 3;
pub const ENDED: u32 = 4;
pub const FAILED: u32 = 5;

/// Where a track's bytes come from.
pub enum Input {
    /// Streamed over HTTP. With a video id, a download that finishes is
    /// kept in the offline cache.
    Http {
        url: String,
        headers: Vec<(String, String)>,
        cache_as: Option<String>,
    },
    /// A cached or downloaded file.
    File(std::path::PathBuf),
}

enum Command {
    Load { input: Input, generation: u64 },
    /// Opens the next song ahead, ready to mix into.
    Prepare { input: Input, token: u64 },
    /// Mixes from the playing song into the prepared one by `plan`.
    Crossfade { token: u64, plan: Transition },
    /// Starts the prepared song the moment the playing one ends, with no
    /// gap. Token zero calls it off.
    Follow { token: u64, gain: f32 },
    /// The levelling gain for the song under `generation`.
    Gain { generation: u64, gain: f32 },
    Play,
    Pause,
    Seek(f32),
    Volume(f32),
    /// Plays into a Cast device's stream from now on, or with None, back
    /// on this computer.
    Route(Option<Arc<Pipe>>),
    Stop,
}

#[derive(Default)]
pub struct Status {
    pub state: AtomicU32,
    pub position_ms: AtomicU64,
    /// Length of the loaded track as the file states it. Zero when unknown.
    pub duration_ms: AtomicU64,
    /// Which `load` call the state belongs to.
    pub generation: AtomicU64,
    /// Which `prepare` call has its song open and ready to fade in. Zero
    /// when none is.
    pub prepared: AtomicU64,
    /// The `follow` token of a song that just started gaplessly. The app
    /// takes it, moves the queue on, and reports the song's generation.
    pub followed: AtomicU64,
    /// How far what is heard trails `position_ms`: about two seconds while
    /// casting, else zero. The position stays the mixer's own, which mixes
    /// are timed by. Only what is shown takes this off.
    pub lag_ms: AtomicU64,
    pub error: Mutex<String>,
}

/// Where a seek sent the song, for what is shown. The engine reports the
/// old place for a moment after a seek, so a timeline drawn from its report
/// jumps back before it catches up. Shown instead is the place picked, until
/// the report comes within reach of it, or two seconds pass.
#[derive(Default)]
pub struct SeekHold {
    held: Mutex<Option<(u64, Instant)>>,
}

impl SeekHold {
    /// A seek to `ms`.
    pub fn set(&self, ms: u64) {
        *self.held.lock().unwrap() = Some((ms, Instant::now()));
    }

    pub fn clear(&self) {
        *self.held.lock().unwrap() = None;
    }

    /// The position to show, from the engine's `reported` one: the place
    /// picked while it is held, moving on with the time since if playing.
    pub fn apply(&self, reported: u64, playing: bool) -> u64 {
        let mut held = self.held.lock().unwrap();
        let Some((target, since)) = *held else { return reported };
        if reported.abs_diff(target) < 1500 || since.elapsed() > Duration::from_secs(2) {
            *held = None;
            return reported;
        }
        target + if playing { since.elapsed().as_millis() as u64 } else { 0 }
    }
}

/// Whether play or pause was last asked for, for what is shown. The engine
/// takes the command on its own thread, so for a moment after a tap it
/// still reports the old state, and a button drawn from that report flips
/// back before it flips over. Shown instead is the state asked for, until
/// the engine reports it (or stops on its own: loading, ended, failed), or
/// two seconds pass.
#[derive(Default)]
pub struct PlayHold {
    held: Mutex<Option<(bool, Instant)>>,
}

impl PlayHold {
    /// Play (`true`) or pause asked for.
    pub fn set(&self, playing: bool) {
        *self.held.lock().unwrap() = Some((playing, Instant::now()));
    }

    /// The engine's `state` to show: `PLAYING` or `PAUSED` as asked while
    /// held, anything else as it is.
    pub fn apply(&self, state: u32) -> u32 {
        let mut held = self.held.lock().unwrap();
        let Some((playing, since)) = *held else { return state };
        let wanted = if playing { PLAYING } else { PAUSED };
        let settled = state == wanted || !matches!(state, PLAYING | PAUSED);
        if settled || since.elapsed() > Duration::from_secs(2) {
            *held = None;
            return state;
        }
        wanted
    }
}

pub struct Audio {
    tx: Sender<Command>,
    pub status: Arc<Status>,
}

impl Audio {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Status::default());
        let shared = status.clone();
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(rx, shared))
            .expect("cannot start the audio thread");
        Audio { tx, status }
    }

    pub fn load(&self, input: Input, generation: u64) {
        self.status.generation.store(generation, Ordering::SeqCst);
        self.status.state.store(LOADING, Ordering::SeqCst);
        self.status.position_ms.store(0, Ordering::SeqCst);
        self.status.duration_ms.store(0, Ordering::SeqCst);
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Load { input, generation });
    }
    /// Opens the song after this one in the background, so a crossfade can
    /// start it without a wait. `token` names this call in `prepared`.
    pub fn prepare(&self, input: Input, token: u64) {
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Prepare { input, token });
    }
    /// Mixes from the playing song into the prepared one by `plan`. The new
    /// song is reported under `generation` from now on. Call only once
    /// `prepared` shows `token`.
    pub fn crossfade(&self, token: u64, generation: u64, plan: Transition) {
        self.status.generation.store(generation, Ordering::SeqCst);
        self.status
            .position_ms
            .store((plan.in_from * 1000.0) as u64, Ordering::SeqCst);
        self.status.duration_ms.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Crossfade { token, plan });
    }
    /// Plays the prepared song straight after this one ends. `gain` is its
    /// levelling gain. Call only once `prepared` shows `token`.
    pub fn follow(&self, token: u64, gain: f32) {
        let _ = self.tx.send(Command::Follow { token, gain });
    }
    /// Calls off a `follow`, as when the queue changes.
    pub fn unfollow(&self) {
        let _ = self.tx.send(Command::Follow { token: 0, gain: 1.0 });
    }
    /// A song that started through `follow` now belongs to `generation`.
    pub fn adopt(&self, generation: u64) {
        self.status.generation.store(generation, Ordering::SeqCst);
    }
    /// Sets the levelling gain of the song under `generation`, on top of
    /// the volume. Ignored once another song has started.
    pub fn gain(&self, generation: u64, gain: f32) {
        let _ = self.tx.send(Command::Gain { generation, gain });
    }
    pub fn play(&self) {
        let _ = self.tx.send(Command::Play);
    }
    pub fn pause(&self) {
        let _ = self.tx.send(Command::Pause);
    }
    pub fn seek(&self, seconds: f32) {
        let _ = self.tx.send(Command::Seek(seconds));
    }
    pub fn volume(&self, volume: f32) {
        let _ = self.tx.send(Command::Volume(volume));
    }
    /// Plays into `pipe` for a Cast device, or with None, here again. The
    /// song playing carries on where it was.
    pub fn route(&self, pipe: Option<Arc<Pipe>>) {
        let _ = self.tx.send(Command::Route(pipe));
    }
    pub fn stop(&self) {
        self.status.prepared.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Stop);
    }
}

/// Where the mixer plays: the sound card, or the stream a Cast device
/// pulls.
enum Out {
    Device(Sink),
    Cast { pipe: Arc<Pipe>, feed: u64 },
}

/// Where to move the mixer to.
enum Target<'a> {
    Device(&'a OutputStreamHandle),
    Cast(&'a Arc<Pipe>),
}

impl Out {
    /// Plays `mixer` on `to`, and stops whatever pulled it before.
    fn attach(mixer: &Arc<Mixer>, rate: u32, to: Target, volume: f32, paused: bool) -> Result<Out, String> {
        let take = || {
            let id = mixer.owner.fetch_add(1, Ordering::SeqCst) + 1;
            Handoff::new(mixer.clone(), id, rate)
        };
        match to {
            Target::Device(handle) => {
                let sink = Sink::try_new(handle).map_err(|e| format!("No audio output: {e}"))?;
                sink.set_volume(volume);
                if paused {
                    sink.pause();
                }
                // The old output stops pulling before the new one starts.
                sink.append(take());
                Ok(Out::Device(sink))
            }
            Target::Cast(pipe) => {
                pipe.set_paused(paused);
                let feed = pipe.attach(Box::new(take()));
                Ok(Out::Cast {
                    pipe: pipe.clone(),
                    feed,
                })
            }
        }
    }

    fn play(&self) {
        match self {
            Out::Device(sink) => sink.play(),
            Out::Cast { pipe, .. } => pipe.set_paused(false),
        }
    }

    fn pause(&self) {
        match self {
            Out::Device(sink) => sink.pause(),
            Out::Cast { pipe, .. } => pipe.set_paused(true),
        }
    }

    fn is_paused(&self) -> bool {
        match self {
            Out::Device(sink) => sink.is_paused(),
            Out::Cast { pipe, .. } => pipe.paused(),
        }
    }

    /// Whether the mixer has run out: the song ended with nothing after.
    fn empty(&self) -> bool {
        match self {
            Out::Device(sink) => sink.empty(),
            Out::Cast { pipe, feed } => !pipe.has(*feed),
        }
    }

    fn seek(&self, to: Duration) {
        match self {
            Out::Device(sink) => {
                let _ = sink.try_seek(to);
            }
            Out::Cast { pipe, feed } => pipe.seek(*feed, to),
        }
    }

    /// The Cast device has a volume of its own, which the app sets.
    fn set_volume(&self, volume: f32) {
        if let Out::Device(sink) = self {
            sink.set_volume(volume);
        }
    }

    /// How long what is heard trails what the mixer has made.
    fn lag(&self) -> Duration {
        match self {
            Out::Device(_) => Duration::ZERO,
            Out::Cast { pipe, .. } => pipe.lead(),
        }
    }
}

impl Drop for Out {
    fn drop(&mut self) {
        if let Out::Cast { pipe, feed } = self {
            pipe.detach(*feed);
        }
    }
}

/// The song playing through the mixer, and how to reach it.
struct Playing {
    out: Out,
    /// The mixer behind the output, kept so it can move to another.
    mixer: Arc<Mixer>,
    /// The mixer's sample rate. Songs prepared for it open at this rate,
    /// even after a move to a device that runs at another.
    rate: u32,
    mix: Sender<mix::Command>,
    monitor: Arc<mix::Monitor>,
    /// The length of the song armed to follow, by its token.
    follow_length: Option<(u64, Option<Duration>)>,
}

impl Playing {
    fn send(&self, command: mix::Command) {
        let _ = self.mix.send(command);
    }

    /// Carries on through `to` from where the old output left off. A song
    /// mixed at another rate is resampled by the sink.
    fn move_to(&mut self, to: Target, volume: f32) -> Result<(), String> {
        let paused = self.out.is_paused();
        self.out = Out::attach(&self.mixer, self.rate, to, volume, paused)?;
        Ok(())
    }
}

/// How many samples a sink takes from the mixer at a time: about 5 ms.
/// Even, so a move between devices never splits a stereo frame.
const HANDOFF: usize = 512;

/// The mixer, shared by the sink that plays it now and any it moves to.
struct Mixer {
    mix: Mutex<mix::Mix>,
    /// The `Handoff` allowed to pull. Older ones end.
    owner: AtomicU64,
}

/// One sink's hold on the mixer. It reads a few milliseconds at a time,
/// so the lock is rare.
struct Handoff {
    mixer: Arc<Mixer>,
    id: u64,
    rate: u32,
    chunk: Vec<f32>,
    at: usize,
    ended: bool,
}

impl Handoff {
    fn new(mixer: Arc<Mixer>, id: u64, rate: u32) -> Handoff {
        Handoff {
            mixer,
            id,
            rate,
            chunk: Vec::with_capacity(HANDOFF),
            at: 0,
            ended: false,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, mix::Mix> {
        self.mixer.mix.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Iterator for Handoff {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at == self.chunk.len() {
            if self.ended || self.mixer.owner.load(Ordering::Relaxed) != self.id {
                return None;
            }
            let mut chunk = std::mem::take(&mut self.chunk);
            chunk.clear();
            chunk.extend((&mut *self.lock()).take(HANDOFF));
            self.ended = chunk.len() < HANDOFF;
            self.chunk = chunk;
            self.at = 0;
            if self.chunk.is_empty() {
                return None;
            }
        }
        let sample = self.chunk[self.at];
        self.at += 1;
        Some(sample)
    }
}

impl rodio::Source for Handoff {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        2
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        // What was read ahead belongs to the old position.
        self.at = self.chunk.len();
        self.lock().try_seek(position)
    }
}

impl crate::cast::Feed for Handoff {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        for (n, slot) in out.iter_mut().enumerate() {
            match self.next() {
                Some(sample) => *slot = sample,
                None => return n,
            }
        }
        out.len()
    }

    fn seek(&mut self, to: Duration) {
        let _ = rodio::Source::try_seek(self, to);
    }
}

fn run(rx: mpsc::Receiver<Command>, status: Arc<Status>) {
    // The device opens on first play, so an idle app holds no audio handle.
    let mut output: Option<Output> = None;
    // When the default device was last checked. A new default takes over
    // the song playing.
    let mut checked = Instant::now();
    let mut playing: Option<Playing> = None;
    // The next song, opened ahead for a crossfade, and its `prepare` token.
    let mut next: Option<(u64, crate::decode::Track)> = None;
    // The latest `prepare` token. Songs opened for older ones are dropped.
    let mut wanted = 0u64;
    // Songs open on their own threads, as a stream can take seconds to
    // start, and hand themselves back here.
    let (opened_tx, opened_rx) = mpsc::channel::<(u64, Result<crate::decode::Track, String>)>();
    // Songs the mixer is done with, freed here rather than in the callback.
    let (done_tx, done_rx) = mpsc::channel::<mix::Deck>();
    let mut volume = 1.0f32;
    // The Cast device's stream, while casting. Songs play into it in place
    // of the sound card, at its rate.
    let mut cast: Option<Arc<Pipe>> = None;
    let fail = |status: &Status, message: String| {
        *status.error.lock().unwrap() = message;
        status.state.store(FAILED, Ordering::SeqCst);
    };
    let length_ms = |track: &crate::decode::Track| {
        rodio::Source::total_duration(track).map_or(0, |l| l.as_millis() as u64)
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Load { input, generation }) => {
                playing = None;
                next = None;
                wanted = 0;
                if generation != status.generation.load(Ordering::SeqCst) {
                    continue; // A newer track was requested while we waited.
                }
                if output.is_none() && cast.is_none() {
                    match open_output() {
                        Ok(opened) => output = Some(opened),
                        Err(e) => {
                            fail(&status, format!("No audio output: {e}"));
                            continue;
                        }
                    }
                }
                let (target, rate) = match (&cast, &output) {
                    (Some(pipe), _) => (Target::Cast(pipe), pipe.rate()),
                    (None, Some(o)) => (Target::Device(&o.handle), o.rate),
                    (None, None) => unreachable!("opened above"),
                };
                let opened = open_track(input, rate).and_then(|track| {
                    status.duration_ms.store(length_ms(&track), Ordering::SeqCst);
                    start(target, rate, track, volume, done_tx.clone())
                });
                match opened {
                    Ok(started) => {
                        playing = Some(started);
                        status.state.store(PLAYING, Ordering::SeqCst);
                    }
                    Err(message) => fail(&status, message),
                }
            }
            Ok(Command::Prepare { input, token }) => {
                wanted = token;
                next = None;
                // A song armed to follow was the old next one.
                if let Some(p) = &mut playing {
                    p.send(mix::Command::Unfollow);
                    p.follow_length = None;
                }
                // Only asked for while a song plays. It opens at the
                // mixer's rate, which a device change leaves as it was.
                if let Some(p) = &playing {
                    let (rate, opened) = (p.rate, opened_tx.clone());
                    std::thread::Builder::new()
                        .name("prepare".into())
                        .spawn(move || {
                            let _ = opened.send((token, open_track(input, rate)));
                        })
                        .expect("cannot start the prepare thread");
                }
            }
            Ok(Command::Crossfade { token, mut plan }) => {
                status.prepared.store(0, Ordering::SeqCst);
                let ready = next.take().filter(|(t, _)| *t == token);
                match (ready, &mut playing) {
                    (Some((_, track)), Some(p)) => {
                        status.duration_ms.store(length_ms(&track), Ordering::SeqCst);
                        p.send(mix::Command::Unfollow);
                        p.follow_length = None;
                        let mut deck = mix::Deck::new(Box::new(track), 1.0);
                        if plan.in_from > 0.0 {
                            if let Err(message) = deck.seek(plan.in_from) {
                                // Not lined up any more, so a plain fade.
                                eprintln!("crossfade: {message}");
                                plan = Transition::fade(plan.length);
                            }
                        }
                        // The app sends the new song's gain straight after.
                        p.send(mix::Command::start(deck, plan, p.rate));
                    }
                    // The app checks `prepared` first, so this is a race lost
                    // to another command. The player retries the song afresh.
                    _ => {
                        playing = None;
                        fail(&status, format!("{STREAM_FAILED}: the next song was not ready"));
                    }
                }
            }
            Ok(Command::Play) => {
                if let Some(p) = &playing {
                    p.out.play();
                    status.state.store(PLAYING, Ordering::SeqCst);
                }
            }
            Ok(Command::Pause) => {
                if let Some(p) = &playing {
                    p.out.pause();
                    status.state.store(PAUSED, Ordering::SeqCst);
                }
            }
            Ok(Command::Seek(seconds)) => {
                // A seek during a crossfade cuts the old song off. The new
                // one plays on at full volume.
                if let Some(p) = &playing {
                    p.out.seek(Duration::from_secs_f32(seconds.max(0.0)));
                }
            }
            Ok(Command::Route(Some(pipe))) => {
                // The stream runs at the rate the playing song is mixed at,
                // so it carries on without resampling. Songs after it open
                // at that rate too.
                let rate = playing
                    .as_ref()
                    .map(|p| p.rate)
                    .or(output.as_ref().map(|o| o.rate))
                    .unwrap_or(48_000);
                pipe.set_rate(rate);
                if let Some(p) = &mut playing {
                    if let Err(e) = p.move_to(Target::Cast(&pipe), volume) {
                        eprintln!("cast: {e}");
                    }
                }
                cast = Some(pipe);
            }
            Ok(Command::Route(None)) => {
                if cast.take().is_some() {
                    status.lag_ms.store(0, Ordering::SeqCst);
                    if output.is_none() {
                        output = open_output().map_err(|e| eprintln!("audio output: {e}")).ok();
                    }
                    if let Some(p) = &mut playing {
                        let moved = match &output {
                            Some(o) => p.move_to(Target::Device(&o.handle), volume),
                            None => Err("no output device".into()),
                        };
                        if let Err(e) = moved {
                            playing = None;
                            fail(&status, format!("No audio output: {e}"));
                        }
                    }
                }
            }
            Ok(Command::Follow { token, gain }) => {
                let Some(p) = &mut playing else {
                    continue;
                };
                if token == 0 {
                    p.send(mix::Command::Unfollow);
                    p.follow_length = None;
                } else if let Some((_, track)) = next.take().filter(|(t, _)| *t == token) {
                    p.follow_length = Some((token, rodio::Source::total_duration(&track)));
                    let deck = mix::Deck::new(Box::new(track), gain);
                    p.send(mix::Command::Follow { deck, token });
                }
            }
            Ok(Command::Gain { generation, gain }) => {
                if generation == status.generation.load(Ordering::SeqCst) {
                    if let Some(p) = &playing {
                        p.send(mix::Command::Gain(gain));
                    }
                }
            }
            Ok(Command::Volume(v)) => {
                volume = v.clamp(0.0, 1.0);
                if let Some(p) = &playing {
                    p.out.set_volume(volume);
                }
            }
            Ok(Command::Stop) => {
                playing = None;
                next = None;
                wanted = 0;
                status.state.store(IDLE, Ordering::SeqCst);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while done_rx.try_recv().is_ok() {}
        while let Ok((token, opened)) = opened_rx.try_recv() {
            // Dropping an older song stops its download.
            if token != wanted {
                continue;
            }
            match opened {
                Ok(track) => {
                    next = Some((token, track));
                    status.prepared.store(token, Ordering::SeqCst);
                }
                // The song ends and the next starts as it did without
                // crossfade, which shows any error then.
                Err(message) => eprintln!("crossfade: {message}"),
            }
        }
        // While casting the sound card is not in use, so its changes wait.
        if output.is_some() && cast.is_none() && checked.elapsed() >= Duration::from_secs(1) {
            checked = Instant::now();
            let now = default_output_name();
            let current = output.as_ref().map(|o| o.name.as_str());
            // With no device at all, the old stream stays until one appears.
            if now.is_some() && now.as_deref() != current {
                match open_output() {
                    Ok(moved) => {
                        if let Some(p) = &mut playing {
                            if let Err(e) = p.move_to(Target::Device(&moved.handle), volume) {
                                eprintln!("audio output: {e}");
                            }
                        }
                        output = Some(moved);
                    }
                    Err(e) => eprintln!("audio output: {e}"),
                }
            }
        }
        let Some(p) = &mut playing else {
            continue;
        };
        // The armed song started the moment the last one ended.
        let followed = p.monitor.followed.swap(0, Ordering::SeqCst);
        if followed != 0 {
            let length = p
                .follow_length
                .take()
                .filter(|(t, _)| *t == followed)
                .and_then(|(_, l)| l);
            status.prepared.store(0, Ordering::SeqCst);
            status
                .duration_ms
                .store(length.map_or(0, |l| l.as_millis() as u64), Ordering::SeqCst);
            status.followed.store(followed, Ordering::SeqCst);
        }
        status
            .position_ms
            .store(p.monitor.position_ms.load(Ordering::Relaxed), Ordering::SeqCst);
        status
            .lag_ms
            .store(p.out.lag().as_millis() as u64, Ordering::SeqCst);
        if p.out.empty() && status.state.load(Ordering::SeqCst) == PLAYING {
            status.state.store(ENDED, Ordering::SeqCst);
        }
    }
}

/// Opens a track for the decoder. Blocks until its first bytes arrive.
fn open_track(input: Input, rate: u32) -> Result<crate::decode::Track, String> {
    match input {
        Input::Http {
            url,
            headers,
            cache_as,
        } => {
            let source = HttpSource::open(url, headers, cache_as);
            let shared = source.shared.clone();
            let spare = source.reader();
            // Opening reads only the head of the file, so playback
            // starts after the first few kilobytes arrive.
            shared.opening.store(true, Ordering::SeqCst);
            let first = crate::decode::Track::open(Box::new(source), rate);
            shared.opening.store(false, Ordering::SeqCst);
            // A file with its index at the end cannot open from the
            // head. Try again the slow way, reading the whole file.
            first
                .or_else(|_| crate::decode::Track::open(Box::new(spare), rate))
                .map_err(|e| {
                    // A failed download looks like a broken file
                    // to the decoder. Report the download error.
                    shared.buffer.lock().unwrap().error.clone().unwrap_or(e)
                })
        }
        Input::File(path) => {
            let opened = std::fs::File::open(&path)
                .map_err(|e| format!("Cannot open the saved song: {e}"))
                .and_then(|file| crate::decode::Track::open(Box::new(file), rate));
            if opened.is_err() {
                crate::offline::discard_broken(&path);
            }
            opened
        }
    }
}

/// Plays `track` through a mixer on an output of its own. The songs after
/// it mix in on the same mixer.
fn start(
    target: Target,
    rate: u32,
    track: crate::decode::Track,
    volume: f32,
    done: Sender<mix::Deck>,
) -> Result<Playing, String> {
    let (tx, rx) = mpsc::channel();
    let monitor = Arc::new(mix::Monitor::default());
    let deck = mix::Deck::new(Box::new(track), 1.0);
    let mixer = Arc::new(Mixer {
        mix: Mutex::new(mix::Mix::new(rate, deck, rx, monitor.clone(), done)),
        owner: AtomicU64::new(0),
    });
    let out = Out::attach(&mixer, rate, target, volume, false)?;
    Ok(Playing {
        out,
        mixer,
        rate,
        mix: tx,
        monitor,
        follow_length: None,
    })
}

// Opens the default output device at its own rate and remembers that
/// rate, so the decoder can resample to it and rodio never has to.
fn open_output() -> Result<Output, String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let device = rodio::cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let name = device.name().unwrap_or_default();
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let (stream, handle) =
        OutputStream::try_from_device_config(&device, config).map_err(|e| e.to_string())?;
    Ok(Output {
        _stream: stream,
        handle,
        rate,
        name,
    })
}

/// An open output device.
struct Output {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    /// The device's own sample rate.
    rate: u32,
    /// Its name, to notice when the system default changes.
    name: String,
}

/// The name of the system's default output device, if there is one.
fn default_output_name() -> Option<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    rodio::cpal::default_host().default_output_device()?.name().ok()
}

// ------------------------------------------------------------ HTTP source

#[derive(Default)]
struct Buffer {
    data: Vec<u8>,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

#[derive(Default)]
struct Shared {
    buffer: Mutex<Buffer>,
    ready: Condvar,
    cancelled: AtomicBool,
    /// Set while the decoder opens the track. The source then says it
    /// cannot seek, so symphonia's MP4 reader stops at the first fragment.
    /// Otherwise it walks every fragment header in the file before it
    /// returns, and that waits for the whole download.
    opening: AtomicBool,
    /// Told the bytes so far and the whole size as the download goes.
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
}

/// Stops the download when the last reader of it goes away.
struct Cancel(Arc<Shared>);

impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
    }
}

/// Fetches a whole track into memory. Blocks, so run it on a worker.
pub fn fetch_all(url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, String> {
    fetch_all_with(url, headers, None)
}

/// `fetch_all`, telling `progress` the bytes so far and the whole size.
pub fn fetch_all_with(
    url: &str,
    headers: &[(String, String)],
    progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
) -> Result<Vec<u8>, String> {
    let shared = Shared {
        progress,
        ..Shared::default()
    };
    download(url, headers, &shared)?;
    let buffer = shared.buffer.into_inner().unwrap();
    if buffer.data.is_empty() || buffer.total.is_some_and(|t| t != buffer.data.len() as u64) {
        return Err("The download stopped early".into());
    }
    Ok(buffer.data)
}

/// A `Read + Seek` view over a download that is still running. Reads block
/// until the bytes they need arrive.
pub struct HttpSource {
    shared: Arc<Shared>,
    position: u64,
    _cancel: Arc<Cancel>,
}

impl HttpSource {
    pub fn open(url: String, headers: Vec<(String, String)>, cache_as: Option<String>) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("download".into())
            .spawn(move || {
                let result = download(&url, &headers, &worker);
                let mut buffer = worker.buffer.lock().unwrap();
                buffer.done = true;
                buffer.error = result.err();
                worker.ready.notify_all();
                // Keep a whole track for next time. A skipped track stops
                // downloading and is not kept.
                let whole = buffer.error.is_none()
                    && !worker.cancelled.load(Ordering::SeqCst)
                    && buffer.total.is_some_and(|t| t == buffer.data.len() as u64);
                if let (true, Some(id)) = (whole, cache_as) {
                    let bytes = buffer.data.clone();
                    drop(buffer);
                    crate::offline::store_cache(&id, &bytes);
                }
            })
            .expect("cannot start the download thread");
        HttpSource {
            _cancel: Arc::new(Cancel(shared.clone())),
            shared,
            position: 0,
        }
    }

    /// A second reader over the same download, from the start.
    fn reader(&self) -> Self {
        HttpSource {
            shared: self.shared.clone(),
            position: 0,
            _cancel: self._cancel.clone(),
        }
    }
}

/// Fetches in ranged chunks. YouTube throttles one long request hard but
/// serves short ranges at full speed.
fn download(url: &str, headers: &[(String, String)], shared: &Shared) -> Result<(), String> {
    let http = crate::innertube::agent(Duration::from_secs(20));
    let mut offset = 0u64;
    loop {
        if shared.cancelled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let range = format!("bytes={offset}-{}", offset + CHUNK - 1);
        let mut request = http.get(url).set("Range", &range);
        if !headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("User-Agent"))
        {
            request = request.set("User-Agent", crate::innertube::USER_AGENT);
        }
        for (key, value) in headers {
            request = request.set(key, value);
        }
        let response = match request.call() {
            Ok(r) => r,
            Err(ureq::Error::Status(416, _)) => return Ok(()),
            Err(e) => return Err(stream_error(e)),
        };
        let total = response
            .header("Content-Range")
            .and_then(|h| h.rsplit('/').next())
            .and_then(|t| t.parse::<u64>().ok())
            .or_else(|| {
                (response.status() == 200)
                    .then(|| response.header("Content-Length")?.parse().ok())
                    .flatten()
            });
        let whole_file = response.status() == 200;
        let mut reader = response.into_reader();
        let mut chunk = [0u8; 16 * 1024];
        let mut received = 0u64;
        loop {
            let n = reader
                .read(&mut chunk)
                .map_err(|e| format!("{STREAM_FAILED}: the connection dropped ({})", e.kind()))?;
            if n == 0 || shared.cancelled.load(Ordering::SeqCst) {
                break;
            }
            let mut buffer = shared.buffer.lock().unwrap();
            if buffer.total.is_none() {
                buffer.total = total;
                if let Some(t) = total {
                    buffer.data.reserve_exact(t as usize);
                }
            }
            buffer.data.extend_from_slice(&chunk[..n]);
            received += n as u64;
            let (so_far, whole) = (buffer.data.len() as u64, buffer.total);
            drop(buffer);
            shared.ready.notify_all();
            if let (Some(report), Some(whole)) = (&shared.progress, whole) {
                report(so_far, whole);
            }
        }
        offset += received;
        if whole_file || received == 0 || total.is_some_and(|t| offset >= t) {
            return Ok(());
        }
    }
}

impl Read for HttpSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let mut buffer = self.shared.buffer.lock().unwrap();
        loop {
            let available = buffer.data.len() as u64;
            if self.position < available {
                let start = self.position as usize;
                let n = out.len().min(buffer.data.len() - start);
                out[..n].copy_from_slice(&buffer.data[start..start + n]);
                self.position += n as u64;
                return Ok(n);
            }
            if let Some(e) = &buffer.error {
                return Err(io::Error::other(e.clone()));
            }
            if buffer.done {
                return Ok(0);
            }
            buffer = self.shared.ready.wait(buffer).unwrap();
        }
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.position as i64 + n,
            SeekFrom::End(n) => {
                let mut buffer = self.shared.buffer.lock().unwrap();
                while buffer.total.is_none() && !buffer.done {
                    buffer = self.shared.ready.wait(buffer).unwrap();
                }
                buffer.total.unwrap_or(buffer.data.len() as u64) as i64 + n
            }
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}

impl symphonia::core::io::MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        !self.shared.opening.load(Ordering::SeqCst)
    }

    /// Waits for the first response, which carries the full length.
    fn byte_len(&self) -> Option<u64> {
        let mut buffer = self.shared.buffer.lock().unwrap();
        while buffer.total.is_none() && !buffer.done {
            buffer = self.shared.ready.wait(buffer).unwrap();
        }
        buffer.total.or(Some(buffer.data.len() as u64))
    }
}

/// Casts a saved song as a device would hear it, over this computer's own
/// network: the engine moves to the stream mid song and back again.
/// `TONEARM_AUDIO_TEST="<song>" cargo test engine_casts -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn engine_casts_and_comes_back() {
    use std::io::{BufRead, BufReader, Write};
    let Ok(spec) = std::env::var("TONEARM_AUDIO_TEST") else {
        return;
    };
    let file = std::path::PathBuf::from(spec.split('|').next().unwrap());
    let audio = Audio::new();
    let s = audio.status.clone();
    let wait = |what: &str, ok: &dyn Fn() -> bool| {
        let start = Instant::now();
        while !ok() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let pos = || s.position_ms.load(Ordering::SeqCst);
    audio.volume(0.0);
    audio.load(Input::File(file), 1);
    audio.seek(60.0);
    wait("playing", &|| s.state.load(Ordering::SeqCst) == PLAYING && pos() > 60_200);

    let pipe = Pipe::new();
    let server = crate::cast::Server::start("127.0.0.1".parse().unwrap(), pipe.clone()).unwrap();
    audio.route(Some(pipe.clone()));
    // Not pulled until the device connects: the song waits where it was.
    std::thread::sleep(Duration::from_millis(400));
    let held = pos();
    std::thread::sleep(Duration::from_millis(300));
    assert!(pos().abs_diff(held) < 50, "moved with no device: {held} -> {}", pos());

    let address = server.url.trim_start_matches("http://");
    let (host, path) = address.split_at(address.find('/').unwrap());
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
    }
    let mut header = [0u8; 44];
    reader.read_exact(&mut header).unwrap();
    let rate = u32::from_le_bytes(header[24..28].try_into().unwrap());
    // A second of the song, which is not silence.
    let mut second = vec![0u8; rate as usize * 4];
    reader.read_exact(&mut second).unwrap();
    let loud = second
        .chunks(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs() as u64)
        .sum::<u64>()
        / (second.len() as u64 / 2);
    assert!(loud > 100, "the stream is silent: {loud}");
    // The engine ran ahead by the lead, and what shows takes it off.
    wait("position moves on", &|| pos() > held + 1500);
    wait("lag reported", &|| s.lag_ms.load(Ordering::SeqCst) > 1000);

    // Paused: the stream stops coming once the lead is out.
    audio.pause();
    std::thread::sleep(Duration::from_millis(200));
    let paused_at = pos();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(pos(), paused_at, "moved while paused");
    audio.play();
    reader.read_exact(&mut second).unwrap();

    // Back on this computer, where it carries on from the same place.
    audio.route(None);
    std::thread::sleep(Duration::from_millis(200));
    let back = pos();
    wait("plays here again", &|| pos() > back + 300);
    assert_eq!(s.lag_ms.load(Ordering::SeqCst), 0);
    assert_eq!(s.state.load(Ordering::SeqCst), PLAYING);
    pipe.close();
    audio.stop();
}

/// Plays saved songs through the real engine and output device, at zero
/// volume: `TONEARM_AUDIO_TEST="<song>|<song>" cargo test engine_plays
/// -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn engine_plays_seeks_mixes_and_follows() {
    let Ok(spec) = std::env::var("TONEARM_AUDIO_TEST") else {
        return;
    };
    let files: Vec<std::path::PathBuf> = spec.split('|').map(Into::into).collect();
    let audio = Audio::new();
    let s = audio.status.clone();
    let wait = |what: &str, ok: &dyn Fn() -> bool| {
        let start = std::time::Instant::now();
        while !ok() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
        eprintln!("{what}: {} ms", start.elapsed().as_millis());
    };
    let pos = || s.position_ms.load(Ordering::SeqCst);
    audio.volume(0.0);
    audio.load(Input::File(files[0].clone()), 1);
    wait("playing", &|| s.state.load(Ordering::SeqCst) == PLAYING && pos() > 200);
    assert!(s.duration_ms.load(Ordering::SeqCst) > 60_000);

    audio.seek(100.0);
    wait("seek to 100 s", &|| (100_000..101_000).contains(&pos()));
    audio.pause();
    std::thread::sleep(Duration::from_millis(300));
    let held = pos();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pos(), held, "position moved while paused");
    audio.play();
    wait("plays again", &|| pos() > held + 100);

    // A mix that waits for 101.5 s of the song going out.
    audio.prepare(Input::File(files[1].clone()), 7);
    wait("prepared", &|| s.prepared.load(Ordering::SeqCst) == 7);
    let mut plan = Transition::fade(2.0);
    let at = (pos() as f64 / 1000.0) + 1.0;
    plan.out_at = Some(at);
    plan.in_from = 30.0;
    audio.crossfade(7, 2, plan);
    // Until then the new song reports where it will start.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pos(), 30_000);
    wait("mix starts", &|| pos() > 30_050);
    wait("mix done", &|| pos() > 32_200);
    assert_eq!(s.state.load(Ordering::SeqCst), PLAYING);

    // Gapless: the first song again, armed to follow the end of this one.
    audio.prepare(Input::File(files[0].clone()), 8);
    wait("prepared again", &|| s.prepared.load(Ordering::SeqCst) == 8);
    audio.follow(8, 1.0);
    let length = s.duration_ms.load(Ordering::SeqCst);
    audio.seek(length as f32 / 1000.0 - 1.0);
    wait("followed", &|| s.followed.load(Ordering::SeqCst) == 8);
    assert!(pos() < 2_000, "the followed song starts at the top, not {}", pos());
    assert_eq!(s.state.load(Ordering::SeqCst), PLAYING);

    // And the end, with nothing to follow.
    let length = s.duration_ms.load(Ordering::SeqCst);
    assert!(length > 60_000, "the followed song's length is known");
    audio.seek(length as f32 / 1000.0 - 0.5);
    wait("ended", &|| s.state.load(Ordering::SeqCst) == ENDED);
    audio.stop();
}
