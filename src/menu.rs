//! The right click menu, and the queue actions it offers.
//!
//! The window asks for a menu with a source and an id. We look the item
//! up, offer only what makes sense for it, and keep what each line does
//! until one is picked. An item's own lines come from the core's `menu`,
//! which the phone shares.

use crate::innertube::model::{Header, Item, Kind, PageKind};
use crate::{quick_access, on_ui, shuffled, App, Icons, MenuEntry, Target};
use tonearm_core::menu::{self as core_menu, artist_songs, collection_songs, is_collection};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::rc::Rc;

/// What to do with the songs of an album or playlist once they are fetched.
#[derive(Clone, Copy)]
pub enum Then {
    Play,
    Shuffle,
    Download,
}

#[derive(Clone)]
pub enum Action {
    PlayNext(Item),
    AddToQueue(Item),
    Browse(String),
    /// A song's radio.
    Radio(Item),
    ToggleDownload(Item),
    /// Likes a song that is not the one playing.
    Like(Item),
    /// Likes or unlikes the song that is playing.
    LikePlaying,
    /// Fetches an album or playlist, then plays or downloads its songs.
    Collection(Item, Then),
    /// Fetches an artist page and plays its shuffle, or its radio.
    Artist(String, bool),
    /// A watch endpoint, such as the shuffle on the artist page open now.
    Endpoint(Item),
    PlayPage(bool),
    /// Lists the user's playlists in the menu, to add a song to one.
    AddToPlaylist(Item),
    /// Adds a song to a playlist: playlist id, its title, the song.
    IntoPlaylist(String, String, Item),
    /// Asks for a name, then makes a playlist, with this song in it.
    NewPlaylist(Option<Item>),
    /// Takes a song out of the playlist on this page.
    OutOfPlaylist(String, Item),
    /// Saves an album or playlist to the library, or takes it out: its
    /// playlist id, true to save, and its title.
    Library(String, bool, String),
    /// Follows an artist, or stops: channel id, true to follow, the name.
    Follow(String, bool, String),
    RenamePlaylist(String, String),
    DeletePlaylist(String, String),
    /// Pins an album, artist or playlist to the side menu's Quick access,
    /// or unpins it.
    Pin(Item),
    /// Copies a Spotify playlist into YouTube Music: its browse id and name.
    CopyToYouTube(String, String),
    /// Pairs a playlist with one on the other service and syncs them: a
    /// Spotify browse id or a YouTube Music browse id, and its name.
    StartSync(String, String),
    /// Syncs a pair now, by its Spotify browse id.
    SyncNow(String),
    /// Undoes a pair's latest sync.
    UndoSync(String),
    /// Stops syncing a pair: its Spotify browse id and name.
    StopSync(String, String),
    /// Makes the removals held for the user, or keeps the songs.
    RemoveHeld(String),
    KeepHeld(String),
    /// One of the hero's buttons, folded into the dots menu for want of
    /// room: 1 shuffle, 2 mix, 3 follow, 4 save, 5 download.
    Hero(u8),
}

/// What the name prompt is asking for.
pub enum Prompt {
    /// A name for a new playlist, which this song goes in.
    NewPlaylist(Option<Item>),
    /// A new name for this playlist.
    Rename(String),
    /// Yes to delete this playlist, with its title.
    Delete(String, String),
}

/// Where a right click came from, as the window numbers it.
const PAGE_ITEM: i32 = 0;
const QUEUE_ITEM: i32 = 1;
const PLAYING: i32 = 2;
const PAGE: i32 = 3;
const QUICK_ACCESS: i32 = 4;

struct Lines<'a> {
    icons: Icons<'a>,
    entries: Vec<MenuEntry>,
    actions: Vec<Action>,
}

