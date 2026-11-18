//! The mixer. One rodio source that plays the current song and, during a
//! transition, the next one with it, to the very frame.
//!
//! Every transition is a plan (`Transition`): where it starts in the song
//! going out, where the song coming in starts from, and lanes that move
//! each song's volume and speed over time. A plain crossfade is one plan,
//! a beat matched mix is another, and later a mix you edit by hand is the
//! same thing again. The mixer only ever plays plans.
//!
//! Both songs come out of this one source, so they share one clock. Two
//! rodio sinks would each start on their own audio callback, a few
//! milliseconds apart, which is enough to make two kick drums flam.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

/// The mixer always puts out stereo. Mono songs play on both sides.
pub const CHANNELS: u16 = 2;
/// Frames between looks at the command queue. About 1.3 ms at 48 kHz.
const BLOCK: u32 = 64;

// ------------------------------------------------------------------ plans

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    /// The song going out.
    Out,
    /// The song coming in.
    In,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Param {
    /// A gain, 0 to 1, on top of levelling and the volume slider.
    Volume,
    /// How fast the song plays, 1 for as recorded. The pitch moves with it.
    Speed,
    /// The bass, below about 200 Hz, as a gain from 0 to 1.
    Bass,
    /// A high pass filter's cutoff in hertz. 20 or less is off.
    HighPass,
    /// A low pass filter's cutoff in hertz. 20 000 or more is off.
    LowPass,
}

/// How a lane moves from one key to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Linear,
    /// Slow at both ends.
    Smooth,
    /// Slow to start, as a quarter cosine. With `EaseOut` on the other
    /// song, a crossfade keeps the loudness steady.
    EaseIn,
    /// Quick to start, as a quarter sine.
    EaseOut,
}

