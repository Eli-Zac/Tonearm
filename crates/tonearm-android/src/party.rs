//! The Party on the phone (docs/party.md): starting and joining, the live
//! connection, and, as host, carrying out what guests ask for on this
//! phone's own queue. It runs on its own thread, so a Party keeps going
//! while the app is in the background and the music plays.
//!
//! Also who added which song, for the queue's little avatars: only songs a
//! Party member added have one, never ones the radio or autoplay put in.
//!
//! As a guest, the phone is a remote for the host: `playback`, `queue` and
//! `now_playing` give the host's song, place and queue, and the player's
//! buttons go to the host (see `party_remote`). The phone's own queue is
//! left as it was, paused, and is back as soon as the guest leaves.

use crate::{song, Song, Tonearm, TonearmError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonearm_core::party::{self, Clock, Event, Live, Member, Pacer, Ticket};

/// Someone in the Party, for the queue's header and the car.
#[derive(uniffi::Record, Clone)]
pub struct PartyPerson {
    pub id: String,
    pub name: String,
    /// "Host", "Added 3 songs", "Joined" or "Away".
    pub line: String,
    pub host: bool,
}

/// The Party this phone is in, as the queue shows it.
#[derive(uniffi::Record, Clone)]
pub struct PartyView {
    pub code: String,
    pub name: String,
    pub link: String,
    pub host: bool,
    pub members: Vec<PartyPerson>,
    /// Guests may play, pause and skip, not only add songs.
    pub others_control: bool,
    /// This phone may play, skip and reorder: the host, or a guest the host
    /// lets change what's playing.
    pub can_control: bool,
    /// A guest's line: what the host is playing. Empty for the host.
    pub playing: String,
    /// "Reconnecting…" while the connection is being made again.
    pub note: String,
}

struct Here {
    ticket: Ticket,
    live: Arc<Live>,
    members: Vec<Member>,
    others_control: bool,
    playing: String,
    note: String,
    /// A guest's view of the host: the last state, the clock run from it,
    /// and its songs (the one playing first) with who added each.
    state: Value,
    clock: Option<Clock>,
    songs: Vec<(Song, Option<String>)>,
}

/// What a guest shows in place of its own player.
pub(crate) struct Remote {
    pub songs: Vec<Song>,
    pub clock: Clock,
    /// "In Eli's Party".
    pub line: String,
}

static PARTY: Mutex<Option<Here>> = Mutex::new(None);
/// The host's state should go again whatever it is: just connected again.
static RESEND: AtomicBool = AtomicBool::new(false);
/// Bumped on every change the queue's header shows.
static VERSION: AtomicU64 = AtomicU64::new(0);
/// What to tell the user once: "Sam added Guide Light", "The host ended the Party".
static NOTES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Who added each queued song, by video id: the member's name.
static ADDERS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// The Party thread for the current Party should stop.
static STOP: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

fn note(text: impl Into<String>) {
    NOTES.lock().unwrap().push(text.into());
}

fn bump() {
    VERSION.fetch_add(1, Ordering::SeqCst);
}

fn person(m: &Member) -> PartyPerson {
    let line = if m.host {
        "Host".to_string()
    } else if !m.online {
        "Away".to_string()
    } else {
        match m.added {
            0 => "Joined".into(),
            1 => "Added 1 song".into(),
            n => format!("Added {n} songs"),
        }
    };
    PartyPerson { id: m.id.clone(), name: m.name.clone(), line, host: m.host }
}

fn adder_set(video_id: &str, name: &str) {
    ADDERS.lock().unwrap().get_or_insert_with(HashMap::new).insert(video_id.to_string(), name.to_string());
}

fn party_song(song: &Song) -> Value {
    json!({
        "video_id": song.video_id,
        "title": song.title,
        "subtitle": song.artist,
        "artists": song.artists.iter().map(|a| &a.name).collect::<Vec<_>>(),
        "art": song.cover,
        "duration": song.duration,
    })
}

/// A queued song as the host sends it: with who added it, when a member did.
fn queued_song(song: &Song, adders: &HashMap<String, String>) -> Value {
    let mut value = party_song(song);
    if let Some(name) = adders.get(&song.video_id) {
        value["added_by"] = json!(name);
    }
    value
}

/// A guest's view of the host's state: its songs, and a clock for its place.
fn take_state(here: &mut Here, state: Value, stored: bool) {
    here.playing = playing_line(&state);
    here.clock = state["song"].is_object().then(|| Clock::of(&state, stored));
    here.songs = party::state_songs(&state)
        .into_iter()
        .filter_map(|(item, by)| Some((song(&item)?, by)))
        .collect();
    here.state = state;
}

/// The guest's songs and adders in one line, to tell when they changed.
fn songs_key(here: &Here) -> String {
    here.songs.iter().map(|(s, by)| format!("{}/{}", s.video_id, by.as_deref().unwrap_or(""))).collect::<Vec<_>>().join(",")
}

#[uniffi::export]
impl Tonearm {
    /// Starts a Party with this phone as host, shown to guests as `name`.
    /// Needs the Tonearm account. Blocks on the network.
    pub fn party_start(self: Arc<Self>, name: String) -> Result<(), TonearmError> {
        if PARTY.lock().unwrap().is_some() {
            return Ok(());
        }
        let ticket = party::start(&name)?;
        self.party_enter(ticket);
        Ok(())
    }

    /// Joins a Party by its code, or a link to it. Blocks on the network.
    pub fn party_join(self: Arc<Self>, code: String, name: String) -> Result<(), TonearmError> {
        if PARTY.lock().unwrap().is_some() {
            return Ok(());
        }
        let code = party::code_from(&code).ok_or_else(|| "That isn't a Party code or link".to_string())?;
        let ticket = party::join(&code, &name)?;
        self.party_enter(ticket);
        Ok(())
    }

    /// The Party code in a scan or a pasted link, or None if it isn't one.
    pub fn party_code_from(&self, text: String) -> Option<String> {
        party::code_from(&text)
    }

    /// The Party, as the queue's header shows it. None when not in one.
    pub fn party(&self) -> Option<PartyView> {
        let party = PARTY.lock().unwrap();
        let here = party.as_ref()?;
        Some(PartyView {
            code: here.ticket.code.clone(),
            name: here.ticket.name.clone(),
            link: party::link(&here.ticket.code),
            host: here.ticket.host,
            members: here.members.iter().map(person).collect(),
            others_control: here.others_control,
            can_control: here.ticket.host || here.others_control,
            playing: here.playing.clone(),
            note: here.note.clone(),
        })
    }

    /// Changes when anything `party()` shows changes.
    pub fn party_version(&self) -> u64 {
        VERSION.load(Ordering::SeqCst)
    }

    /// What to tell the user, once each.
    pub fn party_notes(&self) -> Vec<String> {
        std::mem::take(&mut *NOTES.lock().unwrap())
    }

    /// Who added a queued song, by name. None for songs nobody in a Party
    /// added (the radio, autoplay, this phone's own picks).
    pub fn party_adder(&self, video_id: String) -> Option<String> {
        // A guest knows them from the host's state.
        if let Some(here) = PARTY.lock().unwrap().as_ref().filter(|h| !h.ticket.host) {
            return here.songs.iter().find(|(s, _)| s.video_id == video_id).and_then(|(_, by)| by.clone());
        }
        ADDERS.lock().unwrap().as_ref()?.get(&video_id).cloned()
    }

    /// The host ends the Party; a guest leaves it, and has their own queue
    /// and song back, paused where they were.
    pub fn party_end(&self) {
        let Some(here) = PARTY.lock().unwrap().take() else { return };
        self.changed();
        if let Some(stop) = STOP.lock().unwrap().take() {
            stop.store(true, Ordering::SeqCst);
        }
        if here.ticket.host {
            let ticket = here.ticket.clone();
            std::thread::spawn(move || {
                let _ = party::end(&ticket);
            });
        } else {
            here.live.send(json!({ "type": "leave" }));
            let live = here.live.clone();
            // A moment for the message to go before the connection closes.
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800));
                drop(live);
            });
        }
        ADDERS.lock().unwrap().take();
        bump();
    }

    pub fn party_set_others(&self, on: bool) {
        if let Some(here) = PARTY.lock().unwrap().as_mut() {
            here.live.send(json!({ "type": "settings", "others_control": on }));
            here.others_control = on;
        }
        bump();
    }

    pub fn party_remove(&self, member_id: String) {
        if let Some(here) = PARTY.lock().unwrap().as_ref() {
            here.live.send(json!({ "type": "remove", "member": member_id }));
        }
    }
}