impl Lines<'_> {
    fn glyph(&mut self, label: impl Into<slint::SharedString>, icon: slint::SharedString, action: Action) {
        self.entries.push(MenuEntry {
            label: label.into(),
            icon,
        });
        self.actions.push(action);
    }

    /// The window's glyph for one of the core's line icons.
    fn icon(&self, icon: core_menu::Icon) -> slint::SharedString {
        use core_menu::Icon as I;
        let icons = &self.icons;
        match icon {
            I::Play => icons.get_play(),
            I::Shuffle => icons.get_shuffle(),
            I::PlayNext => icons.get_play_next(),
            I::AddQueue => icons.get_add_queue(),
            I::Artist => icons.get_artist(),
            I::Album => icons.get_album(),
            I::Radio => icons.get_radio(),
            I::Download => icons.get_download(),
            I::Trash => icons.get_trash(),
            I::Heart => icons.get_heart(),
            I::Open => icons.get_open(),
            I::Library => icons.get_library(),
        }
    }
}

impl App {
    /// Fills the menu for what was right clicked. Leaves it empty when the
    /// item offers nothing, and the window then shows no menu.
    pub fn item_menu(self: &Rc<Self>, source: i32, id: i32) {
        let mut lines = Lines {
            icons: self.ui.global::<Icons>(),
            entries: Vec::new(),
            actions: Vec::new(),
        };
        let id = id.max(0) as usize;
        match source {
            PAGE_ITEM => {
                let item = self.page.borrow().items.get(id).cloned();
                if let Some(item) = item {
                    self.item_lines(&mut lines, &item, false);
                    // A song on the user's own playlist can come out of it.
                    let own = self
                        .page
                        .borrow()
                        .header
                        .as_ref()
                        .filter(|h| h.editable)
                        .and_then(|h| h.playlist_id.clone());
                    if let Some(playlist) = own.filter(|_| item.set_video_id.is_some()) {
                        let trash = lines.icons.get_trash();
                        lines.glyph("Remove from playlist", trash, Action::OutOfPlaylist(playlist, item));
                    }
                }
            }
            QUEUE_ITEM | PLAYING => {
                let (item, playing) = {
                    let player = self.player.borrow();
                    let index = if source == PLAYING { player.index } else { id };
                    (player.queue.get(index).cloned(), index == player.index)
                };
                if let Some(item) = item {
                    self.item_lines(&mut lines, &item, playing);
                }
            }
            PAGE => self.page_lines(&mut lines),
            // An entry in the side menu's Quick access: the same lines as
            // its card, ending with Unpin.
            QUICK_ACCESS => {
                let item = self.quick_access.borrow().items().get(id).cloned();
                if let Some(item) = item {
                    self.item_lines(&mut lines, &item, false);
                }
            }
            _ => {}
        }
        let Lines { entries, actions, .. } = lines;
        *self.menu.borrow_mut() = actions;
        self.ui.set_menu_entries(ModelRc::new(VecModel::from(entries)));
    }

    fn item_lines(self: &Rc<Self>, lines: &mut Lines, item: &Item, playing: bool) {
        // The lines both apps offer, from the core, then the desktop's own.
        let context = core_menu::Context {
            signed_in: self.client.signed_in(),
            playing,
            liked: self.player.borrow().liked,
            in_library: self.on_library_page(),
            downloads: true,
        };
        for line in core_menu::lines(item, &context) {
            let icon = lines.icon(line.icon);
            let action = match line.action {
                core_menu::Action::PlayNext => Action::PlayNext(item.clone()),
                core_menu::Action::AddToQueue => Action::AddToQueue(item.clone()),
                core_menu::Action::Browse(id) => Action::Browse(id),
                core_menu::Action::SongRadio => Action::Radio(item.clone()),
                core_menu::Action::ArtistRadio(id) => Action::Artist(id, false),
                core_menu::Action::ArtistShuffle(id) => Action::Artist(id, true),
                core_menu::Action::ToggleDownload => Action::ToggleDownload(item.clone()),
                core_menu::Action::Like => Action::Like(item.clone()),
                core_menu::Action::LikePlaying => Action::LikePlaying,
                core_menu::Action::AddToPlaylist => Action::AddToPlaylist(item.clone()),
                core_menu::Action::Play => Action::Collection(item.clone(), Then::Play),
                core_menu::Action::Shuffle => Action::Collection(item.clone(), Then::Shuffle),
                core_menu::Action::DownloadAll => Action::Collection(item.clone(), Then::Download),
                core_menu::Action::Library { playlist, save } => Action::Library(playlist, save, item.title.clone()),
                core_menu::Action::Follow { channel, follow } => Action::Follow(channel, follow, item.title.clone()),
            };
            lines.glyph(line.label, icon, action);
        }
        // Anything else with a page: a Spotify playlist can be copied.
        let other = item.video_id.is_none() && item.kind != Kind::Artist && !is_collection(item);
        if let Some(id) = item.browse_id.as_ref().filter(|_| other) {
            self.copy_line(lines, id, &item.title);
        }
        if let Some(id) = &item.browse_id {
            self.sync_lines(lines, id, &item.title);
        }
        self.pin_line(lines, item);
    }