impl Curve {
    fn shape(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        let quarter = t * std::f32::consts::FRAC_PI_2;
        match self {
            Curve::Linear => t,
            Curve::Smooth => t * t * (3.0 - 2.0 * t),
            Curve::EaseIn => 1.0 - quarter.cos(),
            Curve::EaseOut => quarter.sin(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Seconds from the start of the transition.
    pub at: f64,
    pub value: f32,
    /// How the lane gets here from the key before.
    pub curve: Curve,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub side: Side,
    pub param: Param,
    /// In time order.
    pub keys: Vec<Key>,
}

impl Lane {
    pub fn new(side: Side, param: Param, keys: &[(f64, f32, Curve)]) -> Lane {
        Lane {
            side,
            param,
            keys: keys
                .iter()
                .map(|&(at, value, curve)| Key { at, value, curve })
                .collect(),
        }
    }

    /// The lane's value `t` seconds in. It holds its first value before
    /// the first key and its last after the last.
    pub fn value(&self, t: f64) -> Option<f32> {
        let first = self.keys.first()?;
        if t <= first.at {
            return Some(first.value);
        }
        for pair in self.keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t < b.at {
                let span = (b.at - a.at).max(1e-9);
                let along = b.curve.shape(((t - a.at) / span) as f32);
                return Some(a.value + (b.value - a.value) * along);
            }
        }
        self.keys.last().map(|k| k.value)
    }

    fn end(&self) -> f64 {
        self.keys.last().map_or(0.0, |k| k.at)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// Where in the song going out it starts, in that song's seconds. None
    /// starts it at once.
    pub out_at: Option<f64>,
    /// Where the song coming in starts from, in its own seconds.
    pub in_from: f64,
    /// Seconds the songs overlap. The song going out stops at the end.
    pub length: f64,
    /// Lanes for the song coming in may run on past `length`, as when its
    /// speed eases back after the mix.
    pub lanes: Vec<Lane>,
}

impl Transition {
    /// The plain crossfade: starts at once, from the top of the next song,
    /// at equal power over `seconds`.
    pub fn fade(seconds: f64) -> Transition {
        let length = seconds.max(0.0);
        Transition {
            out_at: None,
            in_from: 0.0,
            length,
            lanes: vec![
                Lane::new(Side::Out, Param::Volume, &[(0.0, 1.0, Curve::Linear), (length, 0.0, Curve::EaseIn)]),
                Lane::new(Side::In, Param::Volume, &[(0.0, 0.0, Curve::Linear), (length, 1.0, Curve::EaseOut)]),
            ],
        }
    }

    /// When the last lane settles.
    fn end(&self) -> f64 {
        self.lanes.iter().map(Lane::end).fold(self.length, f64::max)
    }
}

// ------------------------------------------------------------------ decks

/// A song the mixer can play.
pub trait Stream: Iterator<Item = f32> + Send {
    fn channels(&self) -> usize;
    fn set_speed(&mut self, speed: f64);
    fn seek(&mut self, seconds: f64) -> Result<(), String>;
}

impl Stream for crate::decode::Track {
    fn channels(&self) -> usize {
        rodio::Source::channels(self) as usize
    }

    fn set_speed(&mut self, speed: f64) {
        crate::decode::Track::set_speed(self, speed);
    }

    fn seek(&mut self, seconds: f64) -> Result<(), String> {
        rodio::Source::try_seek(self, Duration::from_secs_f64(seconds.max(0.0))).map_err(|e| e.to_string())
    }
}

/// A two pole filter, one per channel, from the Audio EQ Cookbook.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    /// The last two inputs and outputs of each channel.
    x: [[f32; 2]; 2],
    y: [[f32; 2]; 2],
}

impl Biquad {
    fn set(&mut self, high: bool, hz: f32, rate: f32) {
        let w = 2.0 * std::f32::consts::PI * (hz / rate).clamp(1e-5, 0.49);
        let (cos, alpha) = (w.cos(), w.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2));
        let a0 = 1.0 + alpha;
        let (b0, b1) = if high { ((1.0 + cos) / 2.0, -(1.0 + cos)) } else { ((1.0 - cos) / 2.0, 1.0 - cos) };
        self.b = [b0 / a0, b1 / a0, b0 / a0];
        self.a = [-2.0 * cos / a0, (1.0 - alpha) / a0];
    }

    fn run(&mut self, channel: usize, x: f32) -> f32 {
        let (xs, ys) = (&mut self.x[channel], &mut self.y[channel]);
        let y = self.b[0] * x + self.b[1] * xs[0] + self.b[2] * xs[1] - self.a[0] * ys[0] - self.a[1] * ys[1];
        *xs = [x, xs[0]];
        *ys = [y, ys[0]];
        y
    }

    fn clear(&mut self) {
        self.x = [[0.0; 2]; 2];
        self.y = [[0.0; 2]; 2];
    }
}

/// Where the bass gain splits the sound.
const BASS_HZ: f32 = 200.0;

/// The EQ and filters a transition moves on one song.
#[derive(Clone, Copy)]
struct Tone {
    bass: f32,
    /// The bass, split off and always running, so turning it down never
    /// starts from a cold filter and clicks.
    low: Biquad,
    high_pass: f32,
    hp: Biquad,
    low_pass: f32,
    lp: Biquad,
    /// The cutoffs the filters were last set for.
    set: (f32, f32),
}

impl Tone {
    fn new(rate: f32) -> Tone {
        let mut low = Biquad::default();
        low.set(false, BASS_HZ, rate);
        Tone {
            bass: 1.0,
            low,
            high_pass: 0.0,
            hp: Biquad::default(),
            low_pass: f32::MAX,
            lp: Biquad::default(),
            set: (0.0, 0.0),
        }
    }

    fn flat(&mut self) {
        self.bass = 1.0;
        self.high_pass = 0.0;
        self.low_pass = f32::MAX;
    }

