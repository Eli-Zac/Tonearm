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
    /// The mids, between the bass and the treble, as a gain from 0 to 1.
    Mid,
    /// The treble, above about 2.5 kHz, as a gain from 0 to 1.
    Treble,
    /// A high pass filter's cutoff in hertz. 20 or less is off.
    HighPass,
    /// A low pass filter's cutoff in hertz. 20 000 or more is off.
    LowPass,
    /// A rush of noise over the song, 0 to 1. It brightens as it rises.
    Noise,
    /// How much of the song feeds the transition's effect, 0 to 1.
    Send,
    /// How loud the effect comes back, 0 to 1.
    Wet,
}

/// An effect on the song going out. Times are in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectKind {
    Reverb,
    /// Echoes that bounce from side to side.
    PingPong(f64),
    Echo(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    pub kind: EffectKind,
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
    /// An effect fed from the song going out, which rings on after it.
    pub effect: Option<Effect>,
    /// Seconds of the song going out, from where the mix starts, that
    /// loop through the mix.
    pub out_loop: Option<f64>,
    /// The song coming in keeps its pitch as its speed changes.
    pub key_lock: bool,
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
            effect: None,
            out_loop: None,
            key_lock: false,
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
/// Where the treble gain splits it.
const TREBLE_HZ: f32 = 2500.0;

/// The EQ and filters a transition moves on one song.
#[derive(Clone, Copy)]
struct Tone {
    bass: f32,
    mid: f32,
    treble: f32,
    /// The bass and treble, split off and always running, so turning them
    /// down never starts from a cold filter and clicks. The mids are what
    /// is left.
    low: Biquad,
    high: Biquad,
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
        let mut high = Biquad::default();
        high.set(true, TREBLE_HZ, rate);
        Tone {
            bass: 1.0,
            mid: 1.0,
            treble: 1.0,
            low,
            high,
            high_pass: 0.0,
            hp: Biquad::default(),
            low_pass: f32::MAX,
            lp: Biquad::default(),
            set: (0.0, 0.0),
        }
    }

    fn flat(&mut self) {
        self.bass = 1.0;
        self.mid = 1.0;
        self.treble = 1.0;
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
            let high = self.high.run(c, *sample);
            if self.bass < 1.0 || self.mid < 1.0 || self.treble < 1.0 {
                let mid = *sample - low - high;
                *sample += (self.bass - 1.0) * low + (self.mid - 1.0) * mid + (self.treble - 1.0) * high;
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
    /// Keeps the pitch as the speed changes: the stream plays at its own
    /// speed, and this stretches it in time. None once back at speed 1.
    stretch: Option<Box<Stretch>>,
    /// A stretch of the song repeating, from when it was armed.
    looper: Option<Looper>,
    /// The noise lane, and the noise's own state.
    noise: f32,
    hiss: Hiss,
    /// The transition's effect, fed by this song as it goes out, and how
    /// much it is fed and comes back.
    fx: Option<Box<FxUnit>>,
    send: f32,
    wet: f32,
    /// The last frame before the volume, which feeds the effect.
    dry: [f32; 2],
    /// The mixer's rate, for counting the position in seconds.
    rate: f64,
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
            stretch: None,
            looper: None,
            noise: 0.0,
            hiss: Hiss::default(),
            fx: None,
            send: 0.0,
            wet: 1.0,
            dry: [0.0; 2],
            rate: 44_100.0,
        }
    }

    fn tone(&mut self, rate: f64) -> &mut Tone {
        self.tone.get_or_insert_with(|| Tone::new(rate as f32))
    }

    /// Keeps the song's pitch when its speed changes. Before it plays.
    pub fn lock_key(&mut self) {
        self.stretch = Some(Box::new(Stretch::new()));
    }

    /// Moves to `seconds` in, before it plays.
    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        self.stream.seek(seconds)?;
        self.position = seconds.max(0.0);
        self.ended = false;
        if let Some(stretch) = self.stretch.as_mut() {
            stretch.reset();
        }
        self.looper = None;
        Ok(())
    }

    fn set_speed(&mut self, speed: f64) {
        if (speed - self.speed).abs() > 1e-7 {
            self.speed = speed;
            // Under key lock the stream stays at its own speed.
            if self.stretch.is_none() {
                self.stream.set_speed(speed);
            }
        }
    }

    /// The next frame of the song as it is, in stereo.
    fn read(stream: &mut Box<dyn Stream>, channels: usize) -> Option<[f32; 2]> {
        let mut frame = [0.0f32; 2];
        for c in 0..channels {
            let sample = stream.next()?;
            if c < 2 {
                frame[c] = sample;
            }
        }
        if channels == 1 {
            frame[1] = frame[0];
        }
        Some(frame)
    }

    /// The next frame at the deck's speed: stretched under key lock, or
    /// from its loop.
    fn source(&mut self) -> Option<[f32; 2]> {
        let speed = self.speed;
        let (stream, channels, stretch) = (&mut self.stream, self.channels, &mut self.stretch);
        let mut live = || -> Option<[f32; 2]> {
            match stretch.as_mut() {
                Some(s) => s.next(speed, &mut || Deck::read(stream, channels)),
                None => Deck::read(stream, channels),
            }
        };
        let (frame, fresh) = match self.looper.as_mut() {
            Some(looper) => looper.next(&mut live)?,
            None => (live()?, true),
        };
        if fresh {
            self.position += speed / self.rate;
        }
        // Back at its own speed, the stretch has handed over to the stream.
        if self.stretch.as_ref().is_some_and(|s| s.done()) {
            self.stretch = None;
        }
        Some(frame)
    }

    /// The next stereo frame, at its gain and volume. None once it ends.
    fn frame(&mut self, rate: f64) -> Option<[f32; 2]> {
        if self.ended {
            return None;
        }
        self.rate = rate;
        let Some(mut frame) = self.source() else {
            self.ended = true;
            return None;
        };
        self.tone(rate).run(&mut frame, rate as f32);
        self.dry = [frame[0] * self.gain, frame[1] * self.gain];
        let level = self.gain * self.volume;
        let mut out = [frame[0] * level, frame[1] * level];
        if self.noise > 0.0 {
            let n = self.hiss.next(self.noise, rate as f32);
            out[0] += n[0];
            out[1] += n[1];
        }
        Some(out)
    }

    /// The effect's output this frame, fed with the song's last frame when
    /// it played one, or silence as its tail rings on.
    fn effect(&mut self, played: bool) -> [f32; 2] {
        let Some(fx) = self.fx.as_mut() else {
            return [0.0; 2];
        };
        let input = if played { [self.dry[0] * self.send, self.dry[1] * self.send] } else { [0.0; 2] };
        let wet = fx.run(input);
        [wet[0] * self.wet, wet[1] * self.wet]
    }
}

