//! Casting to Google Cast devices on the network: Chromecasts, Google TVs,
//! Nest speakers and speaker groups.
//!
//! The device plays Tonearm's own output as a live stream from this
//! computer, so mixes, levelling and offline songs all carry over. The
//! price is a lag of about two seconds on skips and seeks.
//!
//! - `find`: the devices on the network.
//! - `channel`: the Cast protocol's wire.
//! - `stream`: the audio, served to the device.
//! - `session`: one cast, from launch to stop.
//!
//! Nothing here touches the window, so it can move to a shared core with
//! the rest of the engine. Only `find` is Windows specific.

mod channel;
mod find;
mod session;
mod stream;

pub use find::Finder;
#[cfg(not(windows))]
pub use find::{lost, seen};
pub use session::{status, Event, LyricLine, NowPlaying, Session};
pub use stream::{Feed, Pipe};
#[cfg(test)]
pub use stream::Server;

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// The device's own id, which stays the same across restarts.
    pub id: String,
    /// The name it was given, such as "Living Room TV".
    pub name: String,
    /// Its model, such as "Chromecast" or "Google Nest Mini".
    pub model: String,
    pub addr: std::net::SocketAddr,
    /// A speaker group, played by one of its members.
    pub group: bool,
    /// Has a screen: a TV, or a Chromecast plugged into one.
    pub screen: bool,
}