    fn run(&mut self, frame: &mut [f32; 2], rate: f32) {
        let hp_on = self.high_pass > 20.0;
        let lp_on = self.low_pass < 20_000.0;
        if hp_on && self.set.0 != self.high_pass {
            self.hp.set(true, self.high_pass, rate);
            self.set.0 = self.high_pass;
        }
        if lp_on && self.set.1 != self.low_pass {
            self.lp.set(false, self.low_pass, rate);
            self.set.1 = self.low_pass;
        }
        for (c, sample) in frame.iter_mut().enumerate() {
            let low = self.low.run(c, *sample);
            if self.bass < 1.0 {
                *sample += (self.bass - 1.0) * low;
            }
            if hp_on {
                *sample = self.hp.run(c, *sample);
            }
            if lp_on {
                *sample = self.lp.run(c, *sample);
            }
        }
        // A filter that turns off forgets, so it starts clean next time.
        if !hp_on && self.set.0 != 0.0 {
            self.hp.clear();
            self.set.0 = 0.0;
        }
        if !lp_on && self.set.1 != 0.0 {
            self.lp.clear();
            self.set.1 = 0.0;
        }
    }
}

/// One song in the mixer, and where it has got to.
pub struct Deck {
    stream: Box<dyn Stream>,
    channels: usize,
    /// Seconds of the song played, as recorded. Runs faster when sped up.
    position: f64,
    speed: f64,
    /// Levelling gain, and the volume a transition gives it.
    gain: f32,
    volume: f32,
    /// Made when the deck first plays, once the rate is known.
    tone: Option<Tone>,
    ended: bool,
}

impl Deck {
    pub fn new(stream: Box<dyn Stream>, gain: f32) -> Deck {
        Deck {
            channels: stream.channels().max(1),
            stream,
            position: 0.0,
            speed: 1.0,
            gain,
            volume: 1.0,
            tone: None,
            ended: false,
        }
    }

    fn tone(&mut self, rate: f64) -> &mut Tone {
        self.tone.get_or_insert_with(|| Tone::new(rate as f32))
    }

    /// Moves to `seconds` in, before it plays.
    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        self.stream.seek(seconds)?;
        self.position = seconds.max(0.0);
        self.ended = false;
        Ok(())
    }

    fn set_speed(&mut self, speed: f64) {
        if (speed - self.speed).abs() > 1e-7 {
            self.speed = speed;
            self.stream.set_speed(speed);
        }
    }

    /// The next stereo frame, at its gain and volume. None once it ends.
    fn frame(&mut self, rate: f64) -> Option<[f32; 2]> {
        if self.ended {
            return None;
        }
        let mut frame = [0.0f32; 2];
        for c in 0..self.channels {
            let Some(sample) = self.stream.next() else {
                self.ended = true;
                return None;
            };
            if c < 2 {
                frame[c] = sample;
            }
        }
        if self.channels == 1 {
            frame[1] = frame[0];
        }
        self.tone(rate).run(&mut frame, rate as f32);
        self.position += self.speed / rate;
        let level = self.gain * self.volume;
        Some([frame[0] * level, frame[1] * level])
    }
}

// ------------------------------------------------------------------ mixer

pub enum Command {
    /// Mixes into `deck` by `plan`. The deck is already at `plan.in_from`.
    Start { deck: Deck, plan: Transition },
    /// Plays `deck` the moment the current song ends, with no gap, and
    /// reports `token` in `followed`.
    Follow { deck: Deck, token: u64 },
    Unfollow,
    /// The levelling gain of the newest song.
    Gain(f32),
}

/// What the mixer tells the audio thread.
#[derive(Default)]
pub struct Monitor {
    /// Where the newest song is, in milliseconds of it.
    pub position_ms: AtomicU64,
    /// The token of a song that just started through `Follow`.
    pub followed: AtomicU64,
}

