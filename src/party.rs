//! The Party on the desktop (docs/party.md): the header at the top of the
//! Queue panel, the live connection to the Party's room, and, as host,
//! carrying out what guests ask for with the same functions the app's own
//! buttons call. A guest's Play next and Add to queue go to the Party.
//!
//! A guest's player is a remote for the host: the host's songs stand in for
//! the queue (the guest's own is put aside, with its place, and comes back
//! on leaving), the clock runs from the host's state, and play, pause, skip,
//! seek, picks and drags go to the host.

use crate::{on_ui, App};
use serde_json::{json, Value};
use slint::{ComponentHandle, Image, ModelRc, Timer, TimerMode, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tonearm_core::audio;
use tonearm_core::innertube::model::Item;
use tonearm_core::party::{self, Clock, Event, Live, Member, Pacer, Ticket};

/// A guest's own queue, put aside while the host's shows.
struct Own {
    queue: Vec<Item>,
    index: usize,
    unshuffled: Option<Vec<Item>>,
    /// Where in its song, in seconds.
    position: f32,
    playing_from: String,
}

/// The Party this computer is in.
pub(crate) struct Here {
    ticket: Ticket,
    live: Live,
    members: Vec<Member>,
    /// When the host sends its state.
    pacer: Pacer,
    /// Who added each song guests put in, by video id, for the avatars on
    /// the queue's rows. Never the radio's, autoplay's or the host's own.
    /// A guest's come from the host's state.
    adders: HashMap<String, String>,
    others_control: bool,
    /// A guest's: the host's last state and the clock run from it.
    state: Value,
    clock: Option<Clock>,
    /// A guest's own queue while the host's shows. None until the host's
    /// first song comes.
    own: Option<Own>,
}

/// Avatar colours, picked by name, as the car's avatars are.
const COLOURS: [u32; 6] = [0xE0565B, 0x4C8DF6, 0x3FB27F, 0xE39B3B, 0x9B6BE0, 0x2BB3C0];

fn colour(name: &str) -> slint::Color {
    // Java's String.hashCode, so a name has the same colour as on the phone.
    let hash = name.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32));
    let rgb = COLOURS[hash.rem_euclid(COLOURS.len() as i32) as usize];
    slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

fn qr_image(code: &str) -> Option<Image> {
    const QUIET: usize = 2;
    const SCALE: usize = 6;
    let qr = party::qr(code)?;
    let side = (qr.size + QUIET * 2) * SCALE;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(side as u32, side as u32);
    let pixels = buffer.make_mut_slice();
    pixels.fill(slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 });
    for y in 0..qr.size {
        for x in 0..qr.size {
            if qr.dark[y * qr.size + x] {
                for py in 0..SCALE {
                    let row = ((y + QUIET) * SCALE + py) * side + (x + QUIET) * SCALE;
                    pixels[row..row + SCALE].fill(slint::Rgba8Pixel { r: 0, g: 0, b: 0, a: 255 });
                }
            }
        }
    }
    Some(Image::from_rgba8(buffer))
}

/// Puts text on the clipboard with Windows' own clip.exe.
fn copy_text(text: &str) -> bool {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Ok(mut child) = std::process::Command::new("clip")
        .stdin(std::process::Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    else {
        return false;
    };
    let wrote = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(text.as_bytes()).is_ok());
    child.wait().is_ok() && wrote
}

impl App {
    /// How this computer shows in a Party: the YouTube Music profile's
    /// first name, else the Windows user's.
    fn party_display_name(&self) -> String {
        let profile = self.ui.get_account_name().to_string();
        let name = profile.split_whitespace().next().map(str::to_string).unwrap_or_default();
        if !name.is_empty() {
            return name;
        }
        // Windows user names are often all lower case: "eli" shows as "Eli".
        let user = std::env::var("USERNAME").unwrap_or_else(|_| "Someone".into());
        let mut letters = user.chars();
        match letters.next() {
            Some(first) => first.to_uppercase().chain(letters).collect(),
            None => "Someone".into(),
        }
    }

