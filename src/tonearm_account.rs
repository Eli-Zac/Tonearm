//! The Tonearm account page in Settings: sign in with an emailed code or a
//! pairing code, make a pairing code, list and remove the account's devices.
//! The calls are `tonearm_core::account`; each runs on its own thread, and the
//! page is updated from the event loop when it answers.

use super::*;
use tonearm_core::account;

/// "just now", "5 minutes ago", "3 days ago".
fn ago(millis: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let seconds = ((now - millis) / 1000).max(0);
    match seconds {
        0..=89 => "just now".to_string(),
        90..=3599 => format!("{} minutes ago", seconds / 60),
        3600..=86399 => format!("{} hours ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86400),
    }
}

/// A pairing code's QR code as a picture: black on white, a blank border of
/// two modules, each module 8 pixels square.
fn qr_image(code: &str) -> Option<Image> {
    const QUIET: usize = 2;
    const SCALE: usize = 8;
    let qr = account::pair_qr(code)?;
    let side = (qr.size + QUIET * 2) * SCALE;
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(side as u32, side as u32);
    let pixels = buffer.make_mut_slice();
    pixels.fill(slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 });
    for y in 0..qr.size {
        for x in 0..qr.size {
            if !qr.dark[y * qr.size + x] {
                continue;
            }
            for py in 0..SCALE {
                let row = ((y + QUIET) * SCALE + py) * side + (x + QUIET) * SCALE;
                pixels[row..row + SCALE].fill(slint::Rgba8Pixel { r: 0, g: 0, b: 0, a: 255 });
            }
        }
    }
    Some(Image::from_rgba8(buffer))
}

impl App {
    /// Shows who the Tonearm account is signed in as, or that it isn't. Reads
    /// the store only.
    pub(crate) fn show_tonearm_account(&self) {
        let email = account::current().map(|a| a.email).unwrap_or_default();
        if email.is_empty() {
            self.tonearm_devices.borrow_mut().clear();
            self.ui.set_tonearm_devices(ModelRc::new(VecModel::from(Vec::new())));
            self.ui.set_tonearm_pair_code("".into());
        }
        self.ui.set_tonearm_email(email.into());
    }

    /// The page opened: shows the account and asks for its devices.
    pub(crate) fn tonearm_open(self: &Rc<Self>) {
        self.ui.set_tonearm_note("".into());
        self.show_tonearm_account();
        if account::current().is_some() {
            self.tonearm_refresh_devices();
        }
    }

    fn tonearm_refresh_devices(self: &Rc<Self>) {
        std::thread::spawn(|| {
            let result = account::devices();
            on_ui(move |app| match result {
                Ok(list) => {
                    *app.tonearm_devices.borrow_mut() = list.iter().map(|d| d.id.clone()).collect();
                    let rows: Vec<AccountDevice> = list
                        .into_iter()
                        .map(|d| AccountDevice {
                            name: d.name.into(),
                            kind: d.kind.into(),
                            model: d.model.into(),
                            seen: ago(d.last_seen).into(),
                            current: d.current,
                        })
                        .collect();
                    app.ui.set_tonearm_devices(ModelRc::new(VecModel::from(rows)));
                }
                Err(message) => {
                    // A 401 signed this computer out; show that.
                    app.show_tonearm_account();
                    app.ui.set_tonearm_note(message.into());
                }
            });
        });
    }

    /// Emails a sign in code.
    pub(crate) fn tonearm_request_code(self: &Rc<Self>, email: String) {
        if self.ui.get_tonearm_busy() || email.trim().is_empty() {
            return;
        }
        self.ui.set_tonearm_busy(true);
        self.ui.set_tonearm_note("".into());
        std::thread::spawn(move || {
            let result = account::request_code(&email);
            on_ui(move |app| {
                app.ui.set_tonearm_busy(false);
                match result {
                    Ok(()) => {
                        app.ui.set_tonearm_code_sent(true);
                        app.ui.set_tonearm_note("Code sent. It works for 10 minutes.".into());
                    }
                    Err(message) => app.ui.set_tonearm_note(message.into()),
                }
            });
        });
    }

    /// Signs in with the emailed code.
    pub(crate) fn tonearm_sign_in(self: &Rc<Self>, email: String, code: String) {
        self.tonearm_join(move || account::sign_in(&email, &code, ""));
    }

    /// Joins with a pairing code another device made.
    pub(crate) fn tonearm_pair_claim(self: &Rc<Self>, code: String) {
        self.tonearm_join(move || account::pair_claim(&code, ""));
    }