pub struct Mix {
    rate: u32,
    current: Option<Deck>,
    /// The song going out during a transition.
    outgoing: Option<Deck>,
    /// The transition under way, and frames since it started.
    active: Option<(Transition, u64)>,
    /// A transition waiting for the song going out to reach `out_at`.
    queued: Option<(Deck, Transition)>,
    follow: Option<(Deck, u64)>,
    commands: Receiver<Command>,
    monitor: Arc<Monitor>,
    /// Decks done with go back to the audio thread to be freed. Freeing a
    /// song's buffers here would hold up the audio callback.
    done: Sender<Deck>,
    frame: [f32; 2],
    channel: usize,
    until_block: u32,
}

impl Mix {
    pub fn new(
        rate: u32,
        deck: Deck,
        commands: Receiver<Command>,
        monitor: Arc<Monitor>,
        done: Sender<Deck>,
    ) -> Mix {
        Mix {
            rate: rate.max(1),
            current: Some(deck),
            outgoing: None,
            active: None,
            queued: None,
            follow: None,
            commands,
            monitor,
            done,
            frame: [0.0; 2],
            channel: 2,
            until_block: 0,
        }
    }

    fn retire(&self, deck: Option<Deck>) {
        if let Some(deck) = deck {
            let _ = self.done.send(deck);
        }
    }

    fn newest(&mut self) -> Option<&mut Deck> {
        match &mut self.queued {
            Some((deck, _)) => Some(deck),
            None => self.current.as_mut(),
        }
    }

    fn take_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Start { deck, plan } => {
                    // A newer transition replaces one still waiting.
                    let old = self.queued.replace((deck, plan)).map(|(d, _)| d);
                    self.retire(old);
                }
                Command::Follow { deck, token } => {
                    let old = self.follow.replace((deck, token)).map(|(d, _)| d);
                    self.retire(old);
                }
                Command::Unfollow => {
                    let old = self.follow.take().map(|(d, _)| d);
                    self.retire(old);
                }
                Command::Gain(gain) => {
                    if let Some(deck) = self.newest() {
                        deck.gain = gain;
                    }
                }
            }
        }
    }

    /// Starts the waiting transition once the song going out reaches it.
    fn start_due(&mut self, force: bool) {
        let due = match (&self.queued, &self.current) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some((_, plan)), Some(current)) => {
                force || current.ended || plan.out_at.is_none_or(|at| current.position >= at)
            }
        };
        if !due {
            return;
        }
        let (deck, plan) = self.queued.take().unwrap();
        // One still going is cut off for the new one.
        let old = self.outgoing.take();
        self.retire(old);
        self.outgoing = self.current.replace(deck);
        self.active = Some((plan, 0));
        self.automate();
    }

    /// Sets each song's volume and speed from the lanes, for this frame.
    fn automate(&mut self) {
        let Some((plan, frames)) = &self.active else {
            return;
        };
        let rate = self.rate as f64;
        let t = *frames as f64 / rate;
        for lane in &plan.lanes {
            let deck = match lane.side {
                Side::Out => self.outgoing.as_mut(),
                Side::In => self.current.as_mut(),
            };
            let (Some(deck), Some(value)) = (deck, lane.value(t)) else {
                continue;
            };
            match lane.param {
                Param::Volume => deck.volume = value.clamp(0.0, 4.0),
                Param::Speed => deck.set_speed(value as f64),
                Param::Bass => deck.tone(rate).bass = value.clamp(0.0, 1.0),
                Param::HighPass => deck.tone(rate).high_pass = value,
                Param::LowPass => deck.tone(rate).low_pass = value,
            }
        }
    }

    /// Mixes the next frame. None once nothing is left to play.
    fn next_frame(&mut self) -> Option<[f32; 2]> {
        if self.until_block == 0 {
            self.until_block = BLOCK;
            self.take_commands();
            if let Some(deck) = self.newest() {
                let ms = (deck.position * 1000.0) as u64;
                self.monitor.position_ms.store(ms, Ordering::Relaxed);
            }
        }
        self.until_block -= 1;
        self.start_due(false);
        let rate = self.rate as f64;
        if self.active.is_some() {
            self.automate();
        }
        let mut out = match self.current.as_mut().and_then(|d| d.frame(rate)) {
            Some(frame) => frame,
            None => {
                // The song ran out. What follows it starts on this frame.
                if let Some((deck, token)) = self.follow.take() {
                    let old = self.current.replace(deck);
                    self.retire(old);
                    self.monitor.followed.store(token, Ordering::SeqCst);
                    self.current.as_mut().and_then(|d| d.frame(rate)).unwrap_or([0.0; 2])
                } else if self.queued.is_some() {
                    self.start_due(true);
                    self.current.as_mut().and_then(|d| d.frame(rate)).unwrap_or([0.0; 2])
                } else if self.outgoing.is_none() {
                    return None;
                } else {
                    [0.0; 2]
                }
            }
        };
        if let Some((plan, frames)) = &mut self.active {
            let t = *frames as f64 / rate;
            *frames += 1;
            let (length, end) = (plan.length, plan.end());
            if let Some(deck) = self.outgoing.as_mut() {
                match deck.frame(rate).filter(|_| t < length) {
                    Some(frame) => {
                        out[0] += frame[0];
                        out[1] += frame[1];
                    }
                    None => {
                        let old = self.outgoing.take();
                        self.retire(old);
                    }
                }
            }
            if t >= end && self.outgoing.is_none() {
                self.active = None;
            }
        }
        Some(out)
    }
}

