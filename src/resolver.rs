//! Turns a video id into an audio stream URL.
//!
//! This is the one piece the app does not ship yet. YouTube has no public
//! API that hands out audio streams, so every way to get one imitates
//! another client or runs YouTube's player script. That choice belongs to
//! you, so it sits behind this trait. See the README.

pub struct Stream {
    pub url: String,
    /// Seconds. Zero when unknown.
    pub duration: f32,
}

pub trait Resolver: Send + Sync {
    fn resolve(&self, video_id: &str) -> Result<Stream, String>;
}

/// Plays one fixed URL for every track. Set `TONEARM_TEST_STREAM` to an
/// M4A or MP4 audio URL to exercise the audio path end to end.
struct Fixed(String);

impl Resolver for Fixed {
    fn resolve(&self, _video_id: &str) -> Result<Stream, String> {
        Ok(Stream {
            url: self.0.clone(),
            duration: 0.0,
        })
    }
}

pub fn default() -> Option<Box<dyn Resolver>> {
    let url = std::env::var("TONEARM_TEST_STREAM").ok()?;
    Some(Box::new(Fixed(url)))
}