// ---------------------------------------------------------------- key lock

/// Frames in each grain of the stretch, half of which overlap. About 43 ms.
const GRAIN: usize = 2048;
const HOP: usize = GRAIN / 2;
/// How far either side each grain may move to line up with the last.
const SEEK: i64 = 384;
/// Frames compared when lining a grain up.
const MATCH: usize = 512;

/// Plays a song faster or slower at the same pitch, by WSOLA: grains of
/// the song are laid down overlapping, each taken from a little further
/// on, or less far, than it is played, and moved a few milliseconds to
/// where it best continues the grain before. The result runs at `speed`
/// through the song and lines up with it to within `SEEK`.
struct Stretch {
    /// The song's frames from `base` on.
    input: Vec<[f32; 2]>,
    base: u64,
    /// Where in the song the next grain would come from, unmoved.
    next: f64,
    /// Where the last grain came from. None before the first.
    prev: Option<u64>,
    /// Grains overlapping, and how much of the first half has gone out.
    acc: Vec<[f32; 2]>,
    sent: usize,
    window: Vec<f32>,
    /// Once back at speed 1: the song continues from here, as is.
    straight: Option<u64>,
    /// No more stretching: what was read is used up and the stream plays on.
    finished: bool,
    ended: bool,
}

impl Stretch {
    fn new() -> Stretch {
        let window = (0..GRAIN)
            .map(|n| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * n as f32 / GRAIN as f32).cos())
            .collect();
        Stretch {
            input: Vec::with_capacity(GRAIN * 8),
            base: 0,
            next: 0.0,
            prev: None,
            acc: vec![[0.0; 2]; GRAIN],
            sent: HOP,
            window,
            straight: None,
            finished: false,
            ended: false,
        }
    }

    fn reset(&mut self) {
        self.input.clear();
        self.base = 0;
        self.next = 0.0;
        self.prev = None;
        self.acc.iter_mut().for_each(|f| *f = [0.0; 2]);
        self.sent = HOP;
        self.straight = None;
        self.finished = false;
        self.ended = false;
    }

    fn done(&self) -> bool {
        self.finished
    }

    /// Reads the song until frame `upto` is in, or it ends.
    fn fill(&mut self, upto: u64, pull: &mut dyn FnMut() -> Option<[f32; 2]>) {
        while self.base + (self.input.len() as u64) < upto {
            match pull() {
                Some(frame) => self.input.push(frame),
                None => {
                    self.ended = true;
                    return;
                }
            }
        }
    }

    fn at(&self, n: u64) -> [f32; 2] {
        n.checked_sub(self.base)
            .and_then(|i| self.input.get(i as usize))
            .copied()
            .unwrap_or([0.0; 2])
    }

    fn mono(&self, n: u64) -> f32 {
        let f = self.at(n);
        f[0] + f[1]
    }

    /// How well the grain at `candidate` continues the one at `natural`.
    fn score(&self, natural: u64, candidate: u64) -> f32 {
        let (mut dot, mut energy) = (0.0f32, 1e-9f32);
        for i in (0..MATCH as u64).step_by(2) {
            let c = self.mono(candidate + i);
            dot += self.mono(natural + i) * c;
            energy += c * c;
        }
        dot / energy.sqrt()
    }

    /// Lays down the next grain. False once the song has run out.
    fn grain(&mut self, speed: f64, pull: &mut dyn FnMut() -> Option<[f32; 2]>) -> bool {
        let first = self.prev.is_none();
        let natural = self.prev.map_or(self.next.round() as u64, |p| p + HOP as u64);
        // At its own speed again, the grain is the very next part of the
        // song, so the song can carry on from it as is.
        let back = !first && (speed - 1.0).abs() < 1e-6;
        let at = if first || back {
            natural
        } else {
            let centre = self.next.round() as i64;
            let lowest = (self.base as i64).max(centre - SEEK);
            self.fill((centre + SEEK) as u64 + GRAIN as u64, pull);
            let best = |from: i64, to: i64, step: usize| {
                (from..=to)
                    .step_by(step)
                    .max_by(|&a, &b| self.score(natural, a as u64).total_cmp(&self.score(natural, b as u64)))
                    .unwrap_or(centre)
            };
            let rough = best(lowest, centre + SEEK, 4);
            best((rough - 3).max(lowest), rough + 3, 1) as u64
        };
        self.fill(at + GRAIN as u64, pull);
        if self.ended && at >= self.base + self.input.len() as u64 {
            return false;
        }
        self.acc.copy_within(HOP.., 0);
        self.acc[HOP..].iter_mut().for_each(|f| *f = [0.0; 2]);
        for j in 0..GRAIN {
            // The first grain starts at full, as nothing comes before it.
            let w = if first && j < HOP { 1.0 } else { self.window[j] };
            let f = self.at(at + j as u64);
            self.acc[j][0] += f[0] * w;
            self.acc[j][1] += f[1] * w;
        }
        self.prev = Some(at);
        self.next = if first { at as f64 } else { self.next } + HOP as f64 * speed;
        if back {
            self.straight = Some(at + HOP as u64);
        }
        // Frames no grain can reach again.
        let keep = (self.next as i64 - SEEK).min(at as i64).max(0) as u64;
        if keep > self.base + GRAIN as u64 * 4 {
            let drop = ((keep - self.base) as usize).min(self.input.len());
            self.input.drain(..drop);
            self.base += drop as u64;
        }
        true
    }

    fn next(&mut self, speed: f64, pull: &mut dyn FnMut() -> Option<[f32; 2]>) -> Option<[f32; 2]> {
        if self.sent >= HOP {
            if let Some(from) = self.straight {
                // The rest of what was read, then the stream itself.
                if from < self.base + self.input.len() as u64 {
                    self.straight = Some(from + 1);
                    return Some(self.at(from));
                }
                self.finished = true;
                return pull();
            }
            if !self.grain(speed, pull) {
                return None;
            }
            self.sent = 0;
        }
        let frame = self.acc[self.sent];
        self.sent += 1;
        Some(frame)
    }
}

