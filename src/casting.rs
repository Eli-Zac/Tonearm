//! The cast button: the Cast devices on the network in a menu above the
//! player, and the cast to the one picked. `cast` has how the sound gets
//! there.
//!
//! While casting, the volume slider shows and sets the device's volume.
//! This computer's volume is kept and put back when the cast ends.

use crate::{cast, on_ui, App, CastDevice};
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// What the device shows as playing until it hears of a song. Google's
/// Default Media Receiver can't be told when the song changes, so with it
/// this stays: the app and the computer.
const TITLE: &str = "Tonearm";

#[derive(Default)]
pub struct Casting {
    finder: Option<cast::Finder>,
    /// The devices in the menu, in its order.
    devices: Vec<cast::Device>,
    session: Option<cast::Session>,
    /// Whether the device has started playing yet.
    connecting: bool,
    /// This computer's volume, while the slider shows the device's.
    local_volume: Option<f32>,
    /// Counts casts, so word from one that has ended is ignored.
    generation: u64,
}

/// What this computer is called on the network, for the device to show.
fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|n| !n.is_empty())
        .map(|n| format!("Playing from {n}"))
        .unwrap_or_else(|| "Playing from this computer".into())
}

impl App {
    /// Starts looking for devices, so the menu has them when it opens.
    pub(crate) fn start_cast_search(self: &Rc<Self>) {
        let finder = cast::Finder::start(|| on_ui(|app| app.show_cast_devices()));
        self.casting.borrow_mut().finder = Some(finder);
        self.show_cast_devices();
    }

    pub(crate) fn show_cast_devices(&self) {
        let mut casting = self.casting.borrow_mut();
        let devices = casting.finder.as_ref().map(|f| f.devices()).unwrap_or_default();
        // The device cast to stays listed even if it stops answering the
        // search for a while.
        let mut devices = devices;
        if let Some(session) = &casting.session {
            if !devices.iter().any(|d| d.id == session.device.id) {
                devices.push(session.device.clone());
            }
        }
        let rows: Vec<CastDevice> = devices
            .iter()
            .map(|d| CastDevice {
                name: d.name.as_str().into(),
                model: d.model.as_str().into(),
                kind: if d.group {
                    2
                } else if d.screen {
                    1
                } else {
                    0
                },
            })
            .collect();
        casting.devices = devices;
        let active = casting.session.as_ref().and_then(|s| {
            casting.devices.iter().position(|d| d.id == s.device.id)
        });
        let connecting = casting.connecting;
        drop(casting);
        self.ui.set_cast_devices(ModelRc::new(VecModel::from(rows)));
        self.ui.set_cast_active(active.map_or(-1, |i| i as i32));
        self.ui.set_cast_connecting(connecting);
    }

    /// A line of the cast menu: a device, or -1 for this computer.
    pub(crate) fn cast_picked(self: &Rc<Self>, index: i32) {
        let device = usize::try_from(index)
            .ok()
            .and_then(|i| self.casting.borrow().devices.get(i).cloned());
        let current = self.casting.borrow().session.as_ref().map(|s| s.device.id.clone());
        if current.is_some() && current == device.as_ref().map(|d| d.id.clone()) {
            return;
        }
        // From one device straight to another, without a moment here.
        self.end_cast(None, false, device.is_none());
        let Some(device) = device else {
            return;
        };
        let pipe = cast::Pipe::new();
        let generation = {
            let mut casting = self.casting.borrow_mut();
            casting.generation += 1;
            casting.generation
        };
        let session = cast::Session::start(
            device.clone(),
            pipe.clone(),
            TITLE.into(),
            computer_name(),
            move |event| on_ui(move |app| app.cast_event(generation, event)),
        );
        // The song stops here at once and carries on from the same place
        // on the device once it plays.
        self.audio.route(Some(pipe));
        {
            let mut casting = self.casting.borrow_mut();
            casting.session = Some(session);
            casting.connecting = true;
            casting.local_volume = Some(self.ui.get_volume());
        }
        self.ui.set_cast_name(device.name.as_str().into());
        self.show_cast_devices();
    }