    pub(crate) fn party_start(self: &Rc<Self>) {
        if self.party.borrow().is_some() {
            return;
        }
        self.ui.set_party_busy(true);
        self.ui.set_party_note("".into());
        let name = self.party_display_name();
        std::thread::spawn(move || {
            let result = party::start(&name);
            on_ui(move |app| {
                app.ui.set_party_busy(false);
                match result {
                    Ok(ticket) => app.party_enter(ticket),
                    Err(e) => app.ui.set_party_note(e.into()),
                }
            });
        });
    }

    pub(crate) fn party_join(self: &Rc<Self>, text: String) {
        if self.party.borrow().is_some() {
            return;
        }
        let Some(code) = party::code_from(&text) else {
            self.ui.set_party_note("That isn't a Party code or link".into());
            return;
        };
        self.ui.set_party_busy(true);
        self.ui.set_party_note("".into());
        let name = self.party_display_name();
        std::thread::spawn(move || {
            let result = party::join(&code, &name);
            on_ui(move |app| {
                app.ui.set_party_busy(false);
                match result {
                    Ok(ticket) => app.party_enter(ticket),
                    Err(e) => app.ui.set_party_note(e.into()),
                }
            });
        });
    }

    fn party_enter(self: &Rc<Self>, ticket: Ticket) {
        let live = Live::connect(&ticket);
        self.ui.set_party_on(true);
        self.ui.set_party_joining(false);
        self.ui.set_party_host(ticket.host);
        self.ui.set_party_name(ticket.name.clone().into());
        self.ui.set_party_code(ticket.code.clone().into());
        self.ui.set_party_playing("".into());
        self.ui.set_party_others(true);
        // The host sees the invite at once, as Spotify shows it.
        self.ui.set_party_invite_open(ticket.host);
        self.ui.set_party_members_open(false);
        if let Some(image) = qr_image(&ticket.code) {
            self.ui.set_party_qr(image);
        }
        *self.party.borrow_mut() = Some(Here {
            ticket,
            live,
            members: Vec::new(),
            pacer: Pacer::default(),
            adders: HashMap::new(),
            others_control: true,
            state: Value::Null,
            clock: None,
            own: None,
        });
        let weak = Rc::downgrade(self);
        self.party_timer.start(TimerMode::Repeated, Duration::from_millis(250), move || {
            if let Some(app) = weak.upgrade() {
                app.party_tick();
            }
        });
    }

    /// Back to no Party, with `note` as a toast when there is one.
    fn party_leave(self: &Rc<Self>, note: &str) {
        self.party_timer.stop();
        let own = self.party.borrow_mut().take().and_then(|h| h.own);
        self.ui.set_party_line("".into());
        self.ui.set_party_locked(false);
        if let Some(own) = own {
            self.party_own_back(own);
        }
        self.ui.set_party_on(false);
        self.ui.set_party_members(ModelRc::default());
        self.ui.set_party_note("".into());
        self.ui.set_party_invite_open(false);
        self.ui.set_party_members_open(false);
        // The adders' avatars go with the Party.
        self.sync_up_next();
        if !note.is_empty() {
            self.toast(note);
        }
    }

    /// The host ends the Party; a guest leaves it.
    pub(crate) fn party_end(self: &Rc<Self>) {
        let Some(mut here) = self.party.borrow_mut().take() else { return };
        // Put back for party_leave, which gives the guest their own queue.
        let own = here.own.take();
        if here.ticket.host {
            let ticket = here.ticket.clone();
            std::thread::spawn(move || {
                let _ = party::end(&ticket);
            });
            self.party_leave("The Party ended");
        } else {
            if let Some(own) = own {
                self.party_own_back(own);
            }
            here.live.send(json!({ "type": "leave" }));
            // Give the message a moment to go before the connection closes.
            let live = here.live;
            Timer::single_shot(Duration::from_millis(800), move || drop(live));
            self.party_leave("You left the Party");
        }
    }