    /// Sync's lines for a playlist: "Sync with …" to pair it, or, once it
    /// is paired, syncing now, reviewing held removals, the songs one side
    /// cannot have, undoing and stopping.
    fn sync_lines(&self, lines: &mut Lines, id: &str, title: &str) {
        let spotify = crate::spotify::is_playlist(id);
        let youtube = id.starts_with("VL") && !id.starts_with(crate::spotify::PREFIX);
        if !(spotify || youtube) || !self.client.signed_in() || self.settings.borrow().spotify_cookie.is_empty() {
            return;
        }
        let (repeat, trash) = (lines.icons.get_repeat(), lines.icons.get_trash());
        let other = if spotify { "YouTube Music" } else { "Spotify" };
        let Some(pair) = crate::syncing::pair_of(id) else {
            let label = format!("Sync to {other}");
            lines.glyph(&label, repeat, Action::StartSync(id.to_string(), title.to_string()));
            return;
        };
        // The way the pair was made: to the copy from the playlist it was
        // made from. Songs still go both ways.
        let origin = pair.from_spotify == spotify;
        let label = if origin { format!("Sync to {other} now") } else { format!("Sync from {other} now") };
        let key = pair.spotify.clone();
        lines.glyph(&label, repeat, Action::SyncNow(key.clone()));
        if pair.held_count() > 0 {
            let label = format!("Review {} held removals", pair.held_count());
            let icon = lines.icons.get_queue();
            lines.glyph(&label, icon, Action::Browse(format!("{}{key}", crate::syncing::REVIEW)));
        }
        let missing = pair.not_on_youtube.len() + pair.not_on_spotify.len();
        if missing > 0 {
            let label = format!("Songs not synced ({missing})");
            let icon = lines.icons.get_queue();
            lines.glyph(&label, icon, Action::Browse(format!("{}{key}", crate::syncing::MISSING)));
        }
        if !pair.log.is_empty() {
            let icon = lines.icons.get_back();
            lines.glyph("Undo last sync", icon, Action::UndoSync(key.clone()));
        }
        lines.glyph("Stop syncing", trash, Action::StopSync(key, pair.name.clone()));
    }

    /// "Copy to YouTube Music" for a Spotify playlist, or "Update YouTube
    /// Music copy" once it has been copied, which adds what it lacks. A
    /// playlist that syncs has no need of either.
    fn copy_line(&self, lines: &mut Lines, id: &str, title: &str) {
        if !crate::spotify::is_playlist(id) || !self.client.signed_in() || crate::syncing::pair_of(id).is_some() {
            return;
        }
        let label = if crate::spotify::copy_of(id).is_some() {
            "Update YouTube Music copy"
        } else {
            "Copy to YouTube Music"
        };
        let icon = lines.icons.get_library();
        lines.glyph(label, icon, Action::CopyToYouTube(id.to_string(), title.to_string()));
    }

    /// "Pin to Quick access", or "Unpin from Quick access" when it is
    /// pinned. Quick access is the side menu's list of shortcuts.
    fn pin_line(&self, lines: &mut Lines, item: &Item) {
        if !quick_access::can_pin(item) {
            return;
        }
        let label = if self.quick_access.borrow().contains(item) {
            "Unpin from Quick access"
        } else {
            "Pin to Quick access"
        };
        let pin = lines.icons.get_pin();
        lines.glyph(label, pin, Action::Pin(item.clone()));
    }

