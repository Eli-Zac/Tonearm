//! The Party on the desktop (docs/party.md): the header at the top of the
//! Queue panel, the live connection to the Party's room, and, as host,
//! carrying out what guests ask for with the same functions the app's own
//! buttons call. A guest's Play next and Add to queue go to the Party.

use crate::{on_ui, App};
use serde_json::{json, Value};
use slint::{Image, ModelRc, Timer, TimerMode, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tonearm_core::audio;
use tonearm_core::innertube::model::Item;
use tonearm_core::party::{self, Event, Live, Member, Ticket};

/// How often the host sends its state while nothing changes, so a guest
/// who missed a message catches up.
const STATE_EVERY: Duration = Duration::from_secs(15);

/// The Party this computer is in.
pub(crate) struct Here {
    ticket: Ticket,
    live: Live,
    members: Vec<Member>,
    /// The host's last state as sent, to send only changes.
    sent: String,
    sent_at: Instant,
    /// Who added each song guests put in, by video id, for the avatars on
    /// the queue's rows. Never the radio's, autoplay's or the host's own.
    adders: HashMap<String, String>,
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
            sent: String::new(),
            sent_at: Instant::now() - STATE_EVERY,
            adders: HashMap::new(),
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
        *self.party.borrow_mut() = None;
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
        let Some(here) = self.party.borrow_mut().take() else { return };
        if here.ticket.host {
            let ticket = here.ticket.clone();
            std::thread::spawn(move || {
                let _ = party::end(&ticket);
            });
            self.party_leave("The Party ended");
        } else {
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
        let player = self.player.borrow();
        let song = player.queue.get(player.index).map(party::song_json);
        let next: Vec<Value> = player.queue.iter().skip(player.index + 1).take(20).map(party::song_json).collect();
        let playing = self.audio.status.state.load(Ordering::SeqCst) == audio::PLAYING;
        json!({ "song": song, "playing": playing, "next": next })
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
                    self.ui.set_party_others(others_control);
                    self.party_show_members(&members);
                    self.party_show_state(&state);
                    if let Some(here) = self.party.borrow_mut().as_mut() {
                        here.members = members;
                        // Send the state again after a reconnect.
                        here.sent.clear();
                    }
                }
                Event::Members(members) => {
                    self.party_show_members(&members);
                    if let Some(here) = self.party.borrow_mut().as_mut() {
                        here.members = members;
                    }
                }
                Event::State(state) => self.party_show_state(&state),
                Event::Settings { others_control } => self.ui.set_party_others(others_control),
                Event::Command { from, from_name, command, args } => self.party_command(&from, &from_name, &command, &args),
                Event::Refused(message) => self.toast(message),
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
            let text = state.to_string();
            let mut party = self.party.borrow_mut();
            if let Some(here) = party.as_mut() {
                if text != here.sent || here.sent_at.elapsed() > STATE_EVERY {
                    here.live.send_state(state);
                    here.sent = text;
                    here.sent_at = Instant::now();
                }
            }
        }
    }

    /// A guest's line under the Party's name: what the host plays.
    fn party_show_state(&self, state: &Value) {
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
                    self.add_to_queue(item);
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
            "play_item" => {
                if let Some(item) = party::song_from(&args["item"]) {
                    self.play_next(item);
                    self.next();
                }
            }
            _ => {}
        }
    }
}
