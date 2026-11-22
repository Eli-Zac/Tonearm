//! Settings, as the desktop's settings pages have them: the YouTube Music
//! account, Last.fm and Spotify, crossfade and levelling, the Now Playing
//! cover and lyrics translation, and the song cache and history. Read and
//! saved through the core's `settings`, `services` and `offline`, the same
//! way the desktop does.

use crate::{Tonearm, TonearmError};
use std::sync::atomic::Ordering;
use tonearm_core::settings as core;
use tonearm_core::{history, innertube, offline, services, spotify_token, suggest};

/// A dropdown: what it offers, and which is picked.
#[derive(uniffi::Record)]
pub struct Choices {
    pub names: Vec<String>,
    pub selected: u32,
}

fn choices(names: &[&str], selected: usize) -> Choices {
    Choices {
        names: names.iter().map(|n| n.to_string()).collect(),
        selected: selected.min(names.len().saturating_sub(1)) as u32,
    }
}

/// The signed-in YouTube Music account.
#[derive(uniffi::Record, Clone)]
pub struct Account {
    pub name: String,
    /// Its @handle. Empty when it has none.
    pub handle: String,
    /// A link to its picture. Empty when it has none.
    pub photo: String,
}

/// Where Last.fm and Spotify stand, for Connections.
#[derive(uniffi::Record)]
pub struct Connections {
    /// Empty while Last.fm is not linked.
    pub lastfm_user: String,
    /// This build has Last.fm keys of its own, so none need entering.
    pub lastfm_builtin: bool,
    pub lastfm_key: String,
    pub lastfm_secret: String,
    /// Waiting for the user to approve Tonearm on Last.fm.
    pub lastfm_waiting: bool,
    pub spotify_id: String,
    pub spotify_secret: String,
    /// Spotify's client ID and secret are set: track details come from it.
    pub spotify_linked: bool,
    /// The sp_dc cookie of a Spotify web sign in is set.
    pub canvas_linked: bool,
    /// Discord shows what is playing.
    pub discord_on: bool,
    /// This build has a Discord application of its own.
    pub discord_builtin: bool,
    /// What Discord shows after "Listening to": 0 the artist, 1 the song, 2 Tonearm.
    pub discord_status: u32,
}

/// The song cache sizes Settings offers, as the desktop's dropdown.
const CACHE_NAMES: [&str; 5] = ["Off", "512 MB", "1 GB", "2 GB", "5 GB"];

#[uniffi::export]
impl Tonearm {
    // ------------------------------------------------------------- account

    /// The account signed in, as YouTube Music names it. Asks once, then
    /// remembers until the sign in changes. None signed out. Blocks.
    pub fn account(&self) -> Option<Account> {
        if !self.client.signed_in() {
            return None;
        }
        if let Some(known) = self.account.lock().unwrap().clone() {
            return Some(known);
        }
        let found = self.client.account().ok().flatten()?;
        let account = Account {
            name: found.name,
            handle: found.handle,
            photo: if found.photo.starts_with("//") { format!("https:{}", found.photo) } else { found.photo },
        };
        *self.account.lock().unwrap() = Some(account.clone());
        Some(account)
    }

    /// Whether a cookie (a whole `Cookie:` header, or a web view's cookie
    /// string for music.youtube.com) is a signed-in one: it has SAPISID.
    pub fn is_sign_in_cookie(&self, cookie: String) -> bool {
        innertube::normalize_cookie(&cookie).is_some()
    }

    // ------------------------------------------------------------ playback

    pub fn crossfade_choices(&self) -> Choices {
        choices(&core::CROSSFADE_NAMES, core::crossfade_choice())
    }

    pub fn set_crossfade(&self, choice: u32) {
        let seconds = core::set_crossfade_choice(choice as i32);
        self.crossfade.store(seconds, Ordering::SeqCst);
        self.forget_fade();
    }

    /// Whether a song changed by hand (skip, previous, another picked to
    /// play) crossfades with the length above, rather than cutting.
    pub fn crossfade_manual(&self) -> bool {
        self.crossfade_manual.load(Ordering::SeqCst)
    }

    pub fn set_crossfade_manual(&self, on: bool) {
        core::set_crossfade_manual(on);
        self.crossfade_manual.store(on, Ordering::SeqCst);
    }

    pub fn levelling_choices(&self) -> Choices {
        choices(&core::LEVELLING_NAMES, self.levelling.load(Ordering::SeqCst) as usize)
    }