impl Tonearm {
    fn party_enter(self: Arc<Self>, ticket: Ticket) {
        let live = Arc::new(Live::connect(&ticket));
        let stop = Arc::new(AtomicBool::new(false));
        *STOP.lock().unwrap() = Some(stop.clone());
        *ADDERS.lock().unwrap() = Some(HashMap::new());
        *PARTY.lock().unwrap() = Some(Here {
            ticket: ticket.clone(),
            live: live.clone(),
            members: Vec::new(),
            others_control: true,
            playing: String::new(),
            note: String::new(),
            state: Value::Null,
            clock: None,
            songs: Vec::new(),
        });
        if !ticket.host {
            // The host's music is the one playing now. Theirs waits, paused.
            self.pause_here();
            note(format!("You joined {}", ticket.name));
        }
        bump();
        let me = self.clone();
        std::thread::Builder::new()
            .name("party-phone".into())
            .spawn(move || me.party_run(ticket, live, stop))
            .ok();
    }

    /// The Party's thread: the room's events, and the host's state.
    fn party_run(self: Arc<Self>, ticket: Ticket, live: Arc<Live>, stop: Arc<AtomicBool>) {
        let mut pacer = Pacer::default();
        while !stop.load(Ordering::SeqCst) {
            if let Some(event) = live.next_event(Duration::from_millis(250)) {
                if !self.party_event(&ticket, &live, event) {
                    return;
                }
            }
            if ticket.host {
                if RESEND.swap(false, Ordering::SeqCst) {
                    pacer.reset();
                }
                let state = self.party_state();
                if pacer.due(&state) {
                    live.send_state(state);
                }
            }
        }
    }