    fn cast_event(self: &Rc<Self>, generation: u64, event: cast::Event) {
        if self.casting.borrow().generation != generation {
            return;
        }
        match event {
            cast::Event::Playing => {
                self.casting.borrow_mut().connecting = false;
                self.show_cast_devices();
            }
            // The slider follows the device. Setting it here sends nothing
            // back.
            cast::Event::Volume(level) => self.ui.set_volume(level.clamp(0.0, 1.0)),
            cast::Event::Paused(true) => self.audio.pause(),
            cast::Event::Paused(false) => self.audio.play(),
            cast::Event::Ended(reason) => {
                let name = self.ui.get_cast_name().to_string();
                let message = reason.unwrap_or_else(|| format!("{name} stopped playing"));
                self.end_cast(Some(message), true, true);
            }
        }
    }

    /// Stops casting, if casting, and with `here`, plays on this computer
    /// again. A cast that ended on its own pauses first, so the music
    /// doesn't jump to this computer's speakers.
    fn end_cast(self: &Rc<Self>, message: Option<String>, pause: bool, here: bool) {
        let (session, volume) = {
            let mut casting = self.casting.borrow_mut();
            casting.generation += 1;
            casting.connecting = false;
            (casting.session.take(), casting.local_volume.take())
        };
        let Some(session) = session else {
            return;
        };
        drop(session);
        if pause {
            self.audio.pause();
        }
        if here {
            self.audio.route(None);
        }
        if let Some(volume) = volume {
            self.ui.set_volume(volume);
        }
        self.ui.set_cast_name(SharedString::new());
        self.show_cast_devices();
        if let Some(message) = message {
            self.toast(message);
        }
    }

    /// Sends the slider's level to the device while casting. Returns
    /// whether it did, so this computer's volume is left alone.
    pub(crate) fn cast_volume(&self, level: f32) -> bool {
        match &self.casting.borrow().session {
            Some(session) => {
                session.volume(level);
                true
            }
            None => false,
        }
    }

    /// Tells the device the song playing and how far in, for Tonearm's own
    /// receiver to show. `seconds` is where the playhead shows, which
    /// follows what the device plays.
    pub(crate) fn cast_now_playing(&self, seconds: f32, duration: f32, playing: bool) {
        if !cast::shows_songs() {
            return;
        }
        let casting = self.casting.borrow();
        let Some(session) = &casting.session else {
            return;
        };
        let player = self.player.borrow();
        let Some(item) = player.queue.get(player.index) else {
            return;
        };
        let artist = if item.artists.is_empty() {
            item.subtitle.split(" • ").next().unwrap_or_default().to_string()
        } else {
            item.artists.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
        };
        let cover = if item.thumb.starts_with("//") {
            format!("https:{}", item.thumb)
        } else {
            crate::images::sized(&item.thumb, 720)
        };
        session.now_playing(cast::NowPlaying {
            id: item.video_id.clone().unwrap_or_else(|| item.title.clone()),
            title: item.title.clone(),
            artist,
            album: item.album.as_ref().map(|(name, _)| name.clone()).unwrap_or_default(),
            // A downloaded song's cover is a file here, out of the device's reach.
            cover: if cover.starts_with("http") { cover } else { String::new() },
            duration,
            position: seconds,
            paused: !playing,
        });
    }

    /// This computer's volume, also while the slider shows the device's.
    pub(crate) fn local_volume(&self) -> f32 {
        self.casting.borrow().local_volume.unwrap_or_else(|| self.ui.get_volume())
    }

    /// Ends a cast as Tonearm quits, telling the device first.
    pub(crate) fn finish_cast(&self) {
        let session = self.casting.borrow_mut().session.take();
        if let Some(session) = session {
            session.finish();
        }
    }
}