    /// The album, artist or playlist the open page is, as a pin.
    fn page_pin(&self, header: &Header) -> Option<Item> {
        match self.history.borrow().last() {
            Some(crate::Route::Browse { id, params }) => {
                quick_access::from_page(header, id, params.as_deref())
            }
            _ => None,
        }
    }

    /// The menu of the page itself, from its hero.
    fn page_lines(self: &Rc<Self>, lines: &mut Lines) {
        let header = self.page.borrow().header.clone();
        let Some(header) = header else { return };
        // The removals sync held back: make them, or keep the songs.
        let open = self.page_key();
        if let Some(key) = open.strip_prefix(crate::syncing::REVIEW) {
            let (trash, heart) = (lines.icons.get_trash(), lines.icons.get_heart());
            lines.glyph("Remove these songs", trash, Action::RemoveHeld(key.to_string()));
            lines.glyph("Keep these songs", heart, Action::KeepHeld(key.to_string()));
            return;
        }
        let songs = self.page_songs();
        let icons = &lines.icons;
        let (play, radio, artist) = (icons.get_play(), icons.get_radio(), icons.get_artist());
        if header.kind == PageKind::Artist {
            if !songs.is_empty() {
                lines.glyph("Play", play, Action::PlayPage(false));
            }
            if let Some(endpoint) = header.radio.clone() {
                lines.glyph("Start radio", radio, Action::Endpoint(endpoint));
            }
            if let Some(item) = self.page_pin(&header) {
                self.pin_line(lines, &item);
            }
            return;
        }
        if !songs.is_empty() {
            lines.glyph("Play", play, Action::PlayPage(false));
        }
        self.folded_lines(lines);
        if let Some(id) = header.byline_browse_id.clone() {
            lines.glyph("Go to artist", artist, Action::Browse(id));
        }
        if let Some(playlist) = header.playlist_id.clone().filter(|_| header.editable) {
            let (settings, trash) = (lines.icons.get_settings(), lines.icons.get_trash());
            lines.glyph("Rename playlist", settings, Action::RenamePlaylist(playlist.clone(), header.title.clone()));
            lines.glyph("Delete playlist", trash, Action::DeletePlaylist(playlist, header.title.clone()));
        }
        self.copy_line(lines, &open, &header.title);
        self.sync_lines(lines, &open, &header.title);
        if let Some(item) = self.page_pin(&header) {
            self.pin_line(lines, &item);
        }
    }

    /// The hero's buttons that its row had no room for, which the dots
    /// menu offers instead, in the row's order.
    fn folded_lines(&self, lines: &mut Lines) {
        let data = self.ui.get_header();
        let fit = self.ui.get_hero_fit();
        let icons = &lines.icons;
        let (shuffle, heart, download, artist) = (icons.get_shuffle(), icons.get_heart(), icons.get_download(), icons.get_artist());
        let sparkle = icons.get_sparkle();
        if fit < 1 && data.can_shuffle {
            lines.glyph("Shuffle play", shuffle, Action::Hero(1));
        }
        if fit < 2 && data.can_mix {
            lines.glyph(if data.mixing { "Turn off mix" } else { "Mix" }, sparkle, Action::Hero(2));
        }
        if fit < 3 && data.can_follow {
            lines.glyph(if data.followed { "Unfollow" } else { "Follow" }, artist, Action::Hero(3));
        }
        if fit < 4 && data.can_like {
            lines.glyph(if data.liked { "Remove from library" } else { "Save to library" }, heart, Action::Hero(4));
        }
        if fit < 5 && data.download != 0 && data.download != 3 {
            lines.glyph("Download all", download, Action::Hero(5));
        }
    }

    /// The library or one of its lists, where everything shown is saved.
    fn on_library_page(&self) -> bool {
        match self.history.borrow().last() {
            Some(crate::Route::Library) => true,
            Some(crate::Route::Browse { id, .. }) => core_menu::is_library_page(id),
            _ => false,
        }
    }

    /// Opens the page shown now again, so it shows what just changed.
    fn reload_page(self: &Rc<Self>) {
        let current = self.history.borrow().last().cloned();
        if let Some(route) = current {
            self.open(route, false);
        }
    }