// ------------------------------------------------------------------ loops

/// Frames the loop crossfades over where it comes round, so it does not
/// click. About 6 ms.
const LOOP_FADE: usize = 256;

/// A stretch of a song played over and over. The first time round it is
/// the song itself, recorded. After that it plays back, fading at each turn
/// from the song as it carried on into the top of the loop.
struct Looper {
    buffer: Vec<[f32; 2]>,
    length: usize,
    played: usize,
}

impl Looper {
    fn new(frames: usize) -> Looper {
        let length = frames.max(LOOP_FADE * 2);
        Looper { buffer: Vec::with_capacity(length + LOOP_FADE), length, played: 0 }
    }

    /// The next frame, and whether it came from the song rather than the
    /// loop.
    fn next(&mut self, live: &mut dyn FnMut() -> Option<[f32; 2]>) -> Option<([f32; 2], bool)> {
        let n = self.played;
        self.played += 1;
        if n < self.length {
            let frame = live()?;
            self.buffer.push(frame);
            return Some((frame, true));
        }
        let i = n % self.length;
        // The song as it carried on past the loop, for the turns to fade from.
        let fresh = n < self.length + LOOP_FADE;
        if fresh {
            let frame = live().unwrap_or([0.0; 2]);
            self.buffer.push(frame);
        }
        if i < LOOP_FADE {
            let t = i as f32 / LOOP_FADE as f32;
            let (a, b) = (self.buffer[i], self.buffer[self.length + i]);
            return Some(([a[0] * t + b[0] * (1.0 - t), a[1] * t + b[1] * (1.0 - t)], fresh));
        }
        Some((self.buffer[i], false))
    }
}

