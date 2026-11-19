//! Downloads and the cache, as the window shows them: toasts, the
//! Downloads page and the storage card in Settings. The work itself is in
//! `offline`.

use crate::innertube::model::Item;
use crate::offline::{self, Event};
use std::rc::Rc;

impl crate::App {
    /// Sends download events to the window.
    pub fn listen_for_downloads() {
        offline::listen(|event| crate::on_ui(move |app| app.download_event(event)));
    }

    fn download_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::Progress(id) => self.show_download_progress(&id),
            Event::Finished {
                item,
                result,
                n,
                total,
            } => {
                match result {
                    Ok(()) if total == 1 => self.toast(format!("Downloaded {}", item.title)),
                    Ok(()) if n + 1 == total => self.toast(format!("Downloaded {total} songs")),
                    Ok(()) => {}
                    Err(message) => {
                        self.toast(format!("Could not download {}: {message}", item.title))
                    }
                }
                self.downloads_changed();
            }
        }
    }

    /// Downloads songs in the background. Songs already saved or on their
    /// way are skipped.
    pub fn download_items(self: &Rc<Self>, items: Vec<Item>) {
        let fresh = offline::queue(self.resolver.clone(), &self.http, items);
        match fresh.as_slice() {
            [] => self.toast("Already downloaded"),
            [one] => self.toast(format!("Downloading {}", one.title)),
            all => self.toast(format!("Downloading {} songs", all.len())),
        }
        if !fresh.is_empty() {
            self.downloads_changed();
        }
    }

    /// Fills in the artist of downloads saved without one, from their
    /// album pages. Quiet, and a no-op once every song is credited.
    pub fn repair_downloads(self: &Rc<Self>) {
        let client = self.client.clone();
        self.api.run(move || {
            if offline::repair(&client) {
                crate::on_ui(|app| app.downloads_changed());
            }
        });
    }

    /// Downloads a song, or removes it when it is already downloaded.
    pub fn toggle_download(self: &Rc<Self>, item: Item) {
        let Some(video_id) = item.video_id.clone() else {
            return;
        };
        if offline::is_downloaded(&video_id) {
            self.remove_download(&video_id);
        } else if !offline::is_downloading(&video_id) {
            self.download_items(vec![item]);
        }
    }

    pub fn remove_download(self: &Rc<Self>, video_id: &str) {
        let video_id = video_id.to_string();
        let title = offline::title_of(&video_id).unwrap_or_default();
        self.disk.run(move || {
            offline::remove(&video_id);
            crate::on_ui(move |app| {
                app.toast(format!("Removed {title} from downloads"));
                app.downloads_changed();
            });
        });
    }

    /// Fetches the whole next track into the cache, so it starts at once
    /// and keeps playing if the connection drops.
    pub fn cache_ahead(self: &Rc<Self>, video_id: String) {
        if offline::cache_limit_mb() == 0 || offline::has_local(&video_id) {
            return;
        }
        let Some(resolver) = self.resolver.clone() else {
            return;
        };
        self.api.run(move || offline::fetch_ahead(resolver.as_ref(), &video_id));
    }

    /// Refreshes whatever shows what is downloaded: the Downloads page if
    /// it is open, and the storage line in Settings.
    pub fn downloads_changed(self: &Rc<Self>) {
        self.show_storage();
        self.refresh_downloaded();
        let on_downloads = matches!(self.history.borrow().last(), Some(crate::Route::Downloads))
            && self.ui.get_nav() != 3;
        if on_downloads {
            self.open(crate::Route::Downloads, false);
        }
    }

    /// The storage card in Settings.
    pub fn show_storage(&self) {
        self.ui.set_storage_note(offline::storage_note().into());
        self.ui.set_cache_choice(offline::cache_choice() as i32);
    }

    pub fn set_cache_choice(self: &Rc<Self>, choice: i32) {
        let mb = offline::choice_mb(choice);
        self.disk.run(move || {
            offline::set_cache_limit_mb(mb);
            crate::on_ui(|app| app.show_storage());
        });
    }

    pub fn clear_cache(self: &Rc<Self>) {
        self.disk.run(|| {
            offline::clear_cache();
            crate::on_ui(|app| {
                app.show_storage();
                app.toast("Cache cleared");
            });
        });
    }
}