    pub(crate) fn party_copy_link(self: &Rc<Self>) {
        let Some(code) = self.party.borrow().as_ref().map(|h| h.ticket.code.clone()) else { return };
        if copy_text(&party::link(&code)) {
            self.toast("Link copied");
        } else {
            self.toast("Couldn't copy the link");
        }
    }

    pub(crate) fn party_set_others(self: &Rc<Self>, on: bool) {
        if let Some(here) = self.party.borrow().as_ref() {
            here.live.send(json!({ "type": "settings", "others_control": on }));
        }
        self.ui.set_party_others(on);
    }

    pub(crate) fn party_remove(self: &Rc<Self>, id: String) {
        if let Some(here) = self.party.borrow().as_ref() {
            here.live.send(json!({ "type": "remove", "member": id }));
        }
    }

    /// A guest's Play next or Add to queue: to the Party instead of this
    /// computer's queue. False when not a guest in a Party.
    pub(crate) fn party_guest_add(self: &Rc<Self>, item: &Item, next: bool) -> bool {
        let party = self.party.borrow();
        let Some(here) = party.as_ref().filter(|h| !h.ticket.host) else { return false };
        here.live.send(json!({ "type": "add", "item": party::song_json(item), "next": next }));
        let name = here.ticket.name.clone();
        drop(party);
        self.toast(format!("Added {} to {name}", item.title));
        true
    }

    /// Who in the Party added a queued song, as its row's avatar. An empty
    /// letter when nobody in it did.
    pub(crate) fn party_adder(&self, item: &Item) -> crate::PartyAdder {
        let party = self.party.borrow();
        let name = item.video_id.as_ref().and_then(|id| party.as_ref()?.adders.get(id));
        match name {
            Some(name) => crate::PartyAdder {
                letter: name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
                color: colour(name),
            },
            None => crate::PartyAdder::default(),
        }
    }

    fn party_show_members(&self, members: &[Member]) {
        let rows: Vec<crate::PartyMember> = members
            .iter()
            .map(|m| crate::PartyMember {
                id: m.id.clone().into(),
                name: m.name.clone().into(),
                line: if m.host {
                    "Host".into()
                } else if !m.online {
                    "Away".into()
                } else {
                    match m.added {
                        0 => "Joined".into(),
                        1 => "Added 1 song".into(),
                        n => format!("Added {n} songs").into(),
                    }
                },
                letter: m.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
                color: colour(&m.name),
                host: m.host,
            })
            .collect();
        self.ui.set_party_members(ModelRc::new(VecModel::from(rows)));
    }

    /// What the host sends: the song, playing or not, where in it, and the
    /// next few songs.
    fn party_state(&self) -> Value {
        let party = self.party.borrow();
        let adders = party.as_ref().map(|h| &h.adders);
        let queued = |item: &Item| {
            let mut value = party::song_json(item);
            if let Some(name) = item.video_id.as_ref().and_then(|id| adders?.get(id)) {
                value["added_by"] = json!(name);
            }
            value
        };
        let player = self.player.borrow();
        let song = player.queue.get(player.index).map(queued);
        let next: Vec<Value> = player.queue.iter().skip(player.index + 1).take(30).map(queued).collect();
        let status = &self.audio.status;
        let playing = status.state.load(Ordering::SeqCst) == audio::PLAYING;
        // The engine's place belongs to the song shown only once it has
        // opened it; until then it is the top.
        let ours = status.generation.load(Ordering::SeqCst) == player.generation;
        let position = if ours { status.position_ms.load(Ordering::SeqCst) } else { 0 };
        json!({
            "song": song,
            "playing": playing && ours,
            "position_ms": position,
            "duration_ms": (player.duration * 1000.0) as u64,
            "at": party::now_ms(),
            "next": next,
        })
    }