    pub fn set_levelling(&self, choice: u32) {
        let choice = core::set_levelling(choice as i32);
        self.levelling.store(choice, Ordering::SeqCst);
        // The song playing takes the new level at once.
        self.gained.store(0, Ordering::SeqCst);
    }

    // ------------------------------------------------------- customization

    /// What Now Playing shows for the cover: the desktop's choice for
    /// expanded Now playing, which a phone's Now Playing is.
    pub fn cover_choices(&self) -> Choices {
        choices(&core::WIDE_NAMES, core::cover_choices().1 as usize)
    }

    pub fn set_cover(&self, choice: u32) {
        let (panel, _) = core::cover_choices();
        core::save_cover_choices((panel, (choice as i32).clamp(0, 1)));
    }

    /// Whether Now Playing shows the album's moving cover when it has one.
    pub fn animated_covers(&self) -> bool {
        core::cover_choices().1 == core::WIDE_ANIMATED
    }

    pub fn translate_choices(&self) -> Choices {
        choices(&core::TRANSLATE_NAMES, core::translate())
    }

    pub fn set_translate(&self, choice: u32) {
        core::set_translate(choice as i32);
    }

    // ------------------------------------------------- storage and history

    pub fn cache_choices(&self) -> Choices {
        choices(&CACHE_NAMES, offline::cache_choice())
    }

    pub fn set_cache(&self, choice: u32) {
        offline::set_cache_limit_mb(offline::choice_mb(choice as i32));
    }

    /// The storage line, as the desktop's: "Cache 120 MB of 1 GB ·
    /// Downloads 0 MB".
    pub fn storage_note(&self) -> String {
        offline::storage_note()
    }

    pub fn clear_cache(&self) {
        offline::clear_cache();
    }

    /// Forgets what was played and opened, and the recent searches, as the
    /// desktop's Clear does.
    pub fn clear_history(&self) {
        self.taste.lock().unwrap().clear();
        history::History::delete_file();
        *self.recent.lock().unwrap() = Some(Vec::new());
        suggest::save_recent("");
    }

    // --------------------------------------------------------- connections

    pub fn connections(&self) -> Connections {
        let s = self.service_settings.lock().unwrap();
        Connections {
            lastfm_user: s.lastfm_user.clone(),
            lastfm_builtin: !services::LASTFM_KEY.is_empty() && !services::LASTFM_SECRET.is_empty(),
            lastfm_key: s.lastfm_key.clone(),
            lastfm_secret: s.lastfm_secret.clone(),
            lastfm_waiting: !self.lastfm_token.lock().unwrap().is_empty(),
            spotify_id: s.spotify_id.clone(),
            spotify_secret: s.spotify_secret.clone(),
            spotify_linked: !s.spotify_id.is_empty() && !s.spotify_secret.is_empty(),
            canvas_linked: !s.spotify_cookie.is_empty(),
            discord_on: !s.discord_id().is_empty(),
            discord_builtin: !services::DISCORD_APP.is_empty(),
            discord_status: match s.discord_status.as_str() {
                "song" => 1,
                "app" => 2,
                _ => 0,
            },
        }
    }

    /// Turns Discord on or off. `app` is the user's own application ID, for
    /// a build without one of its own, and is ignored when turning off.
    pub fn discord_toggle(&self, app: String) -> Result<(), TonearmError> {
        {
            let mut s = self.service_settings.lock().unwrap();
            if s.discord_id().is_empty() {
                let app = app.trim().to_string();
                if services::DISCORD_APP.is_empty() && app.parse::<u64>().is_err() {
                    return Err("Enter your Discord application ID first.".to_string().into());
                }
                s.discord_on = "1".into();
                s.discord_app = if services::DISCORD_APP.is_empty() { app } else { String::new() };
            } else {
                s.discord_on.clear();
                s.discord_app.clear();
            }
        }
        self.apply_services();
        Ok(())
    }

    /// Clears what Discord shows, for when playback is being shut down: the
    /// status would otherwise stay on the last song. Waits a moment for the
    /// clear to reach the Discord app before the process can end.
    pub fn end_presence(&self) {
        *self.presence.lock().unwrap() = (None, std::time::Instant::now());
        self.services.send(services::Event::Stopped);
        std::thread::sleep(std::time::Duration::from_millis(400));
    }

