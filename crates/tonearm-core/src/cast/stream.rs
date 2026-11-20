//! The stream a Cast device plays: Tonearm's own output, mixes and all, as
//! an endless WAV served over HTTP from this computer.
//!
//! The engine hands its mixer to a `Pipe` in place of the sound card. The
//! device pulls the stream, and we keep only `LEAD` ahead of what it has
//! played, so a skip or a seek is heard within about that long. How far
//! the device has got comes from its media status, with the time since it
//! connected as a floor, so a device that reports oddly never stalls.

use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How far the stream runs ahead of the device.
const LEAD: f64 = 2.0;
/// The most a device's own report may trail the time since it connected
/// before that time is trusted instead.
const SLACK: f64 = 8.0;
/// Frames written at a time: about 10 ms.
const FRAMES: usize = 512;

/// Where the pipe pulls its audio: the engine's mixer.
pub trait Feed: Send {
    /// Fills `out` with interleaved stereo. Fewer than asked means the
    /// song, and everything mixed after it, has ended.
    fn fill(&mut self, out: &mut [f32]) -> usize;
    fn seek(&mut self, to: Duration);
}

/// What the device last said about where it is.
struct Heard {
    seconds: f64,
    at: Instant,
    playing: bool,
}

#[derive(Default)]
struct Clock {
    /// Seconds streamed to the current connection.
    sent: f64,
    /// Seconds the current connection has been open and not paused.
    open: f64,
    heard: Option<Heard>,
    /// Where the device's clock stood when this connection began. A device
    /// that fetches the stream again keeps counting from where it was.
    base: f64,
}

impl Clock {
    /// How far the device has played, as best we know.
    fn played(&self) -> f64 {
        match &self.heard {
            // Until the device says, it plays in real time from connecting.
            None => self.open,
            Some(h) => {
                let reported =
                    h.seconds + if h.playing { h.at.elapsed().as_secs_f64() } else { 0.0 };
                reported.max(self.open - SLACK).max(0.0)
            }
        }
    }
}

#[derive(Default)]
pub struct Pipe {
    /// The stream's sample rate. Zero until the engine routes here.
    rate: AtomicU32,
    feed: Mutex<Option<(u64, Box<dyn Feed>)>>,
    feeds: AtomicU64,
    paused: AtomicBool,
    clock: Mutex<Clock>,
    /// Bumped by each connection from the device. Older ones stop.
    connection: AtomicU64,
    closed: AtomicBool,
    /// What the stream's samples are scaled by, as f32 bits.
    gain: AtomicU32,
}

impl Pipe {
    pub fn new() -> Arc<Pipe> {
        let pipe = Pipe::default();
        pipe.set_gain(1.0);
        Arc::new(pipe)
    }

