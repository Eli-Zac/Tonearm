//! Audio engine. One thread owns the output device and the decoder.
//!
//! Tracks stream over HTTP into memory while they play. A four minute AAC
//! track is about 4 MB, and we free it as soon as the next track loads.

use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const CHUNK: u64 = 512 * 1024;

pub const IDLE: u32 = 0;
pub const LOADING: u32 = 1;
pub const PLAYING: u32 = 2;
pub const PAUSED: u32 = 3;
pub const ENDED: u32 = 4;
pub const FAILED: u32 = 5;

enum Command {
    Load {
        url: String,
        headers: Vec<(String, String)>,
        generation: u64,
    },
    Play,
    Pause,
    Seek(f32),
    Volume(f32),
    Stop,
}

#[derive(Default)]
pub struct Status {
    pub state: AtomicU32,
    pub position_ms: AtomicU64,
    /// Which `load` call the state belongs to.
    pub generation: AtomicU64,
    pub error: Mutex<String>,
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

    pub fn load(&self, url: String, headers: Vec<(String, String)>, generation: u64) {
        self.status.generation.store(generation, Ordering::SeqCst);
        self.status.state.store(LOADING, Ordering::SeqCst);
        self.status.position_ms.store(0, Ordering::SeqCst);
        let _ = self.tx.send(Command::Load {
            url,
            headers,
            generation,
        });
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
    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }
}

fn run(rx: mpsc::Receiver<Command>, status: Arc<Status>) {
    // The device opens on first play, so an idle app holds no audio handle.
    // The stream, its handle, and the device's sample rate.
    let mut output: Option<(OutputStream, OutputStreamHandle, u32)> = None;
    let mut sink: Option<Sink> = None;
    let mut volume = 1.0f32;
    let fail = |status: &Status, message: String| {
        *status.error.lock().unwrap() = message;
        status.state.store(FAILED, Ordering::SeqCst);
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Command::Load {
                url,
                headers,
                generation,
            }) => {
                sink = None;
                if generation != status.generation.load(Ordering::SeqCst) {
                    continue; // A newer track was requested while we waited.
                }
                if output.is_none() {
                    match open_output() {
                        Ok(opened) => output = Some(opened),
                        Err(e) => {
                            fail(&status, format!("No audio output: {e}"));
                            continue;
                        }
                    }
                }
                let (_, handle, rate) = output.as_ref().unwrap();
                let source = HttpSource::open(url, headers);
                let shared = source.shared.clone();
                let opened = crate::decode::Track::open(Box::new(source), *rate)
                    .map_err(|e| {
                        // A failed download looks like a broken file to the
                        // decoder. Report the download error instead.
                        shared.buffer.lock().unwrap().error.clone().unwrap_or(e)
                    })
                    .and_then(|source| {
                        let sink =
                            Sink::try_new(handle).map_err(|e| format!("No audio output: {e}"))?;
                        sink.set_volume(volume);
                        sink.append(source);
                        Ok(sink)
                    });
                match opened {
                    Ok(new) => {
                        sink = Some(new);
                        status.state.store(PLAYING, Ordering::SeqCst);
                    }
                    Err(message) => fail(&status, message),
                }
            }
            Ok(Command::Play) => {
                if let Some(s) = &sink {
                    s.play();
                    status.state.store(PLAYING, Ordering::SeqCst);
                }
            }
            Ok(Command::Pause) => {
                if let Some(s) = &sink {
                    s.pause();
                    status.state.store(PAUSED, Ordering::SeqCst);
                }
            }
            Ok(Command::Seek(seconds)) => {
                if let Some(s) = &sink {
                    let _ = s.try_seek(Duration::from_secs_f32(seconds.max(0.0)));
                }
            }
            Ok(Command::Volume(v)) => {
                volume = v.clamp(0.0, 1.0);
                if let Some(s) = &sink {
                    s.set_volume(volume);
                }
            }
            Ok(Command::Stop) => {
                sink = None;
                status.state.store(IDLE, Ordering::SeqCst);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if let Some(s) = &sink {
            status
                .position_ms
                .store(s.get_pos().as_millis() as u64, Ordering::SeqCst);
            if s.empty() && status.state.load(Ordering::SeqCst) == PLAYING {
                status.state.store(ENDED, Ordering::SeqCst);
            }
        }
    }
}

/// Opens the default output device at its own rate and remembers that
/// rate, so the decoder can resample to it and rodio never has to.
fn open_output() -> Result<(OutputStream, OutputStreamHandle, u32), String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let device = rodio::cpal::default_host()
        .default_output_device()
        .ok_or("no output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let rate = config.sample_rate().0;
    let (stream, handle) =
        OutputStream::try_from_device_config(&device, config).map_err(|e| e.to_string())?;
    Ok((stream, handle, rate))
}

// ------------------------------------------------------------ HTTP source

#[derive(Default)]
struct Buffer {
    data: Vec<u8>,
    total: Option<u64>,
    done: bool,
    error: Option<String>,
}

struct Shared {
    buffer: Mutex<Buffer>,
    ready: Condvar,
    cancelled: AtomicBool,
}

/// A `Read + Seek` view over a download that is still running. Reads block
/// until the bytes they need arrive.
pub struct HttpSource {
    shared: Arc<Shared>,
    position: u64,
}

impl HttpSource {
    pub fn open(url: String, headers: Vec<(String, String)>) -> Self {
        let shared = Arc::new(Shared {
            buffer: Mutex::new(Buffer::default()),
            ready: Condvar::new(),
            cancelled: AtomicBool::new(false),
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("download".into())
            .spawn(move || {
                let result = download(&url, &headers, &worker);
                let mut buffer = worker.buffer.lock().unwrap();
                buffer.done = true;
                buffer.error = result.err();
                worker.ready.notify_all();
            })
            .expect("cannot start the download thread");
        HttpSource {
            shared,
            position: 0,
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
            Err(e) => return Err(format!("Stream failed: {e}")),
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
                .map_err(|e| format!("Stream failed: {e}"))?;
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
            shared.ready.notify_all();
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
        true
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

impl Drop for HttpSource {
    fn drop(&mut self) {
        self.shared.cancelled.store(true, Ordering::SeqCst);
    }
}