    pub fn menu_picked(self: &Rc<Self>, index: usize) {
        let action = self.menu.borrow().get(index).cloned();
        let Some(action) = action else { return };
        match action {
            Action::PlayNext(item) => self.play_next(item),
            Action::AddToQueue(item) => self.add_to_queue(item),
            Action::Browse(id) => self.open_browse(&id),
            Action::Radio(item) => {
                let mut song = item;
                song.playlist_id = None;
                self.open_item(song);
            }
            Action::ToggleDownload(item) => self.toggle_download(item),
            Action::Like(item) => {
                if let Some(video_id) = item.video_id {
                    let client = self.client.clone();
                    self.api.run(move || {
                        let result = client.rate(&video_id, true);
                        on_ui(move |app| match result {
                            Ok(()) => {
                                app.toast(format!("Liked {}", item.title));
                                app.sync_soon(crate::syncing::LIKED_MUSIC);
                            }
                            Err(message) => app.toast(message),
                        });
                    });
                }
            }
            Action::LikePlaying => self.toggle_like(),
            Action::Collection(item, then) => self.collection(item, then),
            Action::Artist(id, shuffle) => self.artist_mix(id, shuffle),
            Action::Endpoint(endpoint) => self.play_endpoint(endpoint),
            Action::PlayPage(shuffle) => self.play_page(shuffle),
            Action::AddToPlaylist(item) => self.playlist_menu(item),
            Action::IntoPlaylist(playlist, title, item) => {
                let Some(video_id) = item.video_id.clone() else { return };
                let client = self.client.clone();
                self.api.run(move || {
                    let result = client.add_to_playlist(&playlist, &video_id);
                    on_ui(move |app| match result {
                        Ok(()) => {
                            app.toast(format!("Added {} to {title}", item.title));
                            app.sync_soon(&playlist);
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Action::NewPlaylist(item) => self.ask_new_playlist(item),
            Action::OutOfPlaylist(playlist, item) => {
                let (Some(video_id), Some(set)) = (item.video_id.clone(), item.set_video_id.clone()) else {
                    return;
                };
                let client = self.client.clone();
                self.api.run(move || {
                    let result = client.remove_from_playlist(&playlist, &video_id, &set);
                    on_ui(move |app| match result {
                        Ok(()) => {
                            app.toast(format!("Removed {} from the playlist", item.title));
                            app.reload_page();
                            app.sync_soon(&playlist);
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Action::Library(playlist, save, title) => {
                let client = self.client.clone();
                self.api.run(move || {
                    let result = client.rate_playlist(&playlist, save);
                    on_ui(move |app| match result {
                        Ok(()) => {
                            app.toast(if save {
                                format!("Saved {title} to your library")
                            } else {
                                format!("Removed {title} from your library")
                            });
                            if app.on_library_page() {
                                app.reload_page();
                            }
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Action::Follow(channel, follow, name) => {
                let client = self.client.clone();
                self.api.run(move || {
                    let result = client.subscribe(&channel, follow);
                    on_ui(move |app| match result {
                        Ok(()) => {
                            app.toast(if follow {
                                format!("Following {name}")
                            } else {
                                format!("Stopped following {name}")
                            });
                            if !follow && app.on_library_page() {
                                app.reload_page();
                            }
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Action::RenamePlaylist(playlist, title) => {
                self.ask(Prompt::Rename(playlist), "Rename playlist", "Save", &title, false);
            }
            Action::DeletePlaylist(playlist, title) => {
                let question = format!("Delete {title}?");
                self.ask(Prompt::Delete(playlist, title), &question, "Delete", "", true);
            }
            Action::Pin(item) => self.toggle_pin(&item),
            Action::CopyToYouTube(id, title) => self.copy_spotify(id, title),
            Action::StartSync(id, title) => self.start_sync(id, title),
            Action::SyncNow(key) => self.sync_pair(key, false),
            Action::UndoSync(key) => self.undo_sync(key),
            Action::StopSync(key, name) => {
                crate::syncing::unpair(&key);
                self.toast(format!("Stopped syncing {name}. Both playlists stay as they are."));
                self.refresh_syncs_page();
            }
            Action::Hero(button) => match button {
                1 => self.play_page(true),
                2 => self.toggle_mix(),
                3 => self.toggle_follow(),
                4 => self.toggle_like_page(),
                _ => self.download_page(),
            },
            Action::RemoveHeld(key) => self.settle_held(key, true),
            Action::KeepHeld(key) => self.settle_held(key, false),
        }
    }

    /// Fetches the user's playlists, then opens the menu again listing
    /// them, with a line for a new one first.
    fn playlist_menu(self: &Rc<Self>, item: Item) {
        let client = self.client.clone();
        self.api.run(move || {
            let result = client.my_playlists();
            on_ui(move |app| {
                let playlists = match result {
                    Ok(playlists) => playlists,
                    Err(message) => {
                        app.toast(message);
                        return;
                    }
                };
                let mut lines = Lines {
                    icons: app.ui.global::<Icons>(),
                    entries: Vec::new(),
                    actions: Vec::new(),
                };
                let library = lines.icons.get_library();
                lines.glyph("New playlist", library.clone(), Action::NewPlaylist(Some(item.clone())));
                // The menu grows down the window, so keep it to a screenful.
                for (title, id) in playlists.into_iter().take(14) {
                    let action = Action::IntoPlaylist(id, title.clone(), item.clone());
                    lines.glyph(title, library.clone(), action);
                }
                let Lines { entries, actions, .. } = lines;
                *app.menu.borrow_mut() = actions;
                app.ui.set_menu_entries(ModelRc::new(VecModel::from(entries)));
                app.ui.invoke_reopen_menu();
            });
        });
    }

    pub fn ask_new_playlist(self: &Rc<Self>, item: Option<Item>) {
        self.ask(Prompt::NewPlaylist(item), "New playlist", "Create", "", false);
    }

    /// Opens the name prompt for `prompt`, filled with `text`.
    fn ask(&self, prompt: Prompt, title: &str, action: &str, text: &str, confirm: bool) {
        *self.prompt.borrow_mut() = Some(prompt);
        self.ui.set_prompt_title(title.into());
        self.ui.set_prompt_action(action.into());
        self.ui.set_prompt_text(text.into());
        self.ui.set_prompt_confirm(confirm);
        self.ui.set_prompt_open(true);
    }

    /// The prompt was answered: `text` is the name typed, if it asked one.
    pub fn prompt_done(self: &Rc<Self>, text: String) {
        let Some(prompt) = self.prompt.borrow_mut().take() else { return };
        let name = text.trim().to_string();
        let client = self.client.clone();
        match prompt {
            Prompt::NewPlaylist(item) => {
                if name.is_empty() {
                    return;
                }
                self.api.run(move || {
                    let songs: Vec<String> = item.iter().filter_map(|i| i.video_id.clone()).collect();
                    let result = client.create_playlist(&name, &songs);
                    on_ui(move |app| match result {
                        Ok(_) => {
                            app.toast(match &item {
                                Some(song) => format!("Added {} to {name}", song.title),
                                None => format!("Made {name}"),
                            });
                            if app.on_library_page() {
                                app.reload_page();
                            }
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Prompt::Rename(playlist) => {
                if name.is_empty() {
                    return;
                }
                self.api.run(move || {
                    let result = client.rename_playlist(&playlist, &name);
                    on_ui(move |app| match result {
                        Ok(()) => {
                            app.toast(format!("Renamed to {name}"));
                            app.reload_page();
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
            Prompt::Delete(playlist, title) => {
                self.api.run(move || {
                    let result = client.delete_playlist(&playlist);
                    on_ui(move |app| match result {
                        // The page of a deleted playlist has nothing left
                        // to show, so go back to where it was opened from.
                        Ok(()) => {
                            app.toast(format!("Deleted {title}"));
                            if app.history.borrow().len() > 1 {
                                app.back();
                            } else {
                                app.open(crate::Route::Library, true);
                            }
                        }
                        Err(message) => app.toast(message),
                    });
                });
            }
        }
    }

    /// Fetches an album or playlist, then plays, shuffles or downloads it.
    fn collection(self: &Rc<Self>, item: Item, then: Then) {
        if let Then::Download = then {
            self.toast(format!("Getting the songs of {}", item.title));
        }
        let client = self.client.clone();
        self.api.run(move || {
            let result = collection_songs(&client, &item);
            on_ui(move |app| match result {
                Ok(songs) if !songs.is_empty() => match then {
                    Then::Play => app.set_queue(songs, 0),
                    Then::Shuffle => app.set_queue(shuffled(songs), 0),
                    Then::Download => app.download_items(songs),
                },
                Ok(_) => app.toast("There are no songs in it"),
                Err(message) => app.toast(message),
            });
        });
    }

    /// Plays an artist's shuffle or radio, as their page offers it. Falls
    /// back to the top songs when the page has neither.
    fn artist_mix(self: &Rc<Self>, id: String, shuffle: bool) {
        let client = self.client.clone();
        self.api.run(move || {
            let result = artist_songs(&client, &id, shuffle);
            on_ui(move |app| match result {
                Ok(items) if !items.is_empty() => app.set_queue(items, 0),
                Ok(_) => app.toast("Nothing to play"),
                Err(message) => app.toast(message),
            });
        });
    }

    /// Puts a song right after the one playing. A song already in the
    /// queue moves there instead of showing twice.
    pub fn play_next(self: &Rc<Self>, item: Item) {
        let Some(video_id) = item.video_id.clone() else {
            return;
        };
        if self.player.borrow().queue.is_empty() {
            self.set_queue(vec![item], 0);
            return;
        }
        let title = item.title.clone();
        {
            let mut player = self.player.borrow_mut();
            let current = player.index;
            let same = |i: &Item| i.video_id.as_deref() == Some(video_id.as_str());
            if player.queue.get(current).is_some_and(same) {
                drop(player);
                self.toast("That is playing now");
                return;
            }
            // Already next: moving it would rebuild the queue for nothing.
            if player.queue.get(current + 1).is_some_and(same) {
                drop(player);
                self.toast(format!("{title} already plays next"));
                return;
            }
            if let Some(at) = player.queue.iter().position(same) {
                player.queue.remove(at);
                if at < current {
                    player.index -= 1;
                }
            }
            let at = (player.index + 1).min(player.queue.len());
            player.queue.insert(at, item);
        }
        self.rebuild_queue_model();
        self.prefetch_next();
        self.toast(format!("{title} plays next"));
    }

    /// Adds a song to the end of the queue.
    pub fn add_to_queue(self: &Rc<Self>, item: Item) {
        let Some(video_id) = item.video_id.clone() else {
            return;
        };
        let (empty, known) = {
            let player = self.player.borrow();
            let known = player
                .queue
                .iter()
                .any(|i| i.video_id.as_deref() == Some(video_id.as_str()));
            (player.queue.is_empty(), known)
        };
        if empty {
            self.set_queue(vec![item], 0);
        } else if known {
            self.toast("That is already in the queue");
        } else {
            let title = item.title.clone();
            self.extend_queue(vec![item]);
            self.toast(format!("Added {title} to the queue"));
        }
    }

    /// Builds the queue list again after songs moved. Ids are positions,
    /// so a move shifts every row after it.
    pub(crate) fn rebuild_queue_model(self: &Rc<Self>) {
        let (items, generation, index) = {
            let mut player = self.player.borrow_mut();
            player.queue_generation += 1;
            // A radio fetch for the old list would be dropped as stale and
            // never clear this, so let the next one start.
            player.extending = false;
            player.model = Rc::new(VecModel::default());
            (player.queue.clone(), player.queue_generation, player.index)
        };
        let model = self.player.borrow().model.clone();
        for (i, item) in items.iter().enumerate() {
            model.push(self.item_data(item, i, 64, Target::Queue { generation, index: i }));
        }
        self.ui.set_queue(ModelRc::from(model));
        self.ui.set_queue_index(index as i32);
        self.sync_up_next();
        self.save_session();
    }
}
