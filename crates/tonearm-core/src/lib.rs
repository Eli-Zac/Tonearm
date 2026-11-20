//! Everything in Tonearm that is not the window: YouTube Music, playback
//! and mixing, downloads, the library, Spotify sync, Last.fm and casting.
//! The desktop app draws it; another app (Android, say) could too.
//!
//! Background work reports through listeners the app sets at start
//! (`offline::listen`, `analysis::listen`), never by reaching for a window.

pub mod analysis;
pub mod animated_art;
pub mod audio;
pub mod backdrop;
pub mod cast;
pub mod decode;
pub mod explore;
pub mod history;
pub mod home;
pub mod hook;
pub mod innertube;
pub mod library;
pub mod listen;
pub mod lyrics;
pub mod menu;
pub mod mix;
pub mod offline;
pub mod pool;
pub mod queue;
pub mod quick_access;
pub mod releases;
pub mod resolver;
pub mod services;
pub mod session;
pub mod settings;
pub mod spotify;
pub mod spotify_token;
pub mod store;
pub mod suggest;
pub mod sync;
pub mod syncing;
pub mod transition;
