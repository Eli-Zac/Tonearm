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
    DownloadPage,
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
            path: "".into(),
        });
        self.actions.push(action);
    }

    fn path(&mut self, label: impl Into<slint::SharedString>, path: slint::SharedString, action: Action) {
        self.entries.push(MenuEntry {
            label: label.into(),
            icon: "".into(),
            path,
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
            let path = self.icons.get_artist();
            self.path(label, path, Action::Browse(id));
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
                lines.path("Play next", play_next, Action::PlayNext(item.clone()));
                lines.path("Add to queue", add_queue, Action::AddToQueue(item.clone()));
            }
            lines.artists(item);
            if let Some((_, id)) = &item.album {
                lines.path("Go to album", album, Action::Browse(id.clone()));
            }
            lines.path("Start radio", radio, Action::Radio(item.clone()));
            if offline::is_downloaded(&video_id) {
                lines.path("Remove download", trash, Action::ToggleDownload(item.clone()));
            } else if !offline::is_downloading(&video_id) {
                lines.path("Download", download, Action::ToggleDownload(item.clone()));
            }
            if self.client.signed_in() {
                if playing {
                    let label = if self.player.borrow().liked { "Unlike" } else { "Like" };
                    lines.glyph(label, heart, Action::LikePlaying);
                } else {
                    lines.glyph("Like", heart, Action::Like(item.clone()));
                }
            }
        } else if item.kind == Kind::Artist {
            if let Some(id) = &item.browse_id {
                lines.path("Open", open, Action::Browse(id.clone()));
                lines.glyph("Shuffle", shuffle, Action::Artist(id.clone(), true));
                lines.path("Start radio", radio, Action::Artist(id.clone(), false));
            }
        } else if is_collection(item) {
            lines.glyph("Play", play, Action::Collection(item.clone(), Then::Play));
            lines.glyph("Shuffle", shuffle, Action::Collection(item.clone(), Then::Shuffle));
            lines.path("Download all", download, Action::Collection(item.clone(), Then::Download));
            lines.artists(item);
            if let Some(id) = &item.browse_id {
                lines.path("Open", open, Action::Browse(id.clone()));
            }
        } else if let Some(id) = &item.browse_id {
            lines.path("Open", open, Action::Browse(id.clone()));
        }
    }

    /// The menu of the page itself, from its hero.
    fn page_lines(self: &Rc<Self>, lines: &mut Lines) {
        let header = self.page.borrow().header.clone();
        let Some(header) = header else { return };
        let songs = self.page_songs();
        let icons = &lines.icons;
        let (play, shuffle, radio, download, artist) = (
            icons.get_play(),
            icons.get_shuffle(),
            icons.get_radio(),
            icons.get_download(),
            icons.get_artist(),
        );
        if header.kind == PageKind::Artist {
            if !songs.is_empty() {
                lines.glyph("Play", play, Action::PlayPage(false));
            }
            if let Some(endpoint) = header.shuffle.clone() {
                lines.glyph("Shuffle", shuffle, Action::Endpoint(endpoint));
            }
            if let Some(endpoint) = header.radio.clone() {
                lines.path("Start radio", radio, Action::Endpoint(endpoint));
            }
            return;
        }
        if !songs.is_empty() {
            lines.glyph("Play", play, Action::PlayPage(false));
            lines.glyph("Shuffle", shuffle, Action::PlayPage(true));
            let all_saved = songs.iter().all(|(_, s)| {
                s.video_id.as_deref().is_some_and(offline::is_downloaded)
            });
            if !all_saved {
                lines.path("Download all", download, Action::DownloadPage);
            }
        }
        if let Some(id) = header.byline_browse_id.clone() {
            lines.path("Go to artist", artist, Action::Browse(id));
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
            Action::DownloadPage => {
                let songs = self.page_songs().into_iter().map(|(_, s)| s).collect();
                self.download_items(songs);
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
    fn rebuild_queue_model(self: &Rc<Self>) {
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
    }
}