    /// Handles one event. False when the Party is over for this phone.
    fn party_event(&self, ticket: &Ticket, live: &Live, event: Event) -> bool {
        let mut over: Option<String> = None;
        // The guest's songs changed, so the player and queue read them again.
        let moved;
        {
            let mut party = PARTY.lock().unwrap();
            let Some(here) = party.as_mut() else { return false };
            let before = songs_key(here);
            match event {
                Event::Welcome { name, others_control, members, state } => {
                    here.ticket.name = name;
                    here.others_control = others_control;
                    here.members = members;
                    here.note.clear();
                    if ticket.host {
                        RESEND.store(true, Ordering::SeqCst);
                    } else {
                        take_state(here, state, true);
                    }
                }
                Event::Members(members) => here.members = members,
                Event::State(state) => take_state(here, state, false),
                Event::Settings { others_control } => here.others_control = others_control,
                Event::Refused(message) => {
                    // What a guest expected to happen didn't: back to the
                    // host's last word.
                    if !ticket.host && !here.state.is_null() {
                        let state = here.state.clone();
                        take_state(here, state, true);
                    }
                    let mut letters = message.chars();
                    note(letters.next().map(|c| c.to_uppercase().chain(letters).collect()).unwrap_or(message));
                }
                Event::Reconnecting => here.note = "Reconnecting…".into(),
                Event::Removed => over = Some("The host removed you from the Party".into()),
                Event::Ended(reason) => {
                    over = Some(match reason.as_str() {
                        "ended by the host" => "The host ended the Party".into(),
                        "" => "The Party ended".into(),
                        other => format!("The Party ended: {other}"),
                    })
                }
                Event::Command { from, from_name, command, args } => {
                    drop(party);
                    if ticket.host {
                        self.party_command(live, &from, &from_name, &command, &args);
                    }
                    bump();
                    return true;
                }
            }
            moved = songs_key(here) != before;
        }
        if moved {
            self.changed();
        }
        bump();
        if let Some(message) = over {
            PARTY.lock().unwrap().take();
            ADDERS.lock().unwrap().take();
            note(message);
            // A guest's own queue and song come back.
            self.changed();
            bump();
            return false;
        }
        true
    }

    /// What the host sends: the song, playing or not, where in it and when
    /// that was measured, and the next songs with who added each.
    fn party_state(&self) -> Value {
        let adders = ADDERS.lock().unwrap().clone().unwrap_or_default();
        let queue = self.queue.lock().unwrap();
        let song = queue.current().map(|s| queued_song(s, &adders));
        let next: Vec<Value> =
            queue.items().iter().skip(queue.index() + 1).take(30).map(|s| queued_song(s, &adders)).collect();
        drop(queue);
        let playback = self.playback();
        json!({
            "song": song,
            "playing": matches!(playback.state, crate::PlayState::Playing),
            "position_ms": playback.position_ms,
            "duration_ms": playback.duration_ms,
            "at": party::now_ms(),
            "next": next,
        })
    }

