//! The right click menu, and the queue actions it offers.
//!
//! The window asks for a menu with a source and an id. We look the item
//! up, offer only what makes sense for it, and keep what each line does
//! until one is picked.

use crate::innertube::model::{Item, Kind, PageKind};
use crate::innertube::Client;
use crate::{offline, on_ui, shuffled, App, Icons, MenuEntry, Target};
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

    /// "Go to artist", or one line per artist when there are several.
    fn artists(&mut self, item: &Item) {
        let linked: Vec<(String, String)> = item
            .artists
            .iter()
            .filter_map(|(name, id)| Some((name.clone(), id.clone()?)))
            .collect();
        let several = linked.len() > 1;
        for (name, id) in linked {
            let label = if several {
                format!("Go to {name}")
            } else {
                "Go to artist".into()
            };
            let icon = self.icons.get_artist();
            self.glyph(label, icon, Action::Browse(id));
        }
    }
}

/// An album or playlist that can be fetched for its songs.
fn is_collection(item: &Item) -> bool {
    item.video_id.is_none()
        && (matches!(item.kind, Kind::Album | Kind::Playlist)
            || item.browse_id.as_deref().is_some_and(|id| {
                id.starts_with("MPREb_") || id.starts_with("VL") || id.starts_with("OLAK")
            }))
}