    /// Scales the stream, for a device that ignores its own volume. Heard
    /// once the device has played the lead already sent.
    pub fn set_gain(&self, gain: f32) {
        self.gain.store(gain.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::SeqCst))
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::SeqCst);
    }

    /// The rate songs open at while they play here. Zero before the
    /// engine has routed to it.
    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::SeqCst)
    }

    /// Plays `feed` from now on. Its id lets the engine let go of it later
    /// without dropping a newer one.
    pub fn attach(&self, feed: Box<dyn Feed>) -> u64 {
        let id = self.feeds.fetch_add(1, Ordering::SeqCst) + 1;
        *self.feed.lock().unwrap() = Some((id, feed));
        id
    }

    pub fn detach(&self, id: u64) {
        let mut feed = self.feed.lock().unwrap();
        if feed.as_ref().is_some_and(|(i, _)| *i == id) {
            *feed = None;
        }
    }

    /// Whether `id` is still playing. False once it has run out.
    pub fn has(&self, id: u64) -> bool {
        self.feed.lock().unwrap().as_ref().is_some_and(|(i, _)| *i == id)
    }

    pub fn seek(&self, id: u64, to: Duration) {
        if let Some((i, feed)) = self.feed.lock().unwrap().as_mut() {
            if *i == id {
                feed.seek(to);
            }
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// The device's own word on how far it has played this connection.
    pub fn heard(&self, seconds: f64, playing: bool) {
        let mut clock = self.clock.lock().unwrap();
        clock.heard = Some(Heard {
            seconds: seconds - clock.base,
            at: Instant::now(),
            playing,
        });
    }

    /// How far what the device plays trails what the engine has made. The
    /// engine takes it off the position it shows.
    pub fn lead(&self) -> Duration {
        let clock = self.clock.lock().unwrap();
        Duration::from_secs_f64((clock.sent - clock.played()).clamp(0.0, 30.0))
    }

    /// Where the device's own media clock will stand when it plays what is
    /// being streamed now.
    pub fn stream_seconds(&self) -> f64 {
        let clock = self.clock.lock().unwrap();
        clock.base + clock.sent
    }

    /// Ends the stream and the server. The feed stays, so the engine does
    /// not take the end of the cast for the end of the song: the song waits
    /// to move back to this computer.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Whether the device has fetched the stream yet. When it never does,
    /// Windows Firewall is turning it away.
    pub fn reached(&self) -> bool {
        self.connection.load(Ordering::SeqCst) > 0
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// The server the device fetches the stream from. It ends when the pipe
/// closes.
pub struct Server {
    pub url: String,
}

impl Server {
    pub fn start(ip: IpAddr, pipe: Arc<Pipe>) -> Result<Server, String> {
        let listener = TcpListener::bind((ip, 0))
            .map_err(|e| format!("cannot open the stream for the device ({e})"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        // Only the device, told the link, can find the stream.
        let path = format!("/tonearm/{:016x}.wav", random());
        let url = format!("http://{ip}:{port}{path}");
        std::thread::Builder::new()
            .name("cast-serve".into())
            .spawn(move || serve(listener, path, pipe))
            .map_err(|e| e.to_string())?;
        Ok(Server { url })
    }
}

fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

fn serve(listener: TcpListener, path: String, pipe: Arc<Pipe>) {
    while !pipe.is_closed() {
        match listener.accept() {
            Ok((stream, _)) => {
                let (pipe, path) = (pipe.clone(), path.clone());
                let _ = std::thread::Builder::new()
                    .name("cast-stream".into())
                    .spawn(move || {
                        if let Err(e) = answer(stream, &path, &pipe) {
                            eprintln!("cast stream: {e}");
                        }
                    });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("cast stream: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn answer(mut stream: TcpStream, path: &str, pipe: &Pipe) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.read_line(&mut request)?;
    // The headers say nothing we need. A range asked for is answered with
    // the whole stream from now, as a live stream has no earlier bytes.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
    }
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if target != path || !(method == "GET" || method == "HEAD") {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    }
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nCache-Control: no-cache, no-store\r\n\
          Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
    )?;
    if method == "HEAD" {
        return Ok(());
    }
    // The engine sets the rate as it routes here, a moment after the
    // device is told the link.
    let started = Instant::now();
    while pipe.rate() == 0 {
        if pipe.is_closed() || started.elapsed() > Duration::from_secs(10) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let rate = pipe.rate();
    let id = pipe.connection.fetch_add(1, Ordering::SeqCst) + 1;
    {
        let mut clock = pipe.clock.lock().unwrap();
        let base = clock.base
            + clock.heard.as_ref().map_or(0.0, |h| {
                h.seconds + if h.playing { h.at.elapsed().as_secs_f64() } else { 0.0 }
            });
        *clock = Clock {
            base,
            ..Clock::default()
        };
    }
    stream.write_all(&wav_header(rate))?;

    let mut samples = vec![0f32; FRAMES * 2];
    let mut bytes = Vec::with_capacity(FRAMES * 4);
    let mut last = Instant::now();
    // The gain moves to a new level across one chunk, so it doesn't click.
    let mut gain = pipe.gain();
    loop {
        if pipe.is_closed() || pipe.connection.load(Ordering::SeqCst) != id {
            return Ok(());
        }
        let ahead = {
            let mut clock = pipe.clock.lock().unwrap();
            let now = Instant::now();
            if !pipe.paused() {
                clock.open += (now - last).as_secs_f64();
            }
            last = now;
            clock.sent - clock.played()
        };
        if ahead >= LEAD {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        // Silence while paused or between songs keeps the device from
        // giving up on the stream.
        samples.fill(0.0);
        if !pipe.paused() {
            let mut feed = pipe.feed.lock().unwrap();
            if let Some((_, source)) = feed.as_mut() {
                if source.fill(&mut samples) < samples.len() {
                    *feed = None;
                }
            }
        }
        let wanted = pipe.gain();
        let step = (wanted - gain) / FRAMES as f32;
        bytes.clear();
        for frame in samples.chunks_exact(2) {
            gain += step;
            for s in frame {
                let v = ((s * gain).clamp(-1.0, 1.0) * 32767.0) as i16;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        gain = wanted;
        stream.write_all(&bytes)?;
        pipe.clock.lock().unwrap().sent += FRAMES as f64 / rate as f64;
    }
}

/// A WAV header for 16 bit stereo with no end: both lengths at their most,
/// which players read as a stream.
fn wav_header(rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&2u16.to_le_bytes()); // channels
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes a second
    h.extend_from_slice(&4u16.to_le_bytes()); // bytes a frame
    h.extend_from_slice(&16u16.to_le_bytes()); // bits a sample
    h.extend_from_slice(b"data");
    h.extend_from_slice(&u32::MAX.to_le_bytes());
    h
}

/// A test feed: a tone that ends after `left` samples.
#[cfg(test)]
struct Tone {
    left: usize,
}

#[cfg(test)]
impl Feed for Tone {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        let n = out.len().min(self.left);
        out[..n].fill(0.5);
        self.left -= n;
        n
    }
    fn seek(&mut self, _to: Duration) {}
}

/// Fetches the stream as a device would: the header, then audio paced to
/// real time once the lead is out.
#[test]
fn serves_a_paced_wav() {
    use std::io::Read;
    let pipe = Pipe::new();
    let server = Server::start("127.0.0.1".parse().unwrap(), pipe.clone()).unwrap();
    pipe.set_rate(48_000);
    let id = pipe.attach(Box::new(Tone { left: 48_000 * 2 * 10 }));
    let address = server.url.trim_start_matches("http://");
    let (host, path) = address.split_at(address.find('/').unwrap());
    let mut stream = TcpStream::connect(host).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nRange: bytes=0-\r\n\r\n").unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line.trim().is_empty() {
            break;
        }
    }
    let mut header = [0u8; 44];
    reader.read_exact(&mut header).unwrap();
    assert_eq!(&header[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(header[24..28].try_into().unwrap()), 48_000);
    // The lead comes at once. More than that takes real time.
    let start = Instant::now();
    let mut audio = vec![0u8; (LEAD * 48_000.0) as usize * 4];
    reader.read_exact(&mut audio).unwrap();
    assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
    assert_eq!(i16::from_le_bytes([audio[0], audio[1]]), 16383);
    let mut more = vec![0u8; 48_000 * 4 / 2];
    reader.read_exact(&mut more).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(400), "{:?}", start.elapsed());
    assert!(pipe.has(id));
    // A gain set now is in the stream by the end of what is sent next.
    pipe.set_gain(0.25);
    let mut quieter = vec![0u8; 48_000 * 4 / 2];
    reader.read_exact(&mut quieter).unwrap();
    let n = quieter.len();
    assert_eq!(i16::from_le_bytes([quieter[n - 4], quieter[n - 3]]), 4095);
    // The device's report moves the pacing: it has played 1 s and stopped.
    pipe.heard(1.0, false);
    assert!(pipe.lead() > Duration::from_millis(1000), "{:?}", pipe.lead());
    pipe.close();
}