    /// Carries out a guest's request, as host.
    fn party_command(&self, live: &Live, from: &str, from_name: &str, command: &str, args: &Value) {
        let playing = matches!(self.playback().state, crate::PlayState::Playing);
        match command {
            "add" => {
                let Some(item) = party::song_from(&args["item"]) else { return };
                let Some(song) = song(&item) else { return };
                let title = song.title.clone();
                adder_set(&song.video_id, from_name);
                let next = args["next"].as_bool().unwrap_or(false);
                let result = if next { self.party_play_next(song) } else { self.party_queue(song) };
                if result.is_ok() {
                    live.send(json!({ "type": "added", "member": from }));
                    note(format!("{from_name} added {title}"));
                }
            }
            "play" if !playing => {
                self.play_hold.set(true);
                self.audio.play();
            }
            "pause" if playing => self.pause(),
            "next" => {
                let _ = self.next();
            }
            "previous" => {
                let _ = self.previous();
            }
            "seek" => {
                if let Some(ms) = args["ms"].as_f64() {
                    self.seek((ms / 1000.0) as f32);
                }
            }
            // A song tapped in a guest's queue: `index` is its place among
            // the next songs, when it came from there. From anywhere else
            // (or when the queue moved meanwhile) it plays next, then now.
            "play_item" => {
                let Some(item) = party::song_from(&args["item"]) else { return };
                let Some(song) = song(&item) else { return };
                let at = {
                    let queue = self.queue.lock().unwrap();
                    args["index"]
                        .as_u64()
                        .map(|i| queue.index() + 1 + i as usize)
                        .filter(|&at| queue.items().get(at).is_some_and(|s| s.video_id == song.video_id))
                };
                match at {
                    Some(at) => {
                        let _ = self.skip_to(at as u32);
                    }
                    None => {
                        let id = song.video_id.clone();
                        match self.queue.lock().unwrap().play_next(song, |s| s.video_id == id) {
                            // It is playing already.
                            tonearm_core::queue::Added::Playing => return,
                            // The queue was empty: it starts by being added.
                            added @ tonearm_core::queue::Added::Started => {
                                let _ = self.after_adding(added, String::new(), String::new());
                                return;
                            }
                            _ => {}
                        }
                        self.changed();
                        let _ = self.next();
                    }
                }
            }
            // A guest's drag: places among the next songs, checked against
            // the song they saw there.
            "move" => {
                let (Some(from), Some(to)) = (args["from"].as_u64(), args["to"].as_u64()) else { return };
                let id = args["video_id"].as_str().unwrap_or_default();
                let (from, to) = {
                    let queue = self.queue.lock().unwrap();
                    let start = queue.index() + 1;
                    (start + from as usize, start + to as usize)
                };
                let fits = {
                    let queue = self.queue.lock().unwrap();
                    to < queue.items().len() && queue.items().get(from).is_some_and(|s| s.video_id == id)
                };
                if fits {
                    self.move_in_queue(from as u32, to as u32);
                }
            }
            _ => {}
        }
    }

    fn party_play_next(&self, song: Song) -> Result<String, TonearmError> {
        let id = song.video_id.clone();
        let added = self.queue.lock().unwrap().play_next(song, |s| s.video_id == id);
        self.after_adding(added, String::new(), String::new())
    }

    /// A guest's Add to queue lands after the other songs guests added,
    /// before the rest of the queue, as Spotify's Jam does it.
    fn party_queue(&self, song: Song) -> Result<String, TonearmError> {
        let id = song.video_id.clone();
        let adders = ADDERS.lock().unwrap().clone().unwrap_or_default();
        let mut queue = self.queue.lock().unwrap();
        let current = queue.index();
        let added = queue.play_next(song, |s| s.video_id == id);
        if matches!(added, tonearm_core::queue::Added::Added) {
            // Where the last guest's song sits after the one playing.
            let last_guest = queue
                .items()
                .iter()
                .enumerate()
                .skip(current + 2)
                .filter(|(_, s)| adders.contains_key(&s.video_id) && s.video_id != id)
                .map(|(i, _)| i)
                .last();
            if let Some(last) = last_guest {
                queue.move_item(current + 1, last);
            }
        }
        drop(queue);
        self.after_adding(added, String::new(), String::new())
    }