// ------------------------------------------------------------------ noise

/// White noise through a high pass that rises with it, so it rushes up.
#[derive(Default)]
struct Hiss {
    seed: u32,
    filter: Biquad,
    cutoff: f32,
}

impl Hiss {
    fn next(&mut self, level: f32, rate: f32) -> [f32; 2] {
        let cutoff = 300.0 + 6000.0 * level;
        if (cutoff - self.cutoff).abs() > 20.0 {
            self.filter.set(true, cutoff, rate);
            self.cutoff = cutoff;
        }
        let mut out = [0.0; 2];
        for (c, sample) in out.iter_mut().enumerate() {
            self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = (self.seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            *sample = self.filter.run(c, white) * level * level * 0.25;
        }
        out
    }
}

// ---------------------------------------------------------------- effects

/// A delay line.
struct Line {
    buffer: Vec<f32>,
    at: usize,
}

impl Line {
    fn new(frames: usize) -> Line {
        Line { buffer: vec![0.0; frames.max(1)], at: 0 }
    }

    fn out(&self) -> f32 {
        self.buffer[self.at]
    }

    /// Puts `x` in and moves on.
    fn put(&mut self, x: f32) {
        self.buffer[self.at] = x;
        self.at = (self.at + 1) % self.buffer.len();
    }
}

/// One of Freeverb's damped feedback combs.
struct Comb {
    line: Line,
    store: f32,
}

impl Comb {
    fn run(&mut self, x: f32) -> f32 {
        let y = self.line.out();
        self.store = y * 0.8 + self.store * 0.2;
        self.line.put(x + self.store * 0.84);
        y
    }
}

fn allpass(line: &mut Line, x: f32) -> f32 {
    let delayed = line.out();
    line.put(x + delayed * 0.5);
    delayed - x
}

/// The transition's effect. Built before it reaches the audio thread, as
/// its buffers are large.
enum FxUnit {
    /// Freeverb: combs side by side, then allpasses, each side a little
    /// different so it spreads.
    Reverb { combs: Vec<[Comb; 2]>, passes: Vec<[Line; 2]> },
    PingPong { left: Line, right: Line },
    Echo { line: Line, tone: f32 },
}

impl FxUnit {
    fn new(effect: Effect, rate: u32) -> FxUnit {
        let scale = rate as f64 / 44_100.0;
        let frames = |n: f64| ((n * scale) as usize).max(1);
        let seconds = |s: f64| ((s * rate as f64).round() as usize).max(1);
        match effect.kind {
            EffectKind::Reverb => FxUnit::Reverb {
                combs: [1116.0, 1188.0, 1277.0, 1356.0, 1422.0, 1491.0, 1557.0, 1617.0]
                    .iter()
                    .map(|&n| [0.0, 23.0].map(|spread| Comb { line: Line::new(frames(n + spread)), store: 0.0 }))
                    .collect(),
                passes: [556.0, 441.0, 341.0, 225.0]
                    .iter()
                    .map(|&n| [0.0, 23.0].map(|spread| Line::new(frames(n + spread))))
                    .collect(),
            },
            EffectKind::PingPong(s) => FxUnit::PingPong { left: Line::new(seconds(s)), right: Line::new(seconds(s)) },
            EffectKind::Echo(s) => FxUnit::Echo { line: Line::new(seconds(s)), tone: 0.0 },
        }
    }