    fn party_tick(self: &Rc<Self>) {
        let events = match self.party.borrow().as_ref() {
            Some(here) => here.live.events(),
            None => return,
        };
        for event in events {
            match event {
                Event::Welcome { name, others_control, members, state } => {
                    self.ui.set_party_note("".into());
                    self.ui.set_party_name(name.into());
                    self.party_show_members(&members);
                    if let Some(here) = self.party.borrow_mut().as_mut() {
                        here.members = members;
                        // Send the state again after a reconnect.
                        here.pacer.reset();
                    }
                    self.party_settings(others_control);
                    self.party_show_state(state, true);
                }
                Event::Members(members) => {
                    self.party_show_members(&members);
                    if let Some(here) = self.party.borrow_mut().as_mut() {
                        here.members = members;
                    }
                }
                Event::State(state) => self.party_show_state(state, false),
                Event::Settings { others_control } => self.party_settings(others_control),
                Event::Command { from, from_name, command, args } => self.party_command(&from, &from_name, &command, &args),
                Event::Refused(message) => {
                    // What a guest expected didn't happen: back to the host's
                    // last word.
                    let state = self.party.borrow().as_ref().map(|h| h.state.clone()).filter(|s| !s.is_null());
                    if let Some(state) = state {
                        self.party_show_state(state, true);
                    }
                    let mut letters = message.chars();
                    self.toast(letters.next().map(|c| c.to_uppercase().chain(letters).collect()).unwrap_or(message));
                }
                Event::Reconnecting => self.ui.set_party_note("Reconnecting…".into()),
                Event::Removed => {
                    self.party_leave("The host removed you from the Party");
                    return;
                }
                Event::Ended(reason) => {
                    let note = match reason.as_str() {
                        "ended by the host" => "The host ended the Party".to_string(),
                        "" => "The Party ended".to_string(),
                        other => format!("The Party ended: {other}"),
                    };
                    self.party_leave(&note);
                    return;
                }
            }
        }
        // The host's state, when it changed or now and then.
        let host = self.party.borrow().as_ref().is_some_and(|h| h.ticket.host);
        if host {
            let state = self.party_state();
            let mut party = self.party.borrow_mut();
            if let Some(here) = party.as_mut() {
                if here.pacer.due(&state) {
                    here.live.send_state(state);
                }
            }
        }
    }

    fn party_settings(&self, others_control: bool) {
        self.ui.set_party_others(others_control);
        let guest = match self.party.borrow_mut().as_mut() {
            Some(here) => {
                here.others_control = others_control;
                !here.ticket.host
            }
            None => false,
        };
        self.ui.set_party_locked(guest && !others_control);
    }

    /// A guest's view of the host's state: the line under the Party's name,
    /// and the host's songs in the player and queue in place of their own.
    fn party_show_state(self: &Rc<Self>, state: Value, stored: bool) {
        self.party_show_line(&state);
        let guest = self.party.borrow().as_ref().is_some_and(|h| !h.ticket.host);
        if !guest {
            return;
        }
        let songs = party::state_songs(&state);
        let clock = state["song"].is_object().then(|| Clock::of(&state, stored));
        let showing = {
            let mut party = self.party.borrow_mut();
            let Some(here) = party.as_mut() else { return };
            here.state = state;
            here.clock = clock;
            here.adders = songs
                .iter()
                .filter_map(|(item, by)| Some((item.video_id.clone()?, by.clone()?)))
                .collect();
            here.own.is_some()
        };
        // Nothing playing at the host: the guest's own player stays.
        if songs.is_empty() {
            return;
        }
        if !showing {
            self.party_put_own_aside();
        }
        let items: Vec<Item> = songs.into_iter().map(|(item, _)| item).collect();
        let (same_song, same_list) = {
            let player = self.player.borrow();
            let id = |i: &Item| i.video_id.clone();
            (
                showing && player.queue.first().map(id) == items.first().map(id),
                showing && player.queue.iter().map(id).eq(items.iter().map(id)),
            )
        };
        if !same_list {
            {
                let mut player = self.player.borrow_mut();
                player.queue = items;
                player.index = 0;
                player.unshuffled = None;
                player.resume_at = None;
            }
            self.rebuild_queue_model();
            if !same_song {
                self.show_track(0);
            }
        } else {
            // The adders may have changed.
            self.sync_up_next();
        }
        if !same_song {
            let name = self.party.borrow().as_ref().map(|h| h.ticket.name.clone()).unwrap_or_default();
            self.ui.set_party_line(format!("In {name}").into());
        }
    }