    /// A guest's Play next or Add to queue: to the Party. None when this
    /// phone isn't a guest in one.
    pub(crate) fn party_guest_add(&self, song: &Song, next: bool) -> Option<String> {
        let party = PARTY.lock().unwrap();
        let here = party.as_ref().filter(|h| !h.ticket.host)?;
        here.live.send(json!({ "type": "add", "item": party_song(song), "next": next }));
        Some(format!("Added {} to {}", song.title, here.ticket.name))
    }

    /// A guest's view of the host's song, place and queue. None for the
    /// host, outside a Party, and until the host has said what plays.
    pub(crate) fn party_remote_view(&self) -> Option<Remote> {
        let party = PARTY.lock().unwrap();
        let here = party.as_ref().filter(|h| !h.ticket.host)?;
        let clock = here.clock.clone().filter(|_| !here.songs.is_empty())?;
        Some(Remote {
            songs: here.songs.iter().map(|(s, _)| s.clone()).collect(),
            clock,
            line: format!("In {}", here.ticket.name),
        })
    }

    /// As a guest, asks the host for `command` instead of doing it here.
    /// What's asked shows at once, when the host lets guests change what's
    /// playing. False when this phone isn't a guest showing the host's song.
    pub(crate) fn party_remote(&self, command: &str, args: Value) -> bool {
        let mut party = PARTY.lock().unwrap();
        let Some(here) = party.as_mut().filter(|h| !h.ticket.host && h.clock.is_some()) else { return false };
        if here.others_control {
            if let Some(clock) = here.clock.as_mut() {
                match command {
                    "play" => clock.expect(None, Some(true)),
                    "pause" => clock.expect(None, Some(false)),
                    "seek" => clock.expect(args["ms"].as_u64(), None),
                    _ => {}
                }
            }
        }
        let mut message = args;
        message["type"] = json!(command);
        here.live.send(message);
        true
    }

    /// A guest's tap on a song in the Party's queue, by its place there (0
    /// is the song playing).
    pub(crate) fn party_remote_skip(&self, index: usize) -> bool {
        let party = PARTY.lock().unwrap();
        let Some(here) = party.as_ref().filter(|h| !h.ticket.host && h.clock.is_some()) else { return false };
        if let Some((song, _)) = here.songs.get(index).filter(|_| index > 0) {
            here.live.send(json!({ "type": "play_item", "item": party_song(song), "index": index - 1 }));
        }
        true
    }

    /// A guest's drag in the Party's queue, by places there (0 is the song
    /// playing). It shows moved at once when the host allows it.
    pub(crate) fn party_remote_move(&self, from: usize, to: usize) -> bool {
        let mut party = PARTY.lock().unwrap();
        let Some(here) = party.as_mut().filter(|h| !h.ticket.host && h.clock.is_some()) else { return false };
        if from == 0 || to == 0 || from >= here.songs.len() || to >= here.songs.len() {
            return true;
        }
        let id = here.songs[from].0.video_id.clone();
        here.live.send(json!({ "type": "move", "from": from - 1, "to": to - 1, "video_id": id }));
        if here.others_control {
            let song = here.songs.remove(from);
            here.songs.insert(to, song);
        }
        true
    }

    /// A song picked to play anywhere in the app, by a guest: it plays in
    /// the Party, or joins its queue when the host only lets others add
    /// songs. False when this phone isn't a guest.
    pub(crate) fn party_guest_play(&self, song: &Song) -> bool {
        let party = PARTY.lock().unwrap();
        let Some(here) = party.as_ref().filter(|h| !h.ticket.host) else { return false };
        if here.others_control {
            here.live.send(json!({ "type": "play_item", "item": party_song(song) }));
        } else {
            here.live.send(json!({ "type": "add", "item": party_song(song), "next": false }));
            note(format!("Added {} to {}", song.title, here.ticket.name));
        }
        true
    }

    /// The host's own picks have no avatar, even a song a guest added
    /// before. Called when this phone adds a song itself.
    pub(crate) fn party_forget_adder(&self, video_id: &str) {
        if let Some(map) = ADDERS.lock().unwrap().as_mut() {
            map.remove(video_id);
        }
    }
}

/// A guest's line under the Party's name.
fn playing_line(state: &Value) -> String {
    let song = &state["song"];
    let title = song["title"].as_str().unwrap_or_default();
    if title.is_empty() {
        return String::new();
    }
    let by = song["subtitle"].as_str().unwrap_or_default();
    let what = if by.is_empty() { title.to_string() } else { format!("{title} · {by}") };
    if state["playing"].as_bool().unwrap_or(false) {
        format!("Playing {what}")
    } else {
        format!("Paused on {what}")
    }
}