    fn tonearm_join(
        self: &Rc<Self>,
        work: impl FnOnce() -> Result<account::Account, String> + Send + 'static,
    ) {
        if self.ui.get_tonearm_busy() {
            return;
        }
        self.ui.set_tonearm_busy(true);
        self.ui.set_tonearm_note("".into());
        std::thread::spawn(move || {
            let result = work();
            on_ui(move |app| {
                app.ui.set_tonearm_busy(false);
                match result {
                    Ok(_) => {
                        app.ui.set_tonearm_code_sent(false);
                        app.show_tonearm_account();
                        app.tonearm_refresh_devices();
                        app.sync_settings_soon();
                    }
                    Err(message) => app.ui.set_tonearm_note(message.into()),
                }
            });
        });
    }

    /// Brings this computer's shared settings and pins in step with the
    /// account's, a moment from now so a change just made is on disk first.
    /// Does nothing signed out.
    pub(crate) fn sync_settings_soon(&self) {
        if account::current().is_none() {
            return;
        }
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(1500));
            if let Ok(changed) = tonearm_core::settings_sync::sync() {
                if !changed.is_empty() {
                    on_ui(move |app| app.show_synced(&changed));
                }
            }
        });
    }

    /// Syncs at start, then every few minutes while the app is open.
    pub(crate) fn start_settings_sync(self: &Rc<Self>) {
        self.sync_settings_soon();
        let weak = Rc::downgrade(self);
        self.settings_timer.start(TimerMode::Repeated, Duration::from_secs(300), move || {
            if let Some(app) = weak.upgrade() {
                app.sync_settings_soon();
            }
        });
    }

    /// Shows what a sync changed: the settings it reached and the pins.
    fn show_synced(self: &Rc<Self>, keys: &[String]) {
        use tonearm_core::settings;
        for key in keys {
            match key.as_str() {
                "crossfade" => {
                    self.crossfade.set(settings::crossfade());
                    self.ui.set_crossfade_choice(settings::crossfade_choice() as i32);
                    self.forget_plan();
                }
                "crossfade-manual" => {
                    self.crossfade_manual.set(settings::crossfade_manual());
                    self.ui.set_crossfade_manual_choice(i32::from(settings::crossfade_manual()));
                }
                "levelling" => {
                    self.levelling.set(settings::levelling());
                    self.ui.set_levelling_choice(settings::levelling());
                    let generation = self.player.borrow().generation;
                    self.apply_gain(generation);
                }
                "translate" => {
                    self.translate.set(settings::translate());
                    self.ui.set_translate_choice(settings::translate() as i32);
                    self.translate_lyrics();
                }
                "cover-choices" => {
                    let choices = settings::cover_choices();
                    if choices != self.cover_choices.get() {
                        self.cover_choices.set(choices);
                        self.ui.set_panel_cover(choices.0);
                        self.ui.set_wide_cover(choices.1);
                    }
                }
                _ => {}
            }
        }
        if keys.iter().any(|k| k.starts_with("pin/")) {
            *self.quick_access.borrow_mut() = tonearm_core::quick_access::QuickAccess::load();
            self.show_quick_access();
        }
    }

    /// Makes a pairing code for another device. It is shown until it runs out.
    pub(crate) fn tonearm_pair_start(self: &Rc<Self>) {
        std::thread::spawn(|| {
            let result = account::pair_start();
            on_ui(move |app| match result {
                Ok(made) => {
                    let generation = app.tonearm_pair.get() + 1;
                    app.tonearm_pair.set(generation);
                    if let Some(image) = qr_image(&made.code) {
                        app.ui.set_tonearm_pair_qr(image);
                    }
                    app.ui.set_tonearm_pair_code(made.code.into());
                    app.ui.set_tonearm_pair_note(
                        format!(
                            "Enter this on the other device within {} minutes. It works once.",
                            made.expires_in / 60
                        )
                        .into(),
                    );
                    let weak = Rc::downgrade(app);
                    Timer::single_shot(Duration::from_secs(made.expires_in as u64), move || {
                        if let Some(app) = weak.upgrade() {
                            if app.tonearm_pair.get() == generation {
                                app.ui.set_tonearm_pair_code("".into());
                            }
                        }
                    });
                }
                Err(message) => app.ui.set_tonearm_note(message.into()),
            });
        });
    }

    pub(crate) fn tonearm_sign_out(self: &Rc<Self>) {
        std::thread::spawn(|| {
            account::sign_out();
            on_ui(|app| {
                app.ui.set_tonearm_note("".into());
                app.show_tonearm_account();
            });
        });
    }

    /// Signs the device at `index` in the list out of the account.
    pub(crate) fn tonearm_remove_device(self: &Rc<Self>, index: usize) {
        let Some(id) = self.tonearm_devices.borrow().get(index).cloned() else { return };
        std::thread::spawn(move || {
            let result = account::remove_device(&id);
            on_ui(move |app| {
                if let Err(message) = result {
                    app.ui.set_tonearm_note(message.into());
                }
                app.tonearm_refresh_devices();
            });
        });
    }
}