    /// Puts the guest's own queue aside, stopped where it was, as the
    /// host's first song comes.
    fn party_put_own_aside(&self) {
        let own = {
            let player = self.player.borrow();
            let status = &self.audio.status;
            let current = player.queue.get(player.index).and_then(|i| i.video_id.clone());
            let position = match &player.resume_at {
                Some((at, seconds)) if current.as_ref() == Some(at) => *seconds,
                _ if status.generation.load(Ordering::SeqCst) == player.generation => {
                    status.position_ms.load(Ordering::SeqCst) as f32 / 1000.0
                }
                _ => 0.0,
            };
            Own {
                queue: player.queue.clone(),
                index: player.index,
                unshuffled: player.unshuffled.clone(),
                position,
                playing_from: self.playing_from.borrow().clone(),
            }
        };
        // Saved now, as it is: nothing is saved while the host's songs show.
        self.save_session();
        self.play_hold.set(false);
        self.audio.stop();
        if let Some(here) = self.party.borrow_mut().as_mut() {
            here.own = Some(own);
        }
    }

    /// The guest's own queue and song again, paused where they were.
    fn party_own_back(self: &Rc<Self>, own: Own) {
        *self.playing_from.borrow_mut() = own.playing_from;
        {
            let mut player = self.player.borrow_mut();
            player.queue = own.queue;
            player.index = own.index;
            player.unshuffled = own.unshuffled;
        }
        self.rebuild_queue_model();
        if self.player.borrow().queue.is_empty() {
            // Nothing of its own: the player as a fresh start shows it.
            let ui = &self.ui;
            ui.set_has_track(false);
            ui.set_queue_index(-1);
            ui.set_track_title("".into());
            ui.set_track_artist("".into());
            ui.set_track_has_art(false);
            ui.set_track_art(Image::default());
            ui.set_track_backdrop(Image::default());
            ui.set_progress(0.0);
            ui.set_position_text("0:00".into());
            ui.set_duration_text("0:00".into());
            ui.set_playing(false);
            return;
        }
        if let Some((item, _, _)) = self.show_track(own.index) {
            if let Some(id) = item.video_id {
                self.show_resume_point(id, own.position);
            }
        }
        self.save_session();
    }

    /// Whether the host's songs stand in for this computer's queue.
    pub(crate) fn party_showing(&self) -> bool {
        self.party.borrow().as_ref().is_some_and(|h| h.own.is_some())
    }

    /// The guest's player from the host's clock, each tick. The time into
    /// the song in milliseconds, for the lyrics; None unless a guest.
    pub(crate) fn party_tick_clock(&self) -> Option<u64> {
        let clock = self.party.borrow().as_ref().filter(|h| h.own.is_some())?.clock.clone()?;
        let ui = &self.ui;
        let playing = clock.playing;
        if ui.get_playing() != playing {
            ui.set_playing(playing);
            ui.global::<crate::Now>().set_playing(playing);
            #[cfg(windows)]
            crate::taskbar::set_playing(playing);
        }
        ui.set_buffering(false);
        // The host's length beats the guess from the song's details, which
        // a song added from a list without one doesn't have.
        let mut duration = clock.duration_ms() as f32 / 1000.0;
        if duration > 0.0 {
            let shown = self.player.borrow().duration;
            if (shown - duration).abs() > 1.0 {
                self.player.borrow_mut().duration = duration;
                ui.set_duration_text(crate::clock_text(duration));
            }
        } else {
            duration = self.player.borrow().duration;
        }
        let time = clock.position_ms();
        let seconds = time as f32 / 1000.0;
        ui.set_position_text(crate::clock_text(seconds));
        ui.set_progress(if duration > 0.0 { (seconds / duration).min(1.0) } else { 0.0 });
        Some(time)
    }