/// The songs of an album or playlist, every page of them. Blocks.
fn collection_songs(client: &Client, item: &Item) -> Result<Vec<Item>, String> {
    let songs_of = |page: &crate::innertube::model::Page| -> Vec<Item> {
        page.shelves
            .iter()
            .filter(|s| !s.carousel)
            .flat_map(|s| s.items.iter())
            .filter(|i| i.video_id.is_some())
            .cloned()
            .collect()
    };
    if let Some(id) = &item.browse_id {
        let page = client.browse(id, item.params.as_deref())?;
        let mut songs = songs_of(&page);
        // A long playlist comes in pages. Fetch them all, within reason.
        let mut token = page.continuation;
        for _ in 0..40 {
            let Some(t) = token.take() else { break };
            let more = client.more(&t)?;
            songs.extend(songs_of(&more));
            token = more.continuation;
        }
        Ok(songs)
    } else if let Some(list) = &item.playlist_id {
        Ok(client.queue(None, Some(list))?.0)
    } else {
        Err("Nothing to play".into())
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
            _ => {}
        }
        let Lines { entries, actions, .. } = lines;
        *self.menu.borrow_mut() = actions;
        self.ui.set_menu_entries(ModelRc::new(VecModel::from(entries)));
    }

    fn item_lines(self: &Rc<Self>, lines: &mut Lines, item: &Item, playing: bool) {
        let icons = &lines.icons;
        let (play, shuffle, heart) = (icons.get_play(), icons.get_shuffle(), icons.get_heart());
        let (play_next, add_queue, album, radio, download, trash, open) = (
            icons.get_play_next(),
            icons.get_add_queue(),
            icons.get_album(),
            icons.get_radio(),
            icons.get_download(),
            icons.get_trash(),
            icons.get_open(),
        );
        if let Some(video_id) = item.video_id.clone() {
            if !playing {
                lines.glyph("Play next", play_next, Action::PlayNext(item.clone()));
                lines.glyph("Add to queue", add_queue, Action::AddToQueue(item.clone()));
            }
            lines.artists(item);
            if let Some((_, id)) = &item.album {
                lines.glyph("Go to album", album, Action::Browse(id.clone()));
            }
            lines.glyph("Song radio", radio.clone(), Action::Radio(item.clone()));
            // Each linked artist's own radio, as Spotify offers.
            let linked: Vec<(String, String)> = item
                .artists
                .iter()
                .filter_map(|(name, id)| Some((name.clone(), id.clone()?)))
                .collect();
            let several = linked.len() > 1;
            for (name, id) in linked {
                let label = if several { format!("{name} radio") } else { "Artist radio".into() };
                lines.glyph(label, radio.clone(), Action::Artist(id, false));
            }
            if offline::is_downloaded(&video_id) {
                lines.glyph("Remove download", trash, Action::ToggleDownload(item.clone()));
            } else if !offline::is_downloading(&video_id) {
                lines.glyph("Download", download, Action::ToggleDownload(item.clone()));
            }
            if self.client.signed_in() {
                if playing {
                    let label = if self.player.borrow().liked { "Unlike" } else { "Like" };
                    lines.glyph(label, heart, Action::LikePlaying);
                } else {
                    lines.glyph("Like", heart, Action::Like(item.clone()));
                }
                let add = lines.icons.get_add_queue();
                lines.glyph("Add to playlist", add, Action::AddToPlaylist(item.clone()));
            }
        } else if item.kind == Kind::Artist {
            if let Some(id) = &item.browse_id {
                lines.glyph("Open", open, Action::Browse(id.clone()));
                lines.glyph("Shuffle", shuffle, Action::Artist(id.clone(), true));
                lines.glyph("Start radio", radio, Action::Artist(id.clone(), false));
                // An artist's page id is their channel. The menu does not
                // say whether they are followed, but the library only lists
                // artists that are.
                if self.client.signed_in() && id.starts_with("UC") {
                    let follow = !self.on_library_page();
                    let label = if follow { "Follow" } else { "Unfollow" };
                    let icon = lines.icons.get_artist();
                    lines.glyph(label, icon, Action::Follow(id.clone(), follow, item.title.clone()));
                }
            }
        } else if is_collection(item) {
            lines.glyph("Play", play, Action::Collection(item.clone(), Then::Play));
            lines.glyph("Shuffle", shuffle, Action::Collection(item.clone(), Then::Shuffle));
            lines.glyph("Download all", download, Action::Collection(item.clone(), Then::Download));
            lines.artists(item);
            if let Some(id) = &item.browse_id {
                lines.glyph("Open", open, Action::Browse(id.clone()));
            }
            if let Some((playlist, saved)) = item.library.clone().filter(|_| self.client.signed_in()) {
                let (label, icon) = if saved {
                    ("Remove from library", lines.icons.get_trash())
                } else {
                    ("Save to library", lines.icons.get_library())
                };
                lines.glyph(label, icon, Action::Library(playlist, !saved, item.title.clone()));
            }
        } else if let Some(id) = &item.browse_id {
            lines.glyph("Open", open, Action::Browse(id.clone()));
        }
    }

    /// The menu of the page itself, from its hero.
    fn page_lines(self: &Rc<Self>, lines: &mut Lines) {
        let header = self.page.borrow().header.clone();
        let Some(header) = header else { return };
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
            return;
        }
        if !songs.is_empty() {
            lines.glyph("Play", play, Action::PlayPage(false));
        }
        if let Some(id) = header.byline_browse_id.clone() {
            lines.glyph("Go to artist", artist, Action::Browse(id));
        }
        if let Some(playlist) = header.playlist_id.clone().filter(|_| header.editable) {
            let (settings, trash) = (lines.icons.get_settings(), lines.icons.get_trash());
            lines.glyph("Rename playlist", settings, Action::RenamePlaylist(playlist.clone(), header.title.clone()));
            lines.glyph("Delete playlist", trash, Action::DeletePlaylist(playlist, header.title.clone()));
        }
    }

    /// The library or one of its lists, where everything shown is saved.
    fn on_library_page(&self) -> bool {
        match self.history.borrow().last() {
            Some(crate::Route::Library) => true,
            Some(crate::Route::Browse { id, .. }) => {
                id.starts_with("FEmusic_library") || id.starts_with("FEmusic_liked")
            }
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
                            Ok(()) => app.toast(format!("Liked {}", item.title)),
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
                        Ok(()) => app.toast(format!("Added {} to {title}", item.title)),
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
            let result = client.browse(&id, None).and_then(|page| {
                let header = page.header.clone().unwrap_or_default();
                let endpoint = if shuffle { header.shuffle } else { header.radio };
                match endpoint {
                    Some(e) => client
                        .queue_with(e.video_id.as_deref(), e.playlist_id.as_deref(), e.params.as_deref())
                        .map(|(items, _)| items),
                    None => Ok(page
                        .shelves
                        .into_iter()
                        .filter(|s| !s.carousel)
                        .flat_map(|s| s.items)
                        .filter(|i| i.video_id.is_some())
                        .collect()),
                }
            });
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