    /// Picks what Discord shows after "Listening to": 0 the artist, 1 the
    /// song, 2 Tonearm.
    pub fn set_discord_status(&self, choice: u32) {
        self.service_settings.lock().unwrap().discord_status = match choice {
            1 => "song",
            2 => "app",
            _ => "artist",
        }
        .to_string();
        self.apply_services();
    }

    /// Starts linking Last.fm: gives the page to open, where the user
    /// approves Tonearm. `lastfm_finish` then asks whether they have.
    /// `key` and `secret` are the user's own, for a build without its own.
    pub fn lastfm_begin(&self, key: String, secret: String) -> Result<String, TonearmError> {
        let (key, secret) = {
            let mut s = self.service_settings.lock().unwrap();
            let (key, secret) = (key.trim().to_string(), secret.trim().to_string());
            if key != s.lastfm_key || secret != s.lastfm_secret {
                // New keys cannot reuse the old sign in.
                s.lastfm_session.clear();
                s.lastfm_user.clear();
            }
            s.lastfm_key = key;
            s.lastfm_secret = secret;
            let (key, secret) = s.lastfm_keys();
            (key.to_string(), secret.to_string())
        };
        if key.is_empty() || secret.is_empty() {
            return Err("Enter your Last.fm API key and shared secret first.".to_string().into());
        }
        let (token, url) = services::lastfm_begin(&key, &secret)?;
        *self.lastfm_token.lock().unwrap() = token;
        Ok(url)
    }

    /// Whether the user has approved Tonearm on Last.fm yet: the name it
    /// linked as, or None while it waits. Blocks.
    pub fn lastfm_finish(&self) -> Option<String> {
        let token = self.lastfm_token.lock().unwrap().clone();
        if token.is_empty() {
            return None;
        }
        let (key, secret) = {
            let s = self.service_settings.lock().unwrap();
            let (key, secret) = s.lastfm_keys();
            (key.to_string(), secret.to_string())
        };
        let (session, user) = services::lastfm_finish(&key, &secret, &token).ok()?;
        // A newer attempt took over, or it was given up on.
        if *self.lastfm_token.lock().unwrap() != token {
            return None;
        }
        self.lastfm_token.lock().unwrap().clear();
        {
            let mut s = self.service_settings.lock().unwrap();
            s.lastfm_session = session;
            s.lastfm_user = user.clone();
        }
        self.apply_services();
        Some(user)
    }

    /// Stops waiting for Last.fm, as when the user leaves without approving.
    pub fn lastfm_cancel(&self) {
        self.lastfm_token.lock().unwrap().clear();
    }

    pub fn lastfm_unlink(&self) {
        {
            let mut s = self.service_settings.lock().unwrap();
            s.lastfm_session.clear();
            s.lastfm_user.clear();
        }
        self.lastfm_token.lock().unwrap().clear();
        self.apply_services();
    }

    /// Saves Spotify's client ID and secret, as the desktop's Connect does.
    pub fn save_spotify(&self, id: String, secret: String) {
        {
            let mut s = self.service_settings.lock().unwrap();
            s.spotify_id = id.trim().to_string();
            s.spotify_secret = secret.trim().to_string();
        }
        self.apply_services();
        spotify_token::forget_token();
    }

    /// Takes the sp_dc cookie of a Spotify web sign in from a web view's
    /// cookies for open.spotify.com. False when they have none.
    pub fn spotify_sign_in(&self, cookies: String) -> bool {
        let found = cookies
            .split(';')
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, _)| *name == "sp_dc")
            .map(|(_, value)| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let Some(cookie) = found else { return false };
        self.service_settings.lock().unwrap().spotify_cookie = cookie;
        self.apply_services();
        spotify_token::forget_token();
        true
    }

    /// Forgets the Spotify client ID and secret.
    pub fn spotify_unlink(&self) {
        {
            let mut s = self.service_settings.lock().unwrap();
            s.spotify_id.clear();
            s.spotify_secret.clear();
        }
        self.apply_services();
    }

    /// Forgets the Spotify web sign in.
    pub fn canvas_unlink(&self) {
        self.service_settings.lock().unwrap().spotify_cookie.clear();
        spotify_token::forget_token();
        self.apply_services();
    }
}

impl Tonearm {
    /// Saves the services' settings and hands them to the services thread.
    fn apply_services(&self) {
        let settings = self.service_settings.lock().unwrap().clone();
        if let Err(message) = settings.save() {
            eprintln!("tonearm: could not save the connections: {message}");
        }
        self.services.send(services::Event::Configure(settings));
    }
}