impl Iterator for Mix {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.channel >= 2 {
            self.frame = self.next_frame()?;
            self.channel = 0;
        }
        let sample = self.frame[self.channel];
        self.channel += 1;
        Some(sample)
    }
}

impl rodio::Source for Mix {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }

    /// Seeks the newest song. A transition under way ends there: the song
    /// going out stops, and the one coming in plays on at full volume and
    /// its own speed.
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        if self.active.take().is_some() {
            let old = self.outgoing.take();
            self.retire(old);
            if let Some(deck) = self.current.as_mut() {
                deck.volume = 1.0;
                deck.set_speed(1.0);
                if let Some(tone) = deck.tone.as_mut() {
                    tone.flat();
                }
            }
        }
        let Some(deck) = self.newest() else {
            return Ok(());
        };
        deck.seek(position.as_secs_f64())
            .map_err(|e| rodio::source::SeekError::Other(Box::new(std::io::Error::other(e))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A mono song where every sample is `level`, `frames` long.
    struct Flat {
        level: f32,
        left: usize,
        speed: f64,
    }

    impl Iterator for Flat {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            self.left = self.left.checked_sub(1)?;
            Some(self.level)
        }
    }

    impl Stream for Flat {
        fn channels(&self) -> usize {
            1
        }
        fn set_speed(&mut self, speed: f64) {
            self.speed = speed;
        }
        fn seek(&mut self, _: f64) -> Result<(), String> {
            Ok(())
        }
    }

    fn flat(level: f32, frames: usize) -> Deck {
        Deck::new(Box::new(Flat { level, left: frames, speed: 1.0 }), 1.0)
    }

    fn mixer(deck: Deck) -> (Mix, Sender<Command>, Arc<Monitor>) {
        let (tx, rx) = mpsc::channel();
        let (done, _) = mpsc::channel();
        let monitor = Arc::new(Monitor::default());
        (Mix::new(100, deck, rx, monitor.clone(), done), tx, monitor)
    }

    fn frames(mix: &mut Mix, count: usize) -> Vec<f32> {
        (0..count).filter_map(|_| mix.next_frame()).map(|f| f[0]).collect()
    }

    #[test]
    fn a_crossfade_starts_with_the_old_song_and_ends_with_the_new() {
        let plan = Transition::fade(1.0);
        let out = &plan.lanes[0];
        let inn = &plan.lanes[1];
        assert_eq!((out.value(0.0), inn.value(0.0)), (Some(1.0), Some(0.0)));
        assert!(out.value(1.0).unwrap().abs() < 1e-6);
        assert!((inn.value(1.0).unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(out.value(-1.0), out.value(0.0));
        assert_eq!(inn.value(5.0), inn.value(1.0));
    }

    #[test]
    fn a_crossfade_keeps_the_loudness_steady() {
        let plan = Transition::fade(1.0);
        for step in 0..=100 {
            let t = step as f64 / 100.0;
            let (a, b) = (plan.lanes[0].value(t).unwrap(), plan.lanes[1].value(t).unwrap());
            assert!((a * a + b * b - 1.0).abs() < 1e-4, "at {t}");
        }
    }

    #[test]
    fn a_transition_waits_for_its_frame_in_the_old_song() {
        let (mut mix, tx, _) = mixer(flat(1.0, 1000));
        let mut plan = Transition::fade(0.5);
        plan.out_at = Some(0.2);
        tx.send(Command::Start { deck: flat(0.25, 1000), plan }).unwrap();
        let out = frames(&mut mix, 30);
        // 100 frames a second: the old song alone for 20 frames.
        assert!(out[..20].iter().all(|&s| s == 1.0));
        assert_eq!(out[20], 1.0); // the fade's first frame is all old song
        assert!((out[29] - 1.0).abs() > 1e-3);
    }

    #[test]
    fn the_old_song_stops_when_the_transition_ends() {
        let (mut mix, tx, _) = mixer(flat(1.0, 1000));
        tx.send(Command::Start { deck: flat(0.25, 1000), plan: Transition::fade(0.5) }).unwrap();
        let out = frames(&mut mix, 80);
        assert!(out[60..].iter().all(|&s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn a_followed_song_starts_on_the_next_frame() {
        let (mut mix, tx, monitor) = mixer(flat(1.0, 10));
        tx.send(Command::Follow { deck: flat(0.5, 10), token: 7 }).unwrap();
        let out = frames(&mut mix, 20);
        assert_eq!(out.len(), 20);
        assert_eq!((out[9], out[10]), (1.0, 0.5));
        assert_eq!(monitor.followed.load(Ordering::SeqCst), 7);
        assert!(mix.next_frame().is_none());
    }

    #[test]
    fn speed_lanes_move_the_song_coming_in() {
        let (mut mix, tx, monitor) = mixer(flat(1.0, 1000));
        let mut plan = Transition::fade(0.2);
        plan.lanes.push(Lane::new(Side::In, Param::Speed, &[(0.0, 1.5, Curve::Linear), (0.2, 1.5, Curve::Linear), (0.4, 1.0, Curve::Smooth)]));
        tx.send(Command::Start { deck: flat(0.5, 1000), plan }).unwrap();
        frames(&mut mix, 100);
        let speed = mix.current.as_ref().unwrap().speed;
        assert_eq!(speed, 1.0);
        // 0.2 s at 1.5, then a smooth ease to 1 over 0.2 s, then 0.6 s.
        let seconds = mix.current.as_ref().unwrap().position;
        assert!((seconds - (0.3 + 0.25 + 0.6)).abs() < 0.03, "{seconds}");
        let _ = monitor;
    }
}

/// Renders a beat matched mix between two saved songs into a WAV, through
/// the real mixer: `TONEARM_MIX="<file out> <file in> <wav>" cargo test
/// render_mix --release -- --ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore]
fn render_mix() {
    use std::io::Write;
    let Ok(spec) = std::env::var("TONEARM_MIX") else {
        return;
    };
    let parts: Vec<&str> = spec.split('|').collect();
    let (out_path, in_path, wav) = (parts[0], parts[1], parts[2]);
    const RATE: u32 = 44_100;
    let open = |path: &str| crate::decode::Track::open(Box::new(std::fs::File::open(path).unwrap()), RATE).unwrap();
    let analyze = |path: &str| crate::analysis::analyze_file(std::path::Path::new(path)).unwrap();
    let (a, b) = (analyze(out_path), analyze(in_path));
    eprintln!("out {:.2} BPM ({:.2}), in {:.2} BPM ({:.2})", a.bpm, a.bpm_confidence, b.bpm, b.bpm_confidence);
    let out_track = open(out_path);
    let length = rodio::Source::total_duration(&out_track).unwrap().as_secs_f64();
    let preset = std::env::var("TONEARM_MIX_PRESET").unwrap_or_default();
    let preset = crate::transition::Preset::OFFERED
        .into_iter()
        .find(|p| p.name().eq_ignore_ascii_case(&preset))
        .unwrap_or(crate::transition::Preset::Blend);
    let settings = crate::transition::Settings::preset(preset);
    let plan = crate::transition::plan(settings, Some(&a), length, Some(&b), 8.0);
    assert!(plan.out_at.is_some(), "these two cannot be beat matched");
    let at = plan.out_at.unwrap();
    eprintln!("mix at {at:.2}s of {length:.0}s, into {:.2}s, {:.1}s long", plan.in_from, plan.length);
    let lead = 12.0;
    let seconds = lead + plan.length + 20.0;
    // The mix, and each song alone through the same plan, to measure.
    let render = |keep_out: bool, keep_in: bool| -> Vec<f32> {
        let mut plan = plan.clone();
        for lane in plan.lanes.iter_mut().filter(|l| l.param == Param::Volume) {
            let keep = if lane.side == Side::Out { keep_out } else { keep_in };
            if !keep {
                lane.keys.iter_mut().for_each(|k| k.value = 0.0);
            }
        }
        let mut out = Deck::new(Box::new(open(out_path)), 1.0);
        out.seek(at - lead).unwrap();
        let mut inn = Deck::new(Box::new(open(in_path)), 1.0);
        inn.seek(plan.in_from).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (done, _keep) = std::sync::mpsc::channel();
        let mut mix = Mix::new(RATE, out, rx, Arc::new(Monitor::default()), done);
        tx.send(Command::Start { deck: inn, plan }).unwrap();
        (&mut mix).take((seconds * RATE as f64) as usize * 2).collect()
    };
    // How far the songs' onsets sit apart over the overlap, in ms.
    let mono = |s: &[f32]| -> Vec<f32> { s.chunks(2).map(|f| 0.5 * (f[0] + f[1])).collect() };
    let window = |s: Vec<f32>| -> Vec<f32> {
        let from = (lead * RATE as f64) as usize;
        s[from..from + (plan.length * RATE as f64) as usize].to_vec()
    };
    let (a_curve, frame_rate) = crate::analysis::onset_curve(&window(mono(&render(true, false))), RATE as f32);
    let (b_curve, _) = crate::analysis::onset_curve(&window(mono(&render(false, true))), RATE as f32);
    let score = |lag: i32| -> f32 {
        (0..a_curve.len() as i32)
            .filter_map(|i| Some(a_curve[i as usize] * *b_curve.get((i + lag).max(0) as usize)?))
            .sum()
    };
    let lags: Vec<(i32, f32)> = (-12..=12).map(|l| (l, score(l))).collect();
    let best = lags.iter().max_by(|x, y| x.1.total_cmp(&y.1)).unwrap();
    eprintln!(
        "onsets of the song coming in sit {:+.0} ms from the song going out (one frame is {:.1} ms)",
        best.0 as f32 * 1000.0 / frame_rate,
        1000.0 / frame_rate
    );
    let samples: Vec<i16> = render(true, true)
        .into_iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();
    let mut file = std::fs::File::create(wav).unwrap();
    let bytes = samples.len() as u32 * 2;
    let mut header = Vec::new();
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36 + bytes).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&RATE.to_le_bytes());
    header.extend_from_slice(&(RATE * 4).to_le_bytes());
    header.extend_from_slice(&4u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&bytes.to_le_bytes());
    file.write_all(&header).unwrap();
    file.write_all(&samples.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    eprintln!("wrote {wav}: the mix starts {lead}s in");
}