    fn run(&mut self, input: [f32; 2]) -> [f32; 2] {
        let x = (input[0] + input[1]) * 0.5;
        match self {
            FxUnit::Reverb { combs, passes } => {
                let mut out = [0.0; 2];
                for (c, sample) in out.iter_mut().enumerate() {
                    let mut y: f32 = combs.iter_mut().map(|pair| pair[c].run(x * 0.03)).sum();
                    for pair in passes.iter_mut() {
                        y = allpass(&mut pair[c], y);
                    }
                    *sample = y * 3.0;
                }
                out
            }
            FxUnit::PingPong { left, right } => {
                let (l, r) = (left.out(), right.out());
                left.put(x + r * 0.5);
                right.put(l * 0.5);
                [l * 0.8, r * 0.8]
            }
            FxUnit::Echo { line, tone } => {
                let y = line.out();
                // Each echo a little darker, as on tape.
                *tone += (y - *tone) * 0.4;
                line.put(x + *tone * 0.55);
                [y * 0.8, y * 0.8]
            }
        }
    }
}

// ------------------------------------------------------------------ mixer

pub enum Command {
    /// Mixes into `deck` by `plan`. The deck is already at `plan.in_from`.
    /// Made by `Command::start`, which builds what the plan needs.
    Start { deck: Deck, plan: Transition, extras: Extras },
    /// Plays `deck` the moment the current song ends, with no gap, and
    /// reports `token` in `followed`.
    Follow { deck: Deck, token: u64 },
    Unfollow,
    /// The levelling gain of the newest song.
    Gain(f32),
}

/// What a transition needs besides its plan, built before it reaches the
/// audio thread so nothing large is made there.
pub struct Extras {
    fx: Option<Box<FxUnit>>,
    looper: Option<Looper>,
}

impl Command {
    /// Mixes into `deck` by `plan`, at the mixer's `rate`.
    pub fn start(mut deck: Deck, plan: Transition, rate: u32) -> Command {
        if plan.key_lock {
            deck.lock_key();
        }
        let extras = Extras {
            fx: plan.effect.map(|e| Box::new(FxUnit::new(e, rate))),
            looper: plan.out_loop.map(|seconds| Looper::new((seconds * rate as f64).round() as usize)),
        };
        Command::Start { deck, plan, extras }
    }
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
    queued: Option<(Deck, Transition, Extras)>,
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
            Some((deck, _, _)) => Some(deck),
            None => self.current.as_mut(),
        }
    }

    fn take_commands(&mut self) {
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Command::Start { deck, plan, extras } => {
                    // A newer transition replaces one still waiting.
                    let old = self.queued.replace((deck, plan, extras)).map(|(d, _, _)| d);
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
            (Some((_, plan, _)), Some(current)) => {
                force || current.ended || plan.out_at.is_none_or(|at| current.position >= at)
            }
        };
        if !due {
            return;
        }
        let (deck, plan, extras) = self.queued.take().unwrap();
        // One still going is cut off for the new one.
        let old = self.outgoing.take();
        self.retire(old);
        self.outgoing = self.current.replace(deck);
        if let Some(out) = self.outgoing.as_mut() {
            out.fx = extras.fx;
            out.looper = extras.looper;
        }
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
                Param::Mid => deck.tone(rate).mid = value.clamp(0.0, 1.0),
                Param::Treble => deck.tone(rate).treble = value.clamp(0.0, 1.0),
                Param::HighPass => deck.tone(rate).high_pass = value,
                Param::LowPass => deck.tone(rate).low_pass = value,
                Param::Noise => deck.noise = value.clamp(0.0, 1.0),
                Param::Send => deck.send = value.clamp(0.0, 1.0),
                Param::Wet => deck.wet = value.clamp(0.0, 1.0),
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
                // The song stops at the end of the mix. Its effect's tail
                // rings on until the plan ends.
                let frame = if t < length { deck.frame(rate) } else { None };
                if let Some(frame) = frame {
                    out[0] += frame[0];
                    out[1] += frame[1];
                }
                let wet = deck.effect(frame.is_some());
                out[0] += wet[0];
                out[1] += wet[1];
                if frame.is_none() && (deck.fx.is_none() || t >= end) {
                    let old = self.outgoing.take();
                    self.retire(old);
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
        tx.send(Command::start(flat(0.25, 1000), plan, 100)).unwrap();
        let out = frames(&mut mix, 30);
        // 100 frames a second: the old song alone for 20 frames.
        assert!(out[..20].iter().all(|&s| s == 1.0));
        assert_eq!(out[20], 1.0); // the fade's first frame is all old song
        assert!((out[29] - 1.0).abs() > 1e-3);
    }

    #[test]
    fn the_old_song_stops_when_the_transition_ends() {
        let (mut mix, tx, _) = mixer(flat(1.0, 1000));
        tx.send(Command::start(flat(0.25, 1000), Transition::fade(0.5), 100)).unwrap();
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
        tx.send(Command::start(flat(0.5, 1000), plan, 100)).unwrap();
        frames(&mut mix, 100);
        let speed = mix.current.as_ref().unwrap().speed;
        assert_eq!(speed, 1.0);
        // 0.2 s at 1.5, then a smooth ease to 1 over 0.2 s, then 0.6 s.
        let seconds = mix.current.as_ref().unwrap().position;
        assert!((seconds - (0.3 + 0.25 + 0.6)).abs() < 0.03, "{seconds}");
        let _ = monitor;
    }

    /// Times a signal crosses zero going up, a second.
    fn pitch(samples: &[f32], rate: f32) -> f32 {
        let ups = samples.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f32 * rate / samples.len() as f32
    }

    #[test]
    fn key_lock_changes_the_speed_but_not_the_pitch() {
        const RATE: f32 = 48_000.0;
        let mut n = 0u64;
        let mut sine = || {
            n += 1;
            let v = (2.0 * std::f32::consts::PI * 440.0 * n as f32 / RATE).sin();
            Some([v, v])
        };
        let mut stretch = Stretch::new();
        let out: Vec<f32> = (0..48_000).map(|_| stretch.next(1.05, &mut sine).unwrap()[0]).collect();
        let heard = pitch(&out[4096..], RATE);
        assert!((heard - 440.0).abs() < 4.0, "{heard} Hz");
        // A second out took about 1.05 s of the song.
        assert!((stretch.next / RATE as f64 - 1.05).abs() < 0.02, "{}", stretch.next);
        // Back at speed 1 it hands over to the song itself, seamlessly.
        let mut last = 0.0;
        for _ in 0..20_000 {
            last = stretch.next(1.0, &mut sine).unwrap()[0];
        }
        assert!(stretch.done());
        let next = stretch.next(1.0, &mut sine).unwrap()[0];
        assert!((next - last).abs() < 0.1, "jumped from {last} to {next}");
    }

    #[test]
    fn a_loop_repeats_the_song_after_its_first_time_round() {
        let mut n = 0.0f32;
        let mut ramp = || {
            n += 1.0;
            Some([n, n])
        };
        let mut looper = Looper::new(1000);
        let first: Vec<f32> = (0..1000).map(|_| looper.next(&mut ramp).unwrap().0[0]).collect();
        let again: Vec<f32> = (0..1000).map(|_| looper.next(&mut ramp).unwrap().0[0]).collect();
        assert_eq!(first[0], 1.0);
        assert_eq!(&again[LOOP_FADE..], &first[LOOP_FADE..]);
        // Each turn fades from the song as it carried on into the loop.
        assert!((again[0] - 1001.0).abs() < 1e-3);
        let third = looper.next(&mut ramp).unwrap();
        assert!(!third.1);
    }

    #[test]
    fn an_echo_rings_on_after_the_old_song_stops() {
        let (mut mix, tx, _) = mixer(flat(1.0, 1000));
        let mut plan = Transition::fade(0.5);
        plan.effect = Some(Effect { kind: EffectKind::Echo(0.1) });
        plan.lanes.push(Lane::new(Side::Out, Param::Send, &[(0.0, 1.0, Curve::Linear)]));
        plan.lanes.push(Lane::new(Side::Out, Param::Wet, &[(0.0, 1.0, Curve::Linear), (2.0, 1.0, Curve::Linear)]));
        tx.send(Command::start(flat(0.0, 1000), plan, 100)).unwrap();
        let out = frames(&mut mix, 300);
        // The new song is silent: all that is heard past 0.5 s is the echo.
        assert!(out[55..80].iter().any(|&s| s.abs() > 0.05));
        assert!(out[250..].iter().all(|&s| s == 0.0));
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
        tx.send(Command::start(inn, plan, RATE)).unwrap();
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