    /// As a guest, asks the host for `command` instead of doing it here.
    /// What's asked shows at once when the host lets guests change what's
    /// playing. False unless the host's songs are showing.
    pub(crate) fn party_remote(&self, command: &str, args: Value) -> bool {
        let mut party = self.party.borrow_mut();
        let Some(here) = party.as_mut().filter(|h| h.own.is_some()) else { return false };
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

    /// A guest's play or pause: the opposite of what the host's clock says.
    pub(crate) fn party_remote_toggle(&self) -> bool {
        let playing = self.party.borrow().as_ref().and_then(|h| h.clock.as_ref().map(|c| c.playing));
        match playing {
            Some(playing) => self.party_remote(if playing { "pause" } else { "play" }, json!({})),
            None => false,
        }
    }

    /// A guest's pick of a song in the host's queue, by its place (0 is the
    /// song playing).
    pub(crate) fn party_remote_pick(&self, index: usize) -> bool {
        if !self.party_showing() {
            return false;
        }
        let item = self.player.borrow().queue.get(index).cloned();
        if let Some(item) = item.filter(|_| index > 0) {
            self.party_remote("play_item", json!({ "item": party::song_json(&item), "index": index - 1 }));
        }
        true
    }

    /// A guest's drag in the host's queue, by places (0 is the song playing).
    pub(crate) fn party_remote_move(&self, from: usize, to: usize) -> bool {
        if !self.party_showing() {
            return false;
        }
        let id = self.player.borrow().queue.get(from).and_then(|i| i.video_id.clone());
        if let Some(id) = id.filter(|_| from > 0 && to > 0) {
            self.party_remote("move", json!({ "from": from - 1, "to": to - 1, "video_id": id }));
        }
        true
    }

    /// A song or list picked to play anywhere in the app, by a guest: the
    /// song plays in the Party, or joins its queue when the host only lets
    /// others add songs. False unless a guest.
    pub(crate) fn party_guest_play(self: &Rc<Self>, item: &Item) -> bool {
        let party = self.party.borrow();
        let Some(here) = party.as_ref().filter(|h| !h.ticket.host) else { return false };
        if here.others_control {
            here.live.send(json!({ "type": "play_item", "item": party::song_json(item) }));
        } else {
            here.live.send(json!({ "type": "add", "item": party::song_json(item), "next": false }));
            let note = format!("Added {} to {}", item.title, here.ticket.name);
            drop(party);
            self.toast(note);
        }
        true
    }

    /// A guest's line under the Party's name: what the host plays.
    fn party_show_line(&self, state: &Value) {
        let song = &state["song"];
        let title = song["title"].as_str().unwrap_or_default();
        let line = if title.is_empty() {
            String::new()
        } else {
            let by = song["subtitle"].as_str().unwrap_or_default();
            let what = if by.is_empty() { title.to_string() } else { format!("{title} · {by}") };
            if state["playing"].as_bool().unwrap_or(false) { format!("Playing {what}") } else { format!("Paused on {what}") }
        };
        self.ui.set_party_playing(line.into());
    }

    /// Carries out a guest's request, as host.
    fn party_command(self: &Rc<Self>, from: &str, from_name: &str, command: &str, args: &Value) {
        let playing = self.audio.status.state.load(Ordering::SeqCst) == audio::PLAYING;
        match command {
            "add" => {
                let Some(item) = party::song_from(&args["item"]) else { return };
                let title = item.title.clone();
                // The guest's avatar goes on the song's row. Not on a song
                // already queued, which Add to queue leaves as it is.
                let next = args["next"].as_bool().unwrap_or(false);
                if let Some(id) = item.video_id.clone() {
                    let queued = self.player.borrow().queue.iter().any(|i| i.video_id.as_deref() == Some(id.as_str()));
                    if next || !queued {
                        if let Some(here) = self.party.borrow_mut().as_mut() {
                            here.adders.insert(id, from_name.to_string());
                        }
                    }
                }
                if next {
                    self.play_next(item);
                } else {
                    self.party_queue(item);
                }
                if let Some(here) = self.party.borrow().as_ref() {
                    here.live.send(json!({ "type": "added", "member": from }));
                }
                self.toast(format!("{from_name} added {title}"));
            }
            "play" if !playing => self.toggle_play(),
            "pause" if playing => self.toggle_play(),
            "next" => self.next(),
            "previous" => self.previous(),
            "seek" => {
                if let Some(ms) = args["ms"].as_f64() {
                    self.seek_to((ms / 1000.0) as f32);
                }
            }
            // A song tapped in a guest's queue: `index` is its place among
            // the next songs, when it came from there. From anywhere else
            // (or when the queue moved meanwhile) it plays next, then now.
            "play_item" => {
                let Some(item) = party::song_from(&args["item"]) else { return };
                let (at, playing_now) = {
                    let player = self.player.borrow();
                    let same = |i: &Item| i.video_id.is_some() && i.video_id == item.video_id;
                    let at = args["index"]
                        .as_u64()
                        .map(|i| player.index + 1 + i as usize)
                        .filter(|&at| player.queue.get(at).is_some_and(same));
                    (at, player.queue.get(player.index).is_some_and(same))
                };
                match at {
                    Some(at) => self.play_index(at),
                    None if playing_now => {}
                    None if self.player.borrow().queue.is_empty() => self.set_queue(vec![item], 0),
                    None => {
                        self.play_next(item);
                        self.next();
                    }
                }
            }
            // A guest's drag: places among the next songs, checked against
            // the song they saw there.
            "move" => {
                let (Some(from), Some(to)) = (args["from"].as_u64(), args["to"].as_u64()) else { return };
                let id = args["video_id"].as_str().unwrap_or_default();
                let (from, to, fits) = {
                    let player = self.player.borrow();
                    let start = player.index + 1;
                    let (from, to) = (start + from as usize, start + to as usize);
                    let fits = to < player.queue.len()
                        && player.queue.get(from).is_some_and(|i| i.video_id.as_deref() == Some(id));
                    (from, to, fits)
                };
                if fits {
                    self.move_queued(from, to);
                }
            }
            _ => {}
        }
    }

    /// A guest's Add to queue lands after the other songs guests added,
    /// before the rest of the queue, as Spotify's Jam does it. A song
    /// already queued stays where it is.
    fn party_queue(self: &Rc<Self>, item: Item) {
        let Some(id) = item.video_id.clone() else { return };
        let (empty, known) = {
            let player = self.player.borrow();
            (player.queue.is_empty(), player.queue.iter().any(|i| i.video_id.as_deref() == Some(id.as_str())))
        };
        if empty {
            self.set_queue(vec![item], 0);
            return;
        }
        if known {
            return;
        }
        {
            let party = self.party.borrow();
            let adders = party.as_ref().map(|h| &h.adders);
            let mut player = self.player.borrow_mut();
            let current = player.index;
            // After the last guest's song after the one playing.
            let last_guest = player
                .queue
                .iter()
                .enumerate()
                .skip(current + 1)
                .filter(|(_, i)| {
                    i.video_id.as_ref().is_some_and(|v| v != &id && adders.is_some_and(|a| a.contains_key(v)))
                })
                .map(|(at, _)| at)
                .last();
            let at = last_guest.map_or(current + 1, |last| last + 1).min(player.queue.len());
            player.queue.insert(at, item);
        }
        self.rebuild_queue_model();
        self.prefetch_next();
    }
}
